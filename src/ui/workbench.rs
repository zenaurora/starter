use gpui_kit::{
    assets::IconName,
    base::Disableable,
    component::{
        ActiveTheme, Icon, Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        menu::{PopupMenu, PopupMenuItem},
    },
    prelude::FluentBuilder,
    *,
};
use starter::{
    monitor,
    organizer::{self, Change, Command, Data, Draft, Entry, Kind},
    ports,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Saved,
    Todos,
    Ports,
}
impl Section {
    fn label(self) -> &'static str {
        match self {
            Self::Saved => "常用收藏",
            Self::Todos => "待办",
            Self::Ports => "端口排查",
        }
    }
    pub fn action(self) -> starter::system_commands::Action {
        use starter::system_commands::Action;
        match self {
            Self::Saved => Action::Saved,
            Self::Todos => Action::Todos,
            Self::Ports => Action::Ports,
        }
    }
}
pub fn shortcut(query: &str) -> Option<(Section, &str)> {
    use starter::system_commands::{self, Action};
    let (action, tail) = system_commands::parse(query)?;
    let section = match action {
        Action::Saved => Section::Saved,
        Action::Todos => Section::Todos,
        Action::Ports => Section::Ports,
        _ => return None,
    };
    Some((section, tail))
}
pub struct Close;
struct Editor {
    id: Option<u64>,
    original: Draft,
    kind: Kind,
}
#[derive(Clone)]
enum Action {
    Copy,
    Open,
    Edit,
    Delete,
    Pin,
    ReadLater,
    CopyPid,
    CopyPath,
    Terminate,
    ForceQuit,
}
pub struct Workbench {
    section: Section,
    search: Entity<InputState>,
    quick: Entity<InputState>,
    saved: Entity<TextareaState>,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    link: Entity<InputState>,
    data: Arc<Data>,
    worker: Option<Sender<Command>>,
    loaded: bool,
    pending: bool,
    load_error: bool,
    message: String,
    copied_entry: Option<u64>,
    saving_clipboard: bool,
    undo: Option<Change>,
    proposed_undo: Option<Change>,
    editor: Option<Editor>,
    discard: bool,
    filter: usize,
    visible: Vec<usize>,
    selected: usize,
    scroll: UniformListScrollHandle,
    sockets: Vec<ports::Socket>,
    scanned: bool,
    scanning: bool,
    port_message: String,
    scan_cancel: Arc<AtomicBool>,
    scan_task: Option<Task<()>>,
    process_task: Option<Task<()>>,
    process_pending: bool,
    confirmation: Option<(ports::Socket, monitor::Action)>,
    menu: Option<(Entity<PopupMenu>, Point<Pixels>)>,
    menu_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
    _worker_task: Option<Task<()>>,
}
impl EventEmitter<Close> for Workbench {}
impl Workbench {
    pub fn new(
        path: PathBuf,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索名称、文本内容或链接"));
        let quick = cx.new(|cx| {
            InputState::new(window, cx).placeholder("接下来要做什么？输入后按 Enter 添加")
        });
        let saved = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("粘贴或输入要收藏的内容…")
                .rows(2)
        });
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("留空时使用内容的第一行"));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("要收藏的内容，支持多行文本和链接")
                .rows(6)
        });
        let link = cx.new(|cx| InputState::new(window, cx).placeholder("https://…"));
        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, _, event, _, cx| {
                if let InputEvent::Change = event {
                    this.rebuild(cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&quick, window, |this, input, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. })
                    && input.update(cx, |s, cx| s.marked_text_range(window, cx).is_none())
                {
                    this.add_todo(cx);
                }
            }),
            cx.subscribe(&saved, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        let mut this = Self {
            section,
            search,
            quick,
            saved,
            title,
            body,
            link,
            data: Arc::new(Data::default()),
            worker: None,
            loaded: false,
            pending: false,
            load_error: false,
            message: String::new(),
            copied_entry: None,
            saving_clipboard: false,
            undo: None,
            proposed_undo: None,
            editor: None,
            discard: false,
            filter: 0,
            visible: vec![],
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            sockets: vec![],
            scanned: false,
            scanning: false,
            port_message: String::new(),
            scan_cancel: Arc::new(AtomicBool::new(false)),
            scan_task: None,
            process_task: None,
            process_pending: false,
            confirmation: None,
            menu: None,
            menu_subscription: None,
            _subscriptions: subscriptions,
            _worker_task: None,
        };
        if cx.background_executor().is_test() {
            // Tests inject storage commands and events; OS threads must not wake
            // GPUI's deterministic scheduler.
            this.loaded = true;
        } else {
            match organizer::start(path) {
                Ok((worker, events)) => {
                    this.worker = Some(worker);
                    this._worker_task = Some(cx.spawn_in(window, async move |this, cx| {
                        while let Ok(event) = events.recv().await {
                            if this
                                .update_in(cx, |this, window, cx| this.received(event, window, cx))
                                .is_err()
                            {
                                break;
                            }
                        }
                    }));
                }
                Err(e) => {
                    this.loaded = true;
                    this.load_error = true;
                    this.message = format!("无法启动本地存储：{e:#}");
                }
            }
        }
        this.activate(section, window, cx);
        this
    }
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Keep a focused editor/textarea intact when native activation completes.
        if self.menu.is_some() {
            return;
        }
        if self.editor.is_some() {
            if !self.title.focus_handle(cx).is_focused(window)
                && !self.link.focus_handle(cx).is_focused(window)
                && !self.body.focus_handle(cx).is_focused(window)
            {
                self.title.update(cx, |s, cx| s.focus(window, cx));
            }
        } else if self.section == Section::Todos {
            if !self.quick.focus_handle(cx).is_focused(window)
                && !self.search.focus_handle(cx).is_focused(window)
            {
                self.quick.update(cx, |s, cx| s.focus(window, cx));
            }
        } else if self.section == Section::Saved {
            if !self.saved.focus_handle(cx).is_focused(window)
                && !self.search.focus_handle(cx).is_focused(window)
            {
                self.saved.update(cx, |s, cx| s.focus(window, cx));
            }
        } else if !self.search.focus_handle(cx).is_focused(window) {
            self.search.update(cx, |s, cx| s.focus(window, cx));
        }
    }
    pub fn activate(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.confirmation.is_some() {
            self.focus(window, cx);
            return;
        }
        self.menu = None;
        self.menu_subscription = None;
        self.section = section;
        self.filter = 0;
        self.search.update(cx, |s, cx| {
            s.set_placeholder(
                if section == Section::Ports {
                    "端口号、进程名或 PID，例如 3000"
                } else {
                    "搜索名称、文本内容或链接"
                },
                window,
                cx,
            )
        });
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.rebuild(cx);
        if section == Section::Todos {
            self.quick.update(cx, |s, cx| s.focus(window, cx));
        } else if section == Section::Saved {
            self.saved.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.search.update(cx, |s, cx| s.focus(window, cx));
        }
        if section == Section::Ports && !self.scanned {
            self.refresh_ports(window, cx);
        }
        cx.notify();
    }
    pub fn seed(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        if value.is_empty() || self.editor.is_some() {
            return;
        }
        if self.section == Section::Todos {
            self.quick
                .update(cx, |s, cx| s.set_value(value, window, cx));
        } else {
            self.search
                .update(cx, |s, cx| s.set_value(value, window, cx));
            self.search.update(cx, |s, cx| s.focus(window, cx));
        }
        self.rebuild(cx);
        cx.notify();
    }
    fn received(&mut self, event: organizer::Event, window: &mut Window, cx: &mut Context<Self>) {
        let saving_clipboard = std::mem::take(&mut self.saving_clipboard);
        self.pending = false;
        self.loaded = true;
        let mut select_id = self.selected_entry().map(|e| e.id);
        match event.result {
            Ok(data) => {
                self.load_error = false;
                self.data = data;
                if let Some(change) = event.change {
                    self.undo = self.proposed_undo.take();
                    self.message = match change {
                        Change::Save { draft, .. }
                            if draft.kind == Kind::Todo && self.editor.is_none() =>
                        {
                            self.quick.update(cx, |s, cx| s.set_value("", window, cx));
                            if self.section == Section::Todos {
                                self.filter = 0;
                                self.search.update(cx, |s, cx| s.set_value("", window, cx));
                                select_id = self.data.entries.last().map(|e| e.id);
                            }
                            "已添加待办".into()
                        }
                        Change::Save { id, draft } => {
                            select_id = id.or_else(|| self.data.entries.last().map(|e| e.id));
                            if self.editor.is_none() && !saving_clipboard {
                                self.saved.update(cx, |s, cx| s.set_value("", window, cx));
                            }
                            self.editor = None;
                            self.discard = false;
                            self.filter = 0;
                            self.search.update(cx, |s, cx| s.set_value("", window, cx));
                            self.focus(window, cx);
                            self.copied_entry = None;
                            if draft.kind == Kind::Todo {
                                "已保存修改".into()
                            } else {
                                "已收藏到本机".into()
                            }
                        }
                        Change::Delete(_) => "已删除，可撤销".into(),
                        Change::ReadLater(_) => "已加入待办，链接与备注一并保留".into(),
                        Change::Done(_, done) => {
                            if done {
                                "已完成，可撤销".into()
                            } else {
                                "已恢复为待办".into()
                            }
                        }
                        Change::Restore(_) => "已恢复".into(),
                        Change::Pin(_, pin) => {
                            if pin {
                                "已置顶".into()
                            } else {
                                "已取消置顶".into()
                            }
                        }
                    };
                } else {
                    self.message.clear();
                }
                self.rebuild(cx);
                if self.section != Section::Ports
                    && let Some(id) = select_id
                {
                    self.selected = self
                        .visible
                        .iter()
                        .position(|&i| self.data.entries[i].id == id)
                        .unwrap_or(self.selected);
                    self.scroll
                        .scroll_to_item(self.selected, ScrollStrategy::Nearest);
                }
            }
            Err(error) => {
                self.proposed_undo = None;
                if event.change.is_none() {
                    self.load_error = true;
                }
                self.message = error;
            }
        }
        cx.notify();
    }
    fn send(&mut self, change: Change, undo: Option<Change>, cx: &mut Context<Self>) {
        if self.pending || !self.loaded || self.load_error {
            return;
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|w| w.send(Command::Edit(change)).is_ok())
        {
            self.pending = true;
            self.proposed_undo = undo;
        } else {
            self.message = "本地存储未连接，请重新打开 Starter 后重试".into();
        }
        cx.notify();
    }
    fn rebuild(&mut self, cx: &App) {
        let previous = self.visible.get(self.selected).copied();
        let query = self.search.read(cx).value();
        self.visible = if self.section == Section::Ports {
            self.sockets
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.matches(&query)
                        && (self.filter == 0
                            || (self.filter == 1 && s.protocol == ports::Protocol::Tcp)
                            || (self.filter == 2 && s.protocol == ports::Protocol::Udp))
                })
                .map(|(i, _)| i)
                .collect()
        } else {
            let mut rows: Vec<_> = self
                .data
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    let kind = if self.section == Section::Todos {
                        e.kind == Kind::Todo
                            && match self.filter {
                                0 => !e.done,
                                1 => e.done,
                                _ => true,
                            }
                    } else {
                        e.kind != Kind::Todo
                    };
                    kind && e.matches(&query)
                })
                .map(|(i, _)| i)
                .collect();
            rows.sort_by_key(|&i| {
                let e = &self.data.entries[i];
                (e.done, !e.pinned, std::cmp::Reverse(e.id))
            });
            rows
        };
        self.selected = previous
            .and_then(|old| self.visible.iter().position(|&i| i == old))
            .unwrap_or(0)
            .min(self.visible.len().saturating_sub(1));
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
    }
    fn selected_entry(&self) -> Option<Entry> {
        self.visible
            .get(self.selected)
            .and_then(|&i| self.data.entries.get(i))
            .cloned()
    }
    fn selected_socket(&self) -> Option<ports::Socket> {
        self.visible
            .get(self.selected)
            .and_then(|&i| self.sockets.get(i))
            .cloned()
    }
    fn draft(&self, cx: &App) -> Draft {
        if self.editor.as_ref().is_some_and(|e| e.kind != Kind::Todo) {
            return Draft::saved(
                self.body.read(cx).value().into(),
                &self.title.read(cx).value(),
            );
        }
        Draft {
            kind: self.editor.as_ref().map_or(Kind::Text, |e| e.kind),
            title: self.title.read(cx).value().into(),
            body: self.body.read(cx).value().into(),
            url: self.link.read(cx).value().into(),
        }
    }
    fn dirty(&self, cx: &App) -> bool {
        self.editor.as_ref().is_some_and(|editor| {
            let d = self.draft(cx);
            d.kind != editor.original.kind
                || d.title != editor.original.title
                || d.body != editor.original.body
                || d.url != editor.original.url
        })
    }
    fn edit(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || self.load_error || !self.loaded {
            return;
        }
        let id = Some(entry.id);
        let draft = if entry.kind == Kind::Todo {
            Draft {
                kind: entry.kind,
                title: entry.title,
                body: entry.body,
                url: entry.url,
            }
        } else {
            Draft::saved(entry.saved_content(), &entry.title)
        };
        self.title
            .update(cx, |s, cx| s.set_value(draft.title.clone(), window, cx));
        self.body
            .update(cx, |s, cx| s.set_value(draft.body.clone(), window, cx));
        self.link
            .update(cx, |s, cx| s.set_value(draft.url.clone(), window, cx));
        self.editor = Some(Editor {
            id,
            kind: draft.kind,
            original: draft,
        });
        self.discard = false;
        self.message.clear();
        self.body.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }
    fn add_saved(&mut self, clipboard: bool, cx: &mut Context<Self>) {
        if self.section != Section::Saved
            || self.editor.is_some()
            || self.pending
            || !self.loaded
            || self.load_error
        {
            return;
        }
        let content = if clipboard {
            let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
                self.message = "剪贴板中没有可收藏的文本".into();
                cx.notify();
                return;
            };
            text
        } else {
            self.saved.read(cx).value().to_string()
        };
        if content.trim().is_empty() {
            self.message = "先输入或粘贴要收藏的内容".into();
            cx.notify();
            return;
        }
        let draft = Draft::saved(content, "");
        if let Err(error) = draft.validate() {
            self.message = error.to_string();
            cx.notify();
            return;
        }
        self.saving_clipboard = clipboard;
        self.send(Change::Save { id: None, draft }, None, cx);
        if !self.pending {
            self.saving_clipboard = false;
        }
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let id = editor.id;
        let draft = self.draft(cx);
        match draft.validate() {
            Ok(()) => self.send(Change::Save { id, draft }, None, cx),
            Err(e) => {
                self.message = e.to_string();
                cx.notify();
            }
        }
    }
    fn add_todo(&mut self, cx: &mut Context<Self>) {
        if self.section != Section::Todos || self.editor.is_some() || self.confirmation.is_some() {
            return;
        }
        let draft = Draft {
            kind: Kind::Todo,
            title: self.quick.read(cx).value().into(),
            body: String::new(),
            url: String::new(),
        };
        if draft.title.trim().is_empty() {
            return;
        }
        if let Err(e) = draft.validate() {
            self.message = e.to_string();
            cx.notify();
            return;
        }
        self.send(Change::Save { id: None, draft }, None, cx);
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || self.process_pending {
            return;
        }
        if self.confirmation.take().is_some() {
            self.focus(window, cx);
        } else if self.editor.is_some() {
            if self.dirty(cx) {
                self.discard = true;
            } else {
                self.editor = None;
                self.focus(window, cx);
            }
        } else {
            self.scan_cancel.store(true, Ordering::Relaxed);
            self.scanning = false;
            cx.emit(Close);
        }
        cx.notify();
    }
    fn copy(&mut self, value: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        if self.section == Section::Ports {
            self.port_message = "已复制".into();
        } else {
            self.copied_entry = self.selected_entry().map(|e| e.id);
            self.message = "已复制".into();
        }
        cx.notify();
    }
    fn action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.menu_subscription = None;
        self.focus(window, cx);
        if self.section == Section::Ports {
            if let Some(socket) = self.selected_socket() {
                match action {
                    Action::Copy => self.copy(socket.address.clone(), cx),
                    Action::CopyPid => self.copy(socket.pid.to_string(), cx),
                    Action::CopyPath => {
                        if let Some(path) = socket
                            .process
                            .as_ref()
                            .and_then(|p| p.members.first())
                            .and_then(|p| p.exe.as_ref())
                        {
                            self.copy(path.display().to_string(), cx);
                        }
                    }
                    Action::Terminate | Action::ForceQuit
                        if socket.can_terminate() && !self.process_pending =>
                    {
                        self.confirmation = Some((
                            socket,
                            if matches!(action, Action::ForceQuit) {
                                monitor::Action::ForceQuit
                            } else {
                                monitor::Action::Terminate
                            },
                        ));
                    }
                    _ => {}
                }
            }
        } else if let Some(e) = self.selected_entry() {
            match action {
                Action::Copy => self.copy(e.copy_text().into(), cx),
                Action::Open => match organizer::web_url(&e.url) {
                    Ok(url) => {
                        cx.open_url(url.as_str());
                    }
                    Err(e) => self.message = e.to_string(),
                },
                Action::Edit => self.edit(e, window, cx),
                Action::Delete => self.send(Change::Delete(e.id), Some(Change::Restore(e)), cx),
                Action::Pin => self.send(Change::Pin(e.id, !e.pinned), None, cx),
                Action::ReadLater => self.send(Change::ReadLater(e.id), None, cx),
                _ => {}
            }
        }
        cx.notify();
    }
    fn refresh_ports(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scanning || self.process_pending {
            return;
        }
        self.menu = None;
        self.menu_subscription = None;
        self.focus(window, cx);
        self.scan_cancel.store(true, Ordering::Relaxed);
        self.scan_cancel = Arc::new(AtomicBool::new(false));
        let cancelled = self.scan_cancel.clone();
        self.scanning = true;
        self.port_message = "正在读取 TCP 监听与 UDP 绑定端口…".into();
        let work = cx
            .background_executor()
            .spawn(async move { ports::scan(&cancelled) });
        self.scan_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, _, cx| {
                let selected = this.selected_socket();
                this.scanning = false;
                match result {
                    Ok(rows) => {
                        this.sockets = rows;
                        this.scanned = true;
                        this.port_message = format!(
                            "已刷新 · {} 个端口绑定 · 当前用户可见范围",
                            this.sockets.len()
                        );
                    }
                    Err(e) => this.port_message = format!("{e:#}"),
                }
                this.rebuild(cx);
                if this.section == Section::Ports {
                    this.selected = selected
                        .and_then(|old| {
                            this.visible.iter().position(|&i| {
                                let new = &this.sockets[i];
                                (old.pid, old.protocol, &old.address)
                                    == (new.pid, new.protocol, &new.address)
                            })
                        })
                        .unwrap_or(0);
                    this.scroll
                        .scroll_to_item(this.selected, ScrollStrategy::Nearest);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn perform_process_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.process_pending {
            return;
        }
        let Some((socket, action)) = self.confirmation.clone() else {
            return;
        };
        let Some(process) = socket.process else {
            return;
        };
        self.process_pending = true;
        let work = cx
            .background_executor()
            .spawn(async move { monitor::perform(action, &process) });
        self.process_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.process_pending = false;
                this.confirmation = None;
                match result {
                    Ok(message) => {
                        this.refresh_ports(window, cx);
                        this.port_message = message;
                    }
                    Err(e) => this.port_message = format!("{e:#}"),
                }
                this.focus(window, cx);
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn open_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending || self.scanning || self.process_pending {
            return;
        }
        self.selected = index;
        let entries = if self.section == Section::Ports {
            let Some(socket) = self.selected_socket() else {
                return;
            };
            let mut entries = vec![
                ("复制监听地址", IconName::Copy, Action::Copy),
                ("复制 PID", IconName::Copy, Action::CopyPid),
            ];
            if socket.process.is_some() {
                entries.push(("复制程序路径", IconName::Folder, Action::CopyPath));
            }
            if socket.can_terminate() {
                if cfg!(target_os = "macos") {
                    entries.push(("终止进程…", IconName::X, Action::Terminate));
                }
                entries.push(("强制结束进程…", IconName::TriangleAlert, Action::ForceQuit));
            }
            entries
        } else {
            let Some(e) = self.selected_entry() else {
                return;
            };
            let mut entries = vec![
                ("复制", IconName::Copy, Action::Copy),
                ("编辑…", IconName::Pencil, Action::Edit),
            ];
            if !e.url.is_empty() {
                entries.insert(0, ("打开链接", IconName::ExternalLink, Action::Open));
            }
            if e.kind != Kind::Todo {
                entries.push(("加入待办", IconName::Check, Action::ReadLater));
            }
            entries.push((
                if e.pinned { "取消置顶" } else { "置顶" },
                IconName::Pin,
                Action::Pin,
            ));
            entries.push(("删除", IconName::Trash, Action::Delete));
            entries
        };
        let weak = cx.entity().downgrade();
        let focus = self.search.focus_handle(cx);
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu.action_context(focus).min_w(px(202.)).max_w(px(260.));
            let mut destructive_group = false;
            for (label, icon, action) in entries {
                if matches!(
                    action,
                    Action::Delete | Action::Terminate | Action::ForceQuit
                ) && !destructive_group
                {
                    menu = menu.item(PopupMenuItem::separator());
                    destructive_group = true;
                }
                let weak = weak.clone();
                menu = menu.item(PopupMenuItem::new(label).icon(icon).on_click(
                    move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| this.action(action.clone(), window, cx));
                    },
                ));
            }
            menu
        });
        self.menu_subscription =
            Some(
                cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
                    this.menu = None;
                    this.focus(window, cx);
                    cx.notify();
                }),
            );
        menu.focus_handle(cx).focus(window, cx);
        self.menu = Some((menu, position));
        cx.notify();
    }
    fn composing(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        [&self.search, &self.quick, &self.title, &self.link]
            .iter()
            .any(|s| {
                s.focus_handle(cx).is_focused(window)
                    && s.update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            })
            || [&self.body, &self.saved].iter().any(|s| {
                s.focus_handle(cx).is_focused(window)
                    && s.update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            })
    }
    fn enter(
        &mut self,
        action: &gpui_kit::component::input::Enter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu.is_some() || self.composing(window, cx) {
            cx.propagate();
            return;
        }
        if action.secondary && self.editor.is_some() {
            self.save(cx);
        } else if action.secondary && self.section == Section::Saved {
            self.add_saved(false, cx);
        } else if !action.shift
            && self.section == Section::Saved
            && self.editor.is_none()
            && self.search.focus_handle(cx).is_focused(window)
        {
            self.action(Action::Copy, window, cx);
        } else {
            cx.propagate();
            return;
        }
        cx.stop_propagation();
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            return;
        }
        if self.composing(window, cx) {
            return;
        }
        let key = &event.keystroke;
        let command = key.modifiers.platform || key.modifiers.control;
        if key.key == "escape" {
            self.close(window, cx);
        } else if self.confirmation.is_some() {
            return;
        } else if command && key.key == "enter" && self.editor.is_some() {
            self.save(cx);
        } else if self.editor.is_some() {
            return;
        } else if command && key.key == "enter" && self.section == Section::Saved {
            self.add_saved(false, cx);
        } else if command && key.key == "n" && self.section == Section::Saved {
            self.saved.update(cx, |s, cx| s.focus(window, cx));
        } else if self.saved.focus_handle(cx).is_focused(window) {
            return;
        } else if command && key.key == "r" && self.section == Section::Ports {
            self.refresh_ports(window, cx);
        } else if !command && matches!(key.key.as_str(), "up" | "down") && !self.visible.is_empty()
        {
            if key.key == "up" {
                self.selected = self.selected.saturating_sub(1);
            } else {
                self.selected = (self.selected + 1).min(self.visible.len() - 1);
            }
            self.scroll
                .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        } else if key.key == "enter"
            && !event.is_held
            && !self.quick.focus_handle(cx).is_focused(window)
        {
            if self.section == Section::Saved {
                self.action(Action::Copy, window, cx);
            } else if self.section == Section::Ports {
                self.open_menu(self.selected, point(px(390.), px(200.)), window, cx);
            } else if let Some(e) = self.selected_entry() {
                self.send(
                    Change::Done(e.id, !e.done),
                    Some(Change::Done(e.id, e.done)),
                    cx,
                );
            }
        } else {
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn row(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let actual = self.visible[index];
        let selected = index == self.selected;
        let mut row = div()
            .id(("workbench-row", index))
            .test_support()
            .w_full()
            .h(px(60.))
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .bg(if selected {
                theme.accent
            } else {
                theme.background
            })
            .border_l_2()
            .border_color(if selected {
                theme.primary
            } else {
                theme.background
            })
            .hover(|style| style.bg(theme.muted))
            .active(|style| style.bg(theme.accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.selected = index;
                    this.search.update(cx, |s, cx| s.focus(window, cx));
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    this.open_menu(index, e.position, window, cx)
                }),
            );
        if self.section == Section::Ports {
            let s = &self.sockets[actual];
            row = row
                .child(
                    div()
                        .w(px(66.))
                        .flex_shrink_0()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(16.))
                        .child(s.port.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().text_size(px(13.)).child(s.name.clone()))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(theme.muted_foreground)
                                .child(format!(
                                    "{} · PID {} · {}",
                                    s.protocol.label(),
                                    s.pid,
                                    s.address
                                )),
                        ),
                );
        } else {
            let e = &self.data.entries[actual];
            let id = e.id;
            let done = e.done;
            if self.section == Section::Todos {
                row = row.child(
                    Button::new(("todo-toggle", index))
                        .icon(if done {
                            IconName::CircleCheck
                        } else {
                            IconName::Circle
                        })
                        .ghost()
                        .small()
                        .disabled(self.pending || self.load_error)
                        .tooltip(if done { "恢复为待办" } else { "完成" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(Change::Done(id, !done), Some(Change::Done(id, done)), cx)
                        })),
                );
            } else {
                row = row.child(
                    Icon::new(IconName::FileText)
                        .with_size(px(17.))
                        .text_color(theme.muted_foreground),
                );
            }
            row = row
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.))
                                .text_color(if done {
                                    theme.muted_foreground
                                } else {
                                    theme.foreground
                                })
                                .child(e.title.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(theme.muted_foreground)
                                .child(if e.url.is_empty() {
                                    e.body
                                        .lines()
                                        .next()
                                        .unwrap_or(if done { "已完成" } else { "待完成" })
                                        .to_string()
                                } else {
                                    e.url.clone()
                                }),
                        ),
                )
                .when(e.pinned, |row| {
                    row.child(
                        Icon::new(IconName::Pin)
                            .with_size(px(12.))
                            .text_color(theme.primary),
                    )
                });
        }
        row.child(
            Button::new(("workbench-more", index))
                .icon(IconName::Ellipsis)
                .ghost()
                .small()
                .tooltip("更多操作")
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    this.open_menu(index, e.position(), window, cx)
                })),
        )
        .into_any_element()
    }
    fn detail(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut detail = div()
            .id("workbench-detail")
            .test_support()
            .flex_1()
            .min_w_0()
            .h_full()
            .px_5()
            .py_4()
            .flex()
            .flex_col()
            .gap_3()
            .border_l_1()
            .border_color(theme.border);
        if self.section == Section::Ports {
            if let Some(s) = self.selected_socket() {
                detail = detail
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.primary)
                            .child(format!(
                                "{} · {}",
                                s.protocol.label(),
                                if s.protocol == ports::Protocol::Tcp {
                                    "监听中"
                                } else {
                                    "已绑定"
                                }
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_family(theme.mono_font_family.clone())
                            .child(s.port.to_string()),
                    )
                    .child(div().text_size(px(14.)).child(s.name.clone()))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(format!("PID {}\n{}", s.pid, s.address)),
                    )
                    .child(
                        div()
                            .id("port-exe")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .text_size(px(12.))
                            .child(
                                s.process
                                    .as_ref()
                                    .and_then(|p| p.members.first())
                                    .and_then(|p| p.exe.as_ref())
                                    .map_or(
                                        "无法核实进程身份；可复制地址与 PID，刷新后重试。".into(),
                                        |p| p.display().to_string(),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("port-actions")
                            .label("进程操作")
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_menu(this.selected, point(px(425.), px(350.)), window, cx)
                            })),
                    );
            } else {
                detail = detail.child(
                    div()
                        .text_color(theme.muted_foreground)
                        .text_size(px(13.))
                        .child("选择端口，查看占用它的进程和程序路径。"),
                );
            }
        } else if let Some(e) = self.selected_entry() {
            detail = detail
                .child(
                    div()
                        .id("saved-preview")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .text_size(px(13.))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .mb_3()
                                .child(e.title.clone()),
                        )
                        .child(div().child(e.body.clone())),
                )
                .when(e.kind != Kind::Text && !e.url.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.primary)
                            .truncate()
                            .child(e.url.clone()),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .gap_2()
                        .child(
                            Button::new("copy-saved")
                                .icon(if self.copied_entry == Some(e.id) {
                                    IconName::Check
                                } else {
                                    IconName::Copy
                                })
                                .label(if self.copied_entry == Some(e.id) {
                                    "已复制"
                                } else {
                                    "复制内容"
                                })
                                .secondary()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.action(Action::Copy, window, cx)
                                })),
                        )
                        .when(!e.url.is_empty(), |d| {
                            d.child(
                                Button::new("open-saved")
                                    .icon(IconName::ExternalLink)
                                    .label("打开链接")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.action(Action::Open, window, cx)
                                    })),
                            )
                        })
                        .child(
                            Button::new("edit-saved")
                                .icon(IconName::Pencil)
                                .label("编辑")
                                .ghost()
                                .small()
                                .disabled(self.pending || self.load_error)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.action(Action::Edit, window, cx)
                                })),
                        ),
                );
        } else {
            detail = detail.child(
                div()
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .child("发票抬头、账号、常用语…\n\n在上方粘贴即可收藏，需要时选中复制。"),
            );
        }
        detail.into_any_element()
    }
    fn editor_view(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let editor = self.editor.as_ref().unwrap();
        div()
            .id("workbench-editor")
            .test_support()
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .px_5()
            .py_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if editor.id.is_some() {
                        "编辑内容"
                    } else {
                        "新建收藏"
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(if editor.kind == Kind::Todo {
                        "名称"
                    } else {
                        "名称（选填）"
                    }),
            )
            .child(Input::new(&self.title).disabled(self.pending))
            .when(
                editor.kind == Kind::Todo && !self.link.read(cx).value().is_empty(),
                |d| d.child(Input::new(&self.link).disabled(self.pending)),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("内容"),
            )
            .child(
                div().flex_1().min_h(px(72.)).child(
                    Textarea::new(&self.body)
                        .h(relative(1.))
                        .disabled(self.pending),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("save-entry")
                            .primary()
                            .loading(self.pending)
                            .label(if self.pending {
                                "正在保存…"
                            } else {
                                "保存"
                            })
                            .disabled(self.pending)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .child(
                        Button::new("cancel-entry")
                            .label("取消")
                            .ghost()
                            .disabled(self.pending)
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(if cfg!(target_os = "macos") {
                                "⌘ Enter 保存"
                            } else {
                                "Ctrl Enter 保存"
                            }),
                    ),
            )
            .when(self.discard, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .child("放弃尚未保存的修改？")
                        .child(
                            Button::new("discard-editor")
                                .label("放弃修改")
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.editor = None;
                                    this.discard = false;
                                    this.focus(window, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("continue-editor")
                                .label("继续编辑")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.discard = false;
                                    this.focus(window, cx);
                                    cx.notify();
                                })),
                        ),
                )
            })
            .into_any_element()
    }
}
impl Drop for Workbench {
    fn drop(&mut self) {
        self.scan_cancel.store(true, Ordering::Relaxed);
    }
}
impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let message = if self.section == Section::Ports {
            self.port_message.clone()
        } else {
            self.message.clone()
        };
        let mut root = div()
            .id("workbench")
            .key_context("Workbench")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.background)
            .text_color(theme.foreground)
            .capture_key_down(cx.listener(Self::key_down))
            .capture_action(cx.listener(Self::enter))
            .child(
                div()
                    .h(px(64.))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("workbench-back")
                            .icon(IconName::ArrowLeft)
                            .ghost()
                            .tooltip("返回 · Esc")
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    )
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.section.label()),
                    )
                    .child(div().flex_1())
                    .children(
                        [Section::Saved, Section::Todos, Section::Ports].map(|section| {
                            Button::new(section.label())
                                .label(section.label())
                                .ghost()
                                .small()
                                .disabled(
                                    self.editor.is_some()
                                        || self.confirmation.is_some()
                                        || self.process_pending,
                                )
                                .when(self.section == section, |b| {
                                    b.bg(theme.accent).text_color(theme.primary)
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.activate(section, window, cx)
                                }))
                        }),
                    ),
            );
        if let Some((socket, action)) = &self.confirmation {
            root = root.child(div().flex_1().p_6().flex().flex_col().gap_4()
                .child(Icon::new(IconName::TriangleAlert).with_size(px(28.)).text_color(theme.danger))
                .child(div().text_size(px(20.)).child(format!("{} {}？",action.label(),socket.name)))
                .child(div().text_size(px(13.)).child(format!("PID {} · {} {}\n\n该进程的所有端口都会受影响，未保存的工作可能丢失。执行前会再次核对程序路径与启动时间。",socket.pid,socket.protocol.label(),socket.address)))
                .child(div().flex().gap_3()
                    .child(Button::new("confirm-port-action").label(if self.process_pending { "正在处理…" } else { action.label() }).disabled(self.process_pending).on_click(cx.listener(|this,_,window,cx| this.perform_process_action(window,cx))))
                    .child(Button::new("cancel-port-action").label("取消").ghost().disabled(self.process_pending).on_click(cx.listener(|this,_,window,cx| this.close(window,cx))))));
        } else if self.editor.is_some() {
            root = root.child(self.editor_view(cx));
        } else {
            if self.section == Section::Saved {
                let disabled = !self.loaded || self.pending || self.load_error;
                root = root.child(
                    div()
                        .id("saved-composer")
                        .test_support()
                        .px_4()
                        .pt_3()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(Textarea::new(&self.saved).h(px(64.)).disabled(disabled))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .child(if cfg!(target_os = "macos") {
                                            "⌘ Enter 收藏 · 自动生成名称"
                                        } else {
                                            "Ctrl Enter 收藏 · 自动生成名称"
                                        }),
                                )
                                .child(
                                    Button::new("paste-saved")
                                        .icon(IconName::Clipboard)
                                        .label("收藏剪贴板")
                                        .secondary()
                                        .small()
                                        .loading(self.pending && self.saving_clipboard)
                                        .disabled(disabled)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.add_saved(true, cx)),
                                        ),
                                )
                                .child(
                                    Button::new("add-saved")
                                        .icon(IconName::Plus)
                                        .label(if self.pending && !self.saving_clipboard {
                                            "正在收藏…"
                                        } else {
                                            "收藏"
                                        })
                                        .primary()
                                        .small()
                                        .loading(self.pending && !self.saving_clipboard)
                                        .disabled(
                                            disabled
                                                || self.saved.read(cx).value().trim().is_empty(),
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.add_saved(false, cx)),
                                        ),
                                ),
                        ),
                );
            }
            root = root.child(
                div()
                    .px_4()
                    .py_3()
                    .flex_shrink_0()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.search)
                                .prefix(Icon::new(IconName::Search))
                                .cleanable(true),
                        ),
                    )
                    .when(self.section == Section::Saved, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.muted_foreground)
                                .child(format!("{} 条收藏", self.visible.len())),
                        )
                    })
                    .when(self.section == Section::Ports, |d| {
                        d.child(
                            Button::new("refresh-ports")
                                .icon(IconName::RefreshCw)
                                .label(if self.scanning {
                                    "扫描中…"
                                } else {
                                    "刷新"
                                })
                                .ghost()
                                .small()
                                .disabled(self.scanning || self.process_pending)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.refresh_ports(window, cx)
                                })),
                        )
                    }),
            );
            let filters: &[&str] = match self.section {
                Section::Saved => &[],
                Section::Todos => &["待完成", "已完成", "全部"],
                Section::Ports => &["全部", "TCP", "UDP"],
            };
            if !filters.is_empty() {
                root = root.child(
                    div()
                        .px_4()
                        .pb_2()
                        .flex()
                        .items_center()
                        .gap_1()
                        .flex_shrink_0()
                        .children(filters.iter().enumerate().map(|(index, &label)| {
                            Button::new(label)
                                .label(label)
                                .ghost()
                                .small()
                                .when(self.filter == index, |b| {
                                    b.bg(theme.accent).text_color(theme.primary)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.filter = index;
                                    this.rebuild(cx);
                                    cx.notify();
                                }))
                        }))
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.muted_foreground)
                                .child(format!("{} 项", self.visible.len())),
                        ),
                );
            }
            if self.section == Section::Todos {
                root = root.child(
                    div()
                        .px_4()
                        .pb_3()
                        .flex()
                        .gap_2()
                        .flex_shrink_0()
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.quick)
                                    .disabled(self.pending || !self.loaded || self.load_error),
                            ),
                        )
                        .child(
                            Button::new("add-todo")
                                .icon(IconName::Plus)
                                .label("添加")
                                .disabled(self.pending || !self.loaded || self.load_error)
                                .on_click(cx.listener(|this, _, _, cx| this.add_todo(cx))),
                        ),
                );
            }
            let empty = self.visible.is_empty();
            let list = uniform_list(
                "workbench-list",
                self.visible.len(),
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range.map(|i| this.row(i, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .w_full()
            .flex_1()
            .min_h_0();
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .id("workbench-list-pane")
                            .test_support()
                            .h_full()
                            .w(if self.section == Section::Todos {
                                relative(1.)
                            } else {
                                relative(0.53)
                            })
                            .flex_shrink_0()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .when(!empty, |d| d.child(list))
                            .when(empty, |d| {
                                d.child(
                                    div()
                                        .p_5()
                                        .text_size(px(13.))
                                        .text_color(theme.muted_foreground)
                                        .child(if self.section == Section::Ports {
                                            if self.scanning {
                                                "正在扫描本机端口…"
                                            } else {
                                                "没有匹配的端口。尝试输入完整端口号、进程名或 PID。"
                                            }
                                        } else if !self.loaded {
                                            "正在读取本地数据…"
                                        } else if self.load_error {
                                            "本地数据未能读取。请查看下方原因，原文件不会被覆盖。"
                                        } else if !self.search.read(cx).value().is_empty() {
                                            "没有匹配的条目，试试其他关键词。"
                                        } else if self.section == Section::Todos {
                                            if self.filter == 1 {
                                                "还没有已完成的事项。"
                                            } else {
                                                "暂时没有待办。输入一件小事，按 Enter 开始。"
                                            }
                                        } else {
                                            "还没有收藏。\n\n在上方输入内容，或一键收藏剪贴板。"
                                        }),
                                )
                            }),
                    )
                    .when(self.section != Section::Todos, |d| d.child(self.detail(cx))),
            );
        }
        root = root.child(
            div()
                .id("workbench-status")
                .test_support()
                .flex_shrink_0()
                .px_4()
                .py_2()
                .border_t_1()
                .border_color(theme.border)
                .flex()
                .flex_col()
                .gap_1()
                .when(!message.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(11.))
                                    .text_color(if self.load_error {
                                        theme.danger
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(message),
                            )
                            .when(
                                self.undo.is_some()
                                    && !self.pending
                                    && self.editor.is_none()
                                    && self.section != Section::Ports,
                                |d| {
                                    d.child(
                                        Button::new("undo-organizer")
                                            .label("撤销")
                                            .ghost()
                                            .small()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if let Some(change) = this.undo.clone() {
                                                    this.send(change, None, cx);
                                                }
                                            })),
                                    )
                                },
                            )
                            .when(self.load_error && self.section != Section::Ports, |d| {
                                d.child(
                                    Button::new("reload-organizer")
                                        .label("重新读取")
                                        .small()
                                        .ghost()
                                        .disabled(self.pending)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if this
                                                .worker
                                                .as_ref()
                                                .is_some_and(|w| w.send(Command::Reload).is_ok())
                                            {
                                                this.pending = true;
                                                cx.notify();
                                            }
                                        })),
                                )
                            }),
                    )
                })
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.muted_foreground)
                        .child(match self.section {
                            Section::Saved => "仅保存在本机 · Enter 复制 · 右键更多操作",
                            Section::Todos => "Enter 添加 · 勾选完成 · 右键编辑、置顶或删除",
                            Section::Ports => "TCP 监听 / UDP 绑定 · 搜索完整端口号 · 右键进程操作",
                        }),
                ),
        );
        if let Some((menu, position)) = &self.menu {
            root = root.child(
                deferred(
                    anchored()
                        .position(*position)
                        .snap_to_window_with_margin(px(8.))
                        .child(menu.clone()),
                )
                .with_priority(1),
            );
        }
        root.into_any_element()
    }
}

impl super::Launcher {
    pub(super) fn open_workbench(
        &mut self,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = shortcut(&self.query.text)
            .filter(|(s, _)| *s == section)
            .map(|(_, tail)| tail.to_string())
            .unwrap_or_default();
        self.set_monitor_sampling(false);
        self.cancelled.store(true, Ordering::Relaxed);
        self.generation = self.generation.wrapping_add(1);
        self.searching = false;
        self.monitor_actions = None;
        if self.workbench.is_none() {
            let path = self.config_path.with_file_name("organizer.json");
            let panel = cx.new(|cx| Workbench::new(path, section, window, cx));
            self.workbench_subscription =
                Some(
                    cx.subscribe_in(&panel, window, |this, _, _: &Close, window, cx| {
                        this.workbench_open = false;
                        this.input.update(cx, |s, cx| s.focus(window, cx));
                        this.search(cx);
                        cx.notify();
                    }),
                );
            self.workbench = Some(panel);
        } else if let Some(panel) = &self.workbench {
            panel.update(cx, |panel, cx| panel.activate(section, window, cx));
        }
        if let Some(panel) = &self.workbench {
            panel.update(cx, |panel, cx| panel.seed(&initial, window, cx));
        }
        self.workbench_open = true;
        self.visible = true;
        self.activation_generation = self.activation_generation.wrapping_add(1);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::{base::Root, test::TestWindowExt};
    fn entry(id: u64, kind: Kind) -> Entry {
        Entry {
            id,
            kind,
            title: format!("公司发票抬头 / 论文 {id}"),
            body: "第一行\n账号名 123456789".into(),
            url: if kind == Kind::Paper {
                "https://arxiv.org/abs/1234.5678".into()
            } else {
                String::new()
            },
            pinned: false,
            done: false,
            source: None,
        }
    }
    fn fixture(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<Workbench>) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
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
                            let config = starter::config::Config::default();
                            crate::appearance::apply(config.theme, &config, window, cx);
                            let mut panel = Workbench::new(
                                dir.path().join("organizer.json"),
                                Section::Saved,
                                window,
                                cx,
                            );
                            // A real storage thread can wake GPUI's test scheduler from
                            // outside its deterministic executor, even before teardown.
                            assert!(panel.worker.is_none());
                            assert!(panel._worker_task.is_none());
                            assert!(panel.loaded);
                            let mut data = Data::default();
                            data.entries = (1..40).map(|id| entry(id, Kind::Text)).collect();
                            data.entries[38].title = "很长的发票抬头与账号名称".repeat(12);
                            data.entries[38].body = "保留换行的文本\n".repeat(1000);
                            data.entries[0] = entry(1, Kind::Paper);
                            data.entries[1] = entry(2, Kind::Link);
                            panel.data = Arc::new(data);
                            panel.rebuild(cx);
                            panel
                        })
                    },
                )
            })
            .unwrap();
        (handle.downcast().unwrap(), panel)
    }
    #[test]
    fn command_shortcuts_require_a_boundary_and_preserve_the_query() {
        assert_eq!(shortcut("/ports 3000"), Some((Section::Ports, "3000")));
        assert_eq!(shortcut("/todo 买牛奶"), Some((Section::Todos, "买牛奶")));
        assert_eq!(shortcut("/save 发票"), Some((Section::Saved, "发票")));
        assert_eq!(shortcut("/todo-list"), None);
        assert_eq!(shortcut("https://example.org/ports"), None);
    }
    #[gpui_kit::test]
    fn inline_saved_input_keeps_enter_for_newlines_and_submits_once(cx: &mut TestAppContext) {
        let (handle, panel) = fixture(cx);
        let (tx, rx) = std::sync::mpsc::channel();
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.worker = Some(tx);
                p.activate(Section::Saved, window, cx);
                assert!(p.saved.focus_handle(cx).is_focused(window));
                p.saved
                    .update(cx, |s, cx| s.set_value("公司名称\n税号 123456", window, cx));
            });
            window.render_frame(cx);
            let composer = window.find("saved-composer").bounds();
            assert!(composer.bottom() <= window.find("workbench-list-pane").bounds().top());
            assert!(window.find("add-saved").visible());
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(rx.try_recv().is_err(), "普通 Enter 应保留给多行输入");
        let content = cx.read(|cx| panel.read(cx).saved.read(cx).value().to_string());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-enter"
                } else {
                    "ctrl-enter"
                },
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        let Command::Edit(change @ Change::Save { .. }) = rx.try_recv().unwrap() else {
            panic!("expected save")
        };
        let Change::Save { draft, .. } = &change else {
            unreachable!()
        };
        assert_eq!(draft.body, content);
        assert!(rx.try_recv().is_err());
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                assert!(p.editor.is_none());
                p.received(
                    organizer::Event {
                        change: Some(change),
                        result: Err("磁盘写入失败".into()),
                    },
                    window,
                    cx,
                );
                assert_eq!(p.saved.read(cx).value().as_ref(), content);
            });
            window.render_frame(cx);
            window.click("add-saved", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let Command::Edit(change) = rx.try_recv().unwrap() else {
            panic!("expected retry")
        };
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.received(
                    organizer::Event {
                        change: Some(change),
                        result: Ok(p.data.clone()),
                    },
                    window,
                    cx,
                );
                assert!(p.saved.read(cx).value().is_empty());
                assert_eq!(p.message, "已收藏到本机");
            });
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn clipboard_save_skips_the_editor_and_preserves_the_inline_draft(cx: &mut TestAppContext) {
        let (handle, panel) = fixture(cx);
        let (tx, rx) = std::sync::mpsc::channel();
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.worker = Some(tx);
                p.saved
                    .update(cx, |s, cx| s.set_value("还在写的内容", window, cx));
                cx.write_to_clipboard(ClipboardItem::new_string(
                    "https://example.org/paper\n阅读笔记".into(),
                ));
            });
            window.render_frame(cx);
            window.click("paste-saved", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let Command::Edit(change) = rx.try_recv().unwrap() else {
            panic!("expected clipboard save")
        };
        let Change::Save { draft, .. } = &change else {
            panic!("expected save")
        };
        assert_eq!(draft.body, "https://example.org/paper\n阅读笔记");
        assert_eq!(draft.kind, Kind::Text);
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                assert!(p.editor.is_none());
                p.received(
                    organizer::Event {
                        change: Some(change),
                        result: Ok(p.data.clone()),
                    },
                    window,
                    cx,
                );
                assert_eq!(p.saved.read(cx).value().as_ref(), "还在写的内容");
                p.action(Action::Copy, window, cx);
                assert_eq!(p.copied_entry, p.selected_entry().map(|e| e.id));
            });
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn long_content_keeps_list_detail_and_footer_inside_the_window(cx: &mut TestAppContext) {
        let (handle, panel) = fixture(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.selected = p.visible.iter().position(|&i| i == 0).unwrap();
                cx.notify();
            });
            window.render_frame(cx);
            let list = window.find("workbench-list-pane").bounds();
            let detail = window.find("workbench-detail").bounds();
            let footer = window.find("workbench-status").bounds();
            assert!(list.right() <= detail.left());
            assert!(detail.right() <= px(720.));
            for id in ["copy-saved", "open-saved", "edit-saved"] {
                assert!(window.find(id).visible());
                assert!(window.find(id).bounds().right() <= detail.right());
            }
            assert!(window.try_find("论文").is_none());
            assert!(window.try_find("链接").is_none());
            assert!(list.bottom() <= footer.top());
            assert!(footer.bottom() <= px(560.));
            for i in 0_usize..3 {
                let row = window.find(("workbench-row", i)).bounds();
                assert!(row.right() <= list.right());
                assert_eq!(row.size.height, px(60.));
            }
            panel.update(cx, |p, cx| {
                p.selected = p.visible.iter().position(|&i| i == 38).unwrap();
                p.scroll
                    .scroll_to_item_strict(p.selected, ScrollStrategy::Bottom);
                cx.notify();
            });
            window.render_frame(cx);
            let index = panel.read(cx).selected;
            let row = window.find(("workbench-row", index)).bounds();
            assert!(row.bottom() <= footer.top());
            assert!(window.find("copy-saved").visible());
            assert!(window.find("copy-saved").bounds().bottom() <= footer.top());
        })
        .unwrap();
    }
    #[gpui_kit::test]
    fn chinese_composition_in_saved_input_does_not_submit(cx: &mut TestAppContext) {
        let (handle, panel) = fixture(cx);
        let (tx, rx) = std::sync::mpsc::channel();
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.worker = Some(tx);
                p.activate(Section::Saved, window, cx);
                p.saved.update(cx, |s, cx| {
                    s.replace_and_mark_text_in_range(None, "收藏", Some(2..2), window, cx);
                });
            });
            window.render_frame(cx);
            window.press("secondary-enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(rx.try_recv().is_err());
        cx.read(|cx| assert!(!panel.read(cx).pending));
    }

    #[gpui_kit::test]
    fn editor_keeps_focus_and_requires_explicit_discard(cx: &mut TestAppContext) {
        let (handle, panel) = fixture(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.edit(entry(1, Kind::Text), window, cx);
                p.title
                    .update(cx, |s, cx| s.set_value("待保存的发票抬头", window, cx));
                p.body.update(cx, |s, cx| s.focus(window, cx));
                p.focus(window, cx);
                assert!(p.body.focus_handle(cx).is_focused(window));
                p.close(window, cx);
                assert!(p.discard);
                assert!(p.editor.is_some());
                p.activate(Section::Todos, window, cx);
                assert_eq!(p.section, Section::Saved);
                cx.notify();
            });
            window.render_frame(cx);
            let save = window.find("save-entry");
            assert!(save.visible());
            assert!(save.bounds().bottom() < window.find("workbench-status").bounds().top());
            assert!(window.find("discard-editor").visible());
        })
        .unwrap();
    }
    #[gpui_kit::test]
    fn todo_enter_submits_once_and_completion_can_be_undone(cx: &mut TestAppContext) {
        let (handle, panel) = fixture(cx);
        let (tx, rx) = std::sync::mpsc::channel();
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.worker = Some(tx);
                p.activate(Section::Todos, window, cx);
                assert!(p.quick.focus_handle(cx).is_focused(window));
                p.seed("读一篇论文", window, cx);
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
        assert!(
            matches!(rx.try_recv().unwrap(), Command::Edit(Change::Save { draft, .. }) if draft.title == "读一篇论文")
        );
        assert!(rx.try_recv().is_err());
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                let todo = entry(100, Kind::Todo);
                let mut data = Data::default();
                data.entries.push(todo.clone());
                p.received(
                    organizer::Event {
                        change: Some(Change::Save {
                            id: None,
                            draft: Draft {
                                kind: Kind::Todo,
                                title: todo.title.clone(),
                                body: todo.body.clone(),
                                url: todo.url.clone(),
                            },
                        }),
                        result: Ok(Arc::new(data.clone())),
                    },
                    window,
                    cx,
                );
                assert!(p.quick.read(cx).value().is_empty());
                assert_eq!(p.visible.len(), 1);
                p.send(Change::Done(100, true), Some(Change::Done(100, false)), cx);
                data.entries[0].done = true;
                p.received(
                    organizer::Event {
                        change: Some(Change::Done(100, true)),
                        result: Ok(Arc::new(data)),
                    },
                    window,
                    cx,
                );
                assert!(p.visible.is_empty());
                assert!(matches!(p.undo, Some(Change::Done(100, false))));
            });
        })
        .unwrap();
    }
    #[gpui_kit::test]
    fn composing_chinese_does_not_submit_and_failed_save_preserves_the_draft(
        cx: &mut TestAppContext,
    ) {
        let (handle, panel) = fixture(cx);
        let (tx, rx) = std::sync::mpsc::channel();
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.worker = Some(tx);
                p.activate(Section::Todos, window, cx);
                p.quick.update(cx, |s, cx| {
                    s.replace_and_mark_text_in_range(None, "论文", Some(2..2), window, cx);
                });
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
        assert!(
            rx.try_recv().is_err(),
            "IME confirmation must not create a todo"
        );
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.activate(Section::Saved, window, cx);
                p.edit(entry(1, Kind::Text), window, cx);
                p.title
                    .update(cx, |s, cx| s.set_value("发票抬头", window, cx));
                p.body
                    .update(cx, |s, cx| s.set_value("公司名称\n税号 123456", window, cx));
                let draft = p.draft(cx);
                p.save(cx);
                assert!(p.pending);
                p.received(
                    organizer::Event {
                        change: Some(Change::Save { id: None, draft }),
                        result: Err("测试：磁盘写入失败".into()),
                    },
                    window,
                    cx,
                );
                assert!(p.editor.is_some());
                assert_eq!(p.title.read(cx).value().as_ref(), "发票抬头");
                assert_eq!(p.body.read(cx).value().as_ref(), "公司名称\n税号 123456");
                assert!(!p.pending);
                assert!(!p.load_error);
                assert!(p.message.contains("磁盘写入失败"));
            });
        })
        .unwrap();
    }
    #[gpui_kit::test]
    fn port_filters_are_exact_and_unknown_identity_has_no_kill_action(cx: &mut TestAppContext) {
        let (handle, panel) = fixture(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.section = Section::Ports;
                p.sockets = [3000, 13000]
                    .map(|port| ports::Socket {
                        protocol: ports::Protocol::Tcp,
                        address: format!("127.0.0.1:{port}"),
                        port,
                        pid: 9000,
                        name: "node".into(),
                        process: None,
                    })
                    .to_vec();
                p.search.update(cx, |s, cx| s.set_value("3000", window, cx));
                p.rebuild(cx);
                assert_eq!(p.visible.len(), 1);
                assert!(!p.selected_socket().unwrap().can_terminate());
                p.open_menu(0, point(px(700.), px(540.)), window, cx);
            });
            window.render_frame(cx);
            assert!(panel.read(cx).menu.is_some());
            window.press("escape", cx);
        })
        .unwrap();
    }
}
