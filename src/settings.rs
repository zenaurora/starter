mod hotkey_editor;
use hotkey_editor::{HotkeyEditor, key_label};

use crate::appearance;
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        ActiveTheme, Disableable, Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        switch::Switch,
    },
    prelude::FluentBuilder,
    *,
};
use starter::updates::{self, Status as UpdateStatus};
use starter::{
    catalog::Application,
    config::{AppShortcut, Config, ThemeName, expand_home},
    hotkeys, opening,
};
use std::path::PathBuf;

pub enum Event {
    Save(Box<Config>),
    Preview(ThemeName),
    Close,
    OpenConfig,
    CheckUpdates,
    InstallUpdate,
    CancelUpdate,
    ClearClipboard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Startup,
    Opening,
    Appearance,
    Search,
    Aliases,
    Updates,
    Clipboard,
}

pub struct Settings {
    draft: Config,
    tab: Tab,
    focus: FocusHandle,
    content_scroll: ScrollHandle,
    launcher: Entity<HotkeyEditor>,
    font: Entity<InputState>,
    roots: Entity<TextareaState>,
    alias_app: Entity<InputState>,
    alias_names: Entity<InputState>,
    shortcut_key: Entity<HotkeyEditor>,
    shortcut_apps: Vec<Entity<InputState>>,
    rule: Entity<InputState>,
    rule_app: Entity<InputState>,
    apps: Vec<Application>,
    picker: Option<Entity<InputState>>,
    app_search: Entity<InputState>,
    editing_shortcut: Option<usize>,
    launcher_editor_open: bool,
    shortcut_editor_open: bool,
    _subscriptions: Vec<Subscription>,
    pub error: Option<String>,
    pub update_status: UpdateStatus,
    pub can_install: bool,
    clipboard_clear_confirmation: bool,
    pub clipboard_message: Option<String>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<Event> for Settings {}

impl Settings {
    pub fn set_apps(&mut self, apps: Vec<Application>, cx: &mut Context<Self>) {
        self.apps = apps;
        cx.notify();
    }

    pub fn show_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = Tab::Updates;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn new(
        config: &Config,
        apps: Vec<Application>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let launcher = cx.new(|cx| {
            HotkeyEditor::new("呼出快捷键", &config.launcher_hotkey, window, cx).without_label()
        });
        let font =
            cx.new(|cx| InputState::new(window, cx).default_value(config.monospace_font.clone()));
        let roots = cx.new(|cx| {
            TextareaState::new(window, cx)
                .default_value(
                    config
                        .search_roots
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
                .placeholder("~/Documents\n~/code")
        });
        let alias_app = cx.new(|cx| InputState::new(window, cx).placeholder("例如 Terminal"));
        let alias_names = cx.new(|cx| InputState::new(window, cx).placeholder("term, 终端"));
        let shortcut_key =
            cx.new(|cx| HotkeyEditor::new("应用快捷键", "", window, cx).without_label());
        let shortcut_app =
            cx.new(|cx| InputState::new(window, cx).placeholder("选择应用，或填写名称/路径"));
        let rule = cx.new(|cx| InputState::new(window, cx).placeholder("md / pdf / folder / *"));
        let rule_app = cx.new(|cx| InputState::new(window, cx).placeholder("选择用于打开的应用"));
        let app_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索已安装应用"));
        let subscription = cx.subscribe(&app_search, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            draft: config.clone(),
            tab: Tab::General,
            focus,
            content_scroll: ScrollHandle::new(),
            launcher,
            font,
            roots,
            alias_app,
            alias_names,
            shortcut_key,
            shortcut_apps: vec![shortcut_app],
            rule,
            rule_app,
            apps,
            picker: None,
            app_search,
            editing_shortcut: None,
            launcher_editor_open: false,
            shortcut_editor_open: false,
            _subscriptions: vec![subscription],
            error: None,
            update_status: UpdateStatus::Idle,
            can_install: false,
            clipboard_clear_confirmation: false,
            clipboard_message: None,
            _tasks: Vec::new(),
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let result = self.values(cx);
        match result {
            Ok(config) => cx.emit(Event::Save(Box::new(config))),
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                cx.notify();
            }
        }
    }

    fn values(&self, cx: &App) -> anyhow::Result<Config> {
        let mut config = self.draft.clone();
        config.launcher_hotkey = self.launcher.read(cx).value(cx)?;
        if let Some(shortcut) = self.pending_shortcut(cx)? {
            if let Some(i) = self.editing_shortcut {
                config.shortcuts[i] = shortcut;
            } else {
                config.shortcuts.push(shortcut);
            }
        }
        let rule = self.rule.read(cx).value();
        let app = self.rule_app.read(cx).value();
        if !rule.trim().is_empty() || !app.trim().is_empty() {
            let rule = opening::normalize_rule(&rule)?;
            anyhow::ensure!(!app.trim().is_empty(), "请选择打开应用");
            config.open_with.insert(rule, app.trim().to_string());
        }
        hotkeys::bindings(&config)?;
        config.monospace_font = self.font.read(cx).value().trim().to_string();
        anyhow::ensure!(!config.monospace_font.is_empty(), "请填写等宽字体名称");
        config.search_roots = self
            .roots
            .read(cx)
            .value()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect();
        for root in &config.search_roots {
            anyhow::ensure!(
                expand_home(root).is_dir(),
                "搜索目录不存在：{}",
                root.display()
            );
        }
        Ok(config)
    }

    fn add_alias(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.alias_app.read(cx).value().trim().to_string();
        let names: Vec<String> = self
            .alias_names
            .read(cx)
            .value()
            .split([',', '，'])
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        if app.is_empty() || names.is_empty() {
            self.error = Some("请填写应用名称和至少一个别名".into());
        } else {
            self.draft.aliases.insert(app, names);
            self.alias_app
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.alias_names
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.error = None;
        }
        cx.notify();
    }

    fn add_roots(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("选择搜索目录".into()),
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        let mut value = this.roots.read(cx).value().to_string();
                        for path in paths {
                            let path = path.display().to_string();
                            if !value.lines().any(|line| line == path) {
                                if !value.is_empty() && !value.ends_with('\n') {
                                    value.push('\n');
                                }
                                value.push_str(&path);
                            }
                        }
                        this.roots
                            .update(cx, |input, cx| input.set_value(value, window, cx));
                    }
                    Err(error) => this.error = Some(format!("目录选择失败：{error}")),
                    Ok(Err(error)) => this.error = Some(format!("目录选择失败：{error}")),
                    _ => {}
                }
                cx.notify();
            });
        }));
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let composing = [
            &self.font,
            &self.alias_app,
            &self.alias_names,
            &self.rule,
            &self.rule_app,
            &self.app_search,
        ]
        .into_iter()
        .chain(self.shortcut_apps.iter())
        .any(|input| {
            input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            })
        }) || self.roots.update(cx, |input, cx| {
            input.marked_text_range(window, cx).is_some()
        });
        if composing {
            return;
        }
        if event.keystroke.key == "escape" {
            if self.picker.take().is_some() {
                window.focus(&self.focus, cx);
                self.content_scroll.set_offset(Point::default());
                cx.notify();
            } else if self.tab == Tab::General && self.shortcut_editor_open {
                self.cancel_shortcut(window, cx);
            } else {
                cx.emit(Event::Close);
            }
            cx.stop_propagation();
        } else if event.keystroke.key == "s"
            && (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
        {
            self.save(cx);
            cx.stop_propagation();
        }
    }

    fn pending_shortcut(&self, cx: &App) -> anyhow::Result<Option<AppShortcut>> {
        let hotkey = self.shortcut_key.read(cx).value(cx)?;
        let applications = self
            .shortcut_apps
            .iter()
            .map(|input| input.read(cx).value().trim().to_string())
            .collect::<Vec<_>>();
        if hotkey.is_empty()
            && applications.iter().all(String::is_empty)
            && self.editing_shortcut.is_none()
        {
            return Ok(None);
        }
        anyhow::ensure!(
            !applications.is_empty() && applications.iter().all(|app| !app.is_empty()),
            "请为快捷键选择至少一个应用，并填写所有目标应用；多余的空行可移除"
        );
        let key = hotkeys::parse(&hotkey)?;
        anyhow::ensure!(!key.mods.is_empty(), "全局快捷键必须包含修饰键");
        let enabled = self
            .editing_shortcut
            .is_none_or(|i| self.draft.shortcuts[i].enabled);
        Ok(Some(AppShortcut {
            hotkey,
            applications,
            enabled,
        }))
    }

    fn set_shortcut_apps(
        &mut self,
        applications: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shortcut_apps = applications
            .iter()
            .map(|application| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(application.clone())
                        .placeholder("选择应用，或填写名称/路径")
                })
            })
            .collect();
        if self.shortcut_apps.is_empty() {
            self.add_shortcut_app(window, cx);
        }
    }

    fn add_shortcut_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shortcut_apps.push(
            cx.new(|cx| InputState::new(window, cx).placeholder("选择应用，或填写名称/路径")),
        );
        cx.notify();
    }

    fn edit_shortcut(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let shortcut = self.draft.shortcuts[index].clone();
        self.shortcut_key.update(cx, |input, cx| {
            input.set_value(&shortcut.hotkey, window, cx)
        });
        self.set_shortcut_apps(&shortcut.applications, window, cx);
        self.editing_shortcut = Some(index);
        self.shortcut_editor_open = true;
        self.launcher_editor_open = false;
        self.content_scroll.set_offset(Point::default());
        self.error = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn new_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_shortcut(window, cx);
        self.shortcut_editor_open = true;
        self.launcher_editor_open = false;
        cx.notify();
    }

    fn cancel_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shortcut_key
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.set_shortcut_apps(&[], window, cx);
        self.editing_shortcut = None;
        self.shortcut_editor_open = false;
        self.content_scroll.set_offset(Point::default());
        self.picker = None;
        self.error = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn add_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shortcut = match self.pending_shortcut(cx) {
            Ok(Some(shortcut)) => shortcut,
            Ok(None) => {
                self.error = Some("请选择快捷键和应用".into());
                cx.notify();
                return;
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        let mut config = self.draft.clone();
        let launcher = match self.launcher.read(cx).value(cx) {
            Ok(launcher) => launcher,
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        config.launcher_hotkey = launcher;
        if let Some(i) = self.editing_shortcut {
            config.shortcuts[i] = shortcut;
        } else {
            config.shortcuts.push(shortcut);
        }
        match hotkeys::bindings(&config) {
            Ok(_) => {
                self.draft.shortcuts = config.shortcuts;
                self.editing_shortcut = None;
                self.shortcut_key
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.set_shortcut_apps(&[], window, cx);
                self.shortcut_editor_open = false;
                self.content_scroll.set_offset(Point::default());
                window.focus(&self.focus, cx);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    fn add_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<()> {
            let rule = opening::normalize_rule(&self.rule.read(cx).value())?;
            let app = self.rule_app.read(cx).value().trim().to_string();
            anyhow::ensure!(!app.is_empty(), "请选择打开应用");
            self.draft.open_with.insert(rule, app);
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.error = None;
                self.rule
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.rule_app
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
        cx.notify();
    }

    fn choose_app(
        &mut self,
        input: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker = Some(input);
        self.content_scroll.set_offset(Point::default());
        self.app_search.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn browse_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.picker.clone() else {
            return;
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: cfg!(target_os = "macos"),
            multiple: false,
            prompt: Some("选择应用或可执行文件".into()),
        });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(Some(paths))) if !paths.is_empty() => {
                        input.update(cx, |input, cx| {
                            input.set_value(paths[0].display().to_string(), window, cx);
                            input.focus(window, cx);
                        });
                        this.picker = None;
                    }
                    Ok(Err(error)) => this.error = Some(format!("选择失败：{error}")),
                    Err(error) => this.error = Some(format!("选择失败：{error}")),
                    _ => {}
                }
                cx.notify();
            });
        }));
    }

    fn app_field(
        &self,
        label: &'static str,
        input: &Entity<InputState>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let target = input.clone();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(label)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Input::new(input).aria_label(label))
                    .child(Button::new(label).label("选择应用").on_click(cx.listener(
                        move |this, _, window, cx| this.choose_app(target.clone(), window, cx),
                    ))),
            )
            .into_any_element()
    }

    fn picker_content(&self, cx: &Context<Self>) -> AnyElement {
        let Some(target) = self.picker.clone() else {
            return div().into_any_element();
        };
        let query = self.app_search.read(cx).value().to_lowercase();
        let mut list = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(cx.theme().border)
            .child(Input::new(&self.app_search).aria_label("搜索已安装应用"));
        for app in self
            .apps
            .iter()
            .filter(|app| {
                app.name.to_lowercase().contains(&query)
                    || app
                        .aliases
                        .iter()
                        .any(|a| a.to_lowercase().contains(&query))
            })
            .take(8)
        {
            let path = app.path.display().to_string();
            let input = target.clone();
            list = list.child(
                Button::new(app.id.clone())
                    .label(app.name.clone())
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        input.update(cx, |input, cx| {
                            input.set_value(path.clone(), window, cx);
                            input.focus(window, cx);
                        });
                        this.picker = None;
                        cx.notify();
                    })),
            );
        }
        list.child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("browse-app")
                        .label("浏览…")
                        .on_click(cx.listener(|this, _, window, cx| this.browse_app(window, cx))),
                )
                .child(
                    Button::new("close-picker")
                        .label("收起")
                        .ghost()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.picker = None;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }

    fn shortcut_app_fields(&self, cx: &Context<Self>) -> AnyElement {
        let mut fields = div().flex().flex_col().gap_2();
        for (i, input) in self.shortcut_apps.iter().enumerate() {
            let target = input.clone();
            fields = fields.child(
                div()
                    .id(("shortcut-app", i))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(16.))
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child((i + 1).to_string()),
                    )
                    .child(
                        Input::new(input)
                            .aria_label(format!("目标应用 {}", i + 1))
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        Button::new("choose-shortcut-app")
                            .label("选择")
                            .ghost()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose_app(target.clone(), window, cx)
                            })),
                    )
                    .child(
                        Button::new(("remove-shortcut-app", i))
                            .label("移除")
                            .ghost()
                            .small()
                            .tooltip("移除这个应用")
                            .disabled(self.shortcut_apps.len() == 1)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.shortcut_apps.remove(i);
                                cx.notify();
                            })),
                    ),
            );
        }
        fields
            .child(
                Button::new("add-shortcut-app")
                    .label("＋ 添加应用")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.add_shortcut_app(window, cx))),
            )
            .into_any_element()
    }

    fn app_name(&self, application: &str) -> String {
        if let Some(app) = self
            .apps
            .iter()
            .find(|app| app.path.to_str() == Some(application))
        {
            return app.name.clone();
        }
        let path = std::path::Path::new(application);
        if application.contains(['/', '\\']) || path.extension().is_some() {
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or(application)
                .to_string()
        } else {
            application.to_string()
        }
    }

    fn hotkey_badges(value: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        if value.trim().is_empty() {
            return div()
                .text_size(px(11.))
                .text_color(theme.muted_foreground)
                .child("未启用")
                .into_any_element();
        }
        let parts = hotkeys::parts(value).unwrap_or_else(|_| vec![value.to_string()]);
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .children(parts.iter().map(|part| {
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(key_label(part))
            }))
            .into_any_element()
    }

    fn launcher_hotkey_row(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let value = self.launcher.read(cx).value(cx);
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("呼出 Starter"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .child("打开或收起搜索窗口"),
                            ),
                    )
                    .when(!self.launcher_editor_open, |row| {
                        row.child(Self::hotkey_badges(
                            value.as_deref().unwrap_or("待完善"),
                            cx,
                        ))
                    })
                    .child(
                        Button::new("edit-launcher-hotkey")
                            .label(if self.launcher_editor_open {
                                "收起"
                            } else {
                                "修改"
                            })
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.launcher_editor_open = !this.launcher_editor_open;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.launcher_editor_open, |body| {
                body.child(
                    div()
                        .pt_3()
                        .border_t_1()
                        .border_color(theme.border)
                        .child(self.launcher.clone()),
                )
            })
            .into_any_element()
    }

    fn hotkey_overview(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut list = div().flex().flex_col();
        for (i, shortcut) in self.draft.shortcuts.iter().enumerate() {
            let names = shortcut
                .applications
                .iter()
                .map(|app| self.app_name(app))
                .collect::<Vec<_>>()
                .join(" + ");
            list = list.child(
                div()
                    .id(("shortcut-group", i))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .py_3()
                    .when(i > 0, |row| row.border_t_1().border_color(theme.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if shortcut.enabled {
                                        theme.foreground
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .text_ellipsis()
                                    .child(names),
                            )
                            .child(
                                Switch::new(("shortcut-enabled", i))
                                    .checked(shortcut.enabled)
                                    .on_change(cx.listener(move |this, checked, _, cx| {
                                        this.draft.shortcuts[i].enabled = *checked;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Self::hotkey_badges(&shortcut.hotkey, cx))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .child(if shortcut.enabled {
                                        format!("{} 个应用", shortcut.applications.len())
                                    } else {
                                        "已停用".to_string()
                                    }),
                            )
                            .child(
                                Button::new(("edit-shortcut", i))
                                    .label("编辑")
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.edit_shortcut(i, window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("remove-shortcut", i))
                                    .label("删除")
                                    .ghost()
                                    .small()
                                    .text_color(theme.muted_foreground)
                                    .tooltip("删除这组快捷键")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.draft.shortcuts.remove(i);
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Starter 入口"),
                    )
                    .child(
                        div()
                            .rounded_lg()
                            .bg(theme.muted)
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(self.launcher_hotkey_row(cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("应用快捷键"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} 组", self.draft.shortcuts.len())),
                            )
                            .child(
                                Button::new("new-shortcut")
                                    .label("＋ 新建")
                                    .outline()
                                    .small()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.new_shortcut(window, cx)
                                    })),
                            ),
                    )
                    .when(self.draft.shortcuts.is_empty(), |body| {
                        body.child(
                            div()
                                .rounded_lg()
                                .border_1()
                                .border_color(theme.border)
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("把常用应用放进一组快捷键"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .child("例如一次打开 kitty 和 ChatGPT。点击「新建」开始。"),
                                ),
                        )
                    })
                    .when(!self.draft.shortcuts.is_empty(), |body| body.child(list)),
            )
            .into_any_element()
    }

    fn shortcut_editor(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("cancel-shortcut")
                            .label("取消编辑")
                            .tooltip("放弃本次编辑，返回快捷键列表")
                            .ghost()
                            .small()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.cancel_shortcut(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("add-shortcut")
                            .label(if self.editing_shortcut.is_some() {
                                "更新列表"
                            } else {
                                "加入列表"
                            })
                            .outline()
                            .small()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_shortcut(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .rounded_lg()
                    .bg(theme.muted)
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("按下什么"),
                    )
                    .child(self.shortcut_key.clone())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child("选择一个普通按键和修饰键；＋ 可增加修饰键。"),
                    ),
            )
            .child(
                div()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("打开哪些应用"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} 个目标", self.shortcut_apps.len())),
                            ),
                    )
                    .child(self.shortcut_app_fields(cx))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child("这些应用会同时打开。可选择已安装的应用，或填写名称／路径。"),
                    ),
            )
            .into_any_element()
    }

    fn field(label: &'static str, help: &'static str, input: &Entity<InputState>) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label),
            )
            .child(Input::new(input).aria_label(label))
            .child(div().text_size(px(11.)).child(help))
            .into_any_element()
    }

    fn content(&self, cx: &Context<Self>) -> AnyElement {
        if self.picker.is_some() {
            return self.picker_content(cx);
        }
        let theme = cx.theme();
        let body = div().w_full().flex().flex_col().gap_5();
        match self.tab {
            Tab::Startup => body
                .child(div().flex().items_center().justify_between()
                    .child("开机自启动")
                    .child(Switch::new("launch-at-login")
                        .checked(self.draft.launch_at_login)
                        .on_change(cx.listener(|this, checked, _, cx| {
                            this.draft.launch_at_login = *checked;
                            cx.notify();
                        }))))
                .child(div().text_size(px(12.)).text_color(theme.muted_foreground)
                    .child("登录系统后自动运行 Starter，驻留托盘。需要时按快捷键呼出搜索窗口。"))
                .child(div().text_size(px(11.)).text_color(theme.muted_foreground)
                    .child("保存设置后生效。"))
                .into_any_element(),
            Tab::Clipboard => body
                .child(div().flex().items_center().justify_between().child("记录剪贴板历史")
                    .child(Switch::new("clipboard-history").checked(self.draft.clipboard_history)
                        .on_change(cx.listener(|this, checked, _, cx| { this.draft.clipboard_history = *checked; cx.notify(); }))))
                .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("保存设置后生效。开启时在后台记录新复制的文本、图片、文件和链接，仅保存到本机。暂停后已有历史仍可回搜。"))
                .child(div().text_size(px(12.)).child("输入 /clip 打开历史。关键词搜索文本、网址和文件名；图片支持缩略图，以及类型、复制日期筛选。"))
                .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("保留最近 30 天、最多 200 条，内容总量上限 100 MiB，单条上限 16 MiB。文件记录保存原路径，文件移动或删除后无法再次复制。系统标记为私密或临时的内容不会记录。"))
                .child(Button::new("clear-clipboard").label(if self.clipboard_clear_confirmation { "确认清空所有历史" } else { "清空剪贴板历史" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.clipboard_clear_confirmation { cx.emit(Event::ClearClipboard); this.clipboard_clear_confirmation = false; }
                        else { this.clipboard_clear_confirmation = true; }
                        cx.notify();
                    })))
                .when(self.clipboard_clear_confirmation, |body| body.child(Button::new("cancel-clear-clipboard").label("取消清空").ghost()
                    .on_click(cx.listener(|this, _, _, cx| { this.clipboard_clear_confirmation = false; cx.notify(); }))))
                .when_some(self.clipboard_message.clone(), |body, message| body.child(message))
                .into_any_element(),
            Tab::Updates => body
                .child(div().text_size(px(13.)).child(format!("当前版本 {}", updates::CURRENT_VERSION)))
                .child(div().flex().items_center().justify_between()
                    .child("自动检查更新")
                    .child(Switch::new("auto-check-updates")
                        .checked(self.draft.auto_check_updates)
                        .on_change(cx.listener(|this, checked, _, cx| {
                            this.draft.auto_check_updates = *checked;
                            cx.notify();
                        }))))
                .child(div().text_size(px(12.)).text_color(theme.muted_foreground)
                    .child("启动时及运行期间每 24 小时检查一次。保存设置后生效。"))
                .child(div().text_size(px(12.)).child(self.update_status.label()))
                .child(Button::new("check-updates").label("立即检查")
                    .disabled(self.update_status.busy())
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::CheckUpdates))))
                .when(self.can_install && !self.update_status.busy(), |body| body.child(Button::new("install-update").label("下载并更新").primary()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::InstallUpdate)))))
                .when(matches!(self.update_status, UpdateStatus::Downloading { .. }), |body| body.child(Button::new("cancel-update").label("取消下载")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::CancelUpdate)))))
                .child(div().text_size(px(11.)).text_color(theme.muted_foreground)
                    .child("点击更新后自动下载、校验、安装并重新启动。配置和使用记录会保留。"))
                .into_any_element(),
            Tab::General => {
                if self.shortcut_editor_open {
                    self.shortcut_editor(cx)
                } else {
                    self.hotkey_overview(cx)
                }
            }
            Tab::Opening => {
                let mut list = div().flex().flex_col().gap_2();
                for (rule, application) in &self.draft.open_with {
                    let edit_rule = rule.clone(); let edit_app = application.clone(); let remove_rule = rule.clone();
                    list = list.child(div().flex().items_center().gap_2().py_2().border_b_1().border_color(theme.border)
                        .child(div().w(px(65.)).child(rule.clone()))
                        .child(div().flex_1().min_w_0().text_ellipsis().child(application.clone()))
                        .child(Button::new(format!("edit-{rule}")).label("编辑").ghost().small().on_click(cx.listener(move |this, _, window, cx| {
                            this.rule.update(cx, |input, cx| input.set_value(edit_rule.clone(), window, cx));
                            this.rule_app.update(cx, |input, cx| input.set_value(edit_app.clone(), window, cx)); cx.notify();
                        })))
                        .child(Button::new(format!("remove-{rule}")).icon(IconName::X).ghost().small().on_click(cx.listener(move |this, _, window, cx| {
                            this.draft.open_with.remove(&remove_rule);
                            if opening::normalize_rule(&this.rule.read(cx).value()).is_ok_and(|rule| rule == remove_rule) {
                                this.rule.update(cx, |input, cx| input.set_value("", window, cx));
                                this.rule_app.update(cx, |input, cx| input.set_value("", window, cx));
                            }
                            cx.notify();
                        }))));
                }
                body.gap_3().child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("设置从 Starter 打开文件时使用的应用。没有规则时使用系统默认；移除规则即可恢复。"))
                    .child(list).child(Self::field("文件类型", "例如 md、pdf；folder 表示文件夹，* 表示其余文件", &self.rule))
                    .child(self.app_field("打开应用", &self.rule_app, cx))
                    .child(Button::new("add-rule").label("添加或更新规则").on_click(cx.listener(|this, _, window, cx| this.add_rule(window, cx))))
                    .child(self.picker_content(cx)).into_any_element()
            }
            Tab::Search => body.gap_3()
                .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child("搜索目录"))
                .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("只搜索这里列出的目录。每行一个路径，支持 ~。"))
                .child(Textarea::new(&self.roots).h(px(140.)).aria_label("搜索目录"))
                .child(Button::new("add-roots").icon(IconName::FolderPlus).label("添加目录").on_click(cx.listener(|this, _, window, cx| this.add_roots(window, cx))))
                .child(div().text_size(px(11.)).text_color(theme.muted_foreground).line_height(relative(1.6)).child("/f 匹配文件与文件夹名称，支持模糊和错字。\n/c 查找文本内容，按字面匹配；跳过二进制及大于 2 MiB 的文件。\n遵循 .gitignore / .ignore，跳过隐藏文件和构建缓存。"))
                .into_any_element(),
            Tab::Aliases => {
                let mut aliases = div().w_full().flex().flex_col().gap_2();
                for (app, names) in &self.draft.aliases {
                    let name = app.clone();
                    aliases = aliases.child(div().w_full().flex().items_center().justify_between().gap_3().py_2().border_b_1().border_color(theme.border)
                        .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                            .child(div().text_size(px(13.)).text_ellipsis().child(app.clone()))
                            .child(div().text_size(px(12.)).text_color(theme.muted_foreground).text_ellipsis().child(names.join(", "))))
                        .child(Button::new(app.clone()).icon(IconName::X).ghost().small().tooltip("移除别名").on_click(cx.listener(move |this, _, _, cx| { this.draft.aliases.remove(&name); cx.notify(); }))));
                }
                body.gap_3()
                    .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child("应用名与搜索列表保持一致；别名用逗号分隔。"))
                    .child(Input::new(&self.alias_app).aria_label("应用名称"))
                    .child(Input::new(&self.alias_names).aria_label("别名"))
                    .child(Button::new("add-alias").icon(IconName::Plus).label("添加或更新").on_click(cx.listener(|this, _, window, cx| this.add_alias(window, cx))))
                    .child(aliases)
                    .when(self.draft.aliases.is_empty(), |this| this.child(div().py_3().text_size(px(12.)).text_color(theme.muted_foreground).child("还没有别名，例如给 Terminal 添加 term 和 终端。")))
                    .into_any_element()
            }
            Tab::Appearance => {
                let mut themes = div().flex().flex_col().gap_2();
                for pair in ThemeName::ALL.chunks(2) {
                    let mut row = div().flex().gap_2();
                    for name in pair.iter().copied() {
                        let p = appearance::palette(name);
                        let selected = name == self.draft.theme;
                        let swatches = div().flex().gap_1().children([p.accent, p.green, p.yellow, p.red].map(|color| div().w(px(30.)).h(px(8.)).rounded_sm().bg(color)));
                        row = row.child(div().flex_1().min_w_0().flex().flex_col().gap_2().p_3().rounded_md().bg(p.base).border_1().border_color(if selected { p.accent } else { p.border })
                            .child(Button::new(name.label()).label(name.label()).ghost().text_color(p.text).small().on_click(cx.listener(move |this, _, _, cx| { this.draft.theme = name; cx.emit(Event::Preview(name)); cx.notify(); })))
                            .child(swatches));
                    }
                    themes = themes.child(row);
                }
                body.gap_4().child(themes)
                    .child(Self::field("等宽字体", "默认使用系统等宽字体；可填写已安装的 JetBrains Mono", &self.font))
                    .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child("点击配色可预览，保存后下次启动仍使用该配色。"))
                    .into_any_element()
            }
        }
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tabs = [
            (Tab::Startup, IconName::Settings2, "启动"),
            (Tab::General, IconName::Keyboard, "快捷键"),
            (Tab::Opening, IconName::AppWindow, "打开方式"),
            (Tab::Appearance, IconName::Palette, "外观"),
            (Tab::Search, IconName::FolderSearch, "搜索目录"),
            (Tab::Aliases, IconName::Tag, "应用别名"),
            (Tab::Updates, IconName::RefreshCw, "更新"),
            (Tab::Clipboard, IconName::FileText, "剪贴板"),
        ];
        let titles = match self.tab {
            Tab::Startup => ("启动", "设置 Starter 在登录系统时的行为。"),
            Tab::Opening => ("打开方式", "为文件和文件夹指定常用应用。"),
            Tab::General if self.shortcut_editor_open => (
                if self.editing_shortcut.is_some() {
                    "编辑应用快捷键"
                } else {
                    "新建应用快捷键"
                },
                "一次按键，打开一个或多个应用。",
            ),
            Tab::General => ("快捷键", "管理 Starter 的入口和应用启动组合。"),
            Tab::Appearance => ("外观", "熟悉的编辑器配色，安静的桌面入口。"),
            Tab::Search => ("搜索目录", "限定搜索范围，保持轻量。"),
            Tab::Aliases => ("应用别名", "用你习惯的名字打开应用。"),
            Tab::Updates => ("更新", "检查 Starter 的新版本。"),
            Tab::Clipboard => ("剪贴板", "找回复制过的内容。"),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(
                div()
                    .h(px(64.))
                    .flex_shrink_0()
                    .px_5()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("back")
                            .icon(IconName::ArrowLeft)
                            .ghost()
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                    )
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("设置"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("Starter"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(154.))
                            .flex_shrink_0()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .border_r_1()
                            .border_color(theme.border)
                            .children(tabs.map(|(tab, icon, label)| {
                                Button::new(label)
                                    .icon(icon)
                                    .label(label)
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .when(self.tab == tab, |b| b.bg(theme.accent))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.picker = None;
                                        this.tab = tab;
                                        this.content_scroll.set_offset(Point::default());
                                        window.focus(&this.focus, cx);
                                        this.error = None;
                                        cx.notify();
                                    }))
                            }))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .px_2()
                                    .pb_2()
                                    .text_size(px(10.))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("Starter {}", updates::CURRENT_VERSION)),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-content")
                            .flex_1()
                            .min_w_0()
                            .p_5()
                            .overflow_y_scroll()
                            .track_scroll(&self.content_scroll)
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(titles.0),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .mb_3()
                                    .child(titles.1),
                            )
                            .child(self.content(cx)),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_5()
                        .py_2()
                        .text_size(px(12.))
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .h(px(60.))
                    .flex_shrink_0()
                    .px_5()
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("open-config")
                            .label("配置文件")
                            .icon(IconName::FileText)
                            .ghost()
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::OpenConfig))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .when(self.tab == Tab::General, |row| {
                                row.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .mr_2()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .child("保存后生效"),
                                )
                            })
                            .child(
                                Button::new("cancel")
                                    .label("取消")
                                    .ghost()
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                            )
                            .child(
                                Button::new("save")
                                    .label("保存设置")
                                    .tooltip("保存所有修改并立即生效")
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::base::Root;
    use gpui_kit::test::TestWindowExt;

    fn fixture(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<Settings>) {
        cx.update(gpui_kit::init);
        let (handle, settings) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(720.), px(560.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Settings::new(&Config::default(), Vec::new(), window, cx)),
            )
            .unwrap()
        });
        (handle.downcast().unwrap(), settings)
    }

    #[gpui_kit::test]
    fn login_toggle_is_a_draft_and_cancel_does_not_save(cx: &mut TestAppContext) {
        let (handle, settings) = fixture(cx);
        let saved = std::rc::Rc::new(std::cell::Cell::new(false));
        let saved_event = saved.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&settings, move |_, event, _| {
                if let Event::Save(config) = event {
                    saved_event.set(config.launch_at_login);
                }
            })
        });
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| window.click("启动", cx))
            .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("launch-at-login", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(settings.read(cx).draft.launch_at_login);
            assert!(settings.read(cx).values(cx).unwrap().launch_at_login);
            assert!(!saved.get());
            window.click("cancel", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!saved.get());
        cx.update_window(handle.into(), |_, window, cx| window.click("save", cx))
            .unwrap();
        cx.run_until_parked();
        assert!(saved.get());
    }

    #[gpui_kit::test]
    fn overview_opens_only_the_requested_editor_and_cancel_discards_pending_targets(
        cx: &mut TestAppContext,
    ) {
        let (handle, settings) = fixture(cx);
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("edit-launcher-hotkey", cx);
            assert!(settings.read(cx).launcher_editor_open);
            window.click("edit-launcher-hotkey", cx);
            assert!(!settings.read(cx).launcher_editor_open);
            window.click("edit-launcher-hotkey", cx);
            window.click("new-shortcut", cx);
            settings.update(cx, |this, cx| {
                assert!(this.shortcut_editor_open);
                assert!(!this.launcher_editor_open);
                this.shortcut_key
                    .update(cx, |input, cx| input.set_value("Cmd+Enter", window, cx));
                this.set_shortcut_apps(&["kitty".into(), "ChatGPT".into()], window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("cancel-shortcut", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, _, cx| {
            settings.update(cx, |this, cx| {
                assert!(!this.shortcut_editor_open);
                assert!(this.values(cx).unwrap().shortcuts.is_empty());
            });
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn adding_a_group_returns_to_overview_and_escape_cancels_later_edits(cx: &mut TestAppContext) {
        let (handle, settings) = fixture(cx);
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("new-shortcut", cx);
            settings.update(cx, |this, cx| {
                this.shortcut_key
                    .update(cx, |input, cx| input.set_value("Cmd+Enter", window, cx));
                this.set_shortcut_apps(&["kitty".into(), "ChatGPT".into()], window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("add-shortcut", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert!(!settings.read(cx).shortcut_editor_open);
            window.click(("edit-shortcut", 0usize), cx);
            settings.update(cx, |this, cx| {
                this.shortcut_apps[0]
                    .update(cx, |input, cx| input.set_value("Changed", window, cx));
            });
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, _, cx| {
            settings.update(cx, |this, cx| {
                assert!(!this.shortcut_editor_open);
                assert_eq!(
                    this.values(cx).unwrap().shortcuts[0].applications,
                    vec!["kitty", "ChatGPT"]
                );
            });
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn clearing_clipboard_requires_confirmation_and_can_be_cancelled(cx: &mut TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        let (handle, settings) = fixture(cx);
        let count = Rc::new(Cell::new(0));
        let captured = count.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&settings, move |_, event, _| {
                if matches!(event, Event::ClearClipboard) {
                    captured.set(captured.get() + 1);
                }
            })
        });
        cx.update_window(handle.into(), |_, window, cx| window.click("剪贴板", cx))
            .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("clear-clipboard", cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(count.get(), 0);
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("cancel-clear-clipboard", cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(count.get(), 0);
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("clear-clipboard", cx)
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("clear-clipboard", cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(count.get(), 1);
    }

    #[gpui_kit::test]
    fn save_includes_pending_rule_and_shortcut_without_duplicating_added_rows(
        cx: &mut TestAppContext,
    ) {
        let (handle, settings) = fixture(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            settings.update(cx, |this, cx| {
                this.shortcut_key
                    .update(cx, |input, cx| input.set_value("Cmd+K", window, cx));
                this.set_shortcut_apps(&["kitty".into(), "ChatGPT".into()], window, cx);
                this.rule
                    .update(cx, |input, cx| input.set_value(".MD", window, cx));
                this.rule_app
                    .update(cx, |input, cx| input.set_value("Editor", window, cx));
                let saved = this.values(cx).unwrap();
                assert_eq!(saved.shortcuts.len(), 1);
                assert_eq!(saved.shortcuts[0].applications, vec!["kitty", "ChatGPT"]);
                assert_eq!(saved.open_with["md"], "Editor");
                assert!(this.draft.shortcuts.is_empty());
                assert!(
                    this.draft.open_with.is_empty(),
                    "cancel must still discard pending inputs"
                );
                this.add_shortcut(window, cx);
                assert_eq!(this.shortcut_apps.len(), 1);
                assert!(this.shortcut_apps[0].read(cx).value().is_empty());
                this.add_rule(window, cx);
                let saved = this.values(cx).unwrap();
                assert_eq!(saved.shortcuts.len(), 1);
                assert_eq!(saved.open_with.len(), 1);
                this.shortcut_key.update(cx, |input, cx| {
                    input.set_value(&saved.launcher_hotkey, window, cx)
                });
                this.shortcut_apps[0]
                    .update(cx, |input, cx| input.set_value("Duplicate", window, cx));
                assert!(
                    this.values(cx).is_err(),
                    "save must reject conflicts with built-in keys"
                );
            })
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn editing_a_disabled_shortcut_preserves_its_disabled_state(cx: &mut TestAppContext) {
        let (handle, settings) = fixture(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            settings.update(cx, |this, cx| {
                this.draft.shortcuts.push(AppShortcut {
                    hotkey: "Cmd+K".into(),
                    applications: vec!["kitty".into(), "ChatGPT".into()],
                    enabled: false,
                });
                this.edit_shortcut(0, window, cx);
                assert_eq!(this.shortcut_apps.len(), 2);
                assert_eq!(this.shortcut_apps[0].read(cx).value(), "kitty");
                assert_eq!(this.shortcut_apps[1].read(cx).value(), "ChatGPT");
                this.shortcut_key
                    .update(cx, |input, cx| input.set_value("Cmd+L", window, cx));
                this.shortcut_apps[0]
                    .update(cx, |input, cx| input.set_value("Browser", window, cx));
                let saved = this.values(cx).unwrap();
                assert_eq!(saved.shortcuts.len(), 1);
                assert_eq!(saved.shortcuts[0].applications, vec!["Browser", "ChatGPT"]);
                assert!(!saved.shortcuts[0].enabled);
                this.add_shortcut(window, cx);
                assert_eq!(this.draft.shortcuts, saved.shortcuts);
                assert!(this.editing_shortcut.is_none());
            })
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn adding_and_removing_targets_keeps_other_inputs_and_picker_independent(
        cx: &mut TestAppContext,
    ) {
        let (handle, settings) = fixture(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            settings.update(cx, |this, cx| {
                this.new_shortcut(window, cx);
                this.shortcut_key
                    .update(cx, |input, cx| input.set_value("Cmd+Enter", window, cx));
                this.shortcut_apps[0].update(cx, |input, cx| input.set_value("kitty", window, cx));
                this.add_shortcut_app(window, cx);
                assert!(
                    this.values(cx).is_err(),
                    "empty target rows must be filled or removed"
                );
                this.apps.push(Application {
                    id: "chatgpt".into(),
                    name: "ChatGPT".into(),
                    path: PathBuf::from("/Applications/ChatGPT.app"),
                    aliases: vec![],
                });
                this.choose_app(this.shortcut_apps[1].clone(), window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| window.click("chatgpt", cx))
            .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            settings.update(cx, |this, cx| {
                assert!(this.picker.is_none());
                assert_eq!(
                    this.values(cx).unwrap().shortcuts[0].applications,
                    vec!["kitty", "/Applications/ChatGPT.app"]
                );
            });
            window.scroll(
                "add-shortcut-app",
                ScrollDelta::Pixels(point(px(0.), px(-1000.))),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window
                .within(("shortcut-app", 0usize))
                .click(("remove-shortcut-app", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, _, cx| {
            settings.update(cx, |this, cx| {
                assert_eq!(this.shortcut_apps.len(), 1);
                assert_eq!(
                    this.values(cx).unwrap().shortcuts[0].applications,
                    vec!["/Applications/ChatGPT.app"]
                );
            });
        })
        .unwrap();
    }
}
