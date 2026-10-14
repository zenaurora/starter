use super::*;
use starter::monitor::{Sort, format_bytes};

impl Launcher {
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
                                this.monitor_row(
                                    &snapshot.processes[ranked[index]],
                                    memory_total,
                                    cx,
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0()
                .w_full()
                .px_3()
                .into_any_element()
            };
        }
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
                                    cx.notify();
                                }))
                        },
                    )),
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
                    .child("每秒刷新 · /monitor 后输入名称筛选 · 点击 CPU / 内存排序")
                    .child("CPU 按整机计 · 应用内存为驻留量，含共享页 · 磁盘为容量占用"),
            )
            .into_any_element()
    }
    fn monitor_row(
        &mut self,
        row: &monitor::ProcessUsage,
        memory_total: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                .text_size(px(13.))
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

fn ratio(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}
