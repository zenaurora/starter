use gpui_kit::{
    component::{
        ActiveTheme, Disableable, Sizable,
        button::{Button, ButtonVariants},
        select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
    },
    *,
};
use starter::hotkeys;

#[derive(Clone)]
struct KeyChoice {
    value: String,
    label: String,
}

impl SelectItem for KeyChoice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.label.clone().into()
    }
    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        let aliases = match self.value.as_str() {
            "Super" => "cmd command win windows super",
            "Alt" => "alt option",
            "Ctrl" => "ctrl control",
            _ => "",
        };
        self.label.to_lowercase().contains(&query)
            || self.value.to_lowercase() == query
            || aliases.contains(&query)
    }
    fn value(&self) -> &String {
        &self.value
    }
}

type KeySelect = SelectState<SearchableVec<KeyChoice>>;

/// Click-only combo builder. Configuration is changed only by its caller on save.
pub struct HotkeyEditor {
    label: &'static str,
    slots: Vec<Entity<KeySelect>>,
    subscriptions: Vec<Subscription>,
}

impl HotkeyEditor {
    pub fn new(
        label: &'static str,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editor = Self {
            label,
            slots: Vec::new(),
            subscriptions: Vec::new(),
        };
        editor.set_value(value, window, cx);
        editor
    }

    pub fn value(&self, cx: &App) -> anyhow::Result<String> {
        let parts = self
            .slots
            .iter()
            .filter_map(|slot| slot.read(cx).selected_value().cloned())
            .collect::<Vec<_>>();
        if parts.is_empty() {
            return Ok(String::new());
        }
        anyhow::ensure!(
            parts.len() == self.slots.len(),
            "请为每个格子选择按键，或移除多余的空格子"
        );
        hotkeys::compose(&parts)
    }

    pub fn set_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Preserve invalid manual config so saving cannot silently replace it.
        let parts = hotkeys::parts(value).unwrap_or_else(|_| {
            value
                .split('+')
                .map(|part| part.trim().to_string())
                .collect()
        });
        self.slots.clear();
        self.subscriptions.clear();
        let count = parts.len().max(2);
        for index in 0..count {
            self.add_slot(parts.get(index).cloned(), window, cx);
        }
        cx.notify();
    }

    fn add_slot(&mut self, value: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let mut choices = choices();
        if let Some(value) = &value
            && !choices.iter().any(|choice| &choice.value == value)
        {
            choices.push(KeyChoice {
                label: value.clone(),
                value: value.clone(),
            });
        }
        let slot = cx.new(|cx| {
            let mut state =
                SelectState::new(SearchableVec::new(choices), None, window, cx).searchable(true);
            if let Some(value) = &value {
                state.set_selected_value(value, window, cx);
            }
            state
        });
        self.subscriptions.push(cx.subscribe(
            &slot,
            |_, _, _: &SelectEvent<SearchableVec<KeyChoice>>, cx| cx.notify(),
        ));
        self.slots.push(slot);
    }
}

impl Render for HotkeyEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut row = div().flex().flex_wrap().items_center().gap_2();
        for (index, slot) in self.slots.iter().enumerate() {
            if index > 0 {
                row = row.child(div().text_color(cx.theme().muted_foreground).child("+"));
            }
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Select::new(slot)
                            .id(("key-slot", index))
                            .accessibility_label(format!("{} · 按键 {}", self.label, index + 1))
                            .placeholder("选择按键")
                            .search_placeholder("搜索按键，例如 Cmd、K、Space")
                            .menu_width(px(220.))
                            .menu_max_h(px(220.))
                            .w(px(108.))
                            .small(),
                    )
                    .child(
                        Button::new(("remove-slot", index))
                            .label("×")
                            .ghost()
                            .small()
                            .tooltip("移除这个按键格")
                            .disabled(self.slots.len() <= 2)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.slots.remove(index);
                                drop(this.subscriptions.remove(index));
                                cx.notify();
                            })),
                    ),
            );
        }
        row = row
            .child(
                Button::new("add-slot")
                    .label("＋")
                    .outline()
                    .small()
                    .tooltip("增加按键格")
                    .disabled(self.slots.len() >= 5)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_slot(None, window, cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("clear-keys")
                    .label("清空")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.set_value("", window, cx))),
            );
        let (preview, invalid) = match self.value(cx) {
            Ok(value) if value.is_empty() => ("尚未选择组合键".to_string(), false),
            Ok(value) => (
                format!(
                    "组合：{}",
                    value
                        .split('+')
                        .map(key_label)
                        .collect::<Vec<_>>()
                        .join(" + ")
                ),
                false,
            ),
            Err(error) => (error.to_string(), true),
        };
        div()
            .id(self.label)
            .flex()
            .flex_col()
            .gap_2()
            .child(self.label)
            .child(row)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(if invalid {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(preview),
            )
    }
}

fn key_label(value: &str) -> String {
    match value {
        "Super" => {
            if cfg!(target_os = "macos") {
                "Cmd ⌘"
            } else {
                "Win ⊞"
            }
        }
        "Ctrl" => "Ctrl",
        "Alt" => {
            if cfg!(target_os = "macos") {
                "Option ⌥"
            } else {
                "Alt"
            }
        }
        "Shift" => "Shift ⇧",
        "ArrowUp" => "↑ Up",
        "ArrowDown" => "↓ Down",
        "ArrowLeft" => "← Left",
        "ArrowRight" => "→ Right",
        "Equal" => "=",
        "Minus" => "−",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Backslash" => "\\",
        "Semicolon" => ";",
        "Quote" => "'",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Backquote" => "`",
        _ => value
            .strip_prefix("Key")
            .or_else(|| value.strip_prefix("Digit"))
            .unwrap_or(value),
    }
    .to_string()
}

fn choices() -> Vec<KeyChoice> {
    let mut values = ["Super", "Ctrl", "Alt", "Shift"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    values.extend(('A'..='Z').map(|key| format!("Key{key}")));
    values.extend((0..=9).map(|key| format!("Digit{key}")));
    values.extend(
        [
            "Space",
            "Enter",
            "Tab",
            "Escape",
            "Backspace",
            "Delete",
            "Insert",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Minus",
            "Equal",
            "BracketLeft",
            "BracketRight",
            "Backslash",
            "Semicolon",
            "Quote",
            "Comma",
            "Period",
            "Slash",
            "Backquote",
            "CapsLock",
            "NumLock",
            "PrintScreen",
            "ScrollLock",
            "Pause",
            "AudioVolumeUp",
            "AudioVolumeDown",
            "AudioVolumeMute",
            "MediaPlayPause",
            "MediaStop",
            "MediaTrackNext",
            "MediaTrackPrevious",
        ]
        .into_iter()
        .map(str::to_string),
    );
    values.extend((1..=24).map(|key| format!("F{key}")));
    values.extend((0..=9).map(|key| format!("Numpad{key}")));
    values
        .into_iter()
        .map(|value| KeyChoice {
            label: key_label(&value),
            value,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::{base::Root, test::TestWindowExt};

    fn fixture(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<HotkeyEditor>) {
        cx.update(gpui_kit::init);
        let (window, editor) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(600.), px(400.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| HotkeyEditor::new("测试快捷键", "", window, cx)),
            )
            .unwrap()
        });
        (window.downcast().unwrap(), editor)
    }

    #[gpui_kit::test]
    fn dropdown_selection_builds_combo_without_capturing_keyboard(cx: &mut TestAppContext) {
        let (handle, editor) = fixture(cx);
        cx.run_until_parked();
        for (slot, query) in [(0usize, "Ctrl"), (1usize, "K")] {
            cx.update_window(handle.into(), |_, window, cx| {
                window
                    .within("测试快捷键")
                    .within(("key-slot", slot))
                    .click("input", cx);
                window.input(query, cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(handle.into(), |_, window, cx| {
                window.press("enter", cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
        cx.update_window(handle.into(), |_, window, cx| {
            assert_eq!(editor.read(cx).value(cx).unwrap(), "Ctrl+KeyK");
            window.press("alt-m", cx);
            assert_eq!(
                editor.read(cx).value(cx).unwrap(),
                "Ctrl+KeyK",
                "pressing a combo does not overwrite selection"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn adding_and_removing_boxes_preserves_combo_and_clear_resets(cx: &mut TestAppContext) {
        let (handle, editor) = fixture(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| editor.set_value("Cmd+K", window, cx));
            window.render_frame(cx);
            window.click("add-slot", cx);
            assert!(
                editor.read(cx).value(cx).is_err(),
                "an unfinished extra box cannot be saved"
            );
            window.click(("remove-slot", 2usize), cx);
            assert_eq!(
                hotkeys::parse(&editor.read(cx).value(cx).unwrap()).unwrap(),
                hotkeys::parse("Cmd+K").unwrap()
            );
            for _ in 0..3 {
                window.click("add-slot", cx);
            }
            assert_eq!(editor.read(cx).slots.len(), 5);
            window.click("add-slot", cx);
            assert_eq!(
                editor.read(cx).slots.len(),
                5,
                "four modifiers and one main key are the OS limit"
            );
            window.click("clear-keys", cx);
            assert_eq!(editor.read(cx).slots.len(), 2);
            assert_eq!(editor.read(cx).value(cx).unwrap(), "");
        })
        .unwrap();
    }
}
