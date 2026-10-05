use gpui_kit::component::{
    ActiveTheme,
    button::{Button, ButtonVariants},
};
use gpui_kit::*;

pub enum Event {
    Close,
}

pub struct ErrorDialog {
    message: String,
}

impl EventEmitter<Event> for ErrorDialog {}

impl ErrorDialog {
    pub fn new(message: String, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { message }
    }
}

impl Render for ErrorDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000080))
            .child(
                div()
                    .w(px(480.))
                    .max_w_full()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.danger)
                            .child("配置加载失败"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.foreground)
                            .line_height(relative(1.5))
                            .child(self.message.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(
                                Button::new("close")
                                    .primary()
                                    .child("确定")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(Event::Close);
                                    })),
                            ),
                    ),
            )
    }
}
