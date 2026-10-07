use crate::clipboard::{MAX_ITEM_BYTES, Raw};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    io::Cursor,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{GlobalFree, HWND},
    System::{
        DataExchange::*,
        Memory::*,
        Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT},
    },
    UI::{Shell::DragQueryFileW, WindowsAndMessaging::*},
};

pub struct Clipboard {
    owner: HWND,
}
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.owner);
        }
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn register(s: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(wide(s).as_ptr()) }
}

impl Clipboard {
    pub fn new() -> Result<Self> {
        let owner = unsafe {
            CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("Starter clipboard").as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                null_mut(),
                null(),
            )
        };
        ensure!(!owner.is_null(), "剪贴板窗口创建失败");
        Ok(Self { owner })
    }
    pub fn sequence(&self) -> u64 {
        unsafe { GetClipboardSequenceNumber() as u64 }
    }
    fn open(&self) -> Result<Guard> {
        ensure!(
            unsafe { OpenClipboard(self.owner) } != 0,
            "剪贴板正在使用中"
        );
        Ok(Guard)
    }
    pub fn read(&mut self) -> Result<Option<Raw>> {
        let _guard = self.open()?;
        if unsafe {
            IsClipboardFormatAvailable(register("ExcludeClipboardContentFromMonitorProcessing"))
        } != 0
        {
            return Ok(None);
        }
        if data(register("CanIncludeInClipboardHistory"))?
            .is_some_and(|b| b.get(..4) == Some(&[0, 0, 0, 0]))
        {
            return Ok(None);
        }
        if unsafe { IsClipboardFormatAvailable(CF_HDROP as u32) } != 0 {
            let handle = unsafe { GetClipboardData(CF_HDROP as u32) };
            ensure!(!handle.is_null(), "文件剪贴板读取失败");
            let count = unsafe { DragQueryFileW(handle, u32::MAX, null_mut(), 0) };
            ensure!(count <= 1000, "剪贴板文件数量过多");
            let mut paths = Vec::new();
            for index in 0..count {
                let length = unsafe { DragQueryFileW(handle, index, null_mut(), 0) };
                ensure!(length < 32768, "文件路径过长");
                let mut path = vec![0u16; length as usize + 1];
                ensure!(
                    unsafe { DragQueryFileW(handle, index, path.as_mut_ptr(), length + 1) }
                        == length,
                    "文件路径读取失败"
                );
                paths.push(PathBuf::from(OsString::from_wide(&path[..length as usize])));
            }
            if !paths.is_empty() {
                return Ok(Some(Raw::Files(paths)));
            }
        }
        for (name, mime) in [
            ("PNG", "image/png"),
            ("JFIF", "image/jpeg"),
            ("GIF", "image/gif"),
        ] {
            if let Some(bytes) = data(register(name))? {
                return Ok(Some(Raw::Image {
                    mime: mime.into(),
                    bytes,
                }));
            }
        }
        for format in [CF_DIBV5 as u32, CF_DIB as u32] {
            if let Some(bytes) = data(format)? {
                return Ok(Some(Raw::Image {
                    mime: "image/bmp".into(),
                    bytes: dib_to_bmp(&bytes)?,
                }));
            }
        }
        if let Some(bytes) = data(CF_UNICODETEXT as u32)? {
            let units: Vec<_> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .take_while(|u| *u != 0)
                .collect();
            return Ok(Some(Raw::Text(String::from_utf16(&units)?)));
        }
        Ok(None)
    }
    pub fn write(&mut self, raw: &Raw) -> Result<()> {
        // Prepare all formats before clearing the clipboard.
        let formats = match raw {
            Raw::Text(text) => vec![(
                CF_UNICODETEXT as u32,
                wide(text).iter().flat_map(|u| u.to_le_bytes()).collect(),
            )],
            Raw::Files(paths) => {
                let mut bytes = vec![0u8; 20];
                bytes[..4].copy_from_slice(&20u32.to_le_bytes());
                bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
                for path in paths {
                    bytes.extend(
                        path.as_os_str()
                            .encode_wide()
                            .chain(Some(0))
                            .flat_map(u16::to_le_bytes),
                    );
                }
                bytes.extend([0, 0]);
                vec![(CF_HDROP as u32, bytes)]
            }
            Raw::Image { mime, bytes } => {
                let format = image::ImageFormat::from_mime_type(mime).context("图片格式无效")?;
                let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
                let mut limits = image::Limits::default();
                limits.max_alloc = Some(64 * 1024 * 1024);
                limits.max_image_width = Some(16384);
                limits.max_image_height = Some(16384);
                reader.limits(limits);
                let image = reader.decode()?;
                let mut bmp = Cursor::new(Vec::new());
                image.write_to(&mut bmp, image::ImageFormat::Bmp)?;
                let mut png = Cursor::new(Vec::new());
                image.write_to(&mut png, image::ImageFormat::Png)?;
                vec![
                    (CF_DIB as u32, bmp.into_inner()[14..].to_vec()),
                    (register("PNG"), png.into_inner()),
                ]
            }
        };
        let _guard = self.open()?;
        ensure!(unsafe { EmptyClipboard() } != 0, "剪贴板清空失败");
        for (format, bytes) in formats {
            set_data(format, &bytes)?;
        }
        Ok(())
    }
}

fn data(format: u32) -> Result<Option<Vec<u8>>> {
    if format == 0 || unsafe { IsClipboardFormatAvailable(format) } == 0 {
        return Ok(None);
    }
    let handle = unsafe { GetClipboardData(format) };
    ensure!(!handle.is_null(), "剪贴板读取失败");
    let size = unsafe { GlobalSize(handle) };
    ensure!(size <= MAX_ITEM_BYTES, "剪贴板内容过大");
    let pointer = unsafe { GlobalLock(handle) };
    ensure!(!pointer.is_null(), "剪贴板读取失败");
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec() };
    unsafe {
        GlobalUnlock(handle);
    }
    Ok(Some(bytes))
}

fn set_data(format: u32, bytes: &[u8]) -> Result<()> {
    ensure!(format != 0, "剪贴板格式注册失败");
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
    ensure!(!handle.is_null(), "剪贴板内存分配失败");
    let pointer = unsafe { GlobalLock(handle) };
    if pointer.is_null() {
        unsafe {
            GlobalFree(handle);
        }
        anyhow::bail!("剪贴板内存锁定失败");
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
        GlobalUnlock(handle);
    }
    if unsafe { SetClipboardData(format, handle) }.is_null() {
        unsafe {
            GlobalFree(handle);
        }
        anyhow::bail!("剪贴板写入失败");
    }
    Ok(())
}

fn dib_to_bmp(bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(bytes.len() >= 40, "DIB 图片无效");
    let header = u32::from_le_bytes(bytes[..4].try_into()?) as usize;
    ensure!(
        (40..=124).contains(&header) && header <= bytes.len(),
        "DIB 头无效"
    );
    let bits = u16::from_le_bytes(bytes[14..16].try_into()?);
    let compression = u32::from_le_bytes(bytes[16..20].try_into()?);
    let used = u32::from_le_bytes(bytes[32..36].try_into()?) as usize;
    let colors = if used != 0 {
        used
    } else if bits <= 8 {
        1usize << bits
    } else {
        0
    };
    let masks = if header == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let offset = header
        .checked_add(colors.checked_mul(4).context("DIB 色表无效")?)
        .and_then(|n| n.checked_add(masks))
        .context("DIB 偏移无效")?;
    ensure!(offset <= bytes.len(), "DIB 图片不完整");
    let mut bmp = Vec::with_capacity(bytes.len() + 14);
    bmp.extend(b"BM");
    bmp.extend(((bytes.len() + 14) as u32).to_le_bytes());
    bmp.extend([0u8; 4]);
    bmp.extend(((offset + 14) as u32).to_le_bytes());
    bmp.extend(bytes);
    Ok(bmp)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dib_preserves_pixels_and_rejects_truncated_headers() {
        let image = image::DynamicImage::new_rgba8(3, 2);
        let mut original = Cursor::new(Vec::new());
        image
            .write_to(&mut original, image::ImageFormat::Bmp)
            .unwrap();
        let bmp = dib_to_bmp(&original.get_ref()[14..]).unwrap();
        assert_eq!(
            image::load_from_memory(&bmp).unwrap().to_rgba8(),
            image.to_rgba8()
        );
        assert!(dib_to_bmp(&[0; 20]).is_err());
        let mut bad = vec![0u8; 40];
        bad[..4].copy_from_slice(&124u32.to_le_bytes());
        assert!(dib_to_bmp(&bad).is_err());
    }
}
