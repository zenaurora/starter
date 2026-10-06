use crate::appearance;
use gpui_kit::assets::IconName;
use gpui_kit::{
    component::{
        ActiveTheme, Disableable, Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputState, Textarea, TextareaState},
        switch::Switch,
    },
    prelude::FluentBuilder,
    *,
};
use starter::config::{Config, ThemeName, expand_home};
use starter::updates::{self, Status as UpdateStatus};
use std::{path::PathBuf, str::FromStr};

pub enum Event {
    Save(Box<Config>),
    Preview(ThemeName),
    Close,
    OpenConfig,
    CheckUpdates,
    OpenRelease(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Appearance,
    Search,
    Aliases,
    Updates,
}

pub struct Settings {
    draft: Config,
    tab: Tab,
    focus: FocusHandle,
    launcher: Entity<InputState>,
    terminal_key: Entity<InputState>,
    terminal: Entity<InputState>,
    font: Entity<InputState>,
    roots: Entity<TextareaState>,
    alias_app: Entity<InputState>,
    alias_names: Entity<InputState>,
    pub error: Option<String>,
    pub update_status: UpdateStatus,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<Event> for Settings {}

impl Settings {
    pub fn show_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = Tab::Updates;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn new(config: &Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let launcher =
            cx.new(|cx| InputState::new(window, cx).default_value(config.launcher_hotkey.clone()));
        let terminal_key =
            cx.new(|cx| InputState::new(window, cx).default_value(config.terminal_hotkey.clone()));
        let terminal =
            cx.new(|cx| InputState::new(window, cx).default_value(config.terminal.clone()));
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
        launcher.update(cx, |input, cx| input.focus(window, cx));
        Self {
            draft: config.clone(),
            tab: Tab::General,
            focus: cx.focus_handle(),
            launcher,
            terminal_key,
            terminal,
            font,
            roots,
            alias_app,
            alias_names,
            error: None,
            update_status: UpdateStatus::Idle,
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
        config.launcher_hotkey = self.launcher.read(cx).value().trim().to_string();
        config.terminal_hotkey = self.terminal_key.read(cx).value().trim().to_string();
        let launcher = global_hotkey::hotkey::HotKey::from_str(&config.launcher_hotkey)?;
        let terminal = global_hotkey::hotkey::HotKey::from_str(&config.terminal_hotkey)?;
        anyhow::ensure!(launcher.id() != terminal.id(), "两个快捷键不能相同");
        config.terminal = self.terminal.read(cx).value().trim().to_string();
        anyhow::ensure!(!config.terminal.is_empty(), "请填写终端应用");
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
            &self.launcher,
            &self.terminal_key,
            &self.terminal,
            &self.font,
            &self.alias_app,
            &self.alias_names,
        ]
        .into_iter()
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
            cx.emit(Event::Close);
            cx.stop_propagation();
        } else if event.keystroke.key == "s"
            && (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
        {
            self.save(cx);
            cx.stop_propagation();
        }
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
        let theme = cx.theme();
        let body = div().w_full().flex().flex_col().gap_5();
        match self.tab {
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
                    .disabled(self.update_status == UpdateStatus::Checking)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::CheckUpdates))))
                .when_some(match &self.update_status {
                    UpdateStatus::Available { url, .. } => Some(url.clone()),
                    _ => None,
                }, |body, url| body.child(Button::new("open-release").label("查看并下载新版本")
                    .primary().on_click(cx.listener(move |_, _, _, cx| cx.emit(Event::OpenRelease(url.clone()))))))
                .child(div().text_size(px(11.)).text_color(theme.muted_foreground)
                    .child("通过 GitHub 检查正式版本，下载后使用安装包更新。"))
                .into_any_element(),
            Tab::General => body
                .child(Self::field("呼出快捷键", "格式：Alt+Space、Ctrl+Space、Super+Space", &self.launcher))
                .child(Self::field("终端直达快捷键", "在其他应用中也能直接打开终端", &self.terminal_key))
                .child(Self::field("终端应用", if cfg!(target_os = "macos") { "填写应用名称，例如 Terminal、kitty、Ghostty" } else { "填写可执行文件，例如 wt.exe，或完整路径" }, &self.terminal))
                .into_any_element(),
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
            (Tab::General, IconName::Keyboard, "快捷键与终端"),
            (Tab::Appearance, IconName::Palette, "外观"),
            (Tab::Search, IconName::FolderSearch, "搜索目录"),
            (Tab::Aliases, IconName::Tag, "应用别名"),
            (Tab::Updates, IconName::RefreshCw, "更新"),
        ];
        let titles = match self.tab {
            Tab::General => ("快捷键与终端", "把常用动作缩短到一次按键。"),
            Tab::Appearance => ("外观", "熟悉的编辑器配色，安静的桌面入口。"),
            Tab::Search => ("搜索目录", "限定搜索范围，保持轻量。"),
            Tab::Aliases => ("应用别名", "用你习惯的名字打开应用。"),
            Tab::Updates => ("更新", "检查 Starter 的新版本。"),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::key_down))
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
                                        this.tab = tab;
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
                                    .mb_4()
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
                            .child(
                                Button::new("cancel")
                                    .label("取消")
                                    .ghost()
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close))),
                            )
                            .child(
                                Button::new("save")
                                    .label("保存设置")
                                    .primary()
                                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                            ),
                    ),
            )
    }
}
