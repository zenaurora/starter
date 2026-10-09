mod error_dialog;
mod launcher_commands;
mod reminder_panel;
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
    clipboard,
    config::{self, Config},
    history::History,
    opening, reminders,
    search::{self, Candidate, Kind, Mode, Query},
    system_commands::{self, Action as BuiltinAction, Power},
    uninstall,
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
    clipboard_entries: Vec<clipboard::Entry>,
    clipboard_root: PathBuf,
    clipboard_worker: async_channel::Sender<clipboard::Command>,
    clipboard_restore_pending: bool,
    clipboard_hide_after_restore: bool,
    clipboard_error: Option<String>,
    uninstall_targets: Vec<uninstall::Target>,
    uninstall_confirmation: Option<uninstall::Target>,
    uninstall_busy: bool,
    power_confirmation: Option<Power>,
    power_busy: bool,
    reminder_entries: Vec<reminders::Entry>,
    reminder_worker: async_channel::Sender<reminders::Command>,
    reminder_panel: Option<Entity<reminder_panel::ReminderPanel>>,
    reminder_subscription: Option<Subscription>,
    reminder_draft: Option<reminders::Draft>,
    reminder_pending: bool,
    reminder_feedback: Option<String>,
    notification_warning: Option<String>,
    dialog_focus: FocusHandle,
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
    update_release: Option<(String, Vec<updates::Asset>)>,
    update_cancelled: Arc<AtomicBool>,
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
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("搜索应用、系统命令或设置；/remind 10m 开会")
        });
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
        let clipboard_root = config_path.with_file_name("clipboard");
        let (clipboard_worker, clipboard_events) =
            clipboard::start(clipboard_root.clone(), config.clipboard_history);
        let (reminder_worker, reminder_events) =
            reminders::start(config_path.with_file_name("reminders.json"));
        let (shell_sender, shell_events) = async_channel::unbounded();
        let setup = Shell::start(&config, shell_sender);
        let mut tasks = vec![
            cx.spawn_in(window, async move |this, cx| {
                while let Ok(event) = reminder_events.recv().await {
                    if this
                        .update_in(cx, |this, window, cx| {
                            this.reminder_event(event, window, cx)
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }),
            cx.spawn_in(window, async move |this, cx| {
                while let Ok(event) = clipboard_events.recv().await {
                    if this
                        .update_in(cx, |this, window, cx| {
                            this.clipboard_event(event, window, cx)
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }),
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
        let update_report = updates::take_report();
        if let Some(UpdateStatus::Failed(problem)) = &update_report {
            let message = format!("上次更新失败：{problem}\n\n已尝试恢复原应用。请在设置中重试。");
            if let Some(error) = &mut error {
                error.push_str(&format!("\n\n{message}"));
            } else {
                error = Some(message);
            }
        }
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
            clipboard_entries: Vec::new(),
            clipboard_root,
            clipboard_worker,
            clipboard_restore_pending: false,
            clipboard_hide_after_restore: false,
            clipboard_error: None,
            uninstall_targets: Vec::new(),
            uninstall_confirmation: None,
            uninstall_busy: false,
            power_confirmation: None,
            power_busy: false,
            reminder_entries: Vec::new(),
            reminder_worker,
            reminder_panel: None,
            reminder_subscription: None,
            reminder_draft: None,
            reminder_pending: false,
            reminder_feedback: None,
            notification_warning: None,
            dialog_focus: cx.focus_handle(),
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
            update_status: update_report.unwrap_or_default(),
            update_release: None,
            update_cancelled: Arc::new(AtomicBool::new(false)),
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
                && this.uninstall_confirmation.is_none()
                && this.power_confirmation.is_none()
                && !this.power_busy
                && this.reminder_panel.is_none()
            {
                this.input.update(cx, |input, cx| input.focus(window, cx));
            } else if !window.is_window_active()
                && this.visible
                && this.shell.is_some()
                && this.settings.is_none()
                && this.error_dialog.is_none()
                && this.uninstall_confirmation.is_none()
                && this.power_confirmation.is_none()
                && !this.power_busy
                && this.reminder_panel.is_none()
            {
                this.hide(window, cx);
            }
            cx.notify();
        })
    }

    fn check_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_status.busy() {
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

    fn install_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_status.busy() {
            return;
        }
        let Some((version, assets)) = self.update_release.clone() else {
            return;
        };
        self.update_cancelled = Arc::new(AtomicBool::new(false));
        let cancel = self.update_cancelled.clone();
        let (sender, receiver) = async_channel::unbounded();
        self.set_update_status(
            UpdateStatus::Downloading {
                received: 0,
                total: 0,
            },
            cx,
        );
        let job = cx.background_executor().spawn(async move {
            updates::prepare_and_launch(&version, &assets, &cancel, |status| {
                let _ = sender.try_send(status);
            })
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            while let Ok(status) = receiver.recv().await {
                if this
                    .update_in(cx, |this, _, cx| this.set_update_status(status, cx))
                    .is_err()
                {
                    return;
                }
            }
            let result = job.await;
            let _ = this.update_in(cx, |this, _, cx| match result {
                Ok(()) => cx.quit(),
                Err(error) => {
                    let status = if this.update_cancelled.load(Ordering::Relaxed) {
                        UpdateStatus::Cancelled
                    } else {
                        UpdateStatus::Failed(format!("{error:#}"))
                    };
                    this.set_update_status(status, cx);
                }
            });
        }));
    }

    fn set_update_status(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        match &status {
            UpdateStatus::Available {
                version, assets, ..
            } => self.update_release = Some((version.clone(), assets.clone())),
            UpdateStatus::UpToDate | UpdateStatus::Checking => self.update_release = None,
            _ => {}
        }
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, cx| {
                settings.can_install = self.update_release.is_some();
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
            let mut candidates = self.candidates.clone();
            if !self.query.text.is_empty() {
                candidates.extend(system_commands::candidates());
            }
            self.results = search::rank_apps(
                &candidates,
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
        } else if self.query.mode == Mode::System {
            self.searching = false;
            self.results = search::rank(
                &system_commands::candidates(),
                &self.query.text,
                &Default::default(),
            );
            self.status = "输入英文或中文查找 · 重启和关机需确认".into();
        } else if self.query.mode == Mode::Reminders {
            self.searching = false;
            self.reminder_results();
        } else if self.query.mode == Mode::Uninstall {
            self.searching = false;
            let candidates: Vec<_> = self
                .uninstall_targets
                .iter()
                .map(|target| target.candidate(&self.config))
                .collect();
            self.results = search::rank(&candidates, &self.query.text, &Default::default());
            self.status = format!(
                "{} 个可卸载应用 · 系统应用与 Starter 已排除",
                self.uninstall_targets.len()
            );
        } else if self.query.mode == Mode::Clipboard {
            self.searching = false;
            self.clipboard_results();
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
                uninstall_targets,
            } if generation == self.catalog_generation => {
                self.candidates = search::app_candidates(&catalog.apps, &self.config);
                self.catalog = catalog;
                self.uninstall_targets = uninstall_targets;
                if let Some(settings) = &self.settings {
                    settings.update(cx, |settings, cx| {
                        settings.set_apps(self.catalog.apps.clone(), cx)
                    });
                }
                // Only the app list reads these candidates. Re-running `search` while
                // a Files/Content query is in flight would cancel the running disk
                // scan, clear the streamed results, reset the selection and replay the
                // identical query after another debounce delay.
                if matches!(self.query.mode, Mode::Apps | Mode::Uninstall) {
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

    fn clipboard_results(&mut self) {
        self.results = clipboard::search(&self.clipboard_entries, &self.query.text);
        self.status = self.clipboard_error.clone().unwrap_or_else(|| {
            format!(
                "{} 条历史 · {} · 图片、文件、链接、文本",
                self.clipboard_entries.len(),
                if self.config.clipboard_history {
                    "正在记录"
                } else {
                    "记录已暂停"
                }
            )
        });
    }

    fn clipboard_event(
        &mut self,
        event: clipboard::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            clipboard::Event::Snapshot(entries) => {
                let selected = self.results.get(self.selected).map(|c| c.id.clone());
                self.clipboard_entries = entries;
                self.clipboard_error = None;
                if self.query.mode == Mode::Clipboard {
                    self.clipboard_results();
                    self.selected = selected
                        .and_then(|id| self.results.iter().position(|c| c.id == id))
                        .unwrap_or(0);
                    self.scroll
                        .scroll_to_item(self.selected, ScrollStrategy::Nearest);
                }
            }
            clipboard::Event::Restored => {
                self.clipboard_restore_pending = false;
                self.status = "已复制，可粘贴到其他应用".into();
                if self.clipboard_hide_after_restore {
                    self.hide(window, cx);
                }
            }
            clipboard::Event::Cleared => {
                if let Some(settings) = &self.settings {
                    settings.update(cx, |settings, cx| {
                        settings.clipboard_message = Some("剪贴板历史已清空".into());
                        cx.notify();
                    });
                }
            }
            clipboard::Event::Failed(error) => {
                self.clipboard_restore_pending = false;
                self.clipboard_error = Some(error.clone());
                if self.query.mode == Mode::Clipboard {
                    self.status = error.clone();
                }
                if let Some(settings) = &self.settings {
                    settings.update(cx, |settings, cx| {
                        settings.error = Some(error);
                        cx.notify();
                    });
                }
            }
        }
        cx.notify();
    }

    fn restore_clipboard(&mut self, hide: bool, cx: &mut Context<Self>) {
        if self.clipboard_restore_pending {
            return;
        }
        if let Some(candidate) = self.results.get(self.selected) {
            if self
                .clipboard_worker
                .try_send(clipboard::Command::Restore(candidate.id.clone()))
                .is_ok()
            {
                self.clipboard_restore_pending = true;
                self.clipboard_hide_after_restore = hide;
                self.status = "正在恢复剪贴板…".into();
            } else {
                self.status = "剪贴板历史未运行，请重启后重试".into();
            }
            cx.notify();
        }
    }

    fn confirm_uninstall(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uninstall_busy {
            return;
        }
        let Some(target) = self.uninstall_confirmation.clone() else {
            return;
        };
        self.uninstall_busy = true;
        cx.notify();
        let id = target.id.clone();
        let job = cx
            .background_executor()
            .spawn(async move { target.execute() });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.uninstall_busy = false;
                this.uninstall_confirmation = None;
                this.input.update(cx, |input, cx| input.focus(window, cx));
                match result {
                    Ok(message) => {
                        if cfg!(target_os = "macos") {
                            this.uninstall_targets.retain(|target| target.id != id);
                            this.catalog.apps.retain(|app| app.id != id);
                            this.candidates.retain(|candidate| candidate.id != id);
                            this.search(cx);
                        }
                        this.status = message;
                    }
                    Err(error) => this.status = format!("卸载失败：{error:#}"),
                }
                cx.notify();
            });
        }));
    }

    fn uninstall_dialog(&self, cx: &Context<Self>) -> AnyElement {
        let Some(target) = &self.uninstall_confirmation else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background)
            .track_focus(&self.dialog_focus)
            .capture_key_down(cx.listener(Self::key_down))
            .child(
                div()
                    .w(px(520.))
                    .max_w_full()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("卸载 {}", target.name)),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(relative(1.6))
                            .child(target.description()),
                    )
                    .when(self.uninstall_busy, |body| body.child("正在处理…"))
                    .when(!self.uninstall_busy, |body| {
                        body.child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_3()
                                .child(
                                    Button::new("cancel-uninstall")
                                        .label("取消 · Esc")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.uninstall_confirmation = None;
                                            this.input
                                                .update(cx, |input, cx| input.focus(window, cx));
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("confirm-uninstall")
                                        .label(if cfg!(target_os = "macos") {
                                            "确认移到废纸篓"
                                        } else {
                                            "确认启动卸载"
                                        })
                                        .primary()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.confirm_uninstall(window, cx)
                                        })),
                                ),
                        )
                    }),
            )
            .into_any_element()
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
            ShellEvent::Applications(applications) => {
                self.hide(window, cx);
                let job = cx
                    .background_executor()
                    .spawn(async move { opening::launch_all(&applications) });
                self._tasks.push(cx.spawn_in(window, async move |this, cx| {
                    if let Err(problem) = job.await {
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.status = format!("应用打开失败：{problem:#}");
                            this.show(window, cx);
                        });
                    }
                }));
            }
            ShellEvent::OpenSettings => self.open_settings(window, cx),
            ShellEvent::OpenConfig => {
                if let Err(problem) = platform::open_target(&self.config_path) {
                    self.status = format!("配置文件打开失败：{problem:#}");
                }
            }
            ShellEvent::Reload => {
                let result = config::read(&self.config_path).and_then(|config| {
                    self.apply_config(&config, false)?;
                    Ok(config)
                });
                match result {
                    Ok(config) => {
                        appearance::apply(config.theme, &config, window, cx);
                        self.config = config;
                        let _ = self
                            .clipboard_worker
                            .try_send(clipboard::Command::Enabled(self.config.clipboard_history));
                        self.close_settings(window, cx);
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
            ShellEvent::OpenReminders => {
                self.show(window, cx);
                self.open_reminders("", window, cx);
            }
            ShellEvent::OpenSystem => {
                self.show(window, cx);
                self.input
                    .update(cx, |input, cx| input.set_value("/system ", window, cx));
                self.search(cx);
            }
            ShellEvent::Refresh => self.refresh_all(cx),
            ShellEvent::Quit => cx.quit(),
        }
        cx.notify();
    }

    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uninstall_busy || self.power_busy {
            self.visible = true;
            platform::show(window, cx);
            return;
        }
        self.uninstall_confirmation = None;
        self.power_confirmation = None;
        self.reminder_panel = None;
        self.reminder_subscription = None;
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
            if this.visible
                && this.settings.is_none()
                && this.error_dialog.is_none()
                && this.uninstall_confirmation.is_none()
                && this.power_confirmation.is_none()
                && !this.power_busy
                && this.reminder_panel.is_none()
            {
                this.input.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            }
        });
    }

    fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.is_none() {
            return;
        }
        if self.settings.is_some() {
            self.close_settings(window, cx);
        }
        self.visible = false;
        self.cancelled.store(true, Ordering::Relaxed);
        platform::hide(window, cx);
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uninstall_confirmation.is_some()
            || self.uninstall_busy
            || self.power_confirmation.is_some()
            || self.power_busy
            || self.reminder_panel.is_some()
        {
            return;
        }
        if self.query.mode == Mode::Clipboard {
            self.restore_clipboard(true, cx);
            return;
        }
        if self.query.mode == Mode::Uninstall {
            if let Some(candidate) = self.results.get(self.selected) {
                self.uninstall_confirmation = self
                    .uninstall_targets
                    .iter()
                    .find(|t| t.id == candidate.id)
                    .cloned();
                window.focus(&self.dialog_focus, cx);
                cx.notify();
            }
            return;
        }
        let Some(candidate) = self.results.get(self.selected).cloned() else {
            return;
        };
        if candidate.id == "reminder:create" {
            if let Some(draft) = self.reminder_draft.clone() {
                self.send_reminder(reminders::Command::Create(draft), cx);
            }
            return;
        }
        if candidate.kind == Kind::Reminder || candidate.id == "reminder:new" {
            let initial = if candidate.id == "reminder:new" {
                self.query.text.clone()
            } else {
                String::new()
            };
            self.open_reminders(&initial, window, cx);
            return;
        }
        if let Some(action) = system_commands::resolve(&candidate.id) {
            match action {
                BuiltinAction::Power(power) if power.requires_confirmation() => {
                    self.power_confirmation = Some(power);
                    window.focus(&self.dialog_focus, cx);
                    cx.notify();
                }
                BuiltinAction::Power(power) => self.execute_power(power, window, cx),
                BuiltinAction::Settings(setting) => match setting.open() {
                    Ok(()) => self.hide(window, cx),
                    Err(error) => {
                        self.status = format!("设置打开失败：{error:#}");
                        cx.notify();
                    }
                },
                BuiltinAction::Reminders => self.open_reminders("", window, cx),
            }
            return;
        }
        let app = if candidate.kind == Kind::App {
            None
        } else {
            opening::application_for(
                &candidate.path,
                candidate.kind == Kind::Folder,
                &self.config,
            )
        };
        match opening::open(&candidate.path, app) {
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
        if self.uninstall_confirmation.is_some()
            || self.power_confirmation.is_some()
            || self.power_busy
            || self.reminder_panel.is_some()
        {
            return;
        }
        if let Some(candidate) = self.results.get(self.selected)
            && candidate.path.as_os_str().is_empty()
        {
            self.status = "这条记录没有可定位的文件".into();
            cx.notify();
            return;
        }
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
        if self.uninstall_confirmation.is_some()
            || self.power_confirmation.is_some()
            || self.power_busy
            || self.reminder_panel.is_some()
        {
            return;
        }
        if self.query.mode == Mode::Clipboard {
            self.restore_clipboard(false, cx);
            return;
        }
        if let Some(candidate) = self.results.get(self.selected) {
            if candidate.path.as_os_str().is_empty() {
                self.status = "这项操作没有可复制的文件路径".into();
                cx.notify();
                return;
            }
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
        if self.settings.is_some() || self.reminder_panel.is_some() {
            return;
        }
        if let Some(power) = self.power_confirmation {
            if event.keystroke.key == "escape" && !self.power_busy {
                self.power_confirmation = None;
                self.input.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            } else if event.keystroke.key == "enter"
                && (modifiers.platform || modifiers.control)
                && !self.power_busy
                && !event.is_held
            {
                self.execute_power(power, window, cx);
            }
            if event.keystroke.key != "tab" {
                cx.stop_propagation();
            }
            return;
        }
        if self.power_busy {
            cx.stop_propagation();
            return;
        }
        if self.uninstall_confirmation.is_some() {
            if event.keystroke.key == "escape" && !self.uninstall_busy {
                self.uninstall_confirmation = None;
                self.input.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            }
            // Enter cannot trigger a destructive operation from the search input.
            if event.keystroke.key != "tab" {
                cx.stop_propagation();
            }
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
                    self.query.mode.prefix()
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
        let settings =
            cx.new(|cx| Settings::new(&self.config, self.catalog.apps.clone(), window, cx));
        settings.update(cx, |settings, _| {
            settings.can_install = self.update_release.is_some();
            settings.update_status = self.update_status.clone()
        });
        self.settings_subscription = Some(cx.subscribe_in(
            &settings,
            window,
            |this, _, event, window, cx| {
                match event {
                    settings::Event::Close => this.close_settings(window, cx),
                    settings::Event::OpenConfig => {
                        this.shell_event(ShellEvent::OpenConfig, window, cx)
                    }
                    settings::Event::CheckUpdates => this.check_updates(window, cx),
                    settings::Event::InstallUpdate => this.install_update(window, cx),
                    settings::Event::CancelUpdate => {
                        this.update_cancelled.store(true, Ordering::Relaxed);
                    }
                    settings::Event::ClearClipboard => {
                        if this
                            .clipboard_worker
                            .try_send(clipboard::Command::Clear)
                            .is_err()
                            && let Some(settings) = &this.settings
                        {
                            settings.update(cx, |settings, cx| {
                                settings.error = Some("剪贴板历史未运行，请重启后重试".into());
                                cx.notify();
                            });
                        }
                    }
                    settings::Event::Preview(theme) => {
                        appearance::apply(*theme, &this.config, window, cx)
                    }
                    settings::Event::Save(config) => {
                        this.save_settings(config.as_ref().clone(), window, cx)
                    }
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
        let result = self.apply_config(&config, true);
        match result {
            Ok(()) => {
                let enable_updates = config.auto_check_updates && !self.config.auto_check_updates;
                self.config = config;
                let _ = self
                    .clipboard_worker
                    .try_send(clipboard::Command::Enabled(self.config.clipboard_history));
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

    fn apply_config(&mut self, config: &Config, persist: bool) -> anyhow::Result<()> {
        if let Some(shell) = &mut self.shell {
            shell.rebind(config)?;
        }
        let result =
            starter::startup::apply(config.launch_at_login, self.config.launch_at_login, || {
                if persist {
                    config::save(&self.config_path, config)
                } else {
                    Ok(())
                }
            });
        if let Err(error) = result {
            if let Some(shell) = &mut self.shell {
                shell.rebind(&self.config).map_err(|rollback| {
                    anyhow::anyhow!("{error:#}；快捷键恢复失败：{rollback:#}")
                })?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn start_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.error_dialog.is_none() {
            self.hide(window, cx);
        } else {
            platform::show(window, cx);
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
            Kind::Uninstall => IconName::AppWindow,
            Kind::ClipboardText | Kind::ClipboardLink => IconName::FileText,
            Kind::ClipboardImage | Kind::ClipboardFiles => IconName::File,
            Kind::SystemCommand => IconName::Command,
            Kind::SystemSetting => IconName::Settings2,
            Kind::Reminder | Kind::ReminderDraft => IconName::Clock,
        };
        let thumbnail = self
            .clipboard_entries
            .iter()
            .find(|e| e.id == candidate.id)
            .and_then(|e| e.thumbnail(&self.clipboard_root));
        let image = if candidate.path.as_os_str().is_empty() {
            None
        } else {
            self.icons.get(&candidate.path)
        };
        let clipboard = self.query.mode == Mode::Clipboard;
        let clipboard_id = candidate.id.clone();
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
        let icon = if let Some(path) = thumbnail {
            icon.child(img(path).size(px(36.)).object_fit(ObjectFit::Contain))
        } else {
            match image {
                Some(image) => icon.child(img(image).size(px(36.))),
                None => icon.child(Icon::new(symbol).with_size(px(24.))),
            }
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
                    .when(clipboard, |this| {
                        this.child(
                            Button::new(("remove-clipboard", index))
                                .label("删除")
                                .ghost()
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this
                                        .clipboard_worker
                                        .try_send(clipboard::Command::Remove(clipboard_id.clone()))
                                        .is_err()
                                    {
                                        this.status = "剪贴板历史未运行，请重启后重试".into();
                                        cx.notify();
                                    }
                                })),
                        )
                    })
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
        self.update_cancelled.store(true, Ordering::Relaxed);
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.power_confirmation.is_some() {
            return self.power_dialog(cx);
        }
        if let Some(panel) = &self.reminder_panel {
            return div()
                .size_full()
                .capture_key_down(cx.listener(Self::key_down))
                .child(panel.clone())
                .into_any_element();
        }
        if self.uninstall_confirmation.is_some() {
            return self.uninstall_dialog(cx);
        }
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
        } else if matches!(self.query.mode, Mode::Files | Mode::Content)
            && self.config.search_roots.is_empty()
        {
            "先选择搜索目录"
        } else if self.query.mode == Mode::Content && self.query.text.is_empty() {
            "输入要查找的文本"
        } else if self.query.mode == Mode::Clipboard && self.clipboard_entries.is_empty() {
            if self.config.clipboard_history {
                "复制一些内容后，在这里回搜"
            } else {
                "记录已暂停，可在设置中开启"
            }
        } else {
            "没有匹配结果"
        };
        let show_recent = self.query.mode == Mode::Apps && self.query.text.is_empty();
        let recent = show_recent.then(|| self.recent(cx));
        let theme = cx.theme();
        let input_focused =
            window.is_window_active() && self.input.focus_handle(cx).is_focused(window);
        let modes = [
            (Mode::Apps, IconName::AppWindow, "全部", ""),
            (Mode::Files, IconName::FolderSearch, "文件", "/f "),
            (Mode::Content, IconName::FileText, "内容", "/c "),
            (Mode::Clipboard, IconName::FileText, "剪贴板", "/clip "),
            (Mode::Uninstall, IconName::AppWindow, "卸载", "/uninstall "),
            (Mode::System, IconName::Settings2, "系统", "/system "),
            (Mode::Reminders, IconName::Clock, "提醒", "/remind "),
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
                                Button::new("reminder-badge")
                                    .label(self.reminder_badge())
                                    .icon(IconName::Clock)
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_reminders("", window, cx)
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
                                    if mode == Mode::Reminders {
                                        this.open_reminders("", window, cx);
                                        return;
                                    }
                                    this.input.update(cx, |input, cx| {
                                        input.set_value(prefix, window, cx);
                                        input.focus(window, cx);
                                    });
                                    this.search(cx);
                                }))
                        },
                    ))),
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
                                .child(if self.query.mode == Mode::Clipboard {
                                    "输入关键词，或用 图片 / 文件 / 链接 / 文本 / 日期 筛选"
                                } else {
                                    "试试 bluetooth、锁屏，或 /remind 10m 开会"
                                }),
                        )
                        .when(
                            matches!(self.query.mode, Mode::Files | Mode::Content)
                                && self.config.search_roots.is_empty(),
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
                            .child(if self.query.mode == Mode::Clipboard {
                                "↑↓ 选择   ↵ 复制并收起   ⇧↵ 复制"
                            } else if self.query.mode == Mode::Uninstall {
                                "↑↓ 选择   ↵ 查看卸载确认   Esc 返回"
                            } else if self
                                .results
                                .get(self.selected)
                                .is_some_and(|c| c.path.as_os_str().is_empty())
                            {
                                "↑↓ 选择   ↵ 执行 / 查看   Esc 返回"
                            } else if cfg!(target_os = "macos") {
                                "↑↓ 选择   ↵ 打开   ⌘↵ 定位   ⇧↵ 复制"
                            } else {
                                "↑↓ 选择   ↵ 打开   Ctrl+↵ 定位   ⇧↵ 复制"
                            }),
                    ),
            )
            .into_any_element()
    }
}
