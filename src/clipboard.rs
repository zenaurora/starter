//! Bounded local clipboard history. One worker owns the clipboard and store;
//! polling, image decoding, persistence and restore never block the UI thread.
mod native;
use crate::search::{Candidate, Kind};
use anyhow::{Context, Result, ensure};
use async_channel::{Receiver, Sender};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 200;
const MAX_BYTES: u64 = 100 * 1024 * 1024;
pub(super) const MAX_ITEM_BYTES: usize = 16 * 1024 * 1024;
const MAX_AGE: u64 = 30 * 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Raw {
    Text(String),
    Image { mime: String, bytes: Vec<u8> },
    Files(Vec<PathBuf>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Payload {
    Text(Arc<String>),
    Image { mime: String, file: String },
    Files(Arc<Vec<PathBuf>>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub copied_at: u64,
    #[serde(skip)]
    search_index: Arc<String>,
    bytes: u64,
    payload: Payload,
    thumbnail: Option<String>,
}

impl Entry {
    fn kind(&self) -> Kind {
        match &self.payload {
            Payload::Text(text) if is_link(text) => Kind::ClipboardLink,
            Payload::Text(_) => Kind::ClipboardText,
            Payload::Image { .. } => Kind::ClipboardImage,
            Payload::Files(_) => Kind::ClipboardFiles,
        }
    }

    pub fn candidate(&self) -> Candidate {
        let date = DateTime::from_timestamp(self.copied_at as i64, 0)
            .map(|date| {
                date.with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        let kind = self.kind();
        let path = match &self.payload {
            Payload::Files(paths) => paths.first().cloned().unwrap_or_default(),
            _ => PathBuf::new(),
        };
        Candidate {
            id: self.id.clone(),
            title: self.title.clone(),
            detail: format!("{} · {date}", kind.label()),
            path,
            kind,
            aliases: vec![],
        }
    }

    pub fn thumbnail(&self, root: &Path) -> Option<PathBuf> {
        self.thumbnail.as_ref().map(|file| root.join(file))
    }

    fn index(&mut self) {
        let text = match &self.payload {
            Payload::Text(text) => text.to_lowercase(),
            Payload::Files(paths) => paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase(),
            Payload::Image { mime, .. } => format!("{} {mime}", self.title).to_lowercase(),
        };
        self.search_index = Arc::new(text);
    }
}

pub fn search(entries: &[Entry], query: &str) -> Vec<Candidate> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    entries
        .iter()
        .filter_map(|entry| {
            let candidate = entry.candidate();
            let kind_words = match entry.kind() {
                Kind::ClipboardText => "文本 text",
                Kind::ClipboardLink => "链接 link url",
                Kind::ClipboardImage => "图片 图像 image screenshot 截图",
                Kind::ClipboardFiles => "文件 file folder 文件夹",
                _ => "",
            };
            let detail = candidate.detail.to_lowercase();
            words
                .iter()
                .all(|word| {
                    entry.search_index.contains(word)
                        || detail.contains(word)
                        || kind_words.contains(word)
                })
                .then_some(candidate)
        })
        .take(crate::search::RESULT_LIMIT)
        .collect()
}

fn is_link(text: &str) -> bool {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && !text.chars().any(char::is_whitespace)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

struct Store {
    root: PathBuf,
    entries: Vec<Entry>,
}
impl Store {
    fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let manifest = root.join("history.json");
        let entries = if manifest.exists() {
            ensure!(
                fs::metadata(&manifest)?.len() <= MAX_BYTES + 1024 * 1024,
                "剪贴板索引过大"
            );
            let mut entries: Vec<Entry> = serde_json::from_slice(&fs::read(&manifest)?)?;
            // Reject traversal in on-disk asset names before loading or deleting.
            ensure!(
                entries.iter().all(|e| valid_id(&e.id)
                    && e.thumbnail.as_ref().is_none_or(|s| valid_asset(s))
                    && match &e.payload {
                        Payload::Image { file, .. } => valid_asset(file),
                        _ => true,
                    }),
                "剪贴板索引无效"
            );
            for entry in &mut entries {
                entry.index();
            }
            entries
        } else {
            Vec::new()
        };
        let mut store = Self { root, entries };
        store.prune(now());
        store.save()?;
        store.clean_assets()?;
        Ok(store)
    }

    fn save(&self) -> Result<()> {
        private_write(
            &self.root.join("history.json"),
            &serde_json::to_vec(&self.entries)?,
        )
    }

    fn prune(&mut self, time: u64) {
        self.entries
            .retain(|e| time.saturating_sub(e.copied_at) <= MAX_AGE);
        self.entries.truncate(MAX_ENTRIES);
        let mut total = 0u64;
        self.entries.retain(|entry| {
            total = total.saturating_add(entry.bytes);
            total <= MAX_BYTES
        });
    }

    fn clean_assets(&self) -> Result<()> {
        let keep: std::collections::HashSet<_> = self
            .entries
            .iter()
            .flat_map(|e| {
                let file = match &e.payload {
                    Payload::Image { file, .. } => Some(file),
                    _ => None,
                };
                file.into_iter().chain(e.thumbnail.iter())
            })
            .collect();
        for item in fs::read_dir(&self.root)?.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if valid_asset(&name) && !keep.contains(&name) {
                fs::remove_file(item.path())?;
            }
        }
        Ok(())
    }

    fn record(&mut self, raw: Raw, time: u64) -> Result<bool> {
        let size = match &raw {
            Raw::Text(text) => text.len(),
            Raw::Image { bytes, .. } => bytes.len(),
            Raw::Files(files) => files.iter().map(|p| p.as_os_str().len()).sum(),
        };
        ensure!(size <= MAX_ITEM_BYTES, "剪贴板内容超过 16 MiB，未记录");
        if size == 0 {
            return Ok(false);
        }
        let mut hash = Sha256::new();
        match &raw {
            Raw::Text(text) => {
                hash.update(b"text");
                hash.update(text.as_bytes());
            }
            Raw::Image { mime, bytes } => {
                hash.update(mime);
                hash.update(bytes);
            }
            Raw::Files(paths) => {
                hash.update(b"files");
                hash.update(serde_json::to_vec(paths)?);
            }
        }
        let id = format!("{:x}", hash.finalize());
        if let Some(index) = self.entries.iter().position(|e| e.id == id) {
            let mut entry = self.entries.remove(index);
            entry.copied_at = time;
            self.entries.insert(0, entry);
        } else {
            let (title, payload, thumbnail) = match raw {
                Raw::Text(text) => {
                    let title: String = text
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(160)
                        .collect();
                    (title, Payload::Text(Arc::new(text)), None)
                }
                Raw::Files(paths) => {
                    ensure!(
                        paths.iter().all(|p| p.is_absolute()) && paths.len() <= 1000,
                        "文件路径无效或数量过多"
                    );
                    let names = paths
                        .iter()
                        .map(|p| p.file_name().unwrap_or_default().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("、");
                    (
                        format!(
                            "{} 个文件 · {}",
                            paths.len(),
                            names.chars().take(130).collect::<String>()
                        ),
                        Payload::Files(Arc::new(paths)),
                        None,
                    )
                }
                Raw::Image { mime, bytes } => {
                    let format =
                        image::ImageFormat::from_mime_type(&mime).context("不支持的图片格式")?;
                    let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), format);
                    let mut limits = image::Limits::default();
                    limits.max_alloc = Some(64 * 1024 * 1024);
                    limits.max_image_width = Some(16384);
                    limits.max_image_height = Some(16384);
                    reader.limits(limits);
                    let image = reader.decode().context("图片读取失败")?;
                    let filename = format!("{id}.{}", format.extensions_str()[0]);
                    private_write(&self.root.join(&filename), &bytes)?;
                    let thumbname = format!("{id}.thumb.png");
                    let mut thumb = Cursor::new(Vec::new());
                    image
                        .thumbnail(96, 96)
                        .write_to(&mut thumb, image::ImageFormat::Png)?;
                    private_write(&self.root.join(&thumbname), thumb.get_ref())?;
                    (
                        format!("图片 · {} × {}", image.width(), image.height()),
                        Payload::Image {
                            mime,
                            file: filename,
                        },
                        Some(thumbname),
                    )
                }
            };
            let mut entry = Entry {
                id,
                title,
                search_index: Arc::default(),
                copied_at: time,
                bytes: 0,
                payload,
                thumbnail,
            };
            entry.bytes = serde_json::to_vec(&entry)?.len() as u64 + 32;
            if let Payload::Image { file, .. } = &entry.payload {
                entry.bytes += fs::metadata(self.root.join(file))?.len();
                if let Some(thumb) = &entry.thumbnail {
                    entry.bytes += fs::metadata(self.root.join(thumb))?.len();
                }
            }
            entry.index();
            self.entries.insert(0, entry);
        }
        self.prune(time);
        self.save()?;
        self.clean_assets()?;
        Ok(true)
    }

    fn restore(&self, id: &str) -> Result<Raw> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .context("历史记录已删除")?;
        Ok(match &entry.payload {
            Payload::Text(text) => Raw::Text(text.as_ref().clone()),
            Payload::Image { mime, file } => {
                let path = self.root.join(file);
                ensure!(
                    fs::metadata(&path)?.len() <= MAX_ITEM_BYTES as u64,
                    "图片过大"
                );
                Raw::Image {
                    mime: mime.clone(),
                    bytes: fs::read(path)?,
                }
            }
            Payload::Files(paths) => {
                ensure!(
                    paths.iter().all(|p| p.exists()),
                    "部分文件已移动或删除，无法重新复制"
                );
                Raw::Files(paths.as_ref().clone())
            }
        })
    }

    fn remove(&mut self, id: Option<&str>) -> Result<()> {
        self.entries.retain(|e| id.is_some_and(|id| e.id != id));
        self.save()?;
        self.clean_assets()
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit())
}
fn valid_asset(name: &str) -> bool {
    name.split_once('.').is_some_and(|(id, ext)| {
        valid_id(id)
            && !ext.is_empty()
            && ext.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.')
    })
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("存储路径无效")?)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub enum Command {
    Enabled(bool),
    Restore(String),
    Remove(String),
    Clear,
}
pub enum Event {
    Snapshot(Vec<Entry>),
    Restored,
    Cleared,
    Failed(String),
}

pub fn start(root: PathBuf, enabled: bool) -> (Sender<Command>, Receiver<Event>) {
    let (sender, commands) = async_channel::unbounded();
    let (events, receiver) = async_channel::bounded(16);
    std::thread::Builder::new()
        .name("starter-clipboard".into())
        .spawn(move || {
            let run = || -> Result<()> {
                let mut store = Store::open(root)?;
                let mut clipboard = native::Clipboard::new()?;
                let mut enabled = enabled;
                let mut sequence = clipboard.sequence();
                let mut last_pruned = now();
                let mut read_failures = 0;
                if events
                    .send_blocking(Event::Snapshot(store.entries.clone()))
                    .is_err()
                {
                    return Ok(());
                }
                loop {
                    while let Ok(command) = commands.try_recv() {
                        let result = match command {
                            Command::Enabled(value) => {
                                if enabled != value {
                                    enabled = value;
                                    sequence = clipboard.sequence();
                                    read_failures = 0;
                                }
                                Ok(())
                            }
                            Command::Restore(id) => store.restore(&id).and_then(|raw| {
                                clipboard.write(&raw)?;
                                sequence = clipboard.sequence();
                                let _ = events.send_blocking(Event::Restored);
                                Ok(())
                            }),
                            Command::Remove(id) => store.remove(Some(&id)).map(|_| {
                                let _ =
                                    events.send_blocking(Event::Snapshot(store.entries.clone()));
                            }),
                            Command::Clear => store.remove(None).map(|_| {
                                sequence = clipboard.sequence();
                                let _ = events.send_blocking(Event::Snapshot(vec![]));
                                let _ = events.send_blocking(Event::Cleared);
                            }),
                        };
                        if let Err(error) = result {
                            let _ = events.send_blocking(Event::Failed(format!("{error:#}")));
                        }
                    }
                    if commands.is_closed() || events.is_closed() {
                        break;
                    }
                    let changed = clipboard.sequence();
                    if changed != sequence {
                        if enabled {
                            // None means unsupported/empty/private data. A busy clipboard
                            // returns Err and is retried on the next poll.
                            match clipboard.read() {
                                Ok(raw) => {
                                    sequence = changed;
                                    read_failures = 0;
                                    if let Some(raw) = raw {
                                        match store.record(raw, now()) {
                                            Ok(true) => {
                                                let _ = events.send_blocking(Event::Snapshot(
                                                    store.entries.clone(),
                                                ));
                                            }
                                            Ok(false) => {}
                                            Err(error) => {
                                                let _ = events.send_blocking(Event::Failed(
                                                    format!("{error:#}"),
                                                ));
                                            }
                                        }
                                    }
                                }
                                Err(error) => {
                                    read_failures += 1;
                                    if read_failures >= 5 {
                                        sequence = changed;
                                        read_failures = 0;
                                        let _ = events.send_blocking(Event::Failed(format!(
                                            "剪贴板读取失败：{error:#}"
                                        )));
                                    }
                                }
                            }
                        } else {
                            sequence = changed;
                        }
                    }
                    if now().saturating_sub(last_pruned) >= 60 {
                        last_pruned = now();
                        let previous = store.entries.len();
                        store.prune(last_pruned);
                        if store.entries.len() != previous {
                            store.save()?;
                            store.clean_assets()?;
                            let _ = events.send_blocking(Event::Snapshot(store.entries.clone()));
                        }
                    }
                    std::thread::sleep(Duration::from_millis(400));
                }
                Ok(())
            };
            if let Err(error) = run() {
                let _ = events.send_blocking(Event::Failed(format!("剪贴板历史不可用：{error:#}")));
            }
        })
        .expect("Cannot start clipboard worker");
    (sender, receiver)
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) fn png() -> Raw {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(3, 2)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        Raw::Image {
            mime: "image/png".into(),
            bytes: bytes.into_inner(),
        }
    }
    #[test]
    fn all_types_survive_restart_and_search_and_restore_original_data() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("设计 draft.pdf");
        fs::write(&file, b"fixture").unwrap();
        let raw = vec![
            Raw::Text("第一行\n第二行 needle".into()),
            Raw::Text("https://example.com/docs".into()),
            png(),
            Raw::Files(vec![file.clone()]),
        ];
        let root = dir.path().join("history");
        let mut store = Store::open(root.clone()).unwrap();
        for (i, item) in raw.iter().enumerate() {
            store.record(item.clone(), now() + i as u64).unwrap();
        }
        let store = Store::open(root.clone()).unwrap();
        for (term, item) in ["needle", "example.com", "图片", "设计"]
            .into_iter()
            .zip(&raw)
        {
            let hits = search(&store.entries, term);
            assert_eq!(hits.len(), 1);
            assert_eq!(store.restore(&hits[0].id).unwrap(), *item);
        }
        assert!(store.entries[1].thumbnail(&root).unwrap().exists());
        fs::remove_file(file).unwrap();
        assert!(store.restore(&store.entries[0].id).is_err());
    }
    #[test]
    fn deduplicates_bounds_expires_and_clear_removes_image_assets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("history");
        let mut store = Store::open(root.clone()).unwrap();
        let text = Raw::Text("duplicate".into());
        store.record(text.clone(), now() - MAX_AGE - 1).unwrap();
        store.prune(now());
        assert!(store.entries.is_empty());
        for i in 0..205 {
            store.record(Raw::Text(format!("item {i}")), now()).unwrap();
        }
        assert_eq!(store.entries.len(), MAX_ENTRIES);
        store.record(text.clone(), now()).unwrap();
        store.record(text, now() + 1).unwrap();
        assert_eq!(store.entries.len(), MAX_ENTRIES);
        assert_eq!(store.entries[0].title, "duplicate");
        store.record(png(), now()).unwrap();
        assert!(store.entries[0].thumbnail(&root).unwrap().exists());
        store.remove(None).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        assert!(Store::open(root).unwrap().entries.is_empty());
    }
    #[test]
    fn rejects_oversized_capture_and_asset_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().to_path_buf()).unwrap();
        assert!(
            store
                .record(Raw::Text("x".repeat(MAX_ITEM_BYTES + 1)), now())
                .is_err()
        );
        assert!(!valid_asset("../../secret.png"));
        assert!(!valid_asset(&format!(
            "{}.png/../../secret",
            "a".repeat(64)
        )));
        assert!(store.entries.is_empty());
    }
}
