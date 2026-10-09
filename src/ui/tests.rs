use super::*;
use core::prelude::v1::test;
use gpui_kit::{base::Root, test::TestWindowExt};

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
                        clipboard_worker,
                        clipboard_restore_pending: false,
                        clipboard_hide_after_restore: false,
                        clipboard_error: None,
                        uninstall_targets: vec![],
                        uninstall_confirmation: None,
                        uninstall_busy: false,
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
        for id in ["全部", "文件", "内容", "剪贴板", "卸载", "系统", "提醒"] {
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
