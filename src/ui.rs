mod error_dialog;
#[cfg(test)]
mod tests;

use crate::{
    appearance,
    icons::Icons,
    platform::{self, Event as ShellEvent, Shell},
    settings::{self, Settings},
    worker,
};
use error_dialog::ErrorDialog;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use starter::{
    catalog::Catalog,
    config::{self, Config},
    history::History,
    search::{self, Candidate, Kind, Mode, Query},
    updates::{self, Status as UpdateStatus},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct Launcher {
    input: Entity<InputState>,
    settings: Option<Entity<Settings>>,
    settings_subscription: Option<Subscription>,
    error_dialog: Option<Entity<ErrorDialog>>,
    error_dialog_subscription: Option<Subscription>,
    icons: Icons,
    config: Config,
    config_path: PathBuf,
    shell: Option<Shell>,
    history: History,
    catalog: Catalog,
    candidates: Vec<Candidate>,
    results: Vec<Candidate>,
    selected: usize,
    query: Query,
    generation: u64,
    catalog_generation: u64,
    worker: async_channel::Sender<worker::Command>,
    cancelled: Arc<AtomicBool>,
    scroll: UniformListScrollHandle,
    status: String,
    visible: bool,
    searching: bool,
    update_status: UpdateStatus,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl Launcher {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut error = None;
        let (config, config_path) = match config::load_or_create() {
            Ok(value) => value,
            Err(problem) => {
                error = Some(format!(
                    "配置读取失败：{problem:#}\n\n程序将使用默认设置运行。"
                ));
                (
                    Config::default(),
                    config::config_path().unwrap_or_else(|_| PathBuf::from("config.toml")),
                )
            }
        };
        appearance::apply(config.theme, &config, window, cx);
        let history_path = config_path.with_file_name("usage.json");
        let history = History::load(history_path.clone()).unwrap_or_else(|problem| {
            let msg = format!("使用记录读取失败：{problem:#}\n\n程序将使用空白历史记录。");
            if let Some(existing) = &mut error {
                existing.push_str("\n\n");
                existing.push_str(&msg);
            } else {
                error = Some(msg);
            }
            History::empty(history_path)
        });
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索应用，或输入 /f、/c"));
        input.update(cx, |state, cx| state.focus(window, cx));
        let input_focus = input.focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Change => this.search(cx),
                InputEvent::PressEnter { secondary, shift } => {
                    if *secondary {
                        this.reveal_selected(window, cx);
                    } else if *shift {
                        this.copy_selected(cx);
                    } else {
                        this.open_selected(window, cx);
                    }
                }
                _ => {}
            }),
            Self::observe_activation(window, cx),
            cx.on_focus(&input_focus, window, |_, _, cx| cx.notify()),
            cx.on_blur(&input_focus, window, |_, _, cx| cx.notify()),
        ];
        let (icons, icon_events) = Icons::new();
        let (worker, worker_events) = worker::start();
        let (shell_sender, shell_events) = async_channel::unbounded();
        let setup = Shell::start(&config, shell_sender);
        let mut tasks = vec![
            cx.spawn_in(window, async move |this, cx| {
                while let Ok(icon) = icon_events.recv().await {
                    if this
                        .update_in(cx, |this, _, cx| {
                            this.icons.insert(icon);
                            cx.notify();
                        })
                        .is_err()
                    {
                        eprintln!("Icon loader: main window closed, stopping icon worker");
                        break;
                    }
                }
            }),
            cx.spawn_in(window, async move |this, cx| {
                while let Ok(event) = worker_events.recv().await {
                    if this
                        .update_in(cx, |this, _, cx| this.worker_event(event, cx))
                        .is_err()
                    {
                        eprintln!("Search worker: main window closed, stopping worker");
                        break;
                    }
                }
            }),
            cx.spawn_in(window, async move |this, cx| {
                while let Ok(event) = shell_events.recv().await {
                    if this
                        .update_in(cx, |this, window, cx| this.shell_event(event, window, cx))
                        .is_err()
                    {
                        eprintln!("Shell events: main window closed, stopping shell handler");
                        break;
                    }
                }
            }),
        ];
        let shell = match setup {
            Ok(setup) => {
                if let Some(warning) = setup.warning {
                    error = Some(warning);
                }
                tasks.push(cx.spawn_in(window, async move |this, cx| {
                    while let Ok(id) = setup.hotkey_events.recv().await {
                        if this
                            .update_in(cx, |this, window, cx| {
                                if let Some(event) =
                                    this.shell.as_ref().and_then(|shell| shell.event_for(id))
                                {
                                    this.shell_event(event, window, cx);
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
                Some(setup.shell)
            }
            Err(problem) => {
                error = Some(format!("系统托盘初始化失败：{problem:#}。窗口会保持打开。"));
                None
            }
        };
        // Closing behaves like a launcher dismissal; the tray provides an explicit quit.
        let view = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            view.update(cx, |this, cx| {
                if this.shell.is_none() {
                    return true;
                }
                this.hide(window, cx);
                false
            })
            .unwrap_or(true)
        });

        // Create error dialog if there were startup errors
        let mut error_dialog = None;
        let mut error_dialog_subscription = None;
        if let Some(message) = error {
            let dialog = cx.new(|cx| ErrorDialog::new(message, window, cx));
            error_dialog_subscription =
                Some(
                    cx.subscribe_in(&dialog, window, |this, _, event, _window, cx| match event {
                        error_dialog::Event::Close => {
                            this.error_dialog = None;
                            this.error_dialog_subscription = None;
                            cx.notify();
                        }
                    }),
                );
            error_dialog = Some(dialog);
        }

        let mut this = Self {
            input,
            settings: None,
            settings_subscription: None,
            error_dialog,
            error_dialog_subscription,
            icons,
            config,
            config_path,
            shell,
            history,
            catalog: Catalog::default(),
            candidates: Vec::new(),
            results: Vec::new(),
            selected: 0,
            query: Query::parse(""),
            generation: 0,
            catalog_generation: 0,
            worker,
            cancelled: Arc::new(AtomicBool::new(false)),
            scroll: UniformListScrollHandle::new(),
            status: "正在读取应用列表…".into(),
            visible: true,
            searching: false,
            update_status: UpdateStatus::Idle,
            _subscriptions: subscriptions,
            _tasks: tasks,
        };
        this.refresh_apps();
        this.search(cx);
        if this.config.auto_check_updates {
            this.check_updates(window, cx);
        }
        this._tasks.push(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(24 * 60 * 60))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        if this.config.auto_check_updates {
                            this.check_updates(window, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        this
    }

    fn observe_activation(window: &mut Window, cx: &mut Context<Self>) -> Subscription {
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active()
                && this.visible
                && this.settings.is_none()
                && this.error_dialog.is_none()
            {
                this.input.update(cx, |input, cx| input.focus(window, cx));
            } else if !window.is_window_active()
                && this.visible
                && this.shell.is_some()
                && this.settings.is_none()
            {
                this.hide(window, cx);
            }
            cx.notify();
        })
    }

    fn check_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_status == UpdateStatus::Checking {
            return;
        }
        self.set_update_status(UpdateStatus::Checking, cx);
        let check = cx.background_executor().spawn(async { updates::check() });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let status = match check.await {
                Ok(status) => status,
                Err(error) => UpdateStatus::Failed(format!("{error:#}")),
            };
            let _ = this.update_in(cx, |this, _, cx| this.set_update_status(status, cx));
        }));
    }

    fn set_update_status(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, cx| {
                settings.update_status = status.clone();
                cx.notify();
            });
        }
        self.update_status = status;
        cx.notify();
    }

    /// Re-scan applications without disturbing the cached file index. Cheap
    /// enough to run every time the window is shown, so apps installed while
    /// the launcher was running become searchable without a restart. The
    /// resulting `Event::Apps` re-ranks the list through `search`.
    fn refresh_apps(&mut self) {
        self.catalog_generation += 1;
        let _ = self.worker.try_send(worker::Command::Apps {
            generation: self.catalog_generation,
        });
    }

    /// Explicit user-requested refresh: applications *and* the file index.
    fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.catalog_generation += 1;
        self.status = "正在刷新应用与文件列表…".into();
        let _ = self.worker.try_send(worker::Command::Refresh {
            generation: self.catalog_generation,
        });
        self.search(cx);
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.query = Query::parse(&self.input.read(cx).value());
        self.selected = 0;
        self.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
        if self.query.mode == Mode::Apps {
            self.searching = false;
            self.results = search::rank_apps(
                &self.candidates,
                &self.query.text,
                &self.history.entries,
                &self.config.favorites,
            );
            self.status = format!(
                "{} 个应用{}",
                self.catalog.apps.len(),
                if self.catalog.warnings.is_empty() {
                    String::new()
                } else {
                    format!("，{} 处目录未能读取", self.catalog.warnings.len())
                }
            );
        } else {
            self.results.clear();
            self.searching = true;
            self.status = "搜索中…".into();
            let _ = self.worker.try_send(worker::Command::Search {
                generation: self.generation,
                query: self.query.clone(),
                roots: self.config.search_roots.clone(),
                cancelled: self.cancelled.clone(),
            });
        }
        cx.notify();
    }

    fn worker_event(&mut self, event: worker::Event, cx: &mut Context<Self>) {
        match event {
            worker::Event::Apps {
                generation,
                catalog,
            } if generation == self.catalog_generation => {
                self.candidates = search::app_candidates(&catalog.apps, &self.config);
                self.catalog = catalog;
                // Only the app list reads these candidates. Re-running `search` while
                // a Files/Content query is in flight would cancel the running disk
                // scan, clear the streamed results, reset the selection and replay the
                // identical query after another debounce delay.
                if self.query.mode == Mode::Apps {
                    self.search(cx);
                } else {
                    // `recent` renders straight from `self.candidates`, so repaint.
                    cx.notify();
                }
            }
            worker::Event::Results {
                generation,
                candidates,
            } if generation == self.generation => {
                self.results.extend(candidates);
                cx.notify();
            }
            worker::Event::Done { generation, status } if generation == self.generation => {
                self.searching = false;
                self.status = status;
                cx.notify();
            }
            _ => {}
        }
    }

    fn shell_event(&mut self, event: ShellEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ShellEvent::Toggle => {
                if self.visible && window.is_window_active() {
                    self.hide(window, cx);
                } else {
                    self.show(window, cx);
                }
            }
            ShellEvent::Terminal => match platform::open_terminal(&self.config) {
                Ok(()) => self.hide(window, cx),
                Err(problem) => {
                    self.status = format!("终端打开失败：{problem:#}");
                    self.show(window, cx);
                }
            },
            ShellEvent::OpenSettings => self.open_settings(window, cx),
            ShellEvent::OpenConfig => {
                if let Err(problem) = platform::open_target(&self.config_path) {
                    self.status = format!("配置文件打开失败：{problem:#}");
                }
            }
            ShellEvent::Reload => {
                let result = config::read(&self.config_path).and_then(|config| {
                    if let Some(shell) = &mut self.shell {
                        shell.rebind(&config)?;
                    }
                    Ok(config)
                });
                match result {
                    Ok(config) => {
                        appearance::apply(config.theme, &config, window, cx);
                        self.config = config;
                        self.settings = None;
                        self.settings_subscription = None;
                        self.status = "配置已重新加载".into();
                        self.refresh_apps();
                        self.search(cx);
                    }
                    Err(problem) => {
                        self.status = format!("重新加载失败：{problem:#}");
                        self.show(window, cx);
                    }
                }
            }
            ShellEvent::Refresh => self.refresh_all(cx),
            ShellEvent::Quit => cx.quit(),
        }
        cx.notify();
    }

    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            self.close_settings(window, cx);
        }
        self.visible = true;
        platform::show(window, cx);
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.refresh_apps();
        self.search(cx);
        // Native activation may complete after show() returns. Also restore
        // focus once the current UI update and layout have settled.
        cx.defer_in(window, |this, window, cx| {
            if this.visible && this.settings.is_none() && this.error_dialog.is_none() {
                this.input.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            }
        });
    }

    fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.is_none() {
            return;
        }
        self.visible = false;
        self.cancelled.store(true, Ordering::Relaxed);
        platform::hide(window, cx);
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(candidate) = self.results.get(self.selected) else {
            return;
        };
        match platform::open_target(&candidate.path) {
            Ok(()) => {
                if candidate.kind == Kind::App
                    && let Err(problem) = self.history.record(&candidate.id)
                {
                    self.status = format!("记录保存失败：{problem:#}");
                }
                self.hide(window, cx);
            }
            Err(problem) => {
                self.status = format!("打开失败：{problem:#}");
                cx.notify();
            }
        }
    }

    fn reveal_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(candidate) = self.results.get(self.selected) {
            match platform::reveal(&candidate.path) {
                Ok(()) => self.hide(window, cx),
                Err(problem) => {
                    self.status = format!("显示失败：{problem:#}");
                    cx.notify();
                }
            }
        }
    }

    fn copy_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(candidate) = self.results.get(self.selected) {
            cx.write_to_clipboard(ClipboardItem::new_string(
                candidate.path.display().to_string(),
            ));
            self.status = "已复制路径".into();
            cx.notify();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if cfg!(target_os = "macos")
            && event.keystroke.key == "q"
            && modifiers.platform
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.shift
        {
            cx.stop_propagation();
            cx.quit();
            return;
        }
        let close_modifier = if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        };
        // Window dismissal also works inside settings and during text composition.
        if event.keystroke.key == "w" && close_modifier && !modifiers.alt && !modifiers.shift {
            if self.shell.is_some() {
                self.hide(window, cx);
            } else {
                window.remove_window();
            }
            cx.stop_propagation();
            return;
        }
        if self.settings.is_some() {
            return;
        }
        // Let the native input method handle candidate selection and composition.
        let composing = self.input.update(cx, |input, cx| {
            input.marked_text_range(window, cx).is_some()
        });
        if composing {
            return;
        }
        match event.keystroke.key.as_str() {
            "up" | "down" if !modifiers.alt && !modifiers.control && !modifiers.platform => {
                if !self.results.is_empty() {
                    self.selected = if event.keystroke.key == "down" {
                        (self.selected + 1).min(self.results.len() - 1)
                    } else {
                        self.selected.saturating_sub(1)
                    };
                    self.scroll
                        .scroll_to_item(self.selected, ScrollStrategy::Nearest);
                    cx.notify();
                }
                cx.stop_propagation();
            }
            "escape" => {
                let value = self.input.read(cx).value();
                let next = if !self.query.text.is_empty() && self.query.mode != Mode::Apps {
                    if self.query.mode == Mode::Files {
                        "/f "
                    } else {
                        "/c "
                    }
                } else if !value.is_empty() {
                    ""
                } else {
                    self.hide(window, cx);
                    cx.stop_propagation();
                    return;
                };
                self.input
                    .update(cx, |input, cx| input.set_value(next, window, cx));
                self.search(cx);
                cx.stop_propagation();
            }
            "," if modifiers.platform || modifiers.control => {
                self.open_settings(window, cx);
                cx.stop_propagation();
            }
            "q" if modifiers.platform || modifiers.control => {
                cx.quit();
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = true;
        platform::show(window, cx);
        if self.settings.is_some() {
            return;
        }
        let settings = cx.new(|cx| Settings::new(&self.config, window, cx));
        settings.update(cx, |settings, _| {
            settings.update_status = self.update_status.clone()
        });
        self.settings_subscription = Some(cx.subscribe_in(
            &settings,
            window,
            |this, _, event, window, cx| match event {
                settings::Event::Close => this.close_settings(window, cx),
                settings::Event::OpenConfig => this.shell_event(ShellEvent::OpenConfig, window, cx),
                settings::Event::CheckUpdates => this.check_updates(window, cx),
                settings::Event::OpenRelease(url) => {
                    if let Err(error) = open::that(url) {
                        this.status = format!("无法打开下载页面：{error}");
                        if let Some(settings) = &this.settings {
                            settings.update(cx, |settings, cx| {
                                settings.error = Some(this.status.clone());
                                cx.notify();
                            });
                        }
                    }
                }
                settings::Event::Preview(theme) => {
                    appearance::apply(*theme, &this.config, window, cx)
                }
                settings::Event::Save(config) => {
                    this.save_settings(config.as_ref().clone(), window, cx)
                }
            },
        ));
        self.settings = Some(settings);
        cx.notify();
    }

    fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = None;
        self.settings_subscription = None;
        appearance::apply(self.config.theme, &self.config, window, cx);
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn save_settings(&mut self, config: Config, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<()> {
            if let Some(shell) = &mut self.shell {
                shell.rebind(&config)?;
            }
            if let Err(error) = config::save(&self.config_path, &config) {
                if let Some(shell) = &mut self.shell {
                    shell.rebind(&self.config).map_err(|rollback| {
                        anyhow::anyhow!("{error:#}；快捷键恢复失败：{rollback:#}")
                    })?;
                }
                return Err(error);
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                let enable_updates = config.auto_check_updates && !self.config.auto_check_updates;
                self.config = config;
                self.close_settings(window, cx);
                self.refresh_apps();
                self.search(cx);
                self.status = "设置已保存".into();
                if enable_updates {
                    self.check_updates(window, cx);
                }
            }
            Err(error) => {
                if let Some(settings) = &self.settings {
                    settings.update(cx, |settings, cx| {
                        settings.error = Some(format!("保存失败：{error:#}"));
                        cx.notify();
                    });
                }
            }
        }
    }

    fn toggle_favorite(&mut self, id: &str, cx: &mut Context<Self>) {
        let selected_id = self
            .results
            .get(self.selected)
            .map(|candidate| candidate.id.clone());
        let mut config = self.config.clone();
        if !config.favorites.remove(id) {
            config.favorites.insert(id.to_string());
        }
        match config::save(&self.config_path, &config) {
            Ok(()) => {
                self.config = config;
                self.results = search::rank_apps(
                    &self.candidates,
                    &self.query.text,
                    &self.history.entries,
                    &self.config.favorites,
                );
                self.selected = selected_id
                    .and_then(|id| self.results.iter().position(|candidate| candidate.id == id))
                    .unwrap_or(0);
                self.scroll
                    .scroll_to_item(self.selected, ScrollStrategy::Nearest);
            }
            Err(error) => self.status = format!("收藏保存失败：{error:#}"),
        }
        cx.notify();
    }

    fn recent(&mut self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let recent = search::recent_apps(&self.candidates, &self.history.entries, 5);
        let mut cards = div().w_full().flex().gap_2();
        for candidate in recent.iter() {
            let image = self.icons.get(&candidate.path);
            let path = candidate.path.clone();
            let id = candidate.id.clone();
            let icon = match image {
                Some(image) => img(image).size(px(28.)).into_any_element(),
                None => Icon::new(IconName::AppWindow)
                    .with_size(px(28.))
                    .into_any_element(),
            };
            cards = cards.child(
                div()
                    .id(SharedString::from(format!("recent-{id}")))
                    .flex_1()
                    .min_w_0()
                    .h(px(68.))
                    .p_2()
                    .rounded_md()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.accent))
                    .child(icon)
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_size(px(10.))
                            .text_ellipsis()
                            .child(candidate.title.clone()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.launch_app(&path, &id, window, cx);
                        }),
                    ),
            );
        }
        // Recent visits have their own order, independent of the main ranked list.
        div()
            .mx_3()
            .mb_2()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Clock).with_size(px(13.)))
                    .child("最近使用"),
            )
            .when(recent.is_empty(), |this| {
                this.child(
                    div()
                        .pb_1()
                        .text_size(px(11.))
                        .text_color(theme.muted_foreground)
                        .child("打开过的应用会出现在这里"),
                )
            })
            .when(!recent.is_empty(), |this| this.child(cards))
            .into_any_element()
    }

    fn launch_app(
        &mut self,
        path: &std::path::Path,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match platform::open_target(path) {
            Ok(()) => {
                if let Err(error) = self.history.record(id) {
                    self.status = format!("使用记录保存失败：{error:#}");
                }
                self.hide(window, cx);
            }
            Err(error) => {
                self.status = format!("打开失败：{error:#}");
                cx.notify();
            }
        }
    }

    fn row(&mut self, index: usize, cx: &Context<Self>) -> AnyElement {
        let candidate = &self.results[index];
        let selected = index == self.selected;
        let theme = cx.theme();
        let app = candidate.kind == Kind::App;
        let favorite = self.config.favorites.contains(&candidate.id);
        let favorite_id = candidate.id.clone();
        let symbol = match candidate.kind {
            Kind::App => IconName::AppWindow,
            Kind::File => IconName::File,
            Kind::Folder => IconName::Folder,
            Kind::Content => IconName::FileText,
        };
        let image = self.icons.get(&candidate.path);
        let home = dirs::home_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let detail = if app {
            candidate
                .path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        } else {
            candidate.detail.clone()
        };
        let detail = if !home.is_empty() && detail.starts_with(&home) {
            detail.replacen(&home, "~", 1)
        } else {
            detail
        };
        let icon = div()
            .size(px(36.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.primary);
        let icon = match image {
            Some(image) => icon.child(img(image).size(px(36.))),
            None => icon.child(Icon::new(symbol).with_size(px(24.))),
        };
        div()
            .w_full()
            .h(px(60.))
            .px_3()
            .child(
                div()
                    .id(("result", index))
                    .w_full()
                    .h_full()
                    .px_3()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(if selected {
                        theme.accent
                    } else {
                        theme.background
                    })
                    .hover(|style| style.bg(theme.accent))
                    .child(
                        div()
                            .id(("open", index))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .gap_3()
                            .cursor_pointer()
                            .child(icon)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(if selected {
                                                FontWeight::MEDIUM
                                            } else {
                                                FontWeight::NORMAL
                                            })
                                            .text_ellipsis()
                                            .child(candidate.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_family(theme.mono_font_family.clone())
                                            .text_color(theme.muted_foreground)
                                            .text_ellipsis()
                                            .child(detail),
                                    ),
                            )
                            .when(selected, |this| {
                                this.child(
                                    Icon::new(IconName::CornerDownLeft)
                                        .with_size(px(14.))
                                        .text_color(theme.muted_foreground),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.selected = index;
                                    this.open_selected(window, cx);
                                }),
                            ),
                    )
                    .when(app, |this| {
                        this.child(
                            Button::new(("favorite", index))
                                .icon(if favorite {
                                    IconName::Star
                                } else {
                                    IconName::StarOff
                                })
                                .ghost()
                                .small()
                                .text_color(if favorite {
                                    theme.primary
                                } else {
                                    theme.muted_foreground
                                })
                                .tooltip(if favorite {
                                    "取消收藏"
                                } else {
                                    "收藏应用"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_favorite(&favorite_id, cx)
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(dialog) = &self.error_dialog {
            return div()
                .size_full()
                .capture_key_down(cx.listener(Self::key_down))
                .child(dialog.clone())
                .into_any_element();
        }
        if let Some(settings) = &self.settings {
            return div()
                .size_full()
                .capture_key_down(cx.listener(Self::key_down))
                .child(settings.clone())
                .into_any_element();
        }
        let list = uniform_list(
            "results",
            self.results.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range.map(|index| this.row(index, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0()
        .w_full();
        let empty = if self.searching {
            "正在搜索…"
        } else if self.query.mode != Mode::Apps && self.config.search_roots.is_empty() {
            "先选择搜索目录"
        } else if self.query.mode == Mode::Content && self.query.text.is_empty() {
            "输入要查找的文本"
        } else {
            "没有匹配结果"
        };
        let show_recent = self.query.mode == Mode::Apps && self.query.text.is_empty();
        let recent = show_recent.then(|| self.recent(cx));
        let theme = cx.theme();
        let input_focused =
            window.is_window_active() && self.input.focus_handle(cx).is_focused(window);
        let modes = [
            (Mode::Apps, IconName::AppWindow, "应用", ""),
            (Mode::Files, IconName::FolderSearch, "文件 /f", "/f "),
            (Mode::Content, IconName::FileText, "内容 /c", "/c "),
        ];
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .capture_key_down(cx.listener(Self::key_down))
            .child(
                div()
                    .h(px(38.))
                    .flex_shrink_0()
                    .px_5()
                    .pt_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_color(theme.primary)
                            .child(Icon::new(IconName::Command).with_size(px(14.)))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_family(theme.mono_font_family.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("STARTER"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                Button::new("search-focus-status")
                                    .label(if input_focused {
                                        "可直接输入 · ↑↓ 选择"
                                    } else {
                                        "点击搜索框输入"
                                    })
                                    .ghost()
                                    .small()
                                    .text_color(if input_focused {
                                        theme.primary
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.input.update(cx, |input, cx| input.focus(window, cx));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("settings")
                                    .icon(IconName::Settings2)
                                    .ghost()
                                    .small()
                                    .tooltip(if cfg!(target_os = "macos") {
                                        "设置 ⌘,"
                                    } else {
                                        "设置 Ctrl+,"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_settings(window, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .px_5()
                    .py_3()
                    .border_b_2()
                    .border_color(if input_focused {
                        theme.primary
                    } else {
                        theme.border
                    })
                    .child(
                        Input::new(&self.input)
                            .large()
                            .appearance(false)
                            .font_family(theme.mono_font_family.clone())
                            .prefix(
                                Icon::new(IconName::Search)
                                    .with_size(px(20.))
                                    .text_color(theme.primary),
                            ),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().flex().gap_1().children(modes.map(
                        |(mode, icon, label, prefix)| {
                            Button::new(label)
                                .icon(icon)
                                .label(label)
                                .ghost()
                                .small()
                                .when(self.query.mode == mode, |button| {
                                    button.bg(theme.accent).text_color(theme.primary)
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.input.update(cx, |input, cx| {
                                        input.set_value(prefix, window, cx);
                                        input.focus(window, cx);
                                    });
                                    this.search(cx);
                                }))
                        },
                    )))
                    .child(
                        div()
                            .pr_2()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(format!("{} 条结果", self.results.len())),
                    ),
            )
            .when_some(recent, |this, recent| this.child(recent))
            .when(
                matches!(self.update_status, UpdateStatus::Available { .. }),
                |this| {
                    this.child(
                        Button::new("update-notice")
                            .label(format!("{} · 查看更新", self.update_status.label()))
                            .ghost()
                            .small()
                            .mx_3()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(window, cx);
                                if let Some(settings) = &this.settings {
                                    settings.update(cx, |settings, cx| {
                                        settings.show_updates(window, cx)
                                    });
                                }
                            })),
                    )
                },
            )
            .when(self.results.is_empty(), |this| {
                this.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .child(
                            Icon::new(IconName::Search)
                                .with_size(px(28.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(div().text_size(px(14.)).child(empty))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child("应用名 · 文件与目录 · 文本内容"),
                        )
                        .when(
                            self.query.mode != Mode::Apps && self.config.search_roots.is_empty(),
                            |this| {
                                this.child(
                                    Button::new("configure-search")
                                        .label("设置搜索目录")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_settings(window, cx)
                                        })),
                                )
                            },
                        ),
                )
            })
            .when(!self.results.is_empty(), |this| this.child(list))
            .child(
                div()
                    .px_5()
                    .py_3()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(10.))
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .child(self.status.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .ml_3()
                            .child(if cfg!(target_os = "macos") {
                                "↑↓ 选择   ↵ 打开   ⌘↵ 定位   ⇧↵ 复制"
                            } else {
                                "↑↓ 选择   ↵ 打开   Ctrl+↵ 定位   ⇧↵ 复制"
                            }),
                    ),
            )
            .into_any_element()
    }
}
