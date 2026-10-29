//! Local saved text and todos. A single, lazy worker serializes durable edits;
//! unsuccessful reads or writes never replace the last good document.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, mpsc},
};

const MAX_ITEMS: usize = 5_000;
const MAX_BODY: usize = 256 * 1024;
const MAX_FILE: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Text,
    Link,
    Paper,
    Todo,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "文本",
            Self::Link => "链接",
            Self::Paper => "论文",
            Self::Todo => "待办",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub kind: Kind,
    pub title: String,
    pub body: String,
    pub url: String,
    pub pinned: bool,
    pub done: bool,
    pub source: Option<u64>,
}
impl Entry {
    /// Present legacy link/paper records in the same content field as saved text.
    pub fn saved_content(&self) -> String {
        if self.kind == Kind::Text || self.url.is_empty() {
            self.body.clone()
        } else if self.body.is_empty() {
            self.url.clone()
        } else {
            format!("{}\n\n{}", self.url, self.body)
        }
    }

    pub fn matches(&self, query: &str) -> bool {
        query.split_whitespace().all(|word| {
            let word = word.to_lowercase();
            [&self.title, &self.body, &self.url]
                .iter()
                .any(|s| s.to_lowercase().contains(&word))
        })
    }
    pub fn copy_text(&self) -> &str {
        if self.kind == Kind::Text {
            &self.body
        } else if !self.url.is_empty() {
            &self.url
        } else {
            &self.title
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Data {
    version: u32,
    next_id: u64,
    pub entries: Vec<Entry>,
}
impl Default for Data {
    fn default() -> Self {
        Self {
            version: 1,
            next_id: 1,
            entries: vec![],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Draft {
    pub kind: Kind,
    pub title: String,
    pub body: String,
    pub url: String,
}
impl Draft {
    /// Saving needs only content. A name and an actionable URL are derived locally.
    pub fn saved(content: String, name: &str) -> Self {
        let title = if name.trim().is_empty() {
            let first_line = content
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .trim();
            let mut title: String = first_line.chars().take(48).collect();
            if first_line.chars().count() > 48 {
                title.push('…');
            }
            title
        } else {
            name.trim().to_string()
        };
        let url = content
            .split_whitespace()
            .find(|word| web_url(word).is_ok())
            .unwrap_or("")
            .to_string();
        Self {
            kind: Kind::Text,
            title,
            body: content,
            url,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() {
            bail!("请填写一个名称");
        }
        if self.title.len() > 512 {
            bail!("名称过长，请控制在 512 字节以内");
        }
        if self.body.len() > MAX_BODY {
            bail!("单条内容不能超过 256 KB");
        }
        if self.kind == Kind::Text && self.body.trim().is_empty() {
            bail!("请填写需要保存的文本");
        }
        if matches!(self.kind, Kind::Link | Kind::Paper) && self.url.trim().is_empty() {
            bail!("请填写网页或论文链接");
        }
        if !self.url.trim().is_empty() {
            web_url(&self.url)?;
        }
        Ok(())
    }
}

/// Only ordinary web URLs can be launched from user-maintained records.
pub fn web_url(value: &str) -> Result<url::Url> {
    if value.len() > 8192 {
        bail!("链接过长，请控制在 8 KB 以内");
    }
    let url =
        url::Url::parse(value.trim()).context("链接格式不正确，请使用完整的 https:// 地址")?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("仅支持不含登录凭据的 http / https 网页链接");
    }
    Ok(url)
}

#[derive(Clone, Debug)]
pub enum Change {
    Save { id: Option<u64>, draft: Draft },
    Delete(u64),
    Restore(Entry),
    Done(u64, bool),
    Pin(u64, bool),
    ReadLater(u64),
}
pub enum Command {
    Reload,
    Edit(Change),
}
pub struct Event {
    pub change: Option<Change>,
    pub result: Result<Arc<Data>, String>,
}

struct Store {
    path: PathBuf,
    data: Data,
}
impl Store {
    fn load(path: PathBuf) -> Result<Self> {
        let data = match File::open(&path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_FILE {
                    bail!("收藏文件超过 16 MB，未加载或修改文件");
                }
                let data: Data =
                    serde_json::from_slice(&bytes).context("收藏文件无法读取；原文件已保留")?;
                data.validate()?;
                data
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(e) => return Err(e).context("无法读取收藏文件；原文件已保留"),
        };
        Ok(Self { path, data })
    }
    fn apply(&mut self, change: &Change) -> Result<Arc<Data>> {
        let mut next = self.data.clone();
        next.apply(change)?;
        next.validate()?;
        let bytes = serde_json::to_vec(&next)?;
        if bytes.len() as u64 > MAX_FILE {
            bail!("收藏文件已达到 16 MB 上限，请先整理内容");
        }
        let parent = self.path.parent().context("存储目录不存在")?;
        std::fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path)
            .context("保存失败，原有数据未修改")?;
        self.data = next;
        Ok(Arc::new(self.data.clone()))
    }
}
impl Data {
    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("收藏文件来自其他版本，未修改文件");
        }
        if self.next_id == 0 {
            bail!("收藏文件的条目标识异常，未修改文件");
        }
        if self.entries.len() > MAX_ITEMS {
            bail!("最多保存 5000 条收藏和待办");
        }
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            if entry.id == 0 || entry.id >= self.next_id || !ids.insert(entry.id) {
                bail!("收藏文件的条目标识异常，未修改文件");
            }
            Draft {
                kind: entry.kind,
                title: entry.title.clone(),
                body: entry.body.clone(),
                url: entry.url.clone(),
            }
            .validate()?;
        }
        Ok(())
    }
    fn allocate(&mut self) -> Result<u64> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).context("条目标识已耗尽")?;
        Ok(id)
    }
    fn entry(&mut self, id: u64) -> Result<&mut Entry> {
        self.entries
            .iter_mut()
            .find(|e| e.id == id)
            .context("条目已不存在，请重新选择")
    }
    fn apply(&mut self, change: &Change) -> Result<()> {
        match change {
            Change::Save { id, draft } => {
                draft.validate()?;
                if let Some(id) = id {
                    let entry = self.entry(*id)?;
                    if (entry.kind == Kind::Todo) != (draft.kind == Kind::Todo) {
                        bail!("收藏和待办不能互相覆盖");
                    }
                    entry.kind = draft.kind;
                    entry.title = draft.title.trim().into();
                    entry.body = draft.body.clone();
                    entry.url = draft.url.trim().into();
                } else {
                    let id = self.allocate()?;
                    self.entries.push(Entry {
                        id,
                        kind: draft.kind,
                        title: draft.title.trim().into(),
                        body: draft.body.clone(),
                        url: draft.url.trim().into(),
                        pinned: false,
                        done: false,
                        source: None,
                    });
                }
            }
            Change::Delete(id) => {
                self.entry(*id)?;
                self.entries.retain(|e| e.id != *id);
            }
            Change::Restore(entry) => {
                if self.entries.iter().any(|e| e.id == entry.id) {
                    bail!("条目已存在，无需恢复");
                }
                self.entries.push(entry.clone());
            }
            Change::Done(id, done) => {
                let entry = self.entry(*id)?;
                if entry.kind != Kind::Todo {
                    bail!("只能完成待办事项");
                }
                entry.done = *done;
            }
            Change::Pin(id, pinned) => self.entry(*id)?.pinned = *pinned,
            Change::ReadLater(id) => {
                let source = self.entry(*id)?.clone();
                if source.kind == Kind::Todo {
                    bail!("该条目已经是待办");
                }
                if let Some(existing) = self
                    .entries
                    .iter_mut()
                    .find(|e| e.source == Some(*id) && e.kind == Kind::Todo)
                {
                    existing.done = false;
                } else {
                    let new_id = self.allocate()?;
                    self.entries.push(Entry {
                        id: new_id,
                        kind: Kind::Todo,
                        title: if source.kind == Kind::Paper {
                            format!("阅读：{}", source.title)
                        } else {
                            source.title
                        },
                        body: source.body,
                        url: source.url,
                        pinned: false,
                        done: false,
                        source: Some(*id),
                    });
                }
            }
        }
        Ok(())
    }
}

pub fn start(path: PathBuf) -> Result<(mpsc::Sender<Command>, async_channel::Receiver<Event>)> {
    let (tx, rx) = mpsc::channel();
    let (events, updates) = async_channel::bounded(8);
    std::thread::Builder::new()
        .name("starter-organizer".into())
        .spawn(move || {
            let mut store = Store::load(path.clone());
            let initial = store
                .as_ref()
                .map(|s| Arc::new(s.data.clone()))
                .map_err(|e| format!("{e:#}"));
            if events
                .send_blocking(Event {
                    change: None,
                    result: initial,
                })
                .is_err()
            {
                return;
            }
            while let Ok(command) = rx.recv() {
                let (change, result) = match command {
                    Command::Reload => {
                        store = Store::load(path.clone());
                        (
                            None,
                            store
                                .as_ref()
                                .map(|s| Arc::new(s.data.clone()))
                                .map_err(|e| format!("{e:#}")),
                        )
                    }
                    Command::Edit(change) => {
                        let result = match &mut store {
                            Ok(s) => s.apply(&change).map_err(|e| format!("{e:#}")),
                            Err(e) => Err(format!("读取失败，暂不能保存：{e:#}")),
                        };
                        (Some(change), result)
                    }
                };
                if events.send_blocking(Event { change, result }).is_err() {
                    break;
                }
            }
        })?;
    Ok((tx, updates))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft(kind: Kind) -> Draft {
        Draft {
            kind,
            title: "发票 / 论文".into(),
            body: "第一行\n第二行 123456".into(),
            url: if matches!(kind, Kind::Paper | Kind::Link) {
                "https://arxiv.org/abs/1234.5678".into()
            } else {
                String::new()
            },
        }
    }
    #[test]
    fn content_only_saves_preserve_text_and_derive_a_short_name_and_link() {
        let content = "\n 公司抬头  \n税号 123456\nhttps://example.org/invoice\n";
        let draft = Draft::saved(content.into(), "");
        assert_eq!(draft.title, "公司抬头");
        assert_eq!(draft.url, "https://example.org/invoice");
        let mut data = Data::default();
        data.apply(&Change::Save { id: None, draft }).unwrap();
        assert_eq!(data.entries[0].copy_text(), content);
        assert_eq!(data.entries[0].saved_content(), content);
        assert_eq!(Draft::saved("长".repeat(80), "").title.chars().count(), 49);
        assert_eq!(
            Draft::saved(content.into(), " 自定义名称 ").title,
            "自定义名称"
        );
        assert!(Draft::saved(" \n ".into(), "").validate().is_err());
    }

    #[test]
    fn editing_a_legacy_link_keeps_both_the_address_and_notes() {
        let mut data = Data::default();
        data.apply(&Change::Save {
            id: None,
            draft: draft(Kind::Paper),
        })
        .unwrap();
        let entry = &data.entries[0];
        let content = entry.saved_content();
        let draft = Draft::saved(content.clone(), &entry.title);
        data.apply(&Change::Save { id: Some(1), draft }).unwrap();
        assert_eq!(data.entries[0].kind, Kind::Text);
        assert_eq!(data.entries[0].copy_text(), content);
        assert!(content.contains("https://arxiv.org/abs/1234.5678\n\n第一行\n第二行 123456"));
        data.apply(&Change::ReadLater(1)).unwrap();
        assert_eq!(data.entries[1].body, content);
        assert_eq!(data.entries[1].url, "https://arxiv.org/abs/1234.5678");
    }

    #[test]
    fn edits_roundtrip_and_reading_todos_keep_their_link_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("organizer.json");
        let mut store = Store::load(path.clone()).unwrap();
        store
            .apply(&Change::Save {
                id: None,
                draft: draft(Kind::Paper),
            })
            .unwrap();
        store.apply(&Change::ReadLater(1)).unwrap();
        store.apply(&Change::Done(2, true)).unwrap();
        store.apply(&Change::ReadLater(1)).unwrap();
        let mut store = Store::load(path).unwrap();
        assert_eq!(store.data.entries.len(), 2);
        assert!(!store.data.entries[1].done);
        assert_eq!(store.data.entries[1].url, store.data.entries[0].url);
        let entry = store.data.entries[0].clone();
        store.apply(&Change::Delete(1)).unwrap();
        store.apply(&Change::Restore(entry.clone())).unwrap();
        assert!(store.data.entries.contains(&entry));
        assert!(entry.matches("第二 123456"));
    }
    #[test]
    fn failed_write_does_not_change_memory_and_bad_files_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"broken").unwrap();
        assert!(Store::load(path.clone()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        let mut store = Store {
            path: dir.path().to_path_buf(),
            data: Data::default(),
        };
        assert!(
            store
                .apply(&Change::Save {
                    id: None,
                    draft: draft(Kind::Text)
                })
                .is_err()
        );
        assert!(store.data.entries.is_empty());
        assert_eq!(store.data.next_id, 1);
    }
    #[test]
    fn rejects_unsafe_links_and_preserves_exact_copy_text() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:secret@example.org",
            "not a link",
        ] {
            assert!(web_url(url).is_err());
        }
        let mut data = Data::default();
        data.apply(&Change::Save {
            id: None,
            draft: draft(Kind::Text),
        })
        .unwrap();
        assert_eq!(data.entries[0].copy_text(), "第一行\n第二行 123456");
    }
    #[test]
    fn worker_serializes_and_rejects_edits_after_load_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");
        std::fs::write(&path, "oops").unwrap();
        let (tx, rx) = start(path.clone()).unwrap();
        assert!(rx.recv_blocking().unwrap().result.is_err());
        tx.send(Command::Edit(Change::Save {
            id: None,
            draft: draft(Kind::Text),
        }))
        .unwrap();
        assert!(rx.recv_blocking().unwrap().result.is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "oops");
    }
}
