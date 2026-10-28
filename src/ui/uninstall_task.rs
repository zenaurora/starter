use super::task_terminal::TerminalView;
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use starter::{
    config::ThemeName,
    task_terminal::{self, Session, Size},
    uninstall::{Target, mole},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

type PreparedSession = anyhow::Result<(
    Session,
    async_channel::Receiver<task_terminal::Event>,
    String,
)>;

pub(super) enum Event {
    Hide,
    Close,
    Finished,
    Trash,
}

#[derive(PartialEq)]
enum State {
    Starting,
    Running,
    Stopping,
    Finished,
    Failed,
}

pub(super) struct TaskPanel {
    terminal: Entity<TerminalView>,
    name: String,
    path: std::path::PathBuf,
    state: State,
    detail: String,
    message: String,
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
    _quit: Subscription,
}

impl EventEmitter<Event> for TaskPanel {}

impl TaskPanel {
    pub(super) fn new(
        target: Target,
        theme: ThemeName,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let prepare_cancelled = cancelled.clone();
        let prepare_target = target.clone();
        let prepare = cx.background_executor().spawn(async move {
            let prepared = mole::prepare(&prepare_target, &prepare_cancelled)?;
            anyhow::ensure!(!prepare_cancelled.load(Ordering::Relaxed), "任务已取消");
            let (session, events) = Session::start(
                prepared.request,
                Size {
                    rows: 18,
                    cols: 88,
                    ..Size::default()
                },
            )?;
            Ok::<_, anyhow::Error>((session, events, prepared.detail))
        });
        Self::from_preparation(
            target.name,
            target.path,
            theme,
            cancelled,
            prepare,
            window,
            cx,
        )
    }

    fn from_preparation(
        name: String,
        path: std::path::PathBuf,
        theme: ThemeName,
        cancelled: Arc<AtomicBool>,
        prepare: Task<PreparedSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let terminal = cx.new(|cx| TerminalView::new(theme, cx));
        terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
        let quit = cx.on_app_quit(|this, cx| {
            this.cancelled.store(true, Ordering::Relaxed);
            let finished = this.terminal.update(cx, |terminal, _| terminal.shutdown());
            let task = std::mem::replace(&mut this._task, Task::ready(()));
            async move {
                task.await;
                if let Some(finished) = finished {
                    let _ = finished.recv().await;
                }
            }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let (session, events, detail) = match prepare.await {
                Ok(result) => result,
                Err(error) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        if !this.cancelled.load(Ordering::Relaxed) {
                            this.message = format!("{error:#}");
                            this.state = State::Failed;
                        }
                        cx.notify();
                    });
                    return;
                }
            };
            let finished = session.completion();
            let started = this
                .update_in(cx, |this, _, cx| {
                    if this.cancelled.load(Ordering::Relaxed) {
                        return false;
                    }
                    this.terminal
                        .update(cx, |terminal, cx| terminal.attach(session, cx));
                    this.detail = detail;
                    this.state = State::Running;
                    this.message = "按 Mole 提示查看文件并确认；Ctrl+C 可中断".into();
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !started {
                let _ = finished.recv().await;
                return;
            }
            while let Ok(event) = events.recv().await {
                let mut batch = vec![event];
                while batch.len() < 16 {
                    let Ok(event) = events.try_recv() else {
                        break;
                    };
                    batch.push(event);
                }
                if this
                    .update_in(cx, |this, _, cx| {
                        for event in batch {
                            this.terminal
                                .update(cx, |terminal, cx| terminal.feed(&event, cx));
                            if let task_terminal::Event::Exit { code, stopped } = event {
                                this.state = State::Finished;
                                this.message = completion_message(&this.path, code, stopped);
                                cx.emit(Event::Finished);
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            detail: path.display().to_string(),
            name,
            path,
            terminal,
            state: State::Starting,
            message: "正在读取 Mole 应用清单…".into(),
            cancelled,
            _task: task,
            _quit: quit,
        }
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.terminal
            .update(cx, |terminal, cx| terminal.focus(window, cx));
    }

    pub(super) fn set_theme(&mut self, theme: ThemeName, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, cx| terminal.set_theme(theme, cx));
    }

    pub(super) fn can_close(&self) -> bool {
        matches!(self.state, State::Finished | State::Failed)
    }

    #[cfg(test)]
    pub(super) fn terminal(&self) -> Entity<TerminalView> {
        self.terminal.clone()
    }

    #[cfg(test)]
    pub(super) fn fixture(
        theme: ThemeName,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Self, async_channel::Sender<task_terminal::Event>) {
        let (session, _) = Session::start(
            task_terminal::Request {
                program: "/bin/sleep".into(),
                args: vec!["30".into()],
                env: vec![],
            },
            Size::default(),
        )
        .unwrap();
        let (sender, events) = async_channel::unbounded();
        (
            Self::from_preparation(
                "Fixture".into(),
                "/fixture.app".into(),
                theme,
                Arc::new(AtomicBool::new(false)),
                Task::ready(Ok((session, events, "App · 28MB".into()))),
                window,
                cx,
            ),
            sender,
        )
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.state == State::Starting {
            self.cancelled.store(true, Ordering::Relaxed);
            self.state = State::Failed;
            self.message = "已取消准备，未开始卸载".into();
        } else if self.state == State::Running {
            self.terminal.read(cx).stop();
            self.state = State::Stopping;
            self.message = "正在停止任务；已处理的文件不会自动恢复".into();
        }
        cx.notify();
    }
}

impl Drop for TaskPanel {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

fn completion_message(path: &std::path::Path, code: u32, stopped: bool) -> String {
    let status = if stopped {
        "任务已停止".into()
    } else {
        format!("Mole 已退出（{code}）")
    };
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            format!("{status} · 应用已移除；关联文件结果见输出")
        }
        Ok(_) => format!("{status} · 应用仍在原位置；详细结果见输出"),
        Err(_) => format!("{status} · 无法核验应用位置；详细结果见输出"),
    }
}

impl Render for TaskPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let state = match self.state {
            State::Starting => "准备中",
            State::Running => "运行中",
            State::Stopping => "停止中",
            State::Finished => "已结束",
            State::Failed => "未启动",
        };
        let close = self.can_close();
        let error = self.terminal.read(cx).error.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .px_5()
                    .pt_4()
                    .pb_2()
                    .flex()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(div().font_weight(FontWeight::BOLD).child("STARTER"))
                    .child("应用卸载 · Mole"),
            )
            .child(
                div()
                    .mx_5()
                    .px_3()
                    .py_2()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(14.))
                    .child(format!("/uninstall {}", self.name)),
            )
            .child(
                div()
                    .px_5()
                    .py_2()
                    .flex()
                    .gap_4()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child("全部")
                    .child("文件")
                    .child("内容")
                    .child("剪贴板")
                    .child(div().text_color(theme.primary).child("卸载"))
                    .child("系统")
                    .child("提醒"),
            )
            .child(
                div()
                    .px_5()
                    .pt_2()
                    .pb_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.primary)
                                    .child(state),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(self.detail.clone()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .mx_5()
                    .mb_2()
                    .overflow_hidden()
                    .when(self.state != State::Failed, |body| {
                        body.child(self.terminal.clone())
                    })
                    .when(self.state == State::Failed, |body| {
                        body.child(
                            div()
                                .py_4()
                                .text_size(px(13.))
                                .line_height(relative(1.6))
                                .child(self.message.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .px_5()
                    .pb_2()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(error.unwrap_or_else(|| {
                        if self.state == State::Failed {
                            "可返回，或使用原生卸载".into()
                        } else {
                            self.message.clone()
                        }
                    })),
            )
            .child(
                div()
                    .px_5()
                    .py_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("copy-task-output")
                            .small()
                            .ghost()
                            .label("复制输出")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.terminal.update(cx, |terminal, cx| terminal.copy(cx))
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .when(self.state == State::Failed, |body| {
                                body.child(
                                    Button::new("task-native-trash")
                                        .small()
                                        .label("仅移到废纸篓")
                                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Trash))),
                                )
                            })
                            .child(
                                Button::new("hide-uninstall-task")
                                    .small()
                                    .ghost()
                                    .label("收起任务")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Hide))),
                            )
                            .child(
                                Button::new("stop-uninstall-task")
                                    .debug_selector(|| "stop-uninstall-task".into())
                                    .small()
                                    .primary()
                                    .label(if close {
                                        "返回 · Esc"
                                    } else {
                                        "停止任务"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if close {
                                            cx.emit(Event::Close);
                                        } else {
                                            this.stop(cx);
                                        }
                                    })),
                            ),
                    ),
            )
    }
}
