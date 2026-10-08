use super::Power;
use anyhow::{Result, ensure};

#[cfg(target_os = "macos")]
pub fn power(action: Power) -> Result<()> {
    use anyhow::Context;
    use objc2_foundation::{NSAppleEventDescriptor, NSData};
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn AESendMessage(
            event: *const std::ffi::c_void,
            reply: *mut std::ffi::c_void,
            mode: i32,
            timeout: std::ffi::c_long,
        ) -> i32;
    }
    objc2::rc::autoreleasepool(|_| {
        if action == Power::Lock {
            // No Accessibility permission or synthesized keyboard events. This
            // private login entry point is resolved at runtime, never assumed.
            // A missing symbol returns an actionable error rather than sleeping
            // the display and claiming the session is locked.
            unsafe {
                let library = libc::dlopen(
                    c"/System/Library/PrivateFrameworks/login.framework/login".as_ptr(),
                    libc::RTLD_LAZY,
                );
                ensure!(
                    !library.is_null(),
                    "此 macOS 版本不支持直接锁屏，请使用 Control+Command+Q"
                );
                let pointer = libc::dlsym(library, c"SACLockScreenImmediate".as_ptr());
                if pointer.is_null() {
                    libc::dlclose(library);
                    anyhow::bail!("锁屏接口不可用，请使用 Control+Command+Q");
                }
                let lock: unsafe extern "C" fn() = std::mem::transmute(pointer);
                lock();
                libc::dlclose(library);
            }
            return Ok(());
        }
        // Loginwindow's system-process Apple events ask apps to save before
        // restart/shutdown. They do not force-kill applications or need sudo.
        let psn = [0u32, 1u32];
        let data = NSData::with_bytes(unsafe {
            std::slice::from_raw_parts(psn.as_ptr().cast(), std::mem::size_of_val(&psn))
        });
        let target = NSAppleEventDescriptor::descriptorWithDescriptorType_data(
            u32::from_be_bytes(*b"psn "),
            Some(&data),
        )
        .context("系统会话不可用")?;
        let event_id = match action {
            Power::Sleep => *b"slep",
            Power::Restart => *b"rest",
            Power::Shutdown => *b"shut",
            Power::Lock => unreachable!(),
        };
        let event = NSAppleEventDescriptor::appleEventWithEventClass_eventID_targetDescriptor_returnID_transactionID(
            u32::from_be_bytes(*b"aevt"), u32::from_be_bytes(event_id), Some(&target), -1, 0);
        // A no-reply send deliberately has no descriptor to return. Use its
        // OSStatus rather than interpreting Foundation's nil reply as failure.
        let status =
            unsafe { AESendMessage(event.aeDesc().cast(), std::ptr::null_mut(), 1 | 32, 600) };
        ensure!(status == 0, "系统操作未成功发送（macOS 错误 {status}）");
        Ok(())
    })
}

#[cfg(target_os = "windows")]
pub fn power(action: Power) -> Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};
    use windows_sys::Win32::System::Shutdown::LockWorkStation;
    match action {
        Power::Lock => ensure!(
            unsafe { LockWorkStation() } != 0,
            "锁屏请求失败：{}",
            std::io::Error::last_os_error()
        ),
        Power::Sleep => sleep_windows()?,
        Power::Restart | Power::Shutdown => {
            let root = std::env::var_os("SystemRoot")
                .ok_or_else(|| anyhow::anyhow!("SystemRoot 未设置"))?;
            let output = Command::new(std::path::PathBuf::from(root).join("System32/shutdown.exe"))
                .args([
                    if action == Power::Restart { "/r" } else { "/s" },
                    "/t",
                    "0",
                ])
                .creation_flags(0x08000000)
                .output()?;
            ensure!(
                output.status.success(),
                "系统拒绝操作，请检查会话权限（{}）",
                output.status
            );
        }
    }
    Ok(())
}

pub fn open_settings(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/bin/open")
            .arg(url)
            .status()?;
        ensure!(status.success(), "无法打开系统设置，请从系统菜单打开");
    }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        let url: Vec<_> = url.encode_utf16().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        } as isize;
        ensure!(result > 32, "系统设置打开失败（Windows 错误 {result}）");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn sleep_windows() -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, LUID},
        Security::{
            AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
            SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        System::{
            Power::SetSuspendState,
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    };
    // SetSuspendState requires SeShutdownPrivilege enabled on the caller's token.
    // Temporarily enable an existing privilege; restore it even if sleep fails.
    unsafe {
        let mut token = std::ptr::null_mut();
        ensure!(
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token
            ) != 0,
            "无法读取会话权限：{}",
            std::io::Error::last_os_error()
        );
        let result = (|| {
            let name: Vec<_> = "SeShutdownPrivilege"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut luid = LUID::default();
            ensure!(
                LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) != 0,
                "无法查找睡眠权限：{}",
                std::io::Error::last_os_error()
            );
            let requested = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            let mut previous = TOKEN_PRIVILEGES::default();
            let mut size = 0;
            ensure!(
                AdjustTokenPrivileges(
                    token,
                    0,
                    &requested,
                    std::mem::size_of::<TOKEN_PRIVILEGES>() as u32,
                    &mut previous,
                    &mut size
                ) != 0
                    && GetLastError() == 0,
                "当前账户没有睡眠权限：{}",
                std::io::Error::last_os_error()
            );
            let success = SetSuspendState(false, false, false);
            let error = std::io::Error::last_os_error();
            AdjustTokenPrivileges(
                token,
                0,
                &previous,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            ensure!(success, "睡眠请求失败：{error}");
            Ok(())
        })();
        CloseHandle(token);
        result
    }
}
