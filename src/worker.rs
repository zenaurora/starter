use async_channel::{Receiver, Sender};
use starter::{
    catalog::{self, Catalog},
    config,
    search::{self, Candidate, ContentSummary, FileIndex, Mode, Query},
    uninstall,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub enum Command {
    /// Re-scan applications only. Cheap enough to run every time the window is
    /// shown, and it deliberately leaves the cached file index warm.
    Apps { generation: u64 },
    /// Re-scan applications *and* drop the cached file index so the next `/f`
    /// search rebuilds it. Reserved for the explicit tray-menu refresh.
    Refresh { generation: u64 },
    Search {
        generation: u64,
        query: Query,
        roots: Vec<PathBuf>,
        cancelled: Arc<AtomicBool>,
    },
}

pub enum Event {
    Apps {
        generation: u64,
        catalog: Catalog,
        uninstall_targets: Vec<uninstall::Target>,
    },
    Results {
        generation: u64,
        candidates: Vec<Candidate>,
    },
    Done {
        generation: u64,
        status: String,
    },
}

/// One worker owns the file-name cache and serializes disk work. Typing never
/// creates an OS thread per query; superseded requests are canceled before scanning.
pub fn start() -> (Sender<Command>, Receiver<Event>) {
    let (sender, commands) = async_channel::bounded(8);
    let (events, receiver) = async_channel::bounded(32);
    thread::Builder::new()
        .name("starter-search".into())
        .spawn(move || {
            let mut cached_roots: Option<Vec<PathBuf>> = None;
            let mut index = FileIndex::default();
            while let Ok(command) = commands.recv_blocking() {
                match command {
                    command @ (Command::Apps { generation } | Command::Refresh { generation }) => {
                        // Rebuilding the index costs far more than re-scanning the
                        // application folders, so only an explicit refresh drops it.
                        if matches!(command, Command::Refresh { .. }) {
                            cached_roots = None;
                            index = FileIndex::default();
                        }
                        let catalog = catalog::discover();
                        let uninstall_targets = uninstall::discover(&catalog);
                        if events
                            .send_blocking(Event::Apps {
                                generation,
                                catalog,
                                uninstall_targets,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Command::Search {
                        generation,
                        query,
                        roots,
                        cancelled,
                    } => {
                        // Content scans read files on every query, so debounce them.
                        // Filename ranking uses the warm index without a 50 ms delay.
                        if cancelled.load(Ordering::Relaxed) {
                            // 如果被取消了，直接跳过本次循环
                            continue;
                        }
                        if query.mode == Mode::Content {
                            // 文件内容搜索是很慢的，所以需要等一等
                            thread::sleep(Duration::from_millis(50));
                        }
                        if cancelled.load(Ordering::Relaxed) {
                            continue;
                        }
                        let roots: Vec<_> =
                            roots.iter().map(|root| config::expand_home(root)).collect();
                        if roots.is_empty() {
                            let _ = events.send_blocking(Event::Done {
                                generation,
                                status: "请在配置文件中设置 search_roots，然后重新加载配置".into(),
                            });
                            continue;
                        }
                        match query.mode {
                            Mode::Files => {
                                if cached_roots.as_ref() != Some(&roots) {
                                    let next = search::file_index(&roots, &cancelled);
                                    if cancelled.load(Ordering::Relaxed) {
                                        continue;
                                    }
                                    index = next;
                                    cached_roots = Some(roots);
                                }
                                let Some(candidates) = index.search(&query.text, &cancelled) else {
                                    continue;
                                };
                                if cancelled.load(Ordering::Relaxed) {
                                    continue;
                                }
                                let status = format!(
                                    "已扫描 {} 个文件与目录{}{}",
                                    index.len(),
                                    if index.truncated {
                                        "，已达 100,000 项上限"
                                    } else {
                                        ""
                                    },
                                    if index.skipped > 0 {
                                        format!("，跳过 {} 项", index.skipped)
                                    } else {
                                        String::new()
                                    }
                                );
                                if events
                                    .send_blocking(Event::Results {
                                        generation,
                                        candidates,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                                if events
                                    .send_blocking(Event::Done { generation, status })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Mode::Content => {
                                if query.text.is_empty() {
                                    let _ = events.send_blocking(Event::Done {
                                        generation,
                                        status:
                                            "输入要查找的文本；支持纯文本文件，不解析 PDF 或 Office"
                                                .into(),
                                    });
                                    continue;
                                }
                                let result = search::content_search(
                                    &roots,
                                    &query.text,
                                    &cancelled,
                                    |candidates| {
                                        events
                                            .send_blocking(Event::Results {
                                                generation,
                                                candidates,
                                            })
                                            .is_ok()
                                    },
                                );
                                if cancelled.load(Ordering::Relaxed) {
                                    continue;
                                }
                                let status = match result {
                                    Ok(ContentSummary { skipped, limited }) => format!(
                                        "搜索完成{}{}",
                                        if limited {
                                            "，最多显示 100 条命中"
                                        } else {
                                            ""
                                        },
                                        if skipped > 0 {
                                            format!(
                                                "，跳过 {skipped} 项（二进制、编码、大小或权限）"
                                            )
                                        } else {
                                            String::new()
                                        }
                                    ),
                                    Err(error) => format!("搜索失败：{error:#}"),
                                };
                                if events
                                    .send_blocking(Event::Done { generation, status })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Mode::Apps
                            | Mode::Uninstall
                            | Mode::Clipboard
                            | Mode::System
                            | Mode::Reminders
                            | Mode::Monitor => {}
                        }
                    }
                }
            }
        })
        .expect("Cannot start the search worker");
    (sender, receiver)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search_files(
        sender: &Sender<Command>,
        events: &Receiver<Event>,
        root: &std::path::Path,
        generation: u64,
    ) -> Vec<Candidate> {
        sender
            .send_blocking(Command::Search {
                generation,
                query: Query::parse("/f"),
                roots: vec![root.to_owned()],
                cancelled: Arc::new(AtomicBool::new(false)),
            })
            .unwrap();
        let mut results = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match events.try_recv() {
                Ok(Event::Results {
                    generation: received,
                    candidates,
                }) if received == generation => results.extend(candidates),
                Ok(Event::Done {
                    generation: received,
                    ..
                }) if received == generation => return results,
                Ok(_) => {}
                Err(async_channel::TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "worker did not finish"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("worker stopped: {error}"),
            }
        }
    }

    #[test]
    fn app_scans_keep_filename_cache_and_explicit_refresh_rebuilds_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("alpha.txt"), "").unwrap();
        let (sender, events) = start();
        let first = search_files(&sender, &events, root.path(), 1);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].title, "alpha.txt");
        std::fs::write(root.path().join("beta.txt"), "").unwrap();
        sender
            .send_blocking(Command::Apps { generation: 1 })
            .unwrap();
        assert_eq!(search_files(&sender, &events, root.path(), 2).len(), 1);
        sender
            .send_blocking(Command::Refresh { generation: 2 })
            .unwrap();
        assert_eq!(search_files(&sender, &events, root.path(), 3).len(), 2);
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("gamma.txt"), "").unwrap();
        let changed = search_files(&sender, &events, other.path(), 4);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].title, "gamma.txt");
    }
}
