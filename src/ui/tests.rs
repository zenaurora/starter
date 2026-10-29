use super::*;
use core::prelude::v1::test;
use gpui_kit::{InputEvent, base::Root, test::TestWindowExt};

fn fixture(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<Launcher>) {
    cx.update(gpui_kit::init);
    let (window, launcher) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(720.), px(560.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let input = cx.new(|cx| InputState::new(window, cx));
                    input.update(cx, |input, cx| input.focus(window, cx));
                    let config = Config {
                        auto_check_updates: false,
                        ..Config::default()
                    };
                    appearance::apply(config.theme, &config, window, cx);
                    let (worker, _) = async_channel::unbounded();
                    let (reminder_worker, _) = async_channel::unbounded();
                    let (clipboard_worker, _) = async_channel::unbounded();
                    let results = (0..3)
                        .map(|i| Candidate {
                            id: format!("file-{i}"),
                            title: format!("File {i}"),
                            detail: String::new(),
                            path: PathBuf::from(format!("/file-{i}")),
                            kind: Kind::File,
                            aliases: Vec::new(),
                        })
                        .collect();
                    Launcher {
                        input,
                        settings: None,
                        settings_subscription: None,
                        workbench: None,
                        workbench_open: false,
                        workbench_subscription: None,
                        #[cfg(target_os = "macos")]
                        software_panel: None,
                        #[cfg(target_os = "macos")]
                        software_inventory: Default::default(),
                        #[cfg(target_os = "macos")]
                        software_subscription: None,
                        error_dialog: None,
                        error_dialog_subscription: None,
                        icons: Icons::new().0,
                        config,
                        config_path: std::env::temp_dir().join("starter-ui-test-config.toml"),
                        shell: None,
                        history: History::empty(
                            std::env::temp_dir().join("starter-ui-test-history.json"),
                        ),
                        catalog: Catalog::default(),
                        clipboard_entries: vec![],
                        clipboard_root: std::env::temp_dir().join("starter-ui-test-clipboard"),
                        clipboard_preview: None,
                        clipboard_worker,
                        clipboard_restore_pending: false,
                        clipboard_hide_after_restore: false,
                        clipboard_error: None,
                        uninstall_targets: vec![],
                        uninstall_confirmation: None,
                        uninstall_busy: false,
                        #[cfg(target_os = "macos")]
                        uninstall_task: None,
                        #[cfg(target_os = "macos")]
                        uninstall_task_subscription: None,
                        power_confirmation: None,
                        power_busy: false,
                        reminder_entries: vec![],
                        reminder_worker,
                        reminder_panel: None,
                        reminder_subscription: None,
                        reminder_draft: None,
                        reminder_pending: false,
                        reminder_feedback: None,
                        notification_warning: None,
                        monitor_snapshot: None,
                        monitor_ranked: Arc::from([]),
                        monitor_worker: None,
                        monitor_sampling: false,
                        monitor_sort: monitor::Sort::default(),
                        monitor_selected: None,
                        monitor_scroll: UniformListScrollHandle::new(),
                        monitor_actions: None,
                        dialog_focus: cx.focus_handle(),
                        candidates: Vec::new(),
                        results,
                        selected: 0,
                        query: Query::parse(""),
                        generation: 0,
                        catalog_generation: 0,
                        worker,
                        cancelled: Arc::new(AtomicBool::new(false)),
                        scroll: UniformListScrollHandle::new(),
                        status: String::new(),
                        visible: true,
                        activation_generation: 0,
                        searching: false,
                        update_status: UpdateStatus::Idle,
                        update_release: None,
                        update_cancelled: Arc::new(AtomicBool::new(false)),
                        _subscriptions: vec![Launcher::observe_activation(window, cx)],
                        _tasks: Vec::new(),
                    }
                })
            },
        )
        .unwrap()
    });
    (window.downcast().unwrap(), launcher)
}

#[gpui_kit::test]
fn slash_lists_component_entries_without_searching_apps(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    let (worker, commands) = async_channel::bounded(8);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.worker = worker;
            this.candidates.push(Candidate {
                id: "app-that-matches-slash".into(),
                title: "/".into(),
                detail: String::new(),
                path: PathBuf::from("/app"),
                kind: Kind::App,
                aliases: vec![],
            });
            this.input
                .update(cx, |input, cx| input.set_value("/", window, cx));
            this.search(cx);
            for id in [
                "command:saved",
                "command:todos",
                "command:ports",
                "command:monitor",
                "command:reminder",
                "command:clipboard",
                "command:files",
                "command:content",
                "command:uninstall",
                "command:system",
                "command:preferences",
            ] {
                assert!(
                    this.results.iter().any(|candidate| candidate.id == id),
                    "typing / must list {id}; got {:?}",
                    this.results.iter().map(|c| &c.id).collect::<Vec<_>>()
                );
            }
            #[cfg(target_os = "macos")]
            assert!(
                this.results
                    .iter()
                    .any(|c| c.id == "command:software-updates")
            );
            assert!(
                this.results
                    .iter()
                    .all(|c| system_commands::resolve(&c.id).is_some())
            );
            assert!(!this.searching);
            assert!(!this.monitor_sampling);
            assert!(
                commands.try_recv().is_err(),
                "listing commands must not scan files"
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(("command-shortcut", 0usize)).visible());
        let row = window.find(("result", 0usize)).bounds();
        let shortcut = window.find(("command-shortcut", 0usize)).bounds();
        assert!(shortcut.right() <= row.right() && shortcut.bottom() <= row.bottom());
    })
    .unwrap();
}

#[gpui_kit::test]
fn slash_filtering_and_enter_open_the_selected_mode(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    let (worker, _commands) = async_channel::bounded(8);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.worker = worker;
            this._subscriptions.push(cx.subscribe_in(
                &this.input,
                window,
                |this, _, event, window, cx| {
                    if let gpui_kit::component::input::InputEvent::PressEnter { .. } = event {
                        this.open_selected(window, cx);
                    }
                },
            ));
            for (query, action) in [
                ("/mon", BuiltinAction::Monitor),
                ("/待办", BuiltinAction::Todos),
                ("/cli", BuiltinAction::Search(Mode::Clipboard)),
            ] {
                this.input
                    .update(cx, |input, cx| input.set_value(query, window, cx));
                this.search(cx);
                assert_eq!(system_commands::resolve(&this.results[0].id), Some(action));
                assert!(!this.searching);
            }
        });
    })
    .unwrap();
    for mode in [
        Mode::Clipboard,
        Mode::Files,
        Mode::Content,
        Mode::Uninstall,
        Mode::System,
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            launcher.update(cx, |this, cx| {
                this.input
                    .update(cx, |input, cx| input.set_value("/", window, cx));
                this.search(cx);
                this.selected = this
                    .results
                    .iter()
                    .position(|c| {
                        system_commands::resolve(&c.id) == Some(BuiltinAction::Search(mode))
                    })
                    .unwrap();
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert_eq!(launcher.read(cx).query.mode, mode);
            assert_eq!(launcher.read(cx).input.read(cx).value(), mode.prefix());
            assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            launcher.read_with(cx, |this, _| this.query.mode),
            Mode::Apps
        );
    }
}

#[gpui_kit::test]
fn workbench_search_opens_with_seed_and_escape_returns_to_launcher(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    let dir = tempfile::tempdir().unwrap();
    for (query, expected) in [
        ("/save 发票", BuiltinAction::Saved),
        ("/todo 读一篇论文", BuiltinAction::Todos),
        ("/ports 3000", BuiltinAction::Ports),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            launcher.update(cx, |this, cx| {
                this.config_path = dir.path().join("config.toml");
                this.input
                    .update(cx, |s, cx| s.set_value(query, window, cx));
                this.search(cx);
                assert_eq!(this.results.len(), 1);
                assert_eq!(
                    system_commands::resolve(&this.results[0].id),
                    Some(expected)
                );
                if expected != BuiltinAction::Ports {
                    this.open_selected(window, cx);
                    assert!(this.workbench_open);
                    assert!(!this.monitor_sampling);
                    assert!(!this.input.focus_handle(cx).is_focused(window));
                }
            });
        })
        .unwrap();
        if expected == BuiltinAction::Ports {
            continue;
        }
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("workbench-back").visible());
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!launcher.read(cx).workbench_open);
            assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
        })
        .unwrap();
    }
}

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn uninstall_task_preserves_focus_keys_output_and_reopen_state(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    let (panel, events) = cx
        .update_window(handle.into(), |_, window, cx| {
            let mut events = None;
            let panel = cx.new(|cx| {
                let (panel, sender) =
                    uninstall_task::TaskPanel::fixture(config::ThemeName::Catppuccin, window, cx);
                events = Some(sender);
                panel
            });
            launcher.update(cx, |this, cx| {
                this.input.update(cx, |input, cx| {
                    input.set_value("/uninstall Fixture", window, cx)
                });
                this.uninstall_task = Some(panel.clone());
                cx.notify();
            });
            (panel, events.unwrap())
        })
        .unwrap();
    let terminal = panel.read_with(cx, |panel, _| panel.terminal());
    let visual = VisualTestContext::from_window(handle.into(), cx).into_mut();
    events
        .try_send(starter::task_terminal::Event::Output(
            b"preview files\r\nEnter confirm, ESC cancel".to_vec(),
        ))
        .unwrap();
    visual.run_until_parked();
    visual
        .update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert!(terminal.focus_handle(cx).is_focused(window));
            assert!(!launcher.read(cx).input.focus_handle(cx).is_focused(window));
            for key in ["down", "up", "escape", "enter", "ctrl-c"] {
                window.press(key, cx);
            }
            assert_eq!(launcher.read(cx).selected, 0);
            assert!(launcher.read(cx).uninstall_task.is_some());
            assert_eq!(
                launcher.read(cx).input.read(cx).value(),
                "/uninstall Fixture"
            );
        })
        .unwrap();
    visual.run_until_parked();
    let bounds = visual.debug_bounds("task-terminal").unwrap();
    assert!(
        bounds.size.height >= px(260.),
        "terminal needs readable space: {bounds:?}"
    );
    assert!(bounds.right() <= px(720.) && bounds.bottom() < px(560.));
    visual.deactivate_window();
    visual
        .update_window(handle.into(), |_, window, cx| {
            launcher.update(cx, |this, cx| {
                this.visible = false;
                window.blur(cx);
                this.show(window, cx);
                assert_eq!(
                    this.uninstall_task.as_ref().unwrap().entity_id(),
                    panel.entity_id()
                );
                assert_eq!(this.input.read(cx).value(), "/uninstall Fixture");
            });
        })
        .unwrap();
    visual.run_until_parked();
    visual
        .update_window(handle.into(), |_, window, cx| {
            assert!(terminal.focus_handle(cx).is_focused(window));
            launcher.update(cx, |this, cx| this.open_settings(window, cx));
            assert!(launcher.read(cx).settings.is_none());
        })
        .unwrap();
    assert!(visual.debug_bounds("stop-uninstall-task").unwrap().bottom() <= px(560.));
    events
        .try_send(starter::task_terminal::Event::Exit {
            code: 0,
            stopped: false,
        })
        .unwrap();
    visual.run_until_parked();
    visual
        .update_window(handle.into(), |_, window, cx| {
            assert!(panel.read(cx).can_close());
            window.press("escape", cx);
            assert!(launcher.read(cx).uninstall_task.is_none());
            assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
        })
        .unwrap();
}

#[gpui_kit::test]
fn activation_restores_search_focus_and_arrow_navigation(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    // A platform activation can arrive after showing the popup. Reproduce a
    // missing editor focus at that boundary, then send real keyboard events.
    let visual = VisualTestContext::from_window(handle.into(), cx).into_mut();
    visual.deactivate_window();
    visual
        .update_window(handle.into(), |_, window, cx| window.blur(cx))
        .unwrap();
    visual
        .update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    visual.run_until_parked();
    visual
        .update_window(handle.into(), |_, window, cx| {
            assert!(
                launcher.read(cx).input.focus_handle(cx).is_focused(window),
                "search input must regain focus on activation"
            );
            window.press("down", cx);
        })
        .unwrap();
    visual.run_until_parked();
    visual
        .update_window(handle.into(), |_, window, cx| {
            assert_eq!(launcher.read(cx).selected, 1);
            window.press("up", cx);
        })
        .unwrap();
    visual.run_until_parked();
    visual
        .update_window(handle.into(), |_, _, cx| {
            assert_eq!(launcher.read(cx).selected, 0)
        })
        .unwrap();
}

#[gpui_kit::test]
fn repeated_completed_operations_do_not_accumulate_task_handles(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    for operation in 0..100 {
        cx.update_window(handle.into(), |_, window, cx| {
            launcher.update(cx, |this, cx| {
                this.track_task(cx.spawn_in(window, async move |this, cx| {
                    this.update_in(cx, |this, _, _| this.status = operation.to_string())
                        .unwrap();
                }));
            });
        })
        .unwrap();
        cx.run_until_parked();
        launcher.read_with(cx, |this, _| {
            assert_eq!(this.status, operation.to_string());
            assert_eq!(this._tasks.len(), 1);
        });
    }
}

#[gpui_kit::test]
fn late_app_scan_leaves_an_in_flight_file_search_untouched(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    // Reproduce the race: showing the window queues an application scan, then the
    // user immediately starts a file search that is still streaming when the scan
    // lands. The late scan must not disturb the running one.
    cx.update_window(handle.into(), |_, _, cx| {
        launcher.update(cx, |this, cx| {
            this.query = Query::parse("/f report");
            this.generation = 7;
            this.searching = true;
            this.results = vec![Candidate {
                id: "/report.txt".into(),
                title: "report.txt".into(),
                detail: String::new(),
                path: PathBuf::from("/report.txt"),
                kind: Kind::File,
                aliases: Vec::new(),
            }];
            let generation = this.catalog_generation;
            this.worker_event(
                worker::Event::Apps {
                    generation,
                    catalog: Catalog::default(),
                    uninstall_targets: vec![],
                },
                cx,
            );
            assert_eq!(
                this.generation, 7,
                "must not cancel the running disk search"
            );
            assert!(
                this.searching,
                "the file search must still be marked running"
            );
            assert_eq!(this.results.len(), 1, "streamed hits must survive");
        })
    })
    .unwrap();
}

#[gpui_kit::test]
fn app_scan_refreshes_the_app_list(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    // The other half: in Apps mode the scan must re-rank so newly installed apps appear.
    cx.update_window(handle.into(), |_, _, cx| {
        launcher.update(cx, |this, cx| {
            let generation = this.catalog_generation;
            this.worker_event(
                worker::Event::Apps {
                    generation,
                    catalog: Catalog::default(),
                    uninstall_targets: vec![],
                },
                cx,
            );
            assert_eq!(this.status, "0 个应用", "Apps mode must re-rank");
        })
    })
    .unwrap();
}

#[gpui_kit::test]
fn settings_tab_switch_preserves_escape_and_returns_search_focus(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |launcher, cx| launcher.open_settings(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("更新", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(launcher.read(cx).settings.is_some());
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(launcher.read(cx).settings.is_none());
        assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn update_progress_prevents_parallel_checks_and_survives_settings_close(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.set_update_status(
                UpdateStatus::Downloading {
                    received: 10,
                    total: 100,
                },
                cx,
            );
            this.check_updates(window, cx);
            assert!(matches!(
                this.update_status,
                UpdateStatus::Downloading { .. }
            ));
            this.open_settings(window, cx);
            assert_eq!(
                this.settings.as_ref().unwrap().read(cx).update_status,
                this.update_status
            );
            this.close_settings(window, cx);
            assert!(this.update_status.busy());
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_closes_key_dropdown_before_settings_and_cancel_preserves_keys(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    let original = launcher.read_with(cx, |this, _| this.config.launcher_hotkey.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| this.open_settings(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("edit-launcher-hotkey", cx);
        window
            .within("呼出快捷键")
            .within(("key-slot", 0usize))
            .click("input", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(
            launcher.read(cx).settings.is_some(),
            "Esc must first dismiss the dropdown"
        );
        assert_eq!(
            window
                .within("呼出快捷键")
                .find(("key-slot", 0usize))
                .expanded(),
            Some(false)
        );
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, _, cx| {
        assert!(launcher.read(cx).settings.is_none());
        assert_eq!(launcher.read(cx).config.launcher_hotkey, original);
    })
    .unwrap();
}

#[gpui_kit::test]
fn clipboard_and_uninstall_modes_do_not_require_disk_search_roots(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.input
                .update(cx, |input, cx| input.set_value("/clip 文本", window, cx));
            this.search(cx);
            assert_eq!(this.query.mode, Mode::Clipboard);
            assert!(!this.searching);
            assert!(this.status.contains("条历史"));
        });
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(launcher.read(cx).input.read(cx).value(), "/clip ");
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(launcher.read(cx).query.mode, Mode::Apps);
        launcher.update(cx, |this, cx| {
            this.input
                .update(cx, |input, cx| input.set_value("/uninstall", window, cx));
            this.search(cx);
            assert!(!this.searching);
            assert!(this.status.contains("可卸载应用"));
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn clipboard_snapshot_preserves_selection_and_late_app_scan_leaves_it_untouched(
    cx: &mut TestAppContext,
) {
    let (handle, launcher) = fixture(cx);
    let entry = |name: &str| {
        serde_json::from_value::<clipboard::Entry>(serde_json::json!({
        "id":name, "title":name, "copied_at":0, "bytes":10, "payload":{"Text":name}, "thumbnail":null
    })).unwrap()
    };
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.input
                .update(cx, |input, cx| input.set_value("/clip", window, cx));
            this.search(cx);
            this.clipboard_event(
                clipboard::Event::Snapshot(vec![entry("a"), entry("b")]),
                window,
                cx,
            );
            this.selected = 1;
            this.clipboard_event(
                clipboard::Event::Snapshot(vec![entry("c"), entry("a"), entry("b")]),
                window,
                cx,
            );
            assert_eq!(this.results[this.selected].id, "b");
            let generation = this.generation;
            this.worker_event(
                worker::Event::Apps {
                    generation: this.catalog_generation,
                    catalog: Catalog::default(),
                    uninstall_targets: vec![],
                },
                cx,
            );
            assert_eq!(this.generation, generation);
            assert_eq!(this.results[this.selected].id, "b");
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn default_search_finds_commands_and_restart_confirmation_ignores_plain_enter(
    cx: &mut TestAppContext,
) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            for query in ["bluetooth", "display", "sound", "battery", "锁屏"] {
                this.input
                    .update(cx, |input, cx| input.set_value(query, window, cx));
                this.search(cx);
                assert!(system_commands::resolve(&this.results[0].id).is_some());
                assert!(!this.searching);
            }
            this.input
                .update(cx, |input, cx| input.set_value("Restart", window, cx));
            this.search(cx);
            this.open_selected(window, cx);
            assert_eq!(this.power_confirmation, Some(Power::Restart));
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(launcher.read(cx).power_confirmation, Some(Power::Restart));
        assert!(
            !launcher.read(cx).power_busy,
            "plain Enter must not execute a power operation"
        );
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(launcher.read(cx).power_confirmation.is_none());
        assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn quick_reminder_previews_and_submits_once_without_touching_disk_or_notifications(
    cx: &mut TestAppContext,
) {
    let (handle, launcher) = fixture(cx);
    let (sender, receiver) = async_channel::unbounded();
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.reminder_worker = sender;
            this.input.update(cx, |input, cx| {
                input.set_value("/remind 10m 开会", window, cx)
            });
            this.search(cx);
            assert_eq!(this.query.mode, Mode::Reminders);
            assert!(this.results[0].detail.contains("Enter 保存"));
            assert_eq!(this.reminder_draft.as_ref().unwrap().title, "开会");
            this.open_selected(window, cx);
            this.reminder_event(reminders::Event::Snapshot(Vec::new()), window, cx);
            assert!(
                this.reminder_pending,
                "a background snapshot is not a save acknowledgement"
            );
            this.open_selected(window, cx);
        });
    })
    .unwrap();
    let reminders::Command::Create(draft) = receiver.try_recv().unwrap() else {
        panic!("expected create");
    };
    assert_eq!(draft.title, "开会");
    assert!(
        receiver.try_recv().is_err(),
        "pending saves must not create duplicates"
    );
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.reminder_event(reminders::Event::Saved(draft), window, cx);
            assert!(!this.reminder_pending);
            assert_eq!(this.input.read(cx).value(), "/remind ");
            assert!(this.status.contains("已创建"));
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn reminder_panel_owns_focus_presets_and_escape_returns_to_search(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    let (sender, receiver) = async_channel::unbounded();
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.reminder_worker = sender;
            this.open_reminders("开会", window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(!launcher.read(cx).input.focus_handle(cx).is_focused(window));
        window.click("5 分钟", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    let reminders::Command::Create(draft) = receiver.try_recv().unwrap() else {
        panic!("expected create");
    };
    assert_eq!(draft.title, "开会");
    assert!((295..=301).contains(&(draft.due_at - chrono::Local::now().timestamp())));
    cx.update_window(handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(launcher.read(cx).reminder_panel.is_none());
        assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn mode_navigation_fits_the_launcher_width(cx: &mut TestAppContext) {
    let (handle, _) = fixture(cx);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, _| {
        let mut previous_end = px(0.);
        for id in [
            "全部",
            "文件",
            "内容",
            "剪贴板",
            "卸载",
            "系统",
            "提醒",
            "监控",
        ] {
            let button = window.find(id);
            let bounds = button.bounds();
            assert!(button.visible());
            assert!(
                bounds.origin.x >= previous_end,
                "{id} overlaps the preceding mode"
            );
            previous_end = bounds.origin.x + bounds.size.width;
            assert!(previous_end <= px(720.), "{id} exceeds the launcher width");
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn monitor_filters_sorts_and_samples_only_while_its_panel_is_open(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    let (sender, receiver) = std::sync::mpsc::channel();
    launcher.update(cx, |this, _| this.monitor_worker = Some(sender));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.click("监控", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(receiver.try_recv().unwrap());
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            assert_eq!(this.query.mode, Mode::Monitor);
            assert!(this.results.is_empty());
            this.input.update(cx, |input, cx| {
                input.set_value("/monitor Chrome", window, cx)
            });
            this.search(cx);
            assert_eq!(this.query.text, "Chrome");
            assert!(receiver.try_recv().is_err(), "filtering restarted sampling");
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.click("内存", cx))
        .unwrap();
    assert!(launcher.read_with(cx, |this, _| this.monitor_sort == monitor::Sort::Memory));
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| this.open_settings(window, cx));
    })
    .unwrap();
    assert!(!receiver.try_recv().unwrap());
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| this.close_settings(window, cx));
    })
    .unwrap();
    assert!(receiver.try_recv().unwrap());
    for _ in 0..2 {
        cx.update_window(handle.into(), |_, window, cx| window.press("escape", cx))
            .unwrap();
        cx.run_until_parked();
    }
    assert_eq!(
        launcher.read_with(cx, |this, _| this.query.mode),
        Mode::Apps
    );
    assert!(!receiver.try_recv().unwrap());
}

fn monitor_snapshot() -> monitor::Snapshot {
    monitor::Snapshot {
        sampled_at: std::time::Instant::now(),
        cpu_percent: 25.,
        memory_used: 4 << 30,
        memory_total: 16 << 30,
        disk_used: 100 << 30,
        disk_total: 500 << 30,
        processes: [
            ("Alpha", 30., 100 << 20),
            ("Beta", 20., 300 << 20),
            ("Gamma", 10., 200 << 20),
        ]
        .into_iter()
        .map(|(name, cpu_percent, memory)| {
            let path = PathBuf::from(format!("/Applications/Fixture {name}.app"));
            monitor::ProcessUsage {
                id: path.display().to_string(),
                name: format!("Fixture {name}"),
                path: Some(path.clone()),
                process_count: 1,
                // These fixtures can never pass action identity validation.
                // No UI test may send a signal to a real process.
                members: vec![monitor::ProcessIdentity {
                    pid: u32::MAX,
                    start_time: 0,
                    exe: Some(path.join("Contents/MacOS/fixture")),
                }],
                cpu_percent,
                memory,
            }
        })
        .collect(),
    }
}

#[gpui_kit::test]
fn monitor_selection_tracks_identity_across_live_ranking_sorting_and_filtering(
    cx: &mut TestAppContext,
) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.receive_monitor_snapshot(monitor_snapshot());
            this.open_monitor(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            launcher.read(cx).monitor_selected.as_deref(),
            Some("/Applications/Fixture Alpha.app")
        );
        window.press("down", cx);
        assert_eq!(
            launcher.read(cx).monitor_selected.as_deref(),
            Some("/Applications/Fixture Beta.app")
        );
        launcher.update(cx, |this, cx| {
            let mut snapshot = monitor_snapshot();
            snapshot.processes[0].cpu_percent = 5.;
            snapshot.processes.reverse();
            this.receive_monitor_snapshot(snapshot);
            cx.notify();
        });
        assert_eq!(
            launcher.read(cx).monitor_selected.as_deref(),
            Some("/Applications/Fixture Beta.app"),
            "a new ranking must not move selection to a different process"
        );
        window.click("内存", cx);
        assert_eq!(
            launcher.read(cx).monitor_selected.as_deref(),
            Some("/Applications/Fixture Beta.app")
        );
        window.press("down", cx);
        assert_eq!(
            launcher.read(cx).monitor_selected.as_deref(),
            Some("/Applications/Fixture Gamma.app")
        );
        launcher.update(cx, |this, cx| {
            this.input
                .update(cx, |input, cx| input.set_value("/monitor Beta", window, cx));
            this.search(cx);
        });
        assert_eq!(
            launcher.read(cx).monitor_selected.as_deref(),
            Some("/Applications/Fixture Beta.app")
        );
        window.press("up", cx);
        assert_eq!(
            launcher.read(cx).monitor_selected.as_deref(),
            Some("/Applications/Fixture Beta.app")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn monitor_actions_keep_the_sampled_target_and_escape_preserves_the_filter(
    cx: &mut TestAppContext,
) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.receive_monitor_snapshot(monitor_snapshot());
            this.open_monitor(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("monitor-actions-/Applications/Fixture Alpha.app", cx);
        assert!(launcher.read(cx).monitor_actions.is_some());
        assert!(!launcher.read(cx).input.focus_handle(cx).is_focused(window));
        launcher.update(cx, |this, cx| {
            let mut snapshot = monitor_snapshot();
            snapshot.processes[0].path = Some(PathBuf::from("/Applications/Replaced.app"));
            this.receive_monitor_snapshot(snapshot);
            this.input
                .update(cx, |input, cx| input.set_value("/monitor Beta", window, cx));
            this.search(cx);
        });
        window.click("monitor-copy-path", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("/Applications/Fixture Alpha.app".into()),
            "the action panel must retain the originally sampled target"
        );
        window.press("escape", cx);
        assert!(launcher.read(cx).monitor_actions.is_none());
        assert_eq!(launcher.read(cx).input.read(cx).value(), "/monitor Beta");
        assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
        window.press("enter", cx);
        window.click("monitor-copy-path", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("/Applications/Fixture Beta.app".into())
        );
        window.click("monitor-actions-close", cx);
        assert!(launcher.read(cx).monitor_actions.is_none());
        window.right_click("monitor-actions-/Applications/Fixture Beta.app", cx);
        assert!(launcher.read(cx).monitor_actions.is_some());
        assert_eq!(launcher.read(cx).query.text, "Beta");
    })
    .unwrap();
}

#[gpui_kit::test]
fn monitor_destructive_actions_require_confirmation_and_ignore_plain_or_held_enter(
    cx: &mut TestAppContext,
) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.receive_monitor_snapshot(monitor_snapshot());
            this.open_monitor(window, cx);
            this.input.update(cx, |input, cx| {
                input.set_value("/monitor Alpha", window, cx)
            });
            this.search(cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("enter", cx);
        for action in ["monitor-terminate", "monitor-force-quit"] {
            if cfg!(target_os = "windows") && action == "monitor-terminate" {
                continue;
            }
            let task_count = launcher.read(cx)._tasks.len();
            let status = launcher.read(cx).status.clone();
            window.click(action, cx);
            assert!(window.find("monitor-confirm-action").visible());
            assert!(window.try_find("monitor-copy-path").is_none());
            for _ in 0..8 {
                if window.find("monitor-confirm-action").focused() == Some(true) {
                    break;
                }
                window.press("tab", cx);
            }
            assert_eq!(
                window.find("monitor-confirm-action").focused(),
                Some(true),
                "{action}: close={:?}, cancel={:?}, confirm={:?}",
                window.find("monitor-actions-close"),
                window.find("monitor-cancel-action"),
                window.find("monitor-confirm-action"),
            );
            window.press("enter", cx);
            window.dispatch_event(
                KeyDownEvent {
                    keystroke: Keystroke::parse(if cfg!(target_os = "macos") {
                        "cmd-enter"
                    } else {
                        "ctrl-enter"
                    })
                    .unwrap(),
                    is_held: true,
                    prefer_character_input: false,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(
                launcher.read(cx)._tasks.len(),
                task_count,
                "choosing an action, Enter and a held confirmation shortcut must not execute it"
            );
            assert_eq!(launcher.read(cx).status, status);
            window.press("escape", cx);
            assert!(launcher.read(cx).monitor_actions.is_some());
            assert!(window.find("monitor-copy-path").visible());
        }
        window.press("escape", cx);
        assert!(launcher.read(cx).monitor_actions.is_none());
        assert_eq!(launcher.read(cx).input.read(cx).value(), "/monitor Alpha");
        assert!(launcher.read(cx).visible);
        assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn monitor_confirming_a_stale_target_reports_failure_without_closing_the_panel(
    cx: &mut TestAppContext,
) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.receive_monitor_snapshot(monitor_snapshot());
            this.open_monitor(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("enter", cx);
        window.click("monitor-force-quit", cx);
        window.click("monitor-confirm-action", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let state = launcher.read(cx);
        assert!(state.monitor_actions.is_some());
        assert!(state.status.contains("失败"), "{}", state.status);
        assert!(
            state.status.contains("已退出或身份已变化"),
            "{}",
            state.status
        );
        assert!(state.visible);
        window.click("monitor-cancel-action", cx);
        window.click("monitor-actions-close", cx);
        assert!(launcher.read(cx).monitor_actions.is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn monitor_normal_quit_sends_the_request_without_an_extra_confirmation(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.receive_monitor_snapshot(monitor_snapshot());
            this.open_monitor(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("enter", cx);
        window.click("monitor-quit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let state = launcher.read(cx);
        assert!(state.monitor_actions.is_some());
        assert!(state.status.starts_with("退出失败："), "{}", state.status);
        assert!(
            state.status.contains("已退出或身份已变化"),
            "{}",
            state.status
        );
        assert!(window.find("monitor-force-quit").visible());
        assert!(window.try_find("monitor-confirm-action").is_none());
        window.press("escape", cx);
        assert!(launcher.read(cx).monitor_actions.is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn monitor_actions_support_keyboard_focus_navigation_and_activation(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.receive_monitor_snapshot(monitor_snapshot());
            this.open_monitor(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("enter", cx);
        let selected = launcher.read(cx).monitor_selected.clone();
        window.press("tab", cx);
        let first_focus = window.focused(cx).unwrap();
        window.press("down", cx);
        assert_ne!(window.focused(cx), Some(first_focus.clone()));
        window.press("up", cx);
        assert_eq!(window.focused(cx), Some(first_focus));
        let menu_buttons = [
            "monitor-actions-close",
            "monitor-reveal",
            "monitor-copy-path",
            "monitor-quit",
            "monitor-terminate",
            "monitor-force-quit",
        ];
        for iteration in 0..12 {
            window.press("tab", cx);
            assert!(
                menu_buttons.iter().any(|&id| window
                    .try_find(id)
                    .is_some_and(|button| button.focused() == Some(true))),
                "Tab escaped the monitor action menu at {iteration}: {:?}; container={}",
                menu_buttons
                    .map(|id| (id, window.try_find(id).and_then(|button| button.focused()))),
                launcher.read(cx).dialog_focus.is_focused(window)
            );
        }
        for _ in 0..12 {
            window.press("shift-tab", cx);
            assert!(
                menu_buttons.iter().any(|&id| window
                    .try_find(id)
                    .is_some_and(|button| button.focused() == Some(true))),
                "Shift+Tab escaped the monitor action menu"
            );
        }
        assert_eq!(launcher.read(cx).monitor_selected, selected);
        for _ in 0..8 {
            if window.find("monitor-copy-path").focused() == Some(true) {
                break;
            }
            window.press("tab", cx);
        }
        assert_eq!(window.find("monitor-copy-path").focused(), Some(true));
        window.press("enter", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("/Applications/Fixture Alpha.app".into())
        );
        for _ in 0..8 {
            if window.find("monitor-quit").focused() == Some(true) {
                break;
            }
            window.press("down", cx);
        }
        assert_eq!(window.find("monitor-quit").focused(), Some(true));
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(launcher.read_with(cx, |this, _| this.status.starts_with("退出失败：")));
}

#[gpui_kit::test]
fn monitor_context_menu_keeps_the_list_visible_and_closes_on_an_outside_click(
    cx: &mut TestAppContext,
) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.receive_monitor_snapshot(monitor_snapshot());
            this.open_monitor(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        // A row near the right and lower edges exercises menu flipping.
        let trigger = "monitor-actions-/Applications/Fixture Gamma.app";
        let trigger_bounds = window.find(trigger).bounds();
        window.right_click(trigger, cx);
        let menu = window.find("monitor-actions-menu");
        let bounds = menu.bounds();
        assert!(menu.visible());
        assert!(bounds.size.width <= px(320.), "{bounds:?}");
        assert!(bounds.size.height < px(440.), "{bounds:?}");
        assert!(bounds.origin.x >= px(0.) && bounds.origin.y >= px(0.));
        assert!(bounds.origin.x + bounds.size.width <= px(720.));
        assert!(bounds.origin.y + bounds.size.height <= px(560.));
        assert!(
            bounds.origin.x < trigger_bounds.origin.x,
            "the menu should fit beside the right-edge trigger: {bounds:?}"
        );
        assert!(
            window
                .find("monitor-actions-/Applications/Fixture Alpha.app")
                .visible(),
            "opening actions replaced the monitor list"
        );
        assert!(window.find("内存").visible());
        assert_eq!(launcher.read(cx).query.mode, Mode::Monitor);
        let selected = launcher.read(cx).monitor_selected.clone();
        let position = point(px(8.), px(8.));
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        assert!(launcher.read(cx).monitor_actions.is_none());
        assert_eq!(launcher.read(cx).query.mode, Mode::Monitor);
        assert_eq!(launcher.read(cx).monitor_selected, selected);
        assert!(launcher.read(cx).visible);
        assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
    })
    .unwrap();
}

#[gpui_kit::test]
fn monitor_long_application_names_and_paths_keep_actions_inside_the_window(
    cx: &mut TestAppContext,
) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            let mut snapshot = monitor_snapshot();
            let row = &mut snapshot.processes[0];
            row.name = "Very Long Application 名称 ".repeat(5);
            row.path = Some(PathBuf::from(format!(
                "/Users/fixture/{}/{}.app",
                "very-long-application-directory/".repeat(6),
                row.name
            )));
            this.receive_monitor_snapshot(snapshot);
            this.open_monitor(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("enter", cx);
        let menu = window.find("monitor-actions-menu").bounds();
        assert!(menu.size.width <= px(320.), "{menu:?}");
        assert!(menu.size.height < px(440.), "{menu:?}");
        for id in [
            "monitor-actions-close",
            "monitor-reveal",
            "monitor-copy-path",
            "monitor-quit",
            "monitor-force-quit",
        ] {
            let button = window.find(id);
            let bounds = button.bounds();
            assert!(button.visible(), "{id} is not visible");
            assert!(
                bounds.origin.x >= px(0.),
                "{id} exceeds left edge: {bounds:?}"
            );
            assert!(
                bounds.origin.y >= px(0.),
                "{id} exceeds top edge: {bounds:?}"
            );
            assert!(
                bounds.origin.x + bounds.size.width <= px(720.),
                "{id} exceeds right edge: {bounds:?}"
            );
            assert!(
                bounds.origin.y + bounds.size.height <= px(560.),
                "{id} exceeds bottom edge: {bounds:?}"
            );
        }
        window.click("monitor-force-quit", cx);
        let confirmation = window.find("monitor-actions-menu").bounds();
        assert!(confirmation.size.width <= px(320.), "{confirmation:?}");
        assert!(confirmation.size.height < px(440.), "{confirmation:?}");
        for id in [
            "monitor-actions-close",
            "monitor-cancel-action",
            "monitor-confirm-action",
        ] {
            let bounds = window.find(id).bounds();
            assert!(
                bounds.origin.x >= px(0.) && bounds.origin.y >= px(0.),
                "{id} exceeds window origin: {bounds:?}"
            );
            assert!(
                bounds.origin.x + bounds.size.width <= px(720.)
                    && bounds.origin.y + bounds.size.height <= px(560.),
                "{id} exceeds window bounds: {bounds:?}"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn monitor_previous_action_result_cannot_replace_a_new_panel(cx: &mut TestAppContext) {
    let (handle, launcher) = fixture(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        launcher.update(cx, |this, cx| {
            this.receive_monitor_snapshot(monitor_snapshot());
            this.open_monitor(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let new_status = cx
        .update_window(handle.into(), |_, window, cx| {
            window.press("enter", cx);
            window.click("monitor-quit", cx);
            launcher.update(cx, |this, cx| {
                this.shell_event(ShellEvent::OpenSystem, window, cx);
                assert!(this.monitor_actions.is_none());
                this.open_monitor(window, cx);
                this.monitor_selected = Some("/Applications/Fixture Beta.app".into());
                this.open_monitor_actions(window, cx);
                this.status.clone()
            })
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(launcher.read(cx).monitor_actions.is_some());
        assert_eq!(launcher.read(cx).status, new_status);
        assert!(window.find("monitor-force-quit").visible());
        assert!(window.try_find("monitor-confirm-action").is_none());
        window.click("monitor-copy-path", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("/Applications/Fixture Beta.app".into())
        );
    })
    .unwrap();
}
