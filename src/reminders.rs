//! Local reminders: validated scheduling, atomic storage, overdue recovery and
//! notification delivery behind one command/event interface.
use crate::search::{Candidate, Kind};
use anyhow::{Context, Result, ensure};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf, time::Duration};

mod notifications;
pub mod time;

const LIMIT: usize = 500;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub title: String,
    pub due_at: i64,
}
impl Draft {
    pub fn new(title: &str, due_at: i64) -> Result<Self> {
        let title = title.trim();
        ensure!(!title.is_empty(), "请输入提醒事项");
        ensure!(title.chars().count() <= 200, "提醒事项最多 200 个字");
        ensure!(
            !title.chars().any(char::is_control),
            "提醒事项不能包含换行或控制字符"
        );
        ensure!(
            chrono::DateTime::from_timestamp(due_at, 0).is_some(),
            "提醒时间无效"
        );
        Ok(Self {
            title: title.into(),
            due_at,
        })
    }
    pub fn candidate(&self) -> Candidate {
        Candidate {
            id: "reminder:create".into(),
            title: format!("创建提醒：{}", self.title),
            detail: format!("{} · Enter 保存", time::display(self.due_at)),
            path: PathBuf::new(),
            kind: Kind::ReminderDraft,
            aliases: vec![],
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum State {
    Scheduled,
    Due,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub due_at: i64,
    pub state: State,
}
impl Entry {
    pub fn candidate(&self) -> Candidate {
        Candidate {
            id: self.id.clone(),
            title: self.title.clone(),
            detail: format!(
                "{} · {}",
                time::display(self.due_at),
                if self.state == State::Due {
                    "已到时间 · Enter 处理"
                } else {
                    "Enter 查看或取消"
                }
            ),
            path: PathBuf::new(),
            kind: Kind::Reminder,
            aliases: vec![time::display(self.due_at)],
        }
    }
}
#[derive(Clone, Debug)]
pub enum Command {
    Create(Draft),
    Cancel(String),
    Done(String),
    Snooze(String),
}
#[derive(Debug)]
pub enum Event {
    Snapshot(Vec<Entry>),
    Saved(Draft),
    Applied,
    Failed(String),
    NotificationWarning(String),
}

struct Store {
    path: PathBuf,
    entries: Vec<Entry>,
}
impl Store {
    fn open(path: PathBuf) -> Result<Self> {
        let entries = if path.exists() {
            ensure!(fs::metadata(&path)?.len() < 1024 * 1024, "提醒记录过大");
            let entries: Vec<Entry> =
                serde_json::from_slice(&fs::read(&path)?).context("提醒记录损坏，原文件已保留")?;
            ensure!(entries.len() <= LIMIT, "提醒数量超过 {LIMIT}");
            let mut ids = std::collections::HashSet::new();
            for entry in &entries {
                Draft::new(&entry.title, entry.due_at)?;
                ensure!(
                    entry.id.starts_with("reminder:") && ids.insert(&entry.id),
                    "提醒编号无效或重复"
                );
            }
            entries
        } else {
            Vec::new()
        };
        Ok(Self { path, entries })
    }
    fn save(&mut self, mut entries: Vec<Entry>) -> Result<()> {
        entries.sort_by_key(|e| (e.state != State::Due, e.due_at));
        let parent = self.path.parent().context("提醒存储路径无效")?;
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&serde_json::to_vec(&entries)?)?;
        file.as_file().sync_all()?;
        file.persist(&self.path).context("提醒记录保存失败")?;
        self.entries = entries;
        Ok(())
    }
    fn apply(&mut self, command: &Command, now: i64) -> Result<()> {
        let mut entries = self.entries.clone();
        match command {
            Command::Create(draft) => {
                Draft::new(&draft.title, draft.due_at)?;
                ensure!(draft.due_at > now, "提醒时间已经过去，请重新设置");
                ensure!(draft.due_at - now <= 366 * 86400, "请选择 366 天以内的提醒");
                ensure!(
                    entries.len() < LIMIT,
                    "最多保存 {LIMIT} 条提醒，请先完成或取消一些提醒"
                );
                // Collision-free within the store, including rapid creates.
                let base = format!("reminder:{}", Local::now().timestamp_micros());
                let mut id = base.clone();
                let mut suffix = 0;
                while entries.iter().any(|e| e.id == id) {
                    suffix += 1;
                    id = format!("{base}-{suffix}");
                }
                entries.push(Entry {
                    id,
                    title: draft.title.clone(),
                    due_at: draft.due_at,
                    state: State::Scheduled,
                });
            }
            Command::Cancel(id) | Command::Done(id) => {
                ensure!(entries.iter().any(|e| &e.id == id), "这条提醒已经移除");
                entries.retain(|e| &e.id != id);
            }
            Command::Snooze(id) => {
                let entry = entries
                    .iter_mut()
                    .find(|e| &e.id == id)
                    .context("这条提醒已经移除")?;
                entry.due_at = now + 5 * 60;
                entry.state = State::Scheduled;
            }
        }
        self.save(entries)
    }
    fn tick(&mut self, now: i64) -> Result<Vec<Entry>> {
        let due: Vec<_> = self
            .entries
            .iter()
            .filter(|e| e.state == State::Scheduled && e.due_at <= now)
            .cloned()
            .collect();
        if !due.is_empty() {
            let mut entries = self.entries.clone();
            for entry in &mut entries {
                if entry.state == State::Scheduled && entry.due_at <= now {
                    entry.state = State::Due;
                }
            }
            // Persist before delivery. A relaunch must not deliver the same due
            // notification repeatedly; due items remain visible until handled.
            self.save(entries)?;
        }
        Ok(due)
    }
}

pub fn start(
    path: PathBuf,
) -> (
    async_channel::Sender<Command>,
    async_channel::Receiver<Event>,
) {
    let (sender, commands) = async_channel::unbounded();
    let (events, receiver) = async_channel::unbounded();
    std::thread::spawn(move || {
        let mut store = match Store::open(path) {
            Ok(store) => store,
            Err(error) => {
                let _ = events.send_blocking(Event::Failed(format!(
                    "提醒读取失败：{error:#}。请检查配置目录中的 reminders.json，修复后重启。"
                )));
                return;
            }
        };
        let (notifications, notification_receiver) = std::sync::mpsc::channel();
        let notification_events = events.clone();
        std::thread::spawn(move || {
            while let Ok(notification) = notification_receiver.recv() {
                let result = match notification {
                    Notification::Authorize => notifications::authorize(),
                    Notification::Deliver(entry) => notifications::deliver(&entry),
                };
                if let Err(error) = result {
                    let _ = notification_events.send_blocking(Event::NotificationWarning(format!(
                        "{error:#}。到时提醒仍保留在 Starter 的提醒列表中。"
                    )));
                }
            }
        });
        let _ = events.send_blocking(Event::Snapshot(store.entries.clone()));
        let mut authorization_requested = false;
        let mut tick_failed = false;
        loop {
            if events.is_closed() || commands.is_closed() {
                break;
            }
            while let Ok(command) = commands.try_recv() {
                match store.apply(&command, Local::now().timestamp()) {
                    Ok(()) => {
                        let _ = events.send_blocking(Event::Snapshot(store.entries.clone()));
                        if let Command::Create(draft) = command {
                            if !authorization_requested {
                                let _ = notifications.send(Notification::Authorize);
                                authorization_requested = true;
                            }
                            let _ = events.send_blocking(Event::Saved(draft));
                        } else {
                            let _ = events.send_blocking(Event::Applied);
                        }
                    }
                    Err(error) => {
                        let _ = events.send_blocking(Event::Failed(format!("{error:#}")));
                    }
                }
            }
            match store.tick(Local::now().timestamp()) {
                Ok(due) => {
                    tick_failed = false;
                    if !due.is_empty() {
                        let _ = events.send_blocking(Event::Snapshot(store.entries.clone()));
                        for entry in due {
                            let _ = notifications.send(Notification::Deliver(entry));
                        }
                    }
                }
                Err(error) if !tick_failed => {
                    tick_failed = true;
                    let _ =
                        events.send_blocking(Event::Failed(format!("到时提醒保存失败：{error:#}")));
                }
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    (sender, receiver)
}

enum Notification {
    Authorize,
    Deliver(Entry),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_and_sleep_recover_overdue_without_duplicate_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("reminders.json");
        let mut store = Store::open(path.clone()).unwrap();
        store
            .apply(&Command::Create(Draft::new("开会", 200).unwrap()), 100)
            .unwrap();
        assert!(store.tick(199).unwrap().is_empty());
        drop(store);
        let mut store = Store::open(path.clone()).unwrap();
        assert_eq!(store.tick(500).unwrap().len(), 1);
        let id = store.entries[0].id.clone();
        assert_eq!(Store::open(path).unwrap().entries[0].state, State::Due);
        assert!(store.tick(501).unwrap().is_empty());
        store.apply(&Command::Snooze(id.clone()), 501).unwrap();
        assert_eq!(store.entries[0].due_at, 801);
        assert_eq!(store.tick(801).unwrap().len(), 1);
        store.apply(&Command::Done(id), 802).unwrap();
        assert!(store.entries.is_empty());
    }
    #[test]
    fn invalid_or_failed_writes_do_not_mutate_state() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("reminders.json");
        let mut store = Store::open(path.clone()).unwrap();
        assert!(
            store
                .apply(&Command::Create(Draft::new("开会", 90).unwrap()), 100)
                .is_err()
        );
        assert!(Draft::new(" ", 200).is_err());
        assert!(Draft::new("a\nb", 200).is_err());
        store
            .apply(&Command::Create(Draft::new("开会", 200).unwrap()), 100)
            .unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let before = store.entries.clone();
        assert!(
            store
                .apply(&Command::Cancel(before[0].id.clone()), 100)
                .is_err()
        );
        assert_eq!(store.entries, before);
    }
    #[test]
    fn corrupted_records_are_preserved_and_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("reminders.json");
        fs::write(&path, b"broken").unwrap();
        assert!(Store::open(path.clone()).is_err());
        assert_eq!(fs::read(path).unwrap(), b"broken");
    }
}
