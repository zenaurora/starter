use async_channel::{Receiver, Sender};
use starter::{
    catalog::{self, Catalog},
    config,
    search::{self, Candidate, ContentSummary, FileIndex, Mode, Query},
    uninstall,
};
use std::{
    collections::BTreeMap,
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
                    Command::Apps { generation } => {
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
                    Command::Refresh { generation } => {
                        // Rebuilding the index costs far more than re-scanning the
                        // application folders, so only an explicit refresh drops it.
                        cached_roots = None;
                        index = FileIndex::default();
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
                        // Debounce disk work; the UI sets this token immediately on any edit.
                        if cancelled.load(Ordering::Relaxed) {
                            continue;
                        }
                        thread::sleep(Duration::from_millis(50));
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
                                let Some(candidates) = search::rank_cancellable(
                                    &index.candidates,
                                    &query.text,
                                    &BTreeMap::new(),
                                    &cancelled,
                                ) else {
                                    continue;
                                };
                                if cancelled.load(Ordering::Relaxed) {
                                    continue;
                                }
                                let status = format!(
                                    "已扫描 {} 个文件与目录{}{}",
                                    index.candidates.len(),
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
                            | Mode::Reminders => {}
                        }
                    }
                }
            }
        })
        .expect("Cannot start the search worker");
    (sender, receiver)
}
