use super::*;
use gpui_kit::base::Disableable;

impl Launcher {
    pub(super) fn reminder_badge(&self) -> String {
        let due = self
            .reminder_entries
            .iter()
            .filter(|e| e.state == reminders::State::Due)
            .count();
        if due == 0 {
            "提醒".into()
        } else {
            format!("{due} 条到时提醒")
        }
    }
    pub(super) fn reminder_results(&mut self) {
        self.reminder_draft = None;
        let candidates: Vec<_> = self
            .reminder_entries
            .iter()
            .map(reminders::Entry::candidate)
            .collect();
        self.results = search::rank(&candidates, &self.query.text, &Default::default());
        if self.query.text.is_empty() {
            self.results.insert(
                0,
                Candidate {
                    id: "reminder:new".into(),
                    title: "新建或管理提醒".into(),
                    detail: "选择快捷时间，或直接输入 /remind 10m 开会".into(),
                    path: PathBuf::new(),
                    kind: Kind::ReminderDraft,
                    aliases: vec![],
                },
            );
            self.status = self.reminder_feedback.clone().unwrap_or_else(|| {
                format!(
                    "{} 条提醒 · 输入时间和事项即可快速创建",
                    self.reminder_entries.len()
                )
            });
        } else {
            match reminders::time::quick(&self.query.text, chrono::Local::now()) {
                Ok(draft) => {
                    self.results.insert(0, draft.candidate());
                    self.reminder_draft = Some(draft);
                    self.status = "确认时间后按 Enter 保存 · Esc 返回".into();
                }
                Err(error) => {
                    self.results.push(Candidate {
                        id: "reminder:new".into(),
                        title: "打开提醒面板".into(),
                        detail: error.to_string(),
                        path: PathBuf::new(),
                        kind: Kind::ReminderDraft,
                        aliases: vec![],
                    });
                    self.status = "例如 /remind 10m 开会，或 /remind 明天 15:00 开会".into();
                }
            }
        }
    }
    pub(super) fn open_reminders(
        &mut self,
        initial: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reminder_panel.is_some() || self.power_busy || self.uninstall_busy {
            return;
        }
        let panel = cx.new(|cx| {
            reminder_panel::ReminderPanel::new(
                self.reminder_entries.clone(),
                initial,
                self.notification_warning.clone(),
                window,
                cx,
            )
        });
        self.reminder_subscription = Some(cx.subscribe_in(
            &panel,
            window,
            |this, _, event, window, cx| match event {
                reminder_panel::Event::Close => {
                    this.reminder_panel = None;
                    this.reminder_subscription = None;
                    this.input.update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                }
                reminder_panel::Event::Command(command) => this.send_reminder(command.clone(), cx),
            },
        ));
        if let Some(feedback) = self.reminder_feedback.clone() {
            panel.update(cx, |panel, cx| panel.failed(feedback, cx));
        }
        self.reminder_panel = Some(panel);
        cx.notify();
    }
    pub(super) fn send_reminder(&mut self, command: reminders::Command, cx: &mut Context<Self>) {
        if self.reminder_pending {
            return;
        }
        if self.reminder_worker.try_send(command).is_err() {
            let error = "提醒服务未运行，请检查提醒记录并重启 Starter".to_string();
            self.reminder_feedback = Some(error.clone());
            self.status = error.clone();
            if let Some(panel) = &self.reminder_panel {
                panel.update(cx, |panel, cx| panel.failed(error, cx));
            }
        } else {
            self.reminder_pending = true;
            self.status = "正在保存提醒…".into();
        }
        cx.notify();
    }
    pub(super) fn reminder_event(
        &mut self,
        event: reminders::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            reminders::Event::Snapshot(entries) => {
                self.reminder_entries = entries.clone();
                if let Some(panel) = &self.reminder_panel {
                    panel.update(cx, |panel, cx| panel.set_entries(entries, cx));
                }
                if self.query.mode == Mode::Reminders {
                    let selected = self.results.get(self.selected).map(|c| c.id.clone());
                    self.reminder_results();
                    self.selected = selected
                        .and_then(|id| self.results.iter().position(|c| c.id == id))
                        .unwrap_or(0);
                }
            }
            reminders::Event::Applied => {
                self.reminder_pending = false;
                if let Some(panel) = &self.reminder_panel {
                    panel.update(cx, |panel, cx| panel.applied(cx));
                }
            }
            reminders::Event::Saved(draft) => {
                self.reminder_pending = false;
                self.reminder_feedback = Some(format!(
                    "已创建：{} · {}",
                    draft.title,
                    reminders::time::display(draft.due_at)
                ));
                if let Some(panel) = &self.reminder_panel {
                    panel.update(cx, |panel, cx| panel.saved(&draft, window, cx));
                } else if self.query.mode == Mode::Reminders {
                    self.input
                        .update(cx, |input, cx| input.set_value("/remind ", window, cx));
                    self.search(cx);
                }
                self.status = self.reminder_feedback.clone().unwrap();
            }
            reminders::Event::Failed(error) => {
                self.reminder_pending = false;
                self.reminder_feedback = Some(error.clone());
                self.status = error.clone();
                if let Some(panel) = &self.reminder_panel {
                    panel.update(cx, |panel, cx| panel.failed(error, cx));
                }
            }
            reminders::Event::NotificationWarning(warning) => {
                self.notification_warning = Some(warning.clone());
                if let Some(panel) = &self.reminder_panel {
                    panel.update(cx, |panel, cx| panel.notification_warning(warning, cx));
                }
            }
        }
        cx.notify();
    }
    pub(super) fn execute_power(
        &mut self,
        power: Power,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.power_busy {
            return;
        }
        self.power_busy = true;
        self.status = format!("正在{}…", power.label());
        let operation = cx
            .background_executor()
            .spawn(async move { power.execute() });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = operation.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.power_busy = false;
                this.power_confirmation = None;
                match result {
                    Ok(()) => this.hide(window, cx),
                    Err(error) => {
                        this.status = format!("{}失败：{error:#}", power.label());
                        this.input.update(cx, |input, cx| input.focus(window, cx));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(super) fn power_dialog(&self, cx: &Context<Self>) -> AnyElement {
        let power = self.power_confirmation.unwrap();
        let theme = cx.theme();
        let shortcut = if cfg!(target_os = "macos") {
            "⌘ Enter"
        } else {
            "Ctrl+Enter"
        };
        div()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .track_focus(&self.dialog_focus)
            .capture_key_down(cx.listener(Self::key_down))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(520.))
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        Icon::new(IconName::Command)
                            .with_size(px(28.))
                            .text_color(theme.primary),
                    )
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{}这台电脑？", power.label())),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(relative(1.6))
                            .child("请先保存正在编辑的内容。执行后，当前工作会被中断。"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child("按 Esc 返回；再次按普通 Enter 不会执行。"),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_3()
                            .child(
                                Button::new("cancel-power")
                                    .label("取消 · Esc")
                                    .disabled(self.power_busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.power_confirmation = None;
                                        this.input.update(cx, |input, cx| input.focus(window, cx));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("confirm-power")
                                    .label(if self.power_busy {
                                        "正在发送…".into()
                                    } else {
                                        format!("确认{} · {shortcut}", power.label())
                                    })
                                    .primary()
                                    .disabled(self.power_busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.execute_power(power, window, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
