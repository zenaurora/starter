use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Window, rgb};
use starter::config::{Config, ThemeName};

#[derive(Clone, Copy)]
pub struct Palette {
    pub base: Hsla,
    pub surface: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub green: Hsla,
    pub yellow: Hsla,
    pub red: Hsla,
}

pub fn palette(name: ThemeName) -> Palette {
    // Omarchy's official palettes, with Latte as the light option.
    let values = match name {
        ThemeName::Catppuccin => [
            0x1e1e2e, 0x313244, 0x45475a, 0xcdd6f4, 0xa6adc8, 0x89b4fa, 0xa6e3a1, 0xf9e2af,
            0xf38ba8,
        ],
        ThemeName::Everforest => [
            0x2d353b, 0x343f44, 0x475258, 0xd3c6aa, 0x9da9a0, 0x7fbbb3, 0xa7c080, 0xdbbc7f,
            0xe67e80,
        ],
        ThemeName::Gruvbox => [
            0x282828, 0x3c3836, 0x504945, 0xd4be98, 0xbdae93, 0x7daea3, 0xa9b665, 0xd8a657,
            0xea6962,
        ],
        ThemeName::CatppuccinLatte => [
            0xeff1f5, 0xe6e9ef, 0xccd0da, 0x4c4f69, 0x6c6f85, 0x1e66f5, 0x40a02b, 0xdf8e1d,
            0xd20f39,
        ],
    };
    let [
        base,
        surface,
        border,
        text,
        muted,
        accent,
        green,
        yellow,
        red,
    ] = values.map(|v| rgb(v).into());
    Palette {
        base,
        surface,
        border,
        text,
        muted,
        accent,
        green,
        yellow,
        red,
    }
}

pub fn apply(name: ThemeName, config: &Config, window: &mut Window, cx: &mut App) {
    Theme::change(
        if name == ThemeName::CatppuccinLatte {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        },
        Some(window),
        cx,
    );
    let p = palette(name);
    Theme::update(cx, |theme| {
        theme.background = p.base;
        theme.foreground = p.text;
        theme.border = p.border;
        theme.accent = p.surface;
        theme.accent_foreground = p.text;
        theme.muted = p.surface;
        theme.muted_foreground = p.muted;
        theme.primary = p.accent;
        theme.primary_foreground = p.base;
        theme.primary_hover = p.accent;
        theme.primary_active = p.accent;
        theme.button_primary = p.accent;
        theme.button_primary_foreground = p.base;
        theme.button_primary_hover = p.accent;
        theme.button_primary_active = p.accent;
        theme.caret = p.accent;
        theme.input = p.border;
        theme.ring = p.accent;
        theme.selection = p.border;
        theme.list_active = p.surface;
        theme.list_hover = p.surface;
        theme.popover = p.base;
        theme.popover_foreground = p.text;
        theme.danger = p.red;
        theme.success = p.green;
        theme.warning = p.yellow;
        theme.mono_font_family = config.monospace_font.clone().into();
    });
}
