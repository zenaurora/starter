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
