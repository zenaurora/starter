use chrono::Local;
use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable;
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use starter::reminders::{self, Command, Draft, Entry, State};

pub enum Event {
    Close,
    Command(Command),
}
pub struct ReminderPanel {
    title: Entity<InputState>,
    time: Entity<InputState>,
    entries: Vec<Entry>,
    pending: bool,
    message: Option<String>,
    notification_warning: Option<String>,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<Event> for ReminderPanel {}
impl ReminderPanel {
    pub fn new(
        entries: Vec<Entry>,
        initial: &str,
        warning: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title =
            cx.new(|cx| InputState::new(window, cx).placeholder("提醒我做什么？例如：产品会议"));
        let time = cx.new(|cx| {
            InputState::new(window, cx).placeholder("10m / 明天 15:00 / 2026-10-20 9:00")
        });
        title.update(cx, |input, cx| input.set_value(initial, window, cx));
        time.update(cx, |input, cx| input.set_value("15m", window, cx));
        if let Ok(draft) = reminders::time::quick(initial, Local::now()) {
            title.update(cx, |input, cx| input.set_value(draft.title, window, cx));
            let value = chrono::DateTime::from_timestamp(draft.due_at, 0)
                .unwrap()
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string();
            time.update(cx, |input, cx| input.set_value(value, window, cx));
        } else if reminders::time::resolve(initial, Local::now()).is_ok() {
            title.update(cx, |input, cx| input.set_value("", window, cx));
            time.update(cx, |input, cx| input.set_value(initial, window, cx));
        }
        let subscriptions = [&title, &time]
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, event, _, cx| match event {
                    InputEvent::Change => {
                        this.message = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.create(cx),
                    _ => {}
                })
            })
            .into_iter()
            .collect();
        title.update(cx, |input, cx| input.focus(window, cx));
        Self {
            title,
            time,
            entries,
            pending: false,
            message: None,
            notification_warning: warning,
            _subscriptions: subscriptions,
        }
    }
    fn draft(&self, cx: &App) -> anyhow::Result<Draft> {
        Draft::new(
            &self.title.read(cx).value(),
            reminders::time::resolve(&self.time.read(cx).value(), Local::now())?,
        )
    }
    fn create(&mut self, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        match self.draft(cx) {
            Ok(draft) => self.send(Command::Create(draft), cx),
            Err(error) => {
                self.message = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        self.pending = true;
        cx.emit(Event::Command(command));
        cx.notify();
    }
    pub fn set_entries(&mut self, entries: Vec<Entry>, cx: &mut Context<Self>) {
        self.entries = entries;
        cx.notify();
    }
    pub fn applied(&mut self, cx: &mut Context<Self>) {
        self.pending = false;
        cx.notify();
    }
    pub fn saved(&mut self, draft: &Draft, window: &mut Window, cx: &mut Context<Self>) {
        self.pending = false;
        self.message = Some(format!(
            "已创建：{} · {}",
            draft.title,
            reminders::time::display(draft.due_at)
        ));
        self.title.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }
    pub fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.pending = false;
        self.message = Some(error);
        cx.notify();
    }
    pub fn notification_warning(&mut self, warning: String, cx: &mut Context<Self>) {
        self.notification_warning = Some(warning);
        cx.notify();
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if [self.title.clone(), self.time.clone()].iter().any(|input| {
            input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            })
        }) {
            return;
        }
        if event.keystroke.key == "escape" {
            cx.emit(Event::Close);
            cx.stop_propagation();
        } else if event.keystroke.key == "enter"
            && (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
        {
            self.create(cx);
            cx.stop_propagation();
        }
    }
}
impl Render for ReminderPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let resolved = reminders::time::resolve(&self.time.read(cx).value(), Local::now());
        let valid = self.draft(cx).is_ok();
        let preview = match resolved {
            Ok(time) => format!("将在 {} 提醒", reminders::time::display(time)),
            Err(error) => error.to_string(),
        };
        let mut list = div()
            .id("reminder-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px_5();
        if self.entries.is_empty() {
            list = list.child(
                div()
                    .py_4()
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .child("还没有提醒。先写事项，再选择时间。"),
            );
        }
        for entry in &self.entries {
            let due = entry.state == State::Due;
            let id = entry.id.clone();
            let remove_id = id.clone();
            list = list.child(
                div()
                    .py_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        Icon::new(IconName::Clock)
                            .with_size(px(18.))
                            .text_color(if due {
                                theme.primary
                            } else {
                                theme.muted_foreground
                            }),
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
                                    .text_size(px(14.))
                                    .text_ellipsis()
                                    .child(entry.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(if due {
                                        theme.primary
                                    } else {
                                        theme.muted_foreground
                                    })
                                    .child(format!(
                                        "{}{}",
                                        if due { "已到时间 · " } else { "" },
                                        reminders::time::display(entry.due_at)
                                    )),
                            ),
                    )
                    .when(due, |row| {
                        row.child(
                            Button::new(format!("snooze-{id}"))
                                .label("5 分钟后")
                                .small()
                                .disabled(self.pending)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.send(Command::Snooze(id.clone()), cx)
                                })),
                        )
                    })
                    .child(
                        Button::new(format!("remove-{remove_id}"))
                            .label(if due { "完成" } else { "取消" })
                            .ghost()
                            .small()
                            .disabled(self.pending)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(
                                    if due {
                                        Command::Done(remove_id.clone())
                                    } else {
                                        Command::Cancel(remove_id.clone())
                                    },
                                    cx,
                                )
                            })),
                    ),
            );
        }
        div().size_full().flex().flex_col().bg(theme.background).text_color(theme.foreground).capture_key_down(cx.listener(Self::key_down))
            .child(div().h(px(52.)).flex_shrink_0().px_5().flex().items_center().justify_between()
                .child(div().flex().items_center().gap_2().child(Icon::new(IconName::Clock).with_size(px(20.)).text_color(theme.primary)).child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("快速提醒")))
                .child(Button::new("back-reminders").label("返回 · Esc").ghost().small().on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close)))))
            .child(div().px_5().pb_4().flex_shrink_0().flex().flex_col().gap_2()
                .child(Input::new(&self.title).large().aria_label("提醒事项"))
                .child(div().flex().items_center().gap_2().child(div().flex_1().child(Input::new(&self.time).aria_label("提醒时间")))
                    .child(Button::new("create-reminder").label(if self.pending { "正在保存…" } else { "创建 · Enter" }).primary().disabled(!valid || self.pending).on_click(cx.listener(|this, _, _, cx| this.create(cx)))))
                .child(div().flex().gap_1().children([("5 分钟", "5m"), ("15 分钟", "15m"), ("30 分钟", "30m"), ("1 小时", "1h"), ("明早 9 点", "明天 9:00")].map(|(label, value)| Button::new(label).label(label).ghost().small().on_click(cx.listener(move |this, _, window, cx| { this.time.update(cx, |input, cx| input.set_value(value, window, cx)); this.title.update(cx, |input, cx| input.focus(window, cx)); cx.notify(); })))))
                .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(preview))
                .when_some(self.message.clone(), |body, message| body.child(div().text_size(px(12.)).child(message))))
            .child(div().px_5().py_2().border_t_1().border_color(theme.border).text_size(px(12.)).text_color(theme.muted_foreground).child(format!("待处理提醒（{}）", self.entries.len())))
            .child(list)
            .child(div().px_5().py_3().border_t_1().border_color(theme.border).text_size(px(11.)).text_color(theme.muted_foreground)
                .child(self.notification_warning.clone().unwrap_or_else(|| "Starter 在后台运行时通知；睡眠或退出期间错过的提醒，会在恢复运行后显示。".into())))
    }
}
