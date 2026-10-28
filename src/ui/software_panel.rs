use super::task_terminal::TerminalView;
use crate::icons::Icons;
use gpui_kit::{
    assets::IconName,
    base::{Disableable, TestSupportExt},
    component::{
        ActiveTheme, Icon, Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
        menu::{PopupMenu, PopupMenuItem},
    },
    prelude::FluentBuilder,
    *,
};
use starter::{
    catalog::Application,
    config::ThemeName,
    software_updates::{
        Action, Inventory, Outcome, Plan, ScanMode, Snapshot, Software, UpdateState, finish_step,
    },
    task_terminal::{self, Session, Size},
};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const ROW_HEIGHT: f32 = 49.;
const COLUMN_GAP: f32 = 8.;
const ICON_WIDTH: f32 = 16.;
const INSTALLED_WIDTH: f32 = 82.;
const STATUS_WIDTH: f32 = 88.;

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Available,
    Tools,
    Apps,
}
pub(super) struct SoftwarePanel {
    input: Entity<InputState>,
    inventory: Arc<Inventory>,
    apps: Vec<Application>,
    icons: Icons,
    snapshot: Arc<Snapshot>,
    visible: Vec<usize>,
    selected: usize,
    checked_ids: HashSet<String>,
    filter: Filter,
    scroll: UniformListScrollHandle,
    scanning: bool,
    menu: Option<(Entity<PopupMenu>, Point<Pixels>)>,
    menu_focus: FocusHandle,
    theme: ThemeName,
    terminal: Option<Entity<TerminalView>>,
    plan: Option<Plan>,
    outcomes: Vec<Outcome>,
    show_task: bool,
    task_started: bool,
    running: bool,
    stopping: bool,
    feedback: String,
    show_warnings: bool,
    cancelled: Arc<AtomicBool>,
    scan_cancelled: Arc<AtomicBool>,
    task_cancelled: Arc<AtomicBool>,
    _scan: Option<Task<()>>,
    _run: Option<Task<()>>,
    _menu: Option<Subscription>,
    _input: Subscription,
    _quit: Subscription,
}
pub(super) struct Close;
impl EventEmitter<Close> for SoftwarePanel {}
impl SoftwarePanel {
    pub(super) fn new(
        theme: ThemeName,
        inventory: Arc<Inventory>,
        apps: Vec<Application>,
        icons: Icons,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::from_inventory(theme, inventory, apps, icons, window, cx);
        this.scan(ScanMode::Cached, window, cx);
        this
    }
    fn from_inventory(
        theme: ThemeName,
        inventory: Arc<Inventory>,
        apps: Vec<Application>,
        icons: Icons,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索软件名称、来源…"));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter_rows(cx);
            }
        });
        let quit = cx.on_app_quit(|this, cx| {
            this.cancelled.store(true, Ordering::Relaxed);
            this.scan_cancelled.store(true, Ordering::Relaxed);
            this.task_cancelled.store(true, Ordering::Relaxed);
            let finished = this
                .terminal
                .as_ref()
                .and_then(|t| t.update(cx, |terminal, _| terminal.shutdown()));
            let run = this._run.take();
            let scan = this._scan.take();
            async move {
                if let Some(run) = run {
                    run.await;
                }
                if let Some(finished) = finished {
                    let _ = finished.recv().await;
                }
                if let Some(scan) = scan {
                    scan.await;
                }
            }
        });
        let snapshot = inventory.cached().unwrap_or_default();
        let mut this = Self {
            input,
            inventory,
            apps,
            icons,
            snapshot,
            visible: vec![],
            selected: 0,
            checked_ids: HashSet::new(),
            filter: Filter::All,
            scroll: UniformListScrollHandle::new(),
            scanning: false,
            menu: None,
            menu_focus: cx.focus_handle(),
            theme,
            terminal: None,
            plan: None,
            outcomes: vec![],
            show_task: false,
            task_started: false,
            running: false,
            stopping: false,
            feedback: String::new(),
            show_warnings: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            scan_cancelled: Arc::new(AtomicBool::new(false)),
            task_cancelled: Arc::new(AtomicBool::new(false)),
            _scan: None,
            _run: None,
            _menu: None,
            _input: subscription,
            _quit: quit,
        };
        this.filter_rows(cx);
        this
    }
    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        if self.show_task {
            if let Some(terminal) = &self.terminal {
                terminal.update(cx, |t, cx| t.focus(window, cx));
            } else {
                window.focus(&self.menu_focus, cx);
            }
        } else if let Some((menu, _)) = &self.menu {
            menu.focus_handle(cx).focus(window, cx);
        } else {
            self.input.update(cx, |input, cx| input.focus(window, cx));
        }
    }
    fn scan(&mut self, mode: ScanMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.scanning || self.running || self.cancelled.load(Ordering::Relaxed) {
            return;
        }
        self.menu = None;
        self.scanning = true;
        let inventory = self.inventory.clone();
        let apps = self.apps.clone();
        self.scan_cancelled = Arc::new(AtomicBool::new(false));
        let cancelled = self.scan_cancelled.clone();
        let operation = cx
            .background_executor()
            .spawn(async move { inventory.scan(mode, &apps, &cancelled) });
        self._scan = Some(cx.spawn_in(window, async move |this, cx| {
            let snapshot = operation.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let selected_id = this
                    .visible
                    .get(this.selected)
                    .map(|&index| this.snapshot.items[index].id.clone());
                this.snapshot = snapshot;
                this.scanning = false;
                this.checked_ids.retain(|id| {
                    this.snapshot
                        .items
                        .iter()
                        .any(|item| &item.id == id && item.can_update())
                });
                this.rebuild_rows(cx);
                this.selected = selected_id
                    .and_then(|id| {
                        this.visible
                            .iter()
                            .position(|&i| this.snapshot.items[i].id == id)
                    })
                    .unwrap_or(this.selected.min(this.visible.len().saturating_sub(1)));
                if mode == ScanMode::Cached && !this.snapshot.checked_online {
                    this.scan(ScanMode::Background, window, cx);
                }
            });
        }));
        cx.notify();
    }
    fn filter_rows(&mut self, cx: &mut Context<Self>) {
        self.rebuild_rows(cx);
        self.selected = 0;
        self.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
        self.menu = None;
        cx.notify();
    }
    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_lowercase();
        self.visible = self
            .snapshot
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                (match self.filter {
                    Filter::All => true,
                    Filter::Available => item.state == UpdateState::Available,
                    Filter::Tools => !item.source.is_app(),
                    Filter::Apps => item.source.is_app(),
                }) && (query.is_empty() || item.search.contains(&query))
            })
            .map(|(i, _)| i)
            .collect();
        cx.notify();
    }
    fn review(&mut self, selected: HashSet<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        match Plan::selected(&self.snapshot, &selected) {
            Ok(plan) => self.review_plan(plan, window, cx),
            Err(error) => {
                self.feedback = error.to_string();
                cx.notify();
            }
        }
    }
    fn review_plan(&mut self, plan: Plan, window: &mut Window, cx: &mut Context<Self>) {
        self.outcomes = vec![Outcome::Pending; plan.steps.len()];
        self.plan = Some(plan);
        self.show_task = true;
        self.task_started = false;
        self.terminal = None;
        self.menu = None;
        self.feedback.clear();
        self.focus(window, cx);
        cx.notify();
    }
    fn activate(&mut self, item: Arc<Software>, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        match &item.action {
            Action::Command(_) if item.can_update() => {
                self.review(HashSet::from([item.id.clone()]), window, cx);
                if self.show_task {
                    self.begin(window, cx);
                    self.show_task = false;
                    self.focus(window, cx);
                }
            }
            Action::OpenApp(path) => {
                if let Err(error) = crate::platform::open_target(path) {
                    self.feedback = format!("无法打开：{error:#}");
                }
                self.close_menu(window, cx);
            }
            Action::OpenUrl(url) => {
                cx.open_url(url);
                self.close_menu(window, cx);
            }
            _ => {}
        }
    }
    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running || self.task_started {
            return;
        }
        let Some(plan) = self.plan.clone() else {
            return;
        };
        self.scan_cancelled.store(true, Ordering::Relaxed);
        self._scan = None;
        self.scanning = false;
        self.running = true;
        self.stopping = false;
        self.task_started = true;
        self.terminal = Some(cx.new(|cx| TerminalView::new(self.theme, cx)));
        self.feedback = "正在核对安装来源和固定状态…".into();
        self.focus(window, cx);
        self.task_cancelled = Arc::new(AtomicBool::new(false));
        let cancelled = self.task_cancelled.clone();
        let validation_plan = plan.clone();
        let validation = cx
            .background_executor()
            .spawn(async move { validation_plan.validate(&cancelled) });
        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            if let Err(error) = validation.await {
                let _ = this.update_in(cx, |this, _, cx| {
                    this.running = false;
                    this.outcomes.fill(Outcome::Skipped);
                    this.feedback = format!("未执行：{error:#}");
                    cx.notify();
                });
                return;
            }
            for index in 0..plan.steps.len() {
                let ready = this
                    .update_in(cx, |this, _, cx| {
                        if this.cancelled.load(Ordering::Relaxed) || this.stopping {
                            this.outcomes[index..].fill(Outcome::Skipped);
                            cx.notify();
                            return false;
                        }
                        this.outcomes[index] = Outcome::Running;
                        this.feedback = format!(
                            "正在更新 {} ({}/{})",
                            plan.steps[index].name,
                            index + 1,
                            plan.steps.len()
                        );
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !ready {
                    break;
                }
                let request = plan.request(index);
                let preparation = cx.background_executor().spawn(async move {
                    Session::start(
                        request,
                        Size {
                            rows: 18,
                            cols: 88,
                            ..Size::default()
                        },
                    )
                });
                let (session, events) = match preparation.await {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = this.update_in(cx, |this, _, cx| {
                            finish_step(&mut this.outcomes, index, 1, false);
                            this.feedback =
                                format!("{} 启动失败：{error:#}", plan.steps[index].name);
                            cx.notify();
                        });
                        break;
                    }
                };
                let finished = session.completion();
                let started = this
                    .update_in(cx, |this, _, cx| {
                        if this.cancelled.load(Ordering::Relaxed) || this.stopping {
                            finish_step(&mut this.outcomes, index, 130, true);
                            return false;
                        }
                        if let Some(terminal) = &this.terminal {
                            terminal.update(cx, |terminal, cx| {
                                terminal.feed(
                                    &task_terminal::Event::Output(
                                        format!(
                                            "\r\n→ {}\r\n",
                                            plan.steps[index].command.display()
                                        )
                                        .into_bytes(),
                                    ),
                                    cx,
                                );
                                terminal.attach(session, cx);
                            });
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !started {
                    let _ = finished.recv().await;
                    break;
                }
                let mut continuation = false;
                let mut exited = false;
                while let Ok(event) = events.recv().await {
                    let mut batch = vec![event];
                    while batch.len() < 16 {
                        let Ok(event) = events.try_recv() else {
                            break;
                        };
                        batch.push(event);
                    }
                    let result = this.update_in(cx, |this, _, cx| {
                        for event in batch {
                            if let Some(terminal) = &this.terminal {
                                terminal.update(cx, |t, cx| t.feed(&event, cx));
                            }
                            if let task_terminal::Event::Exit { code, stopped } = event {
                                exited = true;
                                continuation = finish_step(
                                    &mut this.outcomes,
                                    index,
                                    code,
                                    stopped || this.stopping,
                                );
                            }
                        }
                        cx.notify();
                    });
                    if result.is_err() || exited {
                        break;
                    }
                }
                let _ = finished.recv().await;
                if !exited {
                    let _ = this.update_in(cx, |this, _, cx| {
                        finish_step(&mut this.outcomes, index, 1, false);
                        cx.notify();
                    });
                }
                if !continuation {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.running = false;
                let completed = this
                    .outcomes
                    .iter()
                    .filter(|o| **o == Outcome::Succeeded)
                    .count();
                this.feedback = format!(
                    "已完成 {completed}/{} 项；失败或中断后，后续项目不会执行。",
                    this.outcomes.len()
                );
                if let Some(plan) = &this.plan {
                    for (step, outcome) in plan.steps.iter().zip(&this.outcomes) {
                        if *outcome == Outcome::Succeeded {
                            this.checked_ids.remove(&step.id);
                        }
                    }
                }
                this.inventory.invalidate();
                this.scan(ScanMode::Local, window, cx);
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        if self.show_task {
            self.show_task = false;
            self.terminal = None;
            self.focus(window, cx);
            cx.notify();
        } else {
            cx.emit(Close);
        }
    }
    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.focus(window, cx);
        cx.notify();
    }
    fn open_menu(
        &mut self,
        item: Arc<Software>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blocked = self.running;
        let activate = item.clone();
        let label = match &item.action {
            Action::Command(_) if item.state == UpdateState::Available => item
                .latest
                .as_ref()
                .map(|v| format!("更新到 {v}"))
                .unwrap_or_else(|| "更新".into()),
            Action::Command(_) => "检查并更新".into(),
            Action::OpenUrl(url) if url.starts_with("macappstore:") => "在 App Store 中更新".into(),
            Action::OpenUrl(_) => "打开更新页面".into(),
            _ => "在应用中更新".into(),
        };
        let mut entries = vec![
            self.menu_item(label, cx, move |this, window, cx| {
                this.activate(activate.clone(), window, cx);
            })
            .disabled(
                blocked
                    || matches!(item.action, Action::Manual)
                    || (matches!(item.action, Action::Command(_)) && !item.can_update()),
            ),
        ];
        if item.pin.is_some() {
            let pin = item.clone();
            entries.push(
                self.menu_item(
                    if item.pinned {
                        "取消固定版本"
                    } else {
                        "固定当前版本"
                    },
                    cx,
                    move |this, window, cx| {
                        if let Some(plan) = Plan::pin(&this.snapshot, &pin) {
                            this.review_plan(plan, window, cx);
                            this.begin(window, cx);
                            this.show_task = false;
                            this.focus(window, cx);
                        }
                    },
                )
                .disabled(blocked),
            );
        }
        if item.can_update() {
            let toggle = item.clone();
            entries.push(
                self.menu_item(
                    if self.checked_ids.contains(&item.id) {
                        "取消选择"
                    } else {
                        "加入多选"
                    },
                    cx,
                    move |this, window, cx| {
                        if !this.checked_ids.remove(&toggle.id) {
                            this.checked_ids.insert(toggle.id.clone());
                        }
                        this.close_menu(window, cx);
                    },
                )
                .disabled(blocked),
            );
        }
        entries.push(PopupMenuItem::separator());
        if let Some(path) = item.path.clone() {
            entries.push(self.menu_item("在访达中显示", cx, move |this, window, cx| {
                if let Err(error) = crate::platform::reveal(&path) {
                    this.feedback = format!("无法定位：{error:#}");
                }
                this.close_menu(window, cx);
            }));
        }
        if item.homepage.starts_with("https://") || item.homepage.starts_with("http://") {
            let url = item.homepage.clone();
            entries.push(self.menu_item("打开主页", cx, move |this, window, cx| {
                cx.open_url(&url);
                this.close_menu(window, cx);
            }));
        }
        entries.push(PopupMenuItem::separator());
        let name = item.name.clone();
        entries.push(self.menu_item("拷贝名称", cx, move |this, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(name.clone()));
            this.close_menu(window, cx);
        }));
        if let Some(path) = item.path.clone() {
            entries.push(self.menu_item("拷贝路径", cx, move |this, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
                this.close_menu(window, cx);
            }));
        }
        let focus = self.input.read(cx).focus_handle(cx);
        let menu = PopupMenu::build(window, cx, |menu, _, _| {
            entries.into_iter().fold(
                menu.action_context(focus)
                    .min_w(px(210.))
                    .max_w(px(290.))
                    .scrollable(true),
                |menu, item| menu.item(item),
            )
        });
        self._menu = Some(cx.subscribe_in(
            &menu,
            window,
            |this, _, _: &DismissEvent, window, cx| this.close_menu(window, cx),
        ));
        menu.focus_handle(cx).focus(window, cx);
        self.menu = Some((menu, position));
        cx.notify();
    }
    fn menu_item(
        &self,
        label: impl Into<SharedString>,
        cx: &Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> PopupMenuItem {
        let panel = cx.entity().downgrade();
        let label = label.into();
        let icon =
            if label.starts_with("更新") || label.starts_with("检查") || label.contains("中更新")
            {
                IconName::Download
            } else if label.contains("固定") {
                IconName::Pin
            } else if label.contains("访达") {
                IconName::FolderOpen
            } else if label.contains("拷贝") {
                IconName::Copy
            } else if label.contains("选择") || label.contains("多选") {
                IconName::Check
            } else {
                IconName::ExternalLink
            };
        PopupMenuItem::new(label)
            .icon(icon)
            .on_click(move |_, window, cx| {
                let _ = panel.update(cx, |this, cx| action(this, window, cx));
            })
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            return;
        }
        let key = &event.keystroke;
        if key.key == "escape" {
            self.back(window, cx);
            cx.stop_propagation();
            return;
        }
        if self.show_task
            || self
                .input
                .update(cx, |i, cx| i.marked_text_range(window, cx).is_some())
        {
            return;
        }
        if key.modifiers.platform && key.key == "r" {
            self.scan(ScanMode::Check, window, cx);
        } else if !key.modifiers.platform && !key.modifiers.control && !key.modifiers.alt {
            match key.key.as_str() {
                "up" | "down" if !self.visible.is_empty() => {
                    self.selected = if key.key == "up" {
                        self.selected.saturating_sub(1)
                    } else {
                        (self.selected + 1).min(self.visible.len() - 1)
                    };
                    self.scroll
                        .scroll_to_item(self.selected, ScrollStrategy::Nearest);
                }
                "enter" if !event.is_held => {
                    if let Some(&index) = self.visible.get(self.selected) {
                        self.open_menu(
                            self.snapshot.items[index].clone(),
                            {
                                let scroll = self.scroll.0.borrow();
                                let bounds = scroll.base_handle.bounds();
                                let top = bounds.top()
                                    + scroll.base_handle.offset().y
                                    + px(ROW_HEIGHT) * self.selected;
                                point(
                                    bounds.right() - px(32.),
                                    top.clamp(bounds.top(), bounds.bottom() - px(24.)) + px(24.),
                                )
                            },
                            window,
                            cx,
                        );
                    }
                }
                _ => return,
            }
        } else {
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn row(&mut self, index: usize, cx: &Context<Self>) -> AnyElement {
        let item = self.snapshot.items[self.visible[index]].clone();
        let theme = cx.theme();
        let select = item.clone();
        let right = item.clone();
        let checked = self.checked_ids.contains(&item.id);
        let updating = self.running
            && self.plan.as_ref().is_some_and(|plan| {
                plan.steps
                    .iter()
                    .zip(&self.outcomes)
                    .any(|(step, outcome)| step.id == item.id && *outcome == Outcome::Running)
            });
        let image = item
            .path
            .as_ref()
            .filter(|path| path.extension().is_some_and(|extension| extension == "app"))
            .and_then(|path| self.icons.get(path));
        let icon = match image {
            Some(image) => div()
                .id(("software-icon-image", index))
                .test_support()
                .size(px(ICON_WIDTH))
                .child(
                    img(image)
                        .size(px(ICON_WIDTH))
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            None => div()
                .id(("software-icon-fallback", index))
                .test_support()
                .size(px(ICON_WIDTH))
                .child(
                    Icon::new(if item.source.is_app() {
                        IconName::AppWindow
                    } else {
                        IconName::Command
                    })
                    .with_size(px(ICON_WIDTH))
                    .text_color(theme.muted_foreground),
                )
                .into_any_element(),
        };
        div()
            .id(("software-row", index))
            .test_support()
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_3()
            .child(
                div()
                    .size_full()
                    .px_2()
                    .rounded_md()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(COLUMN_GAP))
                    .when(self.selected == index || checked, |r| r.bg(theme.accent))
                    .hover(|r| r.bg(theme.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            if event.modifiers.shift {
                                for i in this.selected.min(index)..=this.selected.max(index) {
                                    let item = &this.snapshot.items[this.visible[i]];
                                    if item.can_update() {
                                        this.checked_ids.insert(item.id.clone());
                                    }
                                }
                            } else if event.modifiers.platform {
                                if select.can_update() && !this.checked_ids.remove(&select.id) {
                                    this.checked_ids.insert(select.id.clone());
                                }
                            } else {
                                this.checked_ids.clear();
                            }
                            this.selected = index;
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.selected = index;
                            this.open_menu(right.clone(), event.position, window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(div().w(px(ICON_WIDTH)).flex_shrink_0().child(icon))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(item.name.clone()),
                            )
                            .child(
                                div()
                                    .id(("software-description", index))
                                    .test_support()
                                    .w_full()
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .truncate()
                                    .child(format!(
                                        "{} · {}",
                                        item.source.label(),
                                        item.description
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id(("software-installed", index))
                            .test_support()
                            .w(px(INSTALLED_WIDTH))
                            .min_w_0()
                            .flex_shrink_0()
                            .text_right()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(item.installed.clone()),
                    )
                    .child(
                        div()
                            .id(("software-status", index))
                            .test_support()
                            .w(px(STATUS_WIDTH))
                            .min_w_0()
                            .flex_shrink_0()
                            .text_right()
                            .text_size(px(12.))
                            .truncate()
                            .text_color(if item.state == UpdateState::Available {
                                theme.primary
                            } else {
                                theme.muted_foreground
                            })
                            .child(if updating {
                                "正在更新…".into()
                            } else {
                                item.status().to_string()
                            }),
                    ),
            )
            .into_any_element()
    }
    fn menu(&self, _: &Context<Self>) -> AnyElement {
        let Some((menu, position)) = &self.menu else {
            return div().into_any_element();
        };
        deferred(
            anchored()
                .position(*position)
                .snap_to_window_with_margin(px(8.))
                .child(menu.clone()),
        )
        .with_priority(1)
        .into_any_element()
    }
    fn task(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let plan = self.plan.as_ref().expect("task has plan");
        let mut body = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .px_5();
        if !self.task_started {
            body = body.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("仅执行下列选中项。管理器可能调整依赖；没有已知候选版本的项目将在执行时检查。任一项失败或中断后停止队列。"),
            );
        }
        body = body.child(
            uniform_list(
                "software-plan",
                plan.steps.len(),
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    let plan = this.plan.as_ref().unwrap();
                    range
                        .map(|i| {
                            div()
                                .id(("software-plan-row", i))
                                .test_support()
                                .w_full()
                                .h(px(58.))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .child(plan.steps[i].name.clone()),
                                        )
                                        .child(
                                            div().flex_shrink_0().child(this.outcomes[i].label()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(cx.theme().muted_foreground)
                                        .truncate()
                                        .child(plan.steps[i].command.display()),
                                )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .min_h_0()
            .when(self.task_started, |list| list.h(px(116.)).flex_shrink_0())
            .when(!self.task_started, |list| list.flex_1())
            .w_full(),
        );
        if let Some(terminal) = &self.terminal {
            body = body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .child(terminal.clone()),
            );
        }
        body.child(
            div()
                .text_size(px(12.))
                .flex_shrink_0()
                .truncate()
                .text_color(theme.muted_foreground)
                .child(self.feedback.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .gap_2()
                .pb_4()
                .child(div().flex_1())
                .when(!self.task_started, |r| {
                    r.child(
                        Button::new("confirm-software-plan")
                            .label(format!("开始执行 {} 项", plan.steps.len()))
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| this.begin(window, cx))),
                    )
                })
                .when(self.terminal.is_some(), |r| {
                    r.child(
                        Button::new("copy-software-output")
                            .label("拷贝输出")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(t) = &this.terminal {
                                    t.update(cx, |t, cx| t.copy(cx));
                                }
                            })),
                    )
                })
                .when(self.running, |r| {
                    r.child(
                        Button::new("stop-software-update")
                            .label(if self.stopping {
                                "正在中断…"
                            } else {
                                "中断队列"
                            })
                            .small()
                            .disabled(self.stopping)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stopping = true;
                                this.task_cancelled.store(true, Ordering::Relaxed);
                                if let Some(t) = &this.terminal {
                                    t.update(cx, |t, _| t.stop());
                                }
                                cx.notify();
                            })),
                    )
                })
                .when(!self.running, |r| {
                    r.child(
                        Button::new("return-software-list")
                            .label("返回列表")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                    )
                }),
        )
        .into_any_element()
    }
}
impl Drop for SoftwarePanel {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.scan_cancelled.store(true, Ordering::Relaxed);
        self.task_cancelled.store(true, Ordering::Relaxed);
    }
}
impl Render for SoftwarePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let available = self
            .snapshot
            .items
            .iter()
            .filter(|i| i.state == UpdateState::Available)
            .count();
        let mut body = div()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .capture_key_down(cx.listener(Self::key_down))
            .child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("software-back")
                            .icon(IconName::ChevronLeft)
                            .ghost()
                            .small()
                            .disabled(self.running)
                            .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(if self.show_task {
                                        if self.running {
                                            "正在更新"
                                        } else if self.task_started {
                                            "更新结果"
                                        } else {
                                            "更新所选软件"
                                        }
                                    } else {
                                        "软件更新"
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child(if self.scanning {
                                        "正在检查更新…".into()
                                    } else {
                                        format!(
                                            "{} 个软件 · {} 个可更新",
                                            self.snapshot.items.len(),
                                            available
                                        )
                                    }),
                            ),
                    )
                    .when(!self.show_task, |r| {
                        r.child(
                            Button::new("refresh-software")
                                .label("检查更新")
                                .small()
                                .ghost()
                                .disabled(self.scanning || self.running)
                                .tooltip("刷新支持检查的来源 · ⌘R")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.scan(ScanMode::Check, window, cx)
                                })),
                        )
                        .when(!self.checked_ids.is_empty(), |r| {
                            r.child(
                                Button::new("review-selected-software")
                                    .label(format!("更新所选 {}", self.checked_ids.len()))
                                    .small()
                                    .disabled(self.running)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.review(this.checked_ids.clone(), window, cx)
                                    })),
                            )
                        })
                    }),
            );
        if self.show_task {
            return body.child(self.task(cx)).into_any_element();
        }
        body = body
            .child(div().flex_shrink_0().px_5().pb_3().child(
                Input::new(&self.input).prefix(Icon::new(IconName::Search).with_size(px(16.))),
            ))
            .child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .pb_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(
                        [
                            (Filter::All, "全部"),
                            (Filter::Available, "可更新"),
                            (Filter::Tools, "命令行"),
                            (Filter::Apps, "应用"),
                        ]
                        .map(|(filter, label)| {
                            Button::new(label)
                                .label(label)
                                .small()
                                .ghost()
                                .when(self.filter == filter, |b| b.bg(theme.accent))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.filter = filter;
                                    this.filter_rows(cx);
                                }))
                        }),
                    )
                    .child(div().flex_1())
                    .when(!self.checked_ids.is_empty(), |r| {
                        r.child(
                            Button::new("clear-chosen-updates")
                                .label("取消选择")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.checked_ids.clear();
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(px(COLUMN_GAP))
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl(px(ICON_WIDTH + COLUMN_GAP))
                            .truncate()
                            .child(format!("软件 · {}", self.visible.len())),
                    )
                    .child(
                        div()
                            .id("software-installed-heading")
                            .test_support()
                            .w(px(INSTALLED_WIDTH))
                            .flex_shrink_0()
                            .text_right()
                            .child("当前版本"),
                    )
                    .child(
                        div()
                            .id("software-status-heading")
                            .test_support()
                            .w(px(STATUS_WIDTH))
                            .flex_shrink_0()
                            .text_right()
                            .child("可用版本 / 状态"),
                    ),
            )
            .child(div().h(px(1.)).flex_shrink_0().mx_5().bg(theme.border));
        if self.visible.is_empty() {
            body = body.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.muted_foreground)
                    .child(if self.scanning {
                        "正在检测软件…"
                    } else if self.filter == Filter::Available {
                        "暂未发现可更新的软件"
                    } else {
                        "没有匹配的软件"
                    }),
            );
        } else {
            body = body.child(
                uniform_list(
                    "software-list",
                    self.visible.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|i| this.row(i, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0()
                .min_w_0()
                .w_full(),
            );
        }
        if !self.snapshot.warnings.is_empty() {
            body = body.child(
                div()
                    .flex_shrink_0()
                    .min_w_0()
                    .px_5()
                    .child(
                        Button::new("software-warning-summary")
                            .label(format!(
                                "{} 条来源检查提示 · {}",
                                self.snapshot.warnings.len(),
                                if self.show_warnings {
                                    "收起原因"
                                } else {
                                    "查看原因"
                                }
                            ))
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_warnings = !this.show_warnings;
                                cx.notify();
                            })),
                    )
                    .when(self.show_warnings, |body| {
                        body.child(
                            div()
                                .id("software-warning-details")
                                .w_full()
                                .min_w_0()
                                .max_h(px(120.))
                                .overflow_scroll()
                                .p_3()
                                .mb_2()
                                .border_1()
                                .border_color(theme.border)
                                .rounded_md()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .whitespace_normal()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(self.snapshot.warnings.iter().enumerate().map(
                                    |(index, warning)| {
                                        div()
                                            .flex_shrink_0()
                                            .child(format!("{}. {warning}", index + 1))
                                    },
                                )),
                        )
                    }),
            );
        }
        if !self.feedback.is_empty() {
            body = body.child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(self.feedback.clone()),
            );
        }
        body.child(
            div()
                .id("software-footer")
                .test_support()
                .flex_shrink_0()
                .px_5()
                .py_3()
                .border_t_1()
                .border_color(theme.border)
                .flex()
                .gap_2()
                .items_center()
                .text_size(px(11.))
                .text_color(theme.muted_foreground)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(if self.snapshot.checked_online {
                            "已检查公开版本源"
                        } else {
                            "正在读取本地清单与版本信息"
                        }),
                )
                .child("右键 / Enter 操作 · ⌘点击多选")
                .when(self.plan.is_some() && self.task_started, |r| {
                    r.child(
                        Button::new("last-software-results")
                            .label(if self.running {
                                "查看进度"
                            } else {
                                "上次结果"
                            })
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_task = true;
                                this.focus(window, cx);
                                cx.notify();
                            })),
                    )
                }),
        )
        .when(self.menu.is_some(), |body| body.child(self.menu(cx)))
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::{base::Root, test::TestWindowExt};
    use starter::software_updates::{Invocation, Source};
    fn software(name: &str, state: UpdateState) -> Arc<Software> {
        Arc::new(Software {
            id: name.into(),
            name: name.into(),
            source: Source::Npm,
            installed: "1".into(),
            latest: None,
            state,
            pinned: false,
            description: String::new(),
            homepage: String::new(),
            path: None,
            action: Action::Command(Invocation {
                program: "/usr/bin/false".into(),
                args: vec![name.into()],
            }),
            pin: None,
            search: name.to_lowercase(),
        })
    }
    #[gpui_kit::test]
    fn application_rows_switch_from_fallback_to_shared_bundle_icons(cx: &mut TestAppContext) {
        use crate::icons::Loaded;
        use image::{Frame, RgbaImage};
        use std::path::PathBuf;

        cx.update(gpui_kit::init);
        let paths = [
            PathBuf::from("/Applications/Store.app"),
            PathBuf::from("/Applications/Brew.app"),
            PathBuf::from("/Applications/Missing.app"),
            PathBuf::from("/opt/homebrew/Cellar/tool/1"),
        ];
        let mut icons = Icons::new().0;
        for path in &paths {
            icons.insert(Loaded {
                path: path.clone(),
                image: None,
            });
        }
        let (handle, panel) = cx
            .update(|cx| {
                gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        let mut panel = SoftwarePanel::from_inventory(
                            ThemeName::Catppuccin,
                            Arc::default(),
                            vec![],
                            icons.clone(),
                            window,
                            cx,
                        );
                        let mut snapshot = Snapshot::default();
                        snapshot.items = [
                            Source::AppStore,
                            Source::Cask,
                            Source::Application,
                            Source::Formula,
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(i, source)| {
                            let mut item =
                                (*software(&format!("item-{i}"), UpdateState::Manual)).clone();
                            item.source = source;
                            item.path = Some(paths[i].clone());
                            Arc::new(item)
                        })
                        .collect();
                        panel.snapshot = Arc::new(snapshot);
                        panel.filter_rows(cx);
                        panel
                    })
                })
            })
            .unwrap();
        let handle: WindowHandle<Root> = handle.downcast().unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            for i in 0_usize..4 {
                assert!(window.find(("software-icon-fallback", i)).visible());
            }
            let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(16, 16))]));
            for path in [&paths[0], &paths[1], &paths[3]] {
                icons.insert(Loaded {
                    path: path.clone(),
                    image: Some(image.clone()),
                });
            }
            panel.update(cx, |_, cx| cx.notify());
            window.render_frame(cx);
            for i in 0_usize..2 {
                assert!(window.find(("software-icon-image", i)).visible());
                assert!(window.try_find(("software-icon-fallback", i)).is_none());
            }
            for i in 2_usize..4 {
                assert!(window.find(("software-icon-fallback", i)).visible());
            }
        })
        .unwrap();
    }
    #[gpui_kit::test]
    fn long_software_rows_keep_columns_aligned_and_actions_inside_the_window(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, panel) = cx
            .update(|cx| {
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
                            let mut panel = SoftwarePanel::from_inventory(
                                ThemeName::Catppuccin,
                                Arc::default(),
                                vec![],
                                Icons::new().0,
                                window,
                                cx,
                            );
                            let mut long = (*software("long-tool", UpdateState::Available)).clone();
                            long.name = "Very long application 名称 ".repeat(8);
                            long.description =
                                "Resource monitor with a very long description ".repeat(12);
                            long.installed = "2026.10.27.1234567890+build.1234567890".into();
                            long.latest = Some("2026.10.28.1234567890+build.1234567890".into());
                            let mut snapshot = Snapshot::default();
                            snapshot.items = vec![
                                Arc::new(long),
                                software("dbx", UpdateState::Available),
                                software("gh", UpdateState::Unchecked),
                            ];
                            snapshot.items.extend(
                                (3..30).map(|i| {
                                    software(&format!("tool-{i}"), UpdateState::Unchecked)
                                }),
                            );
                            snapshot.warnings.push("unavailable source".into());
                            panel.feedback = "软件源检查提示 ".repeat(50);
                            panel.snapshot = Arc::new(snapshot);
                            panel.filter_rows(cx);
                            panel
                        })
                    },
                )
            })
            .unwrap();
        let handle: WindowHandle<Root> = handle.downcast().unwrap();
        cx.run_until_parked();
        for width in [720., 600.] {
            cx.simulate_window_resize(handle.into(), size(px(width), px(560.)));
            cx.update_window(handle.into(), |_, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
                    cx.notify();
                });
                window.render_frame(cx);
                let installed = window.find("software-installed-heading").bounds();
                let status = window.find("software-status-heading").bounds();
                let footer = window.find("software-footer").bounds();
                assert_eq!(footer.bottom(), px(560.));
                for i in 0_usize..3 {
                    let row = window.find(("software-row", i)).bounds();
                    let description = window.find(("software-description", i)).bounds();
                    let current = window.find(("software-installed", i)).bounds();
                    let candidate = window.find(("software-status", i)).bounds();
                    assert!(row.right() <= px(width), "row exceeds window: {row:?}");
                    assert_eq!(
                        current.origin.x, installed.origin.x,
                        "current version column drifted"
                    );
                    assert_eq!(
                        candidate.origin.x, status.origin.x,
                        "candidate version column drifted"
                    );
                    assert!(
                        description.right() <= current.origin.x,
                        "description exceeds its column: {description:?}, {current:?}"
                    );
                    assert!(current.right() <= candidate.origin.x);
                    assert!(candidate.right() <= px(width));
                    for id in ["choose-software", "software-update", "software-menu"] {
                        assert!(
                            window.try_find((id, i)).is_none(),
                            "redundant row action: {id}"
                        );
                    }
                }
                panel.update(cx, |panel, cx| {
                    panel
                        .scroll
                        .scroll_to_item_strict(29, ScrollStrategy::Bottom);
                    cx.notify();
                });
                window.render_frame(cx);
                let last = window.find(("software-row", 29_usize)).bounds();
                assert!(
                    last.bottom() <= footer.origin.y,
                    "last row overlaps footer: {last:?}, {footer:?}"
                );
                assert!(window.find(("software-row", 29_usize)).visible());
            })
            .unwrap();
        }
        cx.simulate_window_resize(handle.into(), size(px(600.), px(400.)));
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                panel.review(HashSet::from(["long-tool".into()]), window, cx);
            });
            window.render_frame(cx);
            let row = window.find(("software-plan-row", 0_usize)).bounds();
            assert!(row.right() <= px(600.));
            for id in ["confirm-software-plan", "return-software-list"] {
                let button = window.find(id);
                assert!(button.visible());
                assert!(button.bounds().bottom() <= px(400.));
                assert!(button.bounds().right() <= px(600.));
                assert!(row.bottom() <= button.bounds().origin.y);
            }
        })
        .unwrap();
    }
    #[gpui_kit::test]
    fn context_menu_stays_in_window_dismisses_and_single_update_starts_directly(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, panel) = cx
            .update(|cx| {
                gpui_kit::open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: Point::default(),
                            size: size(px(600.), px(400.)),
                        })),
                        ..Default::default()
                    },
                    cx,
                    |window, cx| {
                        cx.new(|cx| {
                            let mut panel = SoftwarePanel::from_inventory(
                                ThemeName::Catppuccin,
                                Arc::default(),
                                vec![],
                                Icons::new().0,
                                window,
                                cx,
                            );
                            let mut item = (*software("fixture", UpdateState::Available)).clone();
                            item.action = Action::Command(Invocation {
                                program: "/nonexistent/starter-fixture-manager".into(),
                                args: vec![],
                            });
                            item.latest = Some("2.0".into());
                            let mut snapshot = Snapshot::default();
                            snapshot.items = vec![Arc::new(item)];
                            panel.snapshot = Arc::new(snapshot);
                            panel.filter_rows(cx);
                            panel
                        })
                    },
                )
            })
            .unwrap();
        let handle: WindowHandle<Root> = handle.downcast().unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_menu(
                    panel.snapshot.items[0].clone(),
                    point(px(595.), px(395.)),
                    window,
                    cx,
                )
            });
            window.render_frame(cx);
            let bounds = window.find("popup-menu").bounds();
            assert!(bounds.left() >= px(0.) && bounds.top() >= px(0.));
            assert!(
                bounds.right() <= px(600.) && bounds.bottom() <= px(400.),
                "menu escaped window: {bounds:?}"
            );
        })
        .unwrap();
        cx.simulate_keystrokes(handle.into(), "escape");
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                assert!(panel.menu.is_none());
                panel.scanning = true;
                panel.activate(panel.snapshot.items[0].clone(), window, cx);
                assert!(panel.task_started && panel.running);
                assert!(!panel.show_task, "single updates should stay in the list");
                assert!(!panel.scanning);
                assert!(panel.scan_cancelled.load(Ordering::Relaxed));
                assert_eq!(panel.plan.as_ref().unwrap().steps.len(), 1);
                // Stop before the mock executor runs; no installed software is touched.
                panel.task_cancelled.store(true, Ordering::Relaxed);
                panel._run = None;
                panel.running = false;
            });
        })
        .unwrap();
    }
    #[gpui_kit::test]
    fn selection_survives_filters_review_does_not_execute_and_terminal_is_lazy(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (handle, panel) = cx
            .update(|cx| {
                gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        let mut panel = SoftwarePanel::from_inventory(
                            ThemeName::Catppuccin,
                            Arc::default(),
                            vec![],
                            Icons::new().0,
                            window,
                            cx,
                        );
                        let mut snapshot = Snapshot::default();
                        snapshot.items = vec![
                            software("alpha", UpdateState::Available),
                            software("beta", UpdateState::Unchecked),
                        ];
                        panel.snapshot = Arc::new(snapshot);
                        panel.filter_rows(cx);
                        panel
                    })
                })
            })
            .unwrap();
        let handle: WindowHandle<Root> = handle.downcast().unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                assert!(panel.terminal.is_none());
                assert!(panel.checked_ids.is_empty());
                assert!(!panel.scanning);
                panel.checked_ids.insert("alpha".into());
                panel.filter = Filter::Apps;
                panel.filter_rows(cx);
                assert!(panel.visible.is_empty());
                assert!(panel.checked_ids.contains("alpha"));
                panel.review(panel.checked_ids.clone(), window, cx);
                assert!(panel.show_task);
                assert!(!panel.running);
                assert!(!panel.task_started);
                assert!(panel.terminal.is_none());
                assert_eq!(panel.plan.as_ref().unwrap().steps.len(), 1);
                assert_eq!(panel.plan.as_ref().unwrap().steps[0].id, "alpha");
                panel.back(window, cx);
                assert!(!panel.show_task);
                assert!(panel.checked_ids.contains("alpha"));
                // Finished output is released on return, while its compact per-item report remains.
                panel.show_task = true;
                panel.task_started = true;
                panel.outcomes[0] = Outcome::Succeeded;
                panel.terminal = Some(cx.new(|cx| TerminalView::new(panel.theme, cx)));
                panel.back(window, cx);
                assert!(panel.terminal.is_none());
                assert_eq!(panel.outcomes, [Outcome::Succeeded]);
            })
        })
        .unwrap();
    }
}
