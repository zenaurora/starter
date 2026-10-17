use super::*;
use gpui_kit::base::Disableable;
use starter::monitor::{Action, ProcessUsage, Sort, format_bytes};

/// Hold the sampled target while the user chooses or confirms an action.
/// Live ranking must never change which processes a confirmation refers to.
pub(super) struct Actions {
    target: ProcessUsage,
    session: Arc<()>,
    position: Point<Pixels>,
    confirmation: Option<Action>,
    busy: bool,
    feedback: Option<String>,
}

impl Launcher {
    pub(super) fn receive_monitor_snapshot(&mut self, snapshot: monitor::Snapshot) {
        self.monitor_snapshot = Some(Arc::new(snapshot));
        self.sync_monitor_selection();
    }

    pub(super) fn sync_monitor_selection(&mut self) {
        let Some(snapshot) = &self.monitor_snapshot else {
            return;
        };
        let ranked = snapshot.ranked(&self.query.text, self.monitor_sort);
        if !ranked.iter().any(|&index| {
            self.monitor_selected.as_deref() == Some(snapshot.processes[index].id.as_str())
        }) {
            self.monitor_selected = ranked
                .first()
                .map(|&index| snapshot.processes[index].id.clone());
        }
    }

    fn selected_monitor_row(&self) -> Option<&ProcessUsage> {
        let id = self.monitor_selected.as_ref()?;
        self.monitor_snapshot
            .as_ref()?
            .processes
            .iter()
            .find(|row| &row.id == id)
    }

    pub(super) fn open_monitor_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.monitor_actions.is_some() {
            return;
        }
        let Some(target) = self.selected_monitor_row().cloned() else {
            return;
        };
        // Keyboard actions belong beside the selected row, even after scrolling.
        // Mouse actions instead use the position of the actual click.
        let scroll = self.monitor_scroll.0.borrow();
        let bounds = scroll.base_handle.bounds();
        let index = self
            .monitor_snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .ranked(&self.query.text, self.monitor_sort)
                    .iter()
                    .position(|&index| snapshot.processes[index].id == target.id)
            })
            .unwrap_or(0);
        let y = (bounds.top() + scroll.base_handle.offset().y + px(52.) * index).clamp(
            bounds.top(),
            bounds.bottom().max(bounds.top() + px(24.)) - px(24.),
        );
        let position = point(bounds.right() - px(60.), y + px(26.));
        drop(scroll);
        self.show_monitor_actions(target, position, window, cx);
    }

    fn show_monitor_actions(
        &mut self,
        target: ProcessUsage,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.monitor_selected = Some(target.id.clone());
        self.monitor_actions = Some(Actions {
            target,
            session: Arc::new(()),
            position,
            confirmation: None,
            busy: false,
            feedback: None,
        });
        window.focus(&self.dialog_focus, cx);
        cx.notify();
    }

    fn close_monitor_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .monitor_actions
            .as_ref()
            .is_some_and(|actions| actions.busy)
        {
            return;
        }
        self.monitor_actions = None;
        self.input.update(cx, |input, cx| input.focus(window, cx));
        self.scroll_monitor_selection();
        // A normal quit can activate the application's save dialog while our
        // action menu suppresses dismissal. Recheck now that the menu closed;
        // there may be no further activation event to hide Starter for us.
        self.handle_activation_change(window.is_window_active(), self.shell.is_some(), window, cx);
        cx.notify();
    }

    fn scroll_monitor_selection(&self) {
        if let Some(snapshot) = &self.monitor_snapshot {
            let ranked = snapshot.ranked(&self.query.text, self.monitor_sort);
            if let Some(index) = ranked.iter().position(|&index| {
                self.monitor_selected.as_deref() == Some(snapshot.processes[index].id.as_str())
            }) {
                self.monitor_scroll
                    .scroll_to_item(index, ScrollStrategy::Nearest);
            }
        }
    }

    pub(super) fn monitor_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "up" | "down" if !modifiers.alt && !modifiers.control && !modifiers.platform => {
                if let Some(snapshot) = &self.monitor_snapshot {
                    let ranked = snapshot.ranked(&self.query.text, self.monitor_sort);
                    let index = ranked
                        .iter()
                        .position(|&index| {
                            self.monitor_selected.as_deref()
                                == Some(snapshot.processes[index].id.as_str())
                        })
                        .unwrap_or(0);
                    let next = if event.keystroke.key == "down" {
                        (index + 1).min(ranked.len().saturating_sub(1))
                    } else {
                        index.saturating_sub(1)
                    };
                    self.monitor_selected = ranked
                        .get(next)
                        .map(|&index| snapshot.processes[index].id.clone());
                    self.monitor_scroll
                        .scroll_to_item(next, ScrollStrategy::Nearest);
                }
                cx.notify();
            }
            "enter" if !event.is_held => {
                if modifiers.platform || modifiers.control {
                    self.reveal_monitor_selected(window, cx);
                } else if modifiers.shift {
                    self.copy_monitor_selected(cx);
                } else {
                    self.open_monitor_actions(window, cx);
                }
            }
            "k" if (modifiers.platform || modifiers.control) && !event.is_held => {
                self.open_monitor_actions(window, cx);
            }
            _ => return false,
        }
        cx.stop_propagation();
        true
    }

    pub(super) fn monitor_action_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let actions = self.monitor_actions.as_ref().unwrap();
        let busy = actions.busy;
        let confirming = actions.confirmation.is_some();
        if !busy && !event.is_held {
            if event.keystroke.key == "escape" {
                if self
                    .monitor_actions
                    .as_mut()
                    .unwrap()
                    .confirmation
                    .take()
                    .is_none()
                {
                    self.close_monitor_actions(window, cx);
                } else {
                    window.focus(&self.dialog_focus, cx);
                }
                cx.notify();
            } else if event.keystroke.key == "enter"
                && (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
                && confirming
            {
                self.execute_monitor_action(window, cx);
            }
        }
        if !busy {
            let backwards = match event.keystroke.key.as_str() {
                "up" => Some(true),
                "down" => Some(false),
                "tab" => Some(event.keystroke.modifiers.shift),
                _ => None,
            };
            if let Some(backwards) = backwards {
                self.focus_monitor_action(backwards, window, cx);
                cx.stop_propagation();
                return;
            }
        }
        // Buttons handle Space and Enter. A signal confirmation requires an
        // explicit click, Space on its button, or Cmd/Ctrl+Enter.
        let confirming = self
            .monitor_actions
            .as_ref()
            .is_some_and(|actions| actions.confirmation.is_some());
        if busy
            || !(event.keystroke.key == "space"
                || event.keystroke.key == "enter" && !confirming && !event.is_held)
        {
            cx.stop_propagation();
        }
    }

    fn focus_monitor_action(&self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        if backwards {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        if self.dialog_focus.contains_focused(window, cx) {
            return;
        }
        // The list remains mounted behind the menu. Wrap focus within its
        // controls so Tab and arrows cannot activate a background row.
        window.focus(&self.dialog_focus, cx);
        window.focus_next(cx);
        if backwards {
            while let Some(last) = window.focused(cx) {
                window.focus_next(cx);
                if !self.dialog_focus.contains_focused(window, cx) {
                    window.focus(&last, cx);
                    break;
                }
                if last.is_focused(window) {
                    break;
                }
            }
        }
    }

    pub(super) fn reveal_monitor_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.selected_monitor_row().and_then(|row| row.path.clone());
        self.reveal_monitor_path(path, window, cx);
    }

    fn reveal_monitor_path(
        &mut self,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = path
            .filter(|path| path.exists())
            .ok_or_else(|| anyhow::anyhow!("无法读取应用或可执行文件路径，或文件已被移除"))
            .and_then(|path| platform::reveal(&path));
        match result {
            Ok(()) => {
                self.monitor_actions = None;
                self.hide(window, cx);
            }
            Err(error) => self.monitor_feedback(format!("定位失败：{error:#}")),
        }
        cx.notify();
    }

    pub(super) fn copy_monitor_selected(&mut self, cx: &mut Context<Self>) {
        let path = self.selected_monitor_row().and_then(|row| row.path.clone());
        self.copy_monitor_path(path, cx);
    }

    fn copy_monitor_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if let Some(path) = path {
            cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
            self.monitor_feedback("已复制路径".into());
        } else {
            self.monitor_feedback("系统未提供这项进程的路径".into());
        }
        cx.notify();
    }

    fn monitor_feedback(&mut self, feedback: String) {
        self.status = feedback.clone();
        if let Some(actions) = &mut self.monitor_actions {
            actions.feedback = Some(feedback);
        }
    }

    fn request_monitor_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(actions) = &mut self.monitor_actions
            && !actions.busy
        {
            actions.confirmation = Some(action);
            actions.feedback = None;
            // The clicked action button disappears on the confirmation view.
            // Keep focus on an element that survives the transition so Tab and
            // Escape continue to work, including after returning and retrying.
            window.focus(&self.dialog_focus, cx);
            // The application's own quit workflow handles saving. Signals that
            // can interrupt it still require Starter's explicit confirmation.
            if action == Action::Quit {
                self.execute_monitor_action(window, cx);
            }
            cx.notify();
        }
    }

    fn execute_monitor_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(actions) = &mut self.monitor_actions else {
            return;
        };
        if actions.busy {
            return;
        }
        let Some(action) = actions.confirmation else {
            return;
        };
        actions.busy = true;
        let target = actions.target.clone();
        let session = actions.session.clone();
        actions.feedback = Some(format!("正在{} {}…", action.label(), target.name));
        let operation = cx
            .background_executor()
            .spawn(async move { monitor::perform(action, &target) });
        self.track_task(cx.spawn_in(window, async move |this, cx| {
            let result = operation.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(actions) = &mut this.monitor_actions else {
                    return;
                };
                // A tray command can leave this menu while the OS request is
                // running. Its result must not change a subsequently opened one.
                if !Arc::ptr_eq(&actions.session, &session) {
                    return;
                }
                actions.busy = false;
                // Failed normal quit should return to the other choices,
                // rather than introduce a confirmation after the request.
                if action == Action::Quit {
                    actions.confirmation = None;
                }
                match result {
                    Ok(feedback) => {
                        this.status = feedback;
                        this.close_monitor_actions(window, cx);
                    }
                    Err(error) => {
                        this.monitor_feedback(format!("{}失败：{error:#}", action.label()));
                        window.focus(&this.dialog_focus, cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn monitor_action_menu(&self, cx: &Context<Self>) -> AnyElement {
        let actions = self.monitor_actions.as_ref().unwrap();
        let target = &actions.target;
        let theme = cx.theme();
        let mut content = div().flex().flex_col().gap_1();
        if let Some(action) = actions.confirmation {
            let shortcut = if cfg!(target_os = "macos") {
                "⌘ Enter"
            } else {
                "Ctrl+Enter"
            };
            content = content.p_2().gap_3()
                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{}这组进程？", action.label())))
                .child(div().text_size(px(12.)).line_height(relative(1.5))
                    .text_color(theme.muted_foreground).child(match action {
                        Action::Quit => "应用可以提示保存或取消退出。发送请求不代表已经退出。",
                        Action::Terminate => "发送 SIGTERM 请求结束进程。进程可以处理或忽略，未保存的内容可能丢失。",
                        Action::ForceQuit => "立即终止进程，不等待应用保存。未保存的内容会丢失。",
                    }))
                .child(div().flex().flex_col().gap_2()
                    .child(Button::new("monitor-confirm-action").w_full().small()
                        .label(if actions.busy { "正在发送…".into() } else {
                            format!("确认{} · {shortcut}", action.label())
                        })
                        .when(action == Action::ForceQuit, |button| button.danger())
                        .when(action != Action::ForceQuit, |button| button.primary())
                        .disabled(actions.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.execute_monitor_action(window, cx))))
                    .child(Button::new("monitor-cancel-action").w_full().ghost().small()
                        .label("返回 · Esc").disabled(actions.busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(actions) = &mut this.monitor_actions {
                                actions.confirmation = None; actions.feedback = None;
                            }
                            window.focus(&this.dialog_focus, cx); cx.notify();
                        }))));
        } else {
            let path = target.path.clone();
            let copy_path = path.clone();
            content = content
                .child(menu_section_label("辅助操作", theme.muted_foreground))
                .child(
                    Button::new("monitor-reveal")
                        .icon(IconName::FolderOpen)
                        .w_full()
                        .justify_start()
                        .ghost()
                        .small()
                        .label(if cfg!(target_os = "macos") {
                            "在 Finder 中显示"
                        } else {
                            "在文件管理器中显示"
                        })
                        .disabled(path.is_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reveal_monitor_path(path.clone(), window, cx)
                        })),
                )
                .child(
                    Button::new("monitor-copy-path")
                        .icon(IconName::Copy)
                        .w_full()
                        .justify_start()
                        .ghost()
                        .small()
                        .label("复制路径")
                        .disabled(copy_path.is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.copy_monitor_path(copy_path.clone(), cx)
                        })),
                )
                .child(div().h(px(1.)).my_1().bg(theme.border));
            content = content.child(menu_section_label("进程控制", theme.muted_foreground));
            let can_quit = cfg!(target_os = "windows")
                || target
                    .path
                    .as_ref()
                    .is_some_and(|p| p.extension().is_some_and(|e| e == "app"));
            for (action, label, detail) in [
                (Action::Quit, "退出应用", "正常退出，允许应用提示保存或取消"),
                (
                    Action::Terminate,
                    "终止进程…",
                    "发送 SIGTERM，可能丢失未保存内容",
                ),
                (Action::ForceQuit, "强制退出…", "立即终止，未保存内容会丢失"),
            ] {
                if action == Action::Terminate && cfg!(target_os = "windows") {
                    continue;
                }
                content = content.child(
                    Button::new(match action {
                        Action::Quit => "monitor-quit",
                        Action::Terminate => "monitor-terminate",
                        Action::ForceQuit => "monitor-force-quit",
                    })
                    .w_full()
                    .justify_start()
                    .ghost()
                    .small()
                    .icon(match action {
                        Action::Quit => IconName::LogOut,
                        Action::Terminate => IconName::CircleStop,
                        Action::ForceQuit => IconName::OctagonX,
                    })
                    .label(label)
                    .tooltip(detail)
                    .disabled(action == Action::Quit && !can_quit)
                    .when(action == Action::Terminate, |button| {
                        button.text_color(theme.warning)
                    })
                    .when(action == Action::ForceQuit, |button| {
                        button.text_color(theme.danger)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.request_monitor_action(action, window, cx)
                    })),
                );
            }
        }
        let menu = div()
            .id("monitor-actions-menu")
            .test_support()
            .w(px(300.))
            .p_1()
            .flex()
            .flex_col()
            .gap_1()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .shadow_md()
            .occlude()
            .track_focus(&self.dialog_focus)
            .tab_group()
            .tab_stop(false)
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.close_monitor_actions(window, cx);
                cx.stop_propagation();
            }))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::AppWindow)
                            .with_size(px(14.))
                            .text_color(theme.primary),
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
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_ellipsis()
                                    .child(target.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme.muted_foreground)
                                    .text_ellipsis()
                                    .child(format!(
                                        "{} 个进程 · {}",
                                        target.process_count,
                                        format_bytes(target.memory)
                                    )),
                            ),
                    )
                    .child(
                        Button::new("monitor-actions-close")
                            .icon(IconName::X)
                            .ghost()
                            .small()
                            .tooltip("关闭菜单 · Esc")
                            .disabled(actions.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_monitor_actions(window, cx)
                            })),
                    ),
            )
            .child(div().h(px(1.)).bg(theme.border))
            .child(content)
            .when_some(actions.feedback.clone(), |menu, feedback| {
                menu.child(
                    div()
                        .id("monitor-action-feedback")
                        .px_2()
                        .py_1()
                        .max_h(px(72.))
                        .overflow_y_scroll()
                        .text_size(px(11.))
                        .text_color(theme.warning)
                        .child(feedback),
                )
            });
        // Deferred painting leaves the virtual list visible. The anchor flips or
        // clamps the menu when its click position is near a window edge.
        deferred(anchored().position(actions.position).child(menu))
            .with_priority(1)
            .into_any_element()
    }

    pub(super) fn set_monitor_sampling(&mut self, enabled: bool) {
        if self.monitor_sampling != enabled {
            if let Some(worker) = &self.monitor_worker {
                let _ = worker.send(enabled);
            }
            self.monitor_sampling = enabled;
        }
    }

    pub(super) fn monitor_screen(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut overview = div().w_full().flex().gap_2();
        let mut rows = div()
            .flex_1()
            .text_center()
            .py_6()
            .text_color(theme.muted_foreground)
            .child("正在读取系统资源，约需一秒…")
            .into_any_element();
        let mut count = "正在采样…".to_string();
        if let Some(snapshot) = &self.monitor_snapshot {
            overview = overview
                .child(metric(
                    "CPU",
                    format!("{:.1}%", snapshot.cpu_percent),
                    snapshot.cpu_percent / 100.0,
                    theme.primary,
                    cx,
                ))
                .child(metric(
                    "内存",
                    format!(
                        "{} / {}",
                        format_bytes(snapshot.memory_used),
                        format_bytes(snapshot.memory_total)
                    ),
                    ratio(snapshot.memory_used, snapshot.memory_total),
                    theme.info,
                    cx,
                ))
                .child(metric(
                    "启动盘已用",
                    if snapshot.disk_total == 0 {
                        "暂不可用".into()
                    } else {
                        format!(
                            "{} / {}",
                            format_bytes(snapshot.disk_used),
                            format_bytes(snapshot.disk_total)
                        )
                    },
                    ratio(snapshot.disk_used, snapshot.disk_total),
                    theme.warning,
                    cx,
                ));
            let ranked = snapshot.ranked(&self.query.text, self.monitor_sort);
            count = format!("{} 项", ranked.len());
            if snapshot.sampled_at.elapsed() > Duration::from_secs(3) {
                count.push_str(" · 更新中…");
            }
            rows = if ranked.is_empty() {
                div()
                    .flex_1()
                    .py_6()
                    .text_center()
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .child("没有匹配的应用或进程")
                    .into_any_element()
            } else {
                let memory_total = snapshot.memory_total;
                // Keep this frame's immutable snapshot alive without copying
                // every process. The virtual list draws only visible indices.
                let snapshot = snapshot.clone();
                uniform_list(
                    "monitor-rows",
                    ranked.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| {
                                this.monitor_row(&snapshot, ranked[index], memory_total, cx)
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.monitor_scroll)
                .flex_1()
                .min_h_0()
                .w_full()
                .px_3()
                .into_any_element()
            };
        }
        div()
            .size_full()
            .relative()
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
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(Icon::new(IconName::Activity).with_size(px(14.)))
                            .child("系统监控"),
                    )
                    .child(
                        Button::new("monitor-close")
                            .label("Esc 返回")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.input.update(cx, |input, cx| {
                                    input.set_value("", window, cx);
                                    input.focus(window, cx);
                                });
                                this.search(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .px_5()
                    .py_2()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Input::new(&self.input)
                            .large()
                            .appearance(false)
                            .font_family(theme.mono_font_family.clone())
                            .prefix(
                                Icon::new(IconName::Search)
                                    .with_size(px(18.))
                                    .text_color(theme.primary),
                            ),
                    ),
            )
            .child(div().px_3().py_3().flex_shrink_0().child(overview))
            .child(
                div()
                    .px_6()
                    .h(px(36.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child(format!("应用与后台进程 · {count}")),
                    )
                    .children([(Sort::Cpu, "CPU", 76.), (Sort::Memory, "内存", 96.)].map(
                        |(sort, label, width)| {
                            Button::new(label)
                                .w(px(width))
                                .label(if self.monitor_sort == sort {
                                    format!("{label} ↓")
                                } else {
                                    label.into()
                                })
                                .ghost()
                                .small()
                                .text_color(if self.monitor_sort == sort {
                                    theme.primary
                                } else {
                                    theme.muted_foreground
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.monitor_sort = sort;
                                    this.scroll_monitor_selection();
                                    cx.notify();
                                }))
                        },
                    ))
                    .child(div().w(px(48.))),
            )
            .child(rows)
            .child(
                div()
                    .px_5()
                    .py_3()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_size(px(10.))
                    .text_color(theme.muted_foreground)
                    .child(if cfg!(target_os = "macos") {
                        "↑↓ 选择 · Enter / ⌘K 操作 · ⌘Enter 定位 · Shift Enter 复制路径"
                    } else {
                        "↑↓ 选择 · Enter / Ctrl+K 操作 · Ctrl+Enter 定位 · Shift Enter 复制路径"
                    })
                    .child(self.status.clone()),
            )
            .when(self.monitor_actions.is_some(), |screen| {
                screen.child(self.monitor_action_menu(cx))
            })
            .into_any_element()
    }
    fn monitor_row(
        &mut self,
        snapshot: &Arc<monitor::Snapshot>,
        index: usize,
        memory_total: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = &snapshot.processes[index];
        let theme = cx.theme();
        let icon = row.path.as_ref().and_then(|path| self.icons.get(path));
        let detail = if row
            .path
            .as_ref()
            .is_some_and(|p| p.extension().is_some_and(|e| e == "app"))
        {
            format!("{} 个进程", row.process_count)
        } else {
            format!("后台进程 · {} 个", row.process_count)
        };
        let usage = match self.monitor_sort {
            Sort::Cpu => row.cpu_percent / 100.0,
            Sort::Memory => ratio(row.memory, memory_total),
        }
        .clamp(0.0, 1.0);
        let selected = self.monitor_selected.as_deref() == Some(&row.id);
        let id = row.id.clone();
        let context_snapshot = snapshot.clone();
        let button_snapshot = snapshot.clone();
        div()
            .id(SharedString::from(row.id.clone()))
            .h(px(52.))
            .w_full()
            .flex_shrink_0()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .cursor_pointer()
            .when(selected, |row| row.bg(theme.accent))
            .hover(|row| row.bg(theme.list_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.monitor_selected = Some(id.clone());
                    this.input.update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.show_monitor_actions(
                        context_snapshot.processes[index].clone(),
                        event.position,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .size(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(icon.clone(), |d, icon| d.child(img(icon).size(px(26.))))
                    .when(icon.is_none(), |d| {
                        d.child(
                            Icon::new(IconName::AppWindow)
                                .with_size(px(22.))
                                .text_color(theme.muted_foreground),
                        )
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
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_ellipsis()
                            .child(row.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme.muted_foreground)
                                    .child(detail),
                            )
                            .child(
                                div()
                                    .w(px(90.))
                                    .h(px(3.))
                                    .bg(theme.border)
                                    .child(div().h_full().w(relative(usage)).bg(theme.primary)),
                            ),
                    ),
            )
            .child(
                div()
                    .w(px(76.))
                    .flex_shrink_0()
                    .text_right()
                    .text_size(px(12.))
                    .font_family(theme.mono_font_family.clone())
                    .child(format!("{:.1}%", row.cpu_percent)),
            )
            .child(
                div()
                    .w(px(96.))
                    .flex_shrink_0()
                    .text_right()
                    .text_size(px(12.))
                    .font_family(theme.mono_font_family.clone())
                    .child(format_bytes(row.memory)),
            )
            .child(
                Button::new(SharedString::from(format!("monitor-actions-{}", row.id)))
                    .icon(IconName::Ellipsis)
                    .ghost()
                    .small()
                    .accessibility_label(format!("打开 {} 的进程操作菜单", row.name))
                    .tooltip("退出、结束进程、定位与复制路径")
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.show_monitor_actions(
                            button_snapshot.processes[index].clone(),
                            event.position(),
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    })),
            )
            .into_any_element()
    }
}

fn metric(
    label: &str,
    value: String,
    usage: f32,
    color: Hsla,
    cx: &Context<Launcher>,
) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex_1()
        .min_w_0()
        .p_3()
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(px(11.))
                .text_color(theme.muted_foreground)
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .font_family(theme.mono_font_family.clone())
                .text_ellipsis()
                .child(value),
        )
        .child(
            div()
                .h(px(4.))
                .w_full()
                .bg(theme.border)
                .child(div().h_full().w(relative(usage.clamp(0.0, 1.0))).bg(color)),
        )
        .into_any_element()
}

fn menu_section_label(label: &str, color: Hsla) -> AnyElement {
    div()
        .px_2()
        .pt_1()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(label.to_owned())
        .into_any_element()
}

fn ratio(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}
