#![allow(dead_code)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[path = "../src/appearance.rs"]
mod appearance;
#[path = "../src/icons.rs"]
mod icons;
#[path = "../src/platform.rs"]
mod platform;
#[path = "../src/settings"]
mod settings {
    #[path = "hotkey_editor.rs"]
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
            let font = cx
                .new(|cx| InputState::new(window, cx).default_value(config.monospace_font.clone()));
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
            let rule =
                cx.new(|cx| InputState::new(window, cx).placeholder("md / pdf / folder / *"));
            let rule_app =
                cx.new(|cx| InputState::new(window, cx).placeholder("选择用于打开的应用"));
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
                        Button::new("browse-app").label("浏览…").on_click(
                            cx.listener(|this, _, window, cx| this.browse_app(window, cx)),
                        ),
                    )
                    .child(Button::new("close-picker").label("收起").ghost().on_click(
                        cx.listener(|this, _, window, cx| {
                            this.picker = None;
                            window.focus(&this.focus, cx);
                            cx.notify();
                        }),
                    )),
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
                        .on_click(
                            cx.listener(|this, _, window, cx| this.add_shortcut_app(window, cx)),
                        ),
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
                                            .child(
                                                "例如一次打开 kitty 和 ChatGPT。点击「新建」开始。",
                                            ),
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
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel_shortcut(window, cx)
                                })),
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
                                    cx.listener(|this, _, window, cx| {
                                        this.add_shortcut(window, cx)
                                    }),
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
                                .child(
                                    "这些应用会同时打开。可选择已安装的应用，或填写名称／路径。",
                                ),
                        ),
                )
                .into_any_element()
        }

        fn field(
            label: &'static str,
            help: &'static str,
            input: &Entity<InputState>,
        ) -> AnyElement {
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
                .when_some(self.update_status.progress(), |body, progress| body.child(
                    div()
                        .h(px(6.))
                        .w_full()
                        .bg(theme.border)
                        .rounded_sm()
                        .child(div().h_full().w(relative(progress)).bg(theme.primary)),
                ))
                .when(matches!(self.update_status, UpdateStatus::Connecting), |body| {
                    body.child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child("如果长时间停留在连接阶段，请检查 GitHub 网络或代理；GUI 启动不会自动继承终端代理。"),
                    )
                })
                .child(Button::new("check-updates").label("立即检查")
                    .disabled(self.update_status.busy())
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::CheckUpdates))))
                .when(self.can_install && !self.update_status.busy(), |body| body.child(Button::new("install-update").label("下载并更新").primary()
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::InstallUpdate)))))
                .when(self.update_status.can_cancel(), |body| body.child(Button::new("cancel-update").label("取消更新")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::CancelUpdate)))))
                .child(div().text_size(px(11.)).text_color(theme.muted_foreground)
                    .child("流程：检查版本 → 连接服务器 → 下载 → 校验 SHA-256 → 准备安装 → 重启。配置和使用记录会保留。"))
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
                    |window, cx| {
                        cx.new(|cx| Settings::new(&Config::default(), Vec::new(), window, cx))
                    },
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
        fn adding_a_group_returns_to_overview_and_escape_cancels_later_edits(
            cx: &mut TestAppContext,
        ) {
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
                    this.shortcut_apps[0]
                        .update(cx, |input, cx| input.set_value("kitty", window, cx));
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
}
#[path = "../src/ui"]
mod ui {
    #[path = "error_dialog.rs"]
    mod error_dialog;
    #[path = "launcher_commands.rs"]
    mod launcher_commands;
    #[path = "monitor_panel.rs"]
    mod monitor_panel;
    #[path = "reminder_panel.rs"]
    mod reminder_panel;
    #[cfg(target_os = "macos")]
    #[path = "task_terminal.rs"]
    mod task_terminal;
    #[cfg(test)]
    #[path = "tests.rs"]
    mod tests;
    #[cfg(target_os = "macos")]
    #[path = "uninstall_task.rs"]
    mod uninstall_task;

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
        monitor, opening, reminders,
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
        time::Duration,
    };

    // 普通搜索界面失去窗口激活后，先等 150 ms 再复核是否隐藏。
    // 原生窗口激活、Space 切换等过程可能短暂报告未激活，立即隐藏容易造成闪烁。
    // timer 的等待会让出执行权，不会阻塞 UI 线程；实际复核时间还取决于调度。
    const ACTIVATION_LOSS_GRACE: Duration = Duration::from_millis(150);

    pub struct Launcher {
        input: Entity<InputState>,
        settings: Option<Entity<Settings>>,
        settings_subscription: Option<Subscription>,
        error_dialog: Option<Entity<ErrorDialog>>,
        error_dialog_subscription: Option<Subscription>,
        icons: Icons,
        config: Config,
        config_path: PathBuf,
        // 持有托盘、全局快捷键管理器等系统资源，不是终端或命令行 shell。
        // Some 表示系统外壳已建立；None 时 hide() 会直接返回，避免窗口隐藏后难以再次呼出。
        // Some 不保证每个快捷键都注册成功，具体绑定仍可能有单独的警告。
        shell: Option<Shell>,
        history: History,
        clipboard_entries: Vec<clipboard::Entry>,
        clipboard_root: PathBuf,
        clipboard_preview: Option<PathBuf>,
        clipboard_worker: async_channel::Sender<clipboard::Command>,
        clipboard_restore_pending: bool,
        clipboard_hide_after_restore: bool,
        clipboard_error: Option<String>,
        uninstall_targets: Vec<uninstall::Target>,
        uninstall_confirmation: Option<uninstall::Target>,
        uninstall_busy: bool,
        #[cfg(target_os = "macos")]
        uninstall_task: Option<Entity<uninstall_task::TaskPanel>>,
        #[cfg(target_os = "macos")]
        uninstall_task_subscription: Option<Subscription>,
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
        monitor_snapshot: Option<Arc<monitor::Snapshot>>,
        monitor_ranked: Arc<[usize]>,
        monitor_worker: Option<std::sync::mpsc::Sender<bool>>,
        monitor_sampling: bool,
        monitor_sort: monitor::Sort,
        monitor_selected: Option<String>,
        monitor_scroll: UniformListScrollHandle,
        monitor_actions: Option<monitor_panel::Actions>,
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
        // Starter 维护的“已展开”逻辑状态。show()/打开设置等入口设为 true，hide() 设为 false。
        // 隐藏只是收起窗口，Launcher 实体、托盘和后台服务仍可继续存在。
        // 它不查询系统，也不保证窗口此刻已显示、位于当前 Space 或拿到了键盘焦点。
        // 因此 visible == true 与 window.is_window_active() == false 可以同时成立。
        visible: bool,
        // 窗口显示/隐藏与激活处理的代号，用来使过期的延迟隐藏请求失效。
        // 任务保存创建时的代号，醒来后只有代号仍一致，才允许继续检查是否隐藏。
        // 递增不会立刻终止旧任务；旧任务仍会醒来，但比较不通过就不会操作窗口。
        activation_generation: u64,
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
                InputState::new(window, cx)
                    .placeholder("搜索应用、系统命令或设置；/remind 10m 开会")
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
            let (monitor_worker, monitor_events) = monitor::start();
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
                    while let Ok(snapshot) = monitor_events.recv().await {
                        if this
                            .update_in(cx, |this, _, cx| {
                                this.receive_monitor_snapshot(snapshot);
                                if this.visible && this.query.mode == Mode::Monitor {
                                    cx.notify();
                                }
                            })
                            .is_err()
                        {
                            eprintln!(
                                "Monitor worker: main window closed, stopping monitor handler"
                            );
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
                let message =
                    format!("上次更新失败：{problem}\n\n已尝试恢复原应用。请在设置中重试。");
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
                error_dialog_subscription = Some(cx.subscribe_in(
                    &dialog,
                    window,
                    |this, _, event, _window, cx| match event {
                        error_dialog::Event::Close => {
                            this.error_dialog = None;
                            this.error_dialog_subscription = None;
                            cx.notify();
                        }
                    },
                ));
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
                clipboard_preview: None,
                clipboard_worker,
                clipboard_restore_pending: false,
                clipboard_hide_after_restore: false,
                clipboard_error: None,
                uninstall_targets: Vec::new(),
                uninstall_confirmation: None,
                uninstall_busy: false,
                #[cfg(target_os = "macos")]
                uninstall_task: None,
                #[cfg(target_os = "macos")]
                uninstall_task_subscription: None,
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
                monitor_snapshot: None,
                monitor_ranked: Arc::from([]),
                monitor_worker: Some(monitor_worker),
                monitor_sampling: false,
                monitor_sort: monitor::Sort::default(),
                monitor_selected: None,
                monitor_scroll: UniformListScrollHandle::new(),
                monitor_actions: None,
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
                activation_generation: 0,
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
            this.track_task(cx.spawn_in(window, async move |this, cx| {
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
                // active 是本次通知到来时读取的窗口激活状态快照，不是 Launcher 字段。
                // is_window_active() 是读取函数，不负责设置激活状态；show() 中的平台调用
                // 会向系统请求显示/激活，GPUI 再通过窗口事件更新它所记录的状态。
                // 窗口激活与输入框焦点也是两层状态：窗口激活后，还需要 input.focus()。
                let active = window.is_window_active();
                // shell_ready 表示系统外壳是否已建立，是允许自动隐藏的一个前提。
                // 它不是“现在一定应该隐藏”：设置页、确认框等状态还会阻止自动隐藏。
                let shell_ready = this.shell.is_some();
                this.handle_activation_change(active, shell_ready, window, cx);
            })
        }

        fn track_task(&mut self, task: Task<()>) {
            // Retain cancellation on window close, but release completed operations
            // before tracking another. Repeated show/hide must not grow this forever.
            self._tasks.retain(|task| !task.is_ready());
            self._tasks.push(task);
        }

        fn handle_activation_change(
            &mut self,
            active: bool,
            shell_ready: bool,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            #[cfg(target_os = "macos")]
            if let Some(task) = &self.uninstall_task {
                // 内嵌卸载任务面板单独管理焦点，不走下面普通搜索界面的失活隐藏流程。
                if active && self.visible {
                    task.update(cx, |task, cx| task.focus(window, cx));
                }
                return;
            }
            // 这里组合两个不同来源的状态：active 描述窗口当前是否激活，visible 描述
            // Starter 是否处于展开状态。激活且展开、也没有其他面板占用交互时，
            // 才把键盘焦点交回搜索框，避免抢走设置页、确认框或提醒面板的输入。
            if active
                && self.visible
                && self.settings.is_none()
                && self.error_dialog.is_none()
                && self.uninstall_confirmation.is_none()
                && self.power_confirmation.is_none()
                && self.monitor_actions.is_none()
                && !self.power_busy
                && self.reminder_panel.is_none()
            {
                // 进入这个分支说明普通搜索界面重新激活，使之前安排的隐藏请求过期。
                self.activation_generation = self.activation_generation.wrapping_add(1);
                self.input.update(cx, |input, cx| input.focus(window, cx));
            } else if self.should_hide_on_activation_loss(active, shell_ready) {
                // 第一次检查只决定“是否安排延迟复核”，并不立即隐藏。
                // 例：失活 → 安排任务 A → 用户再次呼出 → show() 递增代号，
                // A 醒来时发现代号已变，就不能把刚呼出的窗口隐藏。
                self.activation_generation = self.activation_generation.wrapping_add(1);
                let generation = self.activation_generation;
                self.track_task(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(ACTIVATION_LOSS_GRACE).await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        // 等待期间用户可能重新呼出窗口、打开设置或恢复窗口激活。
                        // 所以必须同时检查：任务代号仍有效，以及现在仍满足隐藏条件。
                        // active 要重新读取，不能沿用 150 ms 前的快照；面板状态也从 this
                        // 重新读取。shell_ready 则仍是安排任务时捕获的 shell 是否存在。
                        // 即使代号没变，只要窗口已重新激活，下面的判断也会阻止隐藏。
                        if this.activation_generation == generation
                            && this.should_hide_on_activation_loss(
                                window.is_window_active(),
                                shell_ready,
                            )
                        {
                            this.hide(window, cx);
                        }
                    });
                }));
            }
            cx.notify();
        }

        fn should_hide_on_activation_loss(&self, active: bool, shell_ready: bool) -> bool {
            // 只判断，不修改状态、不操作原生窗口。安排任务前和任务醒来后各调用一次。
            // 自动隐藏要求：窗口未激活、Launcher 仍展开、系统外壳已建立，且没有
            // 设置页、错误框、卸载确认、电源确认/操作、提醒面板等需要保留的交互。
            // 内嵌卸载任务面板已在 handle_activation_change() 开头单独处理。
            !active
                && self.visible
                && shell_ready
                && self.settings.is_none()
                && self.error_dialog.is_none()
                && self.uninstall_confirmation.is_none()
                && self.power_confirmation.is_none()
                && self.monitor_actions.is_none()
                && !self.power_busy
                && self.reminder_panel.is_none()
        }

        fn check_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            if self.update_status.busy() {
                return;
            }
            self.set_update_status(UpdateStatus::Checking, cx);
            let check = cx.background_executor().spawn(async { updates::check() });
            self.track_task(cx.spawn_in(window, async move |this, cx| {
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
            self.set_update_status(UpdateStatus::Connecting, cx);
            let job = cx.background_executor().spawn(async move {
                updates::prepare_and_launch(&version, &assets, &cancel, |status| {
                    let _ = sender.try_send(status);
                })
            });
            self.track_task(cx.spawn_in(window, async move |this, cx| {
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
            if self.query.mode != Mode::Monitor {
                self.monitor_actions = None;
            }
            self.set_monitor_sampling(
                self.visible && self.settings.is_none() && self.query.mode == Mode::Monitor,
            );
            self.selected = 0;
            self.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
            if self.query.mode == Mode::Apps {
                self.searching = false;
                let commands = if self.query.text.is_empty() {
                    &[][..]
                } else {
                    system_commands::candidates()
                };
                self.results = search::rank_apps(
                    self.candidates.iter().chain(commands),
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
                    system_commands::candidates(),
                    &self.query.text,
                    &Default::default(),
                );
                self.status = "输入英文或中文查找 · 重启和关机需确认".into();
            } else if self.query.mode == Mode::Reminders {
                self.searching = false;
                self.reminder_results();
            } else if self.query.mode == Mode::Monitor {
                self.searching = false;
                self.results.clear();
                self.status = "每秒更新 · CPU 按整机计 · 应用内存为驻留量，含共享页".into();
                self.sync_monitor_selection();
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

        fn has_uninstall_task(&self) -> bool {
            #[cfg(target_os = "macos")]
            {
                self.uninstall_task.is_some()
            }
            #[cfg(not(target_os = "macos"))]
            {
                false
            }
        }

        #[cfg(target_os = "macos")]
        fn close_uninstall_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            self.uninstall_task = None;
            self.uninstall_task_subscription = None;
            self.input.update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
        }

        #[cfg(target_os = "macos")]
        fn start_mole_uninstall(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            if self.uninstall_busy || self.has_uninstall_task() {
                return;
            }
            let Some(target) = self.uninstall_confirmation.take() else {
                return;
            };
            let native_target = target.clone();
            let panel =
                cx.new(|cx| uninstall_task::TaskPanel::new(target, self.config.theme, window, cx));
            self.uninstall_task_subscription = Some(cx.subscribe_in(
                &panel,
                window,
                move |this, _, event, window, cx| match event {
                    uninstall_task::Event::Hide => this.hide(window, cx),
                    uninstall_task::Event::Close => this.close_uninstall_task(window, cx),
                    uninstall_task::Event::Finished => {
                        this.refresh_apps();
                        this.search(cx);
                    }
                    uninstall_task::Event::Trash => {
                        this.close_uninstall_task(window, cx);
                        this.uninstall_confirmation = Some(native_target.clone());
                        this.confirm_uninstall(window, cx);
                    }
                },
            ));
            self.uninstall_task = Some(panel);
            cx.notify();
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
            self.track_task(cx.spawn_in(window, async move |this, cx| {
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
                            .child(if cfg!(target_os = "macos") {
                                "使用 Mole 查看并清理关联文件，确认和执行过程会显示在 Starter 内。仅移到废纸篓会保留配置和个人数据。".to_string()
                            } else { target.description() }),
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
                                        .when(!cfg!(target_os = "macos"), |button| button.primary())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.confirm_uninstall(window, cx)
                                        })),
                                )
                                .map(|buttons| {
                                    #[cfg(target_os = "macos")]
                                    { buttons.child(Button::new("mole-uninstall").label("卸载并清理关联文件").primary()
                                        .on_click(cx.listener(|this, _, window, cx| this.start_mole_uninstall(window, cx)))) }
                                    #[cfg(not(target_os = "macos"))]
                                    { buttons }
                                }),
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
                    self.track_task(cx.spawn_in(window, async move |this, cx| {
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
                            #[cfg(target_os = "macos")]
                            if let Some(task) = &self.uninstall_task {
                                task.update(cx, |task, cx| task.set_theme(self.config.theme, cx));
                            }
                            let _ = self.clipboard_worker.try_send(clipboard::Command::Enabled(
                                self.config.clipboard_history,
                            ));
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
                    if !self.has_uninstall_task() {
                        self.input
                            .update(cx, |input, cx| input.set_value("/system ", window, cx));
                        self.search(cx);
                    }
                }
                ShellEvent::Refresh => self.refresh_all(cx),
                ShellEvent::Quit => cx.quit(),
            }
            cx.notify();
        }

        fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            #[cfg(target_os = "macos")]
            if let Some(task) = &self.uninstall_task {
                self.visible = true;
                self.activation_generation = self.activation_generation.wrapping_add(1);
                platform::show(window, cx);
                task.update(cx, |task, cx| task.focus(window, cx));
                cx.defer_in(window, |this, window, cx| {
                    if this.visible
                        && let Some(task) = &this.uninstall_task
                    {
                        task.update(cx, |task, cx| task.focus(window, cx));
                    }
                });
                cx.notify();
                return;
            }
            if self.uninstall_busy || self.power_busy || self.monitor_actions.is_some() {
                self.visible = true;
                self.activation_generation = self.activation_generation.wrapping_add(1);
                self.set_monitor_sampling(
                    self.settings.is_none() && self.query.mode == Mode::Monitor,
                );
                platform::show(window, cx);
                if self.monitor_actions.is_some() {
                    window.focus(&self.dialog_focus, cx);
                }
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
            self.activation_generation = self.activation_generation.wrapping_add(1);
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
                    && this.monitor_actions.is_none()
                    && !this.has_uninstall_task()
                {
                    this.input.update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                }
            });
        }

        fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            // 与 shell_ready 来自同一前提，在真正隐藏的入口再保护一次。
            if self.shell.is_none() {
                return;
            }
            if self.settings.is_some() {
                self.close_settings(window, cx);
            }
            self.visible = false;
            self.set_monitor_sampling(false);
            self.activation_generation = self.activation_generation.wrapping_add(1);
            self.cancelled.store(true, Ordering::Relaxed);
            platform::hide(window, cx);
        }

        fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            if self.has_uninstall_task()
                || self.uninstall_confirmation.is_some()
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
            if self.query.mode == Mode::Monitor {
                self.open_monitor_actions(window, cx);
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
                    BuiltinAction::Monitor => self.open_monitor(window, cx),
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
            if self.query.mode == Mode::Monitor {
                if self.monitor_actions.is_none() {
                    self.reveal_monitor_selected(window, cx);
                }
                return;
            }
            if self.has_uninstall_task()
                || self.uninstall_confirmation.is_some()
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
            if self.query.mode == Mode::Monitor {
                if self.monitor_actions.is_none() {
                    self.copy_monitor_selected(cx);
                }
                return;
            }
            if self.has_uninstall_task()
                || self.uninstall_confirmation.is_some()
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
            #[cfg(target_os = "macos")]
            if let Some(task) = &self.uninstall_task {
                if event.keystroke.key == "escape" && task.read(cx).can_close() {
                    self.close_uninstall_task(window, cx);
                    cx.stop_propagation();
                }
                return;
            }
            if self.settings.is_some() || self.reminder_panel.is_some() {
                return;
            }
            if self.monitor_actions.is_some() {
                self.monitor_action_key_down(event, window, cx);
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
            if self.query.mode == Mode::Monitor && self.monitor_key_down(event, window, cx) {
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
            if self.has_uninstall_task() {
                self.show(window, cx);
                return;
            }
            self.visible = true;
            self.set_monitor_sampling(false);
            self.activation_generation = self.activation_generation.wrapping_add(1);
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

        fn open_monitor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            self.visible = true;
            self.activation_generation = self.activation_generation.wrapping_add(1);
            platform::show(window, cx);
            self.input.update(cx, |input, cx| {
                input.set_value("/monitor ", window, cx);
                input.focus(window, cx);
            });
            self.search(cx);
        }

        fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            self.settings = None;
            self.settings_subscription = None;
            self.set_monitor_sampling(self.visible && self.query.mode == Mode::Monitor);
            appearance::apply(self.config.theme, &self.config, window, cx);
            if self.monitor_actions.is_some() {
                window.focus(&self.dialog_focus, cx);
            } else {
                self.input.update(cx, |input, cx| input.focus(window, cx));
            }
            cx.notify();
        }

        fn save_settings(&mut self, config: Config, window: &mut Window, cx: &mut Context<Self>) {
            let result = self.apply_config(&config, true);
            match result {
                Ok(()) => {
                    let enable_updates =
                        config.auto_check_updates && !self.config.auto_check_updates;
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
            let result = starter::startup::apply(
                config.launch_at_login,
                self.config.launch_at_login,
                || {
                    if persist {
                        config::save(&self.config_path, config)
                    } else {
                        Ok(())
                    }
                },
            );
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
                Kind::Monitor => IconName::Activity,
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
                                            .try_send(clipboard::Command::Remove(
                                                clipboard_id.clone(),
                                            ))
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
            #[cfg(target_os = "macos")]
            if let Some(task) = &self.uninstall_task {
                return div()
                    .size_full()
                    .capture_key_down(cx.listener(Self::key_down))
                    .child(task.clone())
                    .into_any_element();
            }
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
            if self.query.mode == Mode::Monitor {
                return self.monitor_screen(cx);
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
                (Mode::Monitor, IconName::Activity, "监控", "/monitor "),
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
                                            this.input
                                                .update(cx, |input, cx| input.focus(window, cx));
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
                            div().flex_shrink_0().ml_3().child(
                                if self.query.mode == Mode::Clipboard {
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
                                },
                            ),
                        ),
                )
                .into_any_element()
        }
    }

    impl Launcher {
        pub fn monitor_preview(window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            let mut launcher = Launcher {
                input,
                settings: None,
                settings_subscription: None,
                error_dialog: None,
                error_dialog_subscription: None,
                icons: Icons::new().0,
                config,
                config_path: std::env::temp_dir().join("starter-ui-test-config.toml"),
                shell: None,
                history: History::empty(std::env::temp_dir().join("starter-ui-test-history.json")),
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
            };
            launcher.receive_monitor_snapshot(monitor_snapshot());
            launcher.open_monitor(window, cx);
            let input = launcher.input.clone();
            launcher._subscriptions.push(cx.subscribe_in(
                &input,
                window,
                |this, _, event, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.search(cx);
                    }
                },
            ));
            launcher
        }
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
}
#[path = "../src/worker.rs"]
mod worker;

use gpui_kit::*;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        AppWindow,
        Activity,
        FolderSearch,
        FolderPlus,
        Keyboard,
        Tag,
        Command,
        CornerDownLeft,
        StarFill,
        Clock
    ]
);

struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        Ok(paths)
    }
}

fn main() {
    if starter::updates::run_helper() {
        return;
    }
    let background_launch = std::env::args_os().any(|arg| arg == starter::startup::BACKGROUND_ARG);
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            // 配置启动器窗口：固定尺寸、无标题栏的悬浮弹出窗口
            let options = WindowOptions {
                // 窗口大小 720x560，并在屏幕上居中显示
                window_bounds: Some(WindowBounds::centered(size(px(720.), px(560.)), cx)),
                // 不使用系统标题栏（采用自定义标题栏）
                titlebar: None,
                // Create a nonactivating NSPanel on macOS. The platform layer
                // lowers its level so it does not cover IME candidate windows.
                kind: WindowKind::PopUp,
                // 禁止用户调整窗口大小
                is_resizable: false,
                // 禁止最小化窗口
                is_minimizable: false,
                // 应用标识符，用于平台层关联窗口与任务栏
                app_id: Some("dev.starter.monitor-preview".into()),
                focus: !background_launch,
                show: !background_launch,
                // 其余字段使用默认值
                ..Default::default()
            };
            match gpui_kit::open_window(options, cx, |window, cx| {
                platform::configure_launcher_window(window);
                cx.new(|cx| {
                    let mut launcher = ui::Launcher::monitor_preview(window, cx);
                    if background_launch {
                        launcher.start_background(window, cx);
                    }
                    launcher
                })
            }) {
                Ok(_) => {
                    cx.set_activation_policy(ActivationPolicy::Accessory);
                    if !background_launch {
                        cx.activate(true);
                    }
                }
                Err(error) => {
                    eprintln!("Cannot open Starter: {error:#}");
                    cx.quit();
                }
            }
        });
}
