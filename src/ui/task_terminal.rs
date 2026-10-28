use crate::appearance::{self, Palette};
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use starter::{
    config::ThemeName,
    task_terminal::{Event, Session, Size},
};
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

const FONT_SIZE: f32 = 13.;
const LINE_HEIGHT: f32 = 20.;
type CellPosition = (u16, u16);

#[derive(Default)]
struct Responses(Vec<Vec<u8>>);
impl vt100::Callbacks for Responses {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        if i1.is_some() || i2.is_some() {
            return;
        }
        match (
            command,
            params
                .first()
                .and_then(|value| value.first())
                .copied()
                .unwrap_or(0),
        ) {
            ('n', 5) => self.0.push(b"\x1b[0n".to_vec()),
            ('n', 6) => {
                let (row, col) = screen.cursor_position();
                self.0
                    .push(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes());
            }
            ('c', 0) => self.0.push(b"\x1b[?1;2c".to_vec()),
            _ => {}
        }
    }
}

pub(super) struct TerminalView {
    parser: vt100::Parser<Responses>,
    session: Option<Session>,
    focus: FocusHandle,
    bounds: Option<Bounds<Pixels>>,
    cell_width: f32,
    selection: Option<(CellPosition, CellPosition)>,
    selecting: bool,
    scroll_delta: f32,
    composing: String,
    composing_selection: Range<usize>,
    echo: bool,
    pub(super) error: Option<String>,
    theme: ThemeName,
}

impl TerminalView {
    pub(super) fn new(theme: ThemeName, cx: &mut Context<Self>) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(18, 88, 2000, Responses::default()),
            session: None,
            focus: cx.focus_handle(),
            bounds: None,
            cell_width: 8.,
            selection: None,
            selecting: false,
            scroll_delta: 0.,
            composing: String::new(),
            composing_selection: 0..0,
            echo: true,
            error: None,
            theme,
        }
    }

    pub(super) fn attach(&mut self, session: Session, cx: &mut Context<Self>) {
        let (rows, cols) = self.parser.screen().size();
        let _ = session.resize(Size {
            rows,
            cols,
            ..Size::default()
        });
        self.session = Some(session);
        cx.notify();
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    pub(super) fn set_theme(&mut self, theme: ThemeName, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    pub(super) fn feed(&mut self, event: &Event, cx: &mut Context<Self>) {
        match event {
            Event::Output(bytes) => {
                self.parser.process(bytes);
                for response in std::mem::take(&mut self.parser.callbacks_mut().0) {
                    self.write(response);
                }
            }
            Event::Echo(echo) => self.echo = *echo,
            Event::Error(error) => self.error = Some(error.clone()),
            Event::Exit { .. } => {
                self.session = None;
                self.composing.clear();
                self.composing_selection = 0..0;
            }
        }
        cx.notify();
    }

    pub(super) fn stop(&self) {
        if let Some(session) = &self.session {
            session.interrupt();
        }
    }

    pub(super) fn shutdown(&mut self) -> Option<async_channel::Receiver<()>> {
        self.session.take().map(Session::close)
    }

    fn write(&mut self, bytes: Vec<u8>) {
        if let Some(session) = &self.session {
            self.parser.screen_mut().set_scrollback(0);
            self.selection = None;
            if let Err(error) = session.input(bytes) {
                self.error = Some(error.to_string());
            }
        }
    }

    fn paste(&mut self, text: &str) {
        if self.parser.screen().bracketed_paste() {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            self.write(bytes);
        } else {
            self.write(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
        }
    }

    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection?;
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        Some(
            self.parser
                .screen()
                .contents_between(start.0, start.1, end.0, end.1),
        )
    }

    pub(super) fn copy(&self, cx: &mut App) {
        let text = self
            .selected_text()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| output_text(self.parser.screen()));
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if !self.composing.is_empty() {
            return;
        }
        if key.modifiers.platform {
            match key.key.as_str() {
                "c" => self.copy(cx),
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.paste(&text);
                    }
                }
                "a" => {
                    let (rows, cols) = self.parser.screen().size();
                    self.selection = Some(((0, 0), (rows - 1, cols)));
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if !event.prefer_character_input
            && let Some(bytes) = key_bytes(key, self.parser.screen().application_cursor())
        {
            self.write(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn position(&self, point: Point<Pixels>) -> CellPosition {
        let Some(bounds) = self.bounds else {
            return (0, 0);
        };
        let (rows, cols) = self.parser.screen().size();
        let row = ((point.y - bounds.top()).as_f32() / LINE_HEIGHT)
            .floor()
            .clamp(0., (rows - 1) as f32) as u16;
        let col = ((point.x - bounds.left()).as_f32() / self.cell_width)
            .round()
            .clamp(0., cols as f32) as u16;
        (row, col)
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_delta += match event.delta {
            ScrollDelta::Pixels(point) => point.y.as_f32() / LINE_HEIGHT,
            ScrollDelta::Lines(point) => point.y,
        };
        let rows = self.scroll_delta.trunc() as isize;
        if rows != 0 {
            self.scroll_delta -= rows as f32;
            let offset = self
                .parser
                .screen()
                .scrollback()
                .saturating_add_signed(rows);
            self.parser.screen_mut().set_scrollback(offset);
            self.selection = None;
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn layout(&mut self, bounds: Bounds<Pixels>, cell_width: f32) {
        self.bounds = Some(bounds);
        self.cell_width = cell_width;
        let rows = (bounds.size.height.as_f32() / LINE_HEIGHT)
            .floor()
            .clamp(1., 200.) as u16;
        let cols = (bounds.size.width.as_f32() / cell_width)
            .floor()
            .clamp(1., 300.) as u16;
        if self.parser.screen().size() != (rows, cols) {
            self.parser.screen_mut().set_size(rows, cols);
            self.selection = None;
            if let Some(session) = &self.session {
                let _ = session.resize(Size {
                    rows,
                    cols,
                    ..Size::default()
                });
            }
        }
    }
}

fn output_text(screen: &vt100::Screen) -> String {
    let mut screen = screen.clone();
    screen.set_scrollback(usize::MAX);
    let history = screen.scrollback();
    let (_, cols) = screen.size();
    let mut lines = Vec::with_capacity(history);
    for offset in (1..=history).rev() {
        screen.set_scrollback(offset);
        lines.push(screen.rows(0, cols).next().unwrap_or_default());
    }
    screen.set_scrollback(0);
    lines.push(screen.contents());
    lines.join("\n").trim_end_matches('\n').to_owned()
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text: Vec<_> = self.composing.encode_utf16().collect();
        let selected = text.get(range.clone())?;
        *actual = Some(range);
        String::from_utf16(selected).ok()
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.session.as_ref().map(|_| UTF16Selection {
            range: self.composing_selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.composing.is_empty()).then(|| 0..self.composing.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.composing.clear();
        self.composing_selection = 0..0;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Committed input is only sent to the PTY. It is never echoed or retained
        // here, including when sudo has disabled the terminal's own echo.
        self.composing.clear();
        self.composing_selection = 0..0;
        self.write(text.as_bytes().to_vec());
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.unwrap_or(0..self.composing.encode_utf16().count());
        let start = byte_offset(&self.composing, range.start);
        let end = byte_offset(&self.composing, range.end);
        self.composing.replace_range(start..end.max(start), text);
        let length = self.composing.encode_utf16().count();
        self.composing_selection = selection
            .map(|range| range.start.min(length)..range.end.min(length))
            .unwrap_or(length..length);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds?;
        let (row, col) = self.parser.screen().cursor_position();
        let prefix = &self.composing[..byte_offset(&self.composing, range.start)];
        let width = UnicodeWidthStr::width(prefix) as f32 * self.cell_width;
        let left = (bounds.left() + px(col as f32 * self.cell_width + width))
            .min(bounds.right() - px(self.cell_width));
        Some(Bounds::new(
            point(left, bounds.top() + px(row as f32 * LINE_HEIGHT)),
            size(px(self.cell_width), px(LINE_HEIGHT)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.composing_selection.end)
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut offset = 0;
    for (index, character) in text.char_indices() {
        if offset >= utf16 {
            return index;
        }
        offset += character.len_utf16();
    }
    text.len()
}

fn key_bytes(key: &Keystroke, application_cursor: bool) -> Option<Vec<u8>> {
    let modifiers = key.modifiers;
    let mut bytes = match key.key.as_str() {
        "enter" => vec![b'\r'],
        "escape" => vec![0x1b],
        "backspace" if modifiers.control => vec![8],
        "backspace" => vec![127],
        "tab" if modifiers.shift => b"\x1b[Z".to_vec(),
        "tab" => vec![b'\t'],
        "up" | "down" | "right" | "left" | "home" | "end" => {
            let suffix = match key.key.as_str() {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                "home" => 'H',
                _ => 'F',
            };
            let flags = 1
                + u8::from(modifiers.shift)
                + 2 * u8::from(modifiers.alt)
                + 4 * u8::from(modifiers.control);
            if flags != 1 {
                format!("\x1b[1;{flags}{suffix}").into_bytes()
            } else {
                format!("\x1b{}{suffix}", if application_cursor { 'O' } else { '[' }).into_bytes()
            }
        }
        "delete" => b"\x1b[3~".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        _ if modifiers.control => {
            let key = key.key.as_bytes();
            if key.len() != 1 {
                return None;
            }
            let byte = key[0].to_ascii_uppercase();
            match byte {
                b'@'..=b'_' => vec![byte & 0x1f],
                b' ' => vec![0],
                b'?' => vec![127],
                _ => return None,
            }
        }
        _ => return None,
    };
    if modifiers.alt
        && !matches!(
            key.key.as_str(),
            "up" | "down" | "left" | "right" | "home" | "end"
        )
    {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

struct PaintState {
    screen: vt100::Screen,
    selection: Option<(CellPosition, CellPosition)>,
    composing: String,
    live: bool,
    focused: bool,
    cell_width: f32,
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let paint_entity = entity.clone();
        let family = cx.theme().mono_font_family.clone();
        let prepaint_family = family.clone();
        let palette = appearance::palette(self.theme);
        div()
            .id("task-terminal")
            .debug_selector(|| "task-terminal".into())
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key_down))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.focus(window, cx);
                    let position = this.position(event.position);
                    this.selection = Some((position, position));
                    this.selecting = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.selecting && event.dragging() {
                    let position = this.position(event.position);
                    if let Some(selection) = &mut this.selection {
                        selection.1 = position;
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let font = font(prepaint_family);
                        let font_id = window.text_system().resolve_font(&font);
                        let width = window
                            .text_system()
                            .advance(font_id, px(FONT_SIZE), 'M')
                            .map(|size| size.width.as_f32())
                            .unwrap_or(8.)
                            .max(1.);
                        entity.update(cx, |this, _| {
                            this.layout(bounds, width);
                            PaintState {
                                screen: this.parser.screen().clone(),
                                selection: this.selection,
                                composing: if this.echo {
                                    this.composing.clone()
                                } else {
                                    String::new()
                                },
                                live: this.session.is_some(),
                                focused: this.focus.is_focused(window),
                                cell_width: width,
                            }
                        })
                    },
                    move |bounds, state, window, cx| {
                        let focus = paint_entity.read(cx).focus.clone();
                        if state.live {
                            window.handle_input(
                                &focus,
                                ElementInputHandler::new(bounds, paint_entity.clone()),
                                cx,
                            );
                        }
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            paint_terminal(bounds, state, palette, family, window, cx);
                        });
                    },
                )
                .size_full(),
            )
    }
}

fn terminal_color(color: vt100::Color, default: Hsla, palette: Palette) -> Hsla {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(index) if index < 16 => [
            palette.muted,
            palette.red,
            palette.green,
            palette.yellow,
            palette.accent,
            palette.red,
            palette.accent,
            palette.text,
        ][index as usize % 8],
        vt100::Color::Idx(index) if index < 232 => {
            let index = index - 16;
            let component = |value: u8| {
                if value == 0 {
                    0
                } else {
                    55 + value as u32 * 40
                }
            };
            rgb((component(index / 36) << 16)
                | (component(index / 6 % 6) << 8)
                | component(index % 6))
            .into()
        }
        vt100::Color::Idx(index) => {
            let gray = 8 + (index as u32 - 232) * 10;
            rgb((gray << 16) | (gray << 8) | gray).into()
        }
        vt100::Color::Rgb(red, green, blue) => {
            rgb(((red as u32) << 16) | ((green as u32) << 8) | blue as u32).into()
        }
    }
}

fn paint_terminal(
    bounds: Bounds<Pixels>,
    state: PaintState,
    palette: Palette,
    family: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    let (rows, cols) = state.screen.size();
    let selection = state.selection.map(|(start, end)| {
        if start <= end {
            (start, end)
        } else {
            (end, start)
        }
    });
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = state.screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let position = point(
                bounds.left() + px(col as f32 * state.cell_width),
                bounds.top() + px(row as f32 * LINE_HEIGHT),
            );
            let width = state.cell_width * if cell.is_wide() { 2. } else { 1. };
            let mut fg = terminal_color(cell.fgcolor(), palette.text, palette);
            let mut bg = terminal_color(cell.bgcolor(), palette.base, palette);
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            if selection.is_some_and(|(start, end)| (row, col) >= start && (row, col) < end) {
                bg = palette.border;
            }
            if bg != palette.base {
                window.paint_quad(fill(
                    Bounds::new(position, size(px(width), px(LINE_HEIGHT))),
                    bg,
                ));
            }
            if cell.has_contents() {
                let mut font = font(family.clone());
                if cell.bold() {
                    font.weight = FontWeight::BOLD;
                }
                if cell.italic() {
                    font.style = FontStyle::Italic;
                }
                if cell.dim() {
                    fg = palette.muted;
                }
                paint_text(
                    cell.contents(),
                    position,
                    font,
                    fg,
                    cell.underline(),
                    window,
                    cx,
                );
            }
        }
    }
    let (row, col) = state.screen.cursor_position();
    let cursor = point(
        bounds.left() + px(col as f32 * state.cell_width),
        bounds.top() + px(row as f32 * LINE_HEIGHT),
    );
    if state.screen.scrollback() == 0 {
        if state.live && state.focused && !state.screen.hide_cursor() {
            window.paint_quad(fill(
                Bounds::new(cursor, size(px(2.), px(LINE_HEIGHT))),
                palette.accent,
            ));
        }
        if !state.composing.is_empty() {
            let width = UnicodeWidthStr::width(state.composing.as_str()) as f32 * state.cell_width;
            window.paint_quad(fill(
                Bounds::new(cursor, size(px(width), px(LINE_HEIGHT))),
                palette.base,
            ));
            paint_text(
                &state.composing,
                cursor,
                font(family),
                palette.text,
                true,
                window,
                cx,
            );
        }
    }
}

fn paint_text(
    text: &str,
    position: Point<Pixels>,
    font: Font,
    color: Hsla,
    underline: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: underline.then_some(UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: false,
        }),
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text.to_owned().into(), px(FONT_SIZE), &[run], None);
    let _ = line.paint(position, px(LINE_HEIGHT), TextAlign::Left, None, window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[gpui_kit::test]
    fn platform_text_ime_and_special_keys_reach_the_pty_without_local_echo(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt;
        use starter::task_terminal::Request;
        use std::time::{Duration, Instant};
        let (session, events) = Session::start(
            Request {
                env: vec![],
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "stty raw -echo; printf ready; dd bs=1 count=10 2>/dev/null | od -An -tx1"
                        .into(),
                ],
            },
            Size::default(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut ready = false;
        while Instant::now() < deadline && !ready {
            if let Ok(Event::Output(bytes)) = events.try_recv() {
                ready = bytes.windows(5).any(|bytes| bytes == b"ready");
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(ready);
        cx.update(gpui_kit::init);
        let (handle, terminal) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut terminal = TerminalView::new(ThemeName::Catppuccin, cx);
                    terminal.attach(session, cx);
                    terminal.focus(window, cx);
                    terminal
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.press("a", cx);
            terminal.update(cx, |terminal, cx| {
                terminal.feed(&Event::Echo(false), cx);
                terminal.replace_and_mark_text_in_range(None, "中", Some(1..1), window, cx);
                assert_eq!(terminal.marked_text_range(window, cx), Some(0..1));
                terminal.replace_text_in_range(None, "中", window, cx);
                assert!(terminal.composing.is_empty());
                assert_eq!(terminal.parser.screen().contents(), "");
            });
            for key in ["up", "escape", "enter", "ctrl-c"] {
                window.press(key, cx);
            }
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut output = String::new();
        let mut exited = false;
        while Instant::now() < deadline && !exited {
            match events.try_recv() {
                Ok(Event::Output(bytes)) => output.push_str(&String::from_utf8_lossy(&bytes)),
                Ok(Event::Exit { code, .. }) => {
                    assert_eq!(code, 0);
                    exited = true;
                }
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        assert!(exited, "PTY input did not reach fixture: {output}");
        assert_eq!(
            output.split_whitespace().collect::<Vec<_>>(),
            ["61", "e4", "b8", "ad", "1b", "5b", "41", "1b", "0d", "03"]
        );
    }

    #[test]
    fn terminal_keys_preserve_application_modes_and_control_input() {
        assert_eq!(
            key_bytes(&Keystroke::parse("up").unwrap(), false).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            key_bytes(&Keystroke::parse("up").unwrap(), true).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            key_bytes(&Keystroke::parse("ctrl-c").unwrap(), false).unwrap(),
            [3]
        );
        assert_eq!(
            key_bytes(&Keystroke::parse("shift-tab").unwrap(), false).unwrap(),
            b"\x1b[Z"
        );
        assert_eq!(
            key_bytes(&Keystroke::parse("ctrl-left").unwrap(), false).unwrap(),
            b"\x1b[1;5D"
        );
        assert!(key_bytes(&Keystroke::parse("a").unwrap(), false).is_none());
    }

    #[test]
    fn ansi_repainting_split_utf8_and_terminal_queries_are_interpreted() {
        let mut parser = vt100::Parser::new_with_callbacks(4, 30, 20, Responses::default());
        parser.process(b"old line\r\x1b[2K\x1b[32mnew ");
        let text = "中文";
        parser.process(&text.as_bytes()[..2]);
        parser.process(&text.as_bytes()[2..]);
        assert_eq!(parser.screen().contents(), "new 中文");
        assert!(parser.screen().cell(0, 4).unwrap().is_wide());
        parser.process(b"\x1b[6n");
        assert_eq!(parser.callbacks().0[0], b"\x1b[1;9R");
        parser.process(b"\x1b[?1049hselector");
        assert!(parser.screen().alternate_screen());
        parser.process(b"\x1b[?1049l");
        assert_eq!(parser.screen().contents(), "new 中文");
    }

    #[test]
    fn copying_output_includes_scrollback_even_while_viewing_old_lines() {
        let mut parser = vt100::Parser::new(2, 40, 20);
        parser.process(b"one\r\ntwo\r\nthree\r\nfour");
        parser.screen_mut().set_scrollback(1);
        assert_eq!(output_text(parser.screen()), "one\ntwo\nthree\nfour");
        assert_eq!(parser.screen().scrollback(), 1);
    }
}
