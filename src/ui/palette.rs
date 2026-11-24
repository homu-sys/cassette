use ratatui::style::Color;

use crate::config::ThemeKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiStyle {
    Classic,
    Compact,
    Minimal,
    Focus,
    Grid,
}

pub(crate) struct Palette {
    pub(crate) bg: Color,
    pub(crate) fg: Color,
    pub(crate) muted: Color,
    pub(crate) border: Color,
    pub(crate) accent: Color,
    pub(crate) secondary: Color,
    pub(crate) tertiary: Color,
    pub(crate) success: Color,
}

pub(crate) fn palette(theme: ThemeKind) -> Palette {
    match theme {
        ThemeKind::Classic => Palette {
            bg: Color::Black,
            fg: Color::White,
            muted: Color::DarkGray,
            border: Color::White,
            accent: Color::Cyan,
            secondary: Color::Yellow,
            tertiary: Color::Magenta,
            success: Color::LightGreen,
        },
        ThemeKind::Monochrome => Palette {
            bg: Color::Black,
            fg: Color::White,
            muted: Color::DarkGray,
            border: Color::Gray,
            accent: Color::White,
            secondary: Color::White,
            tertiary: Color::Gray,
            success: Color::White,
        },
        ThemeKind::Nord => Palette {
            bg: Color::Rgb(46, 52, 64),
            fg: Color::Rgb(216, 222, 233),
            muted: Color::Rgb(76, 86, 106),
            border: Color::Rgb(129, 140, 161),
            accent: Color::Rgb(136, 192, 208),
            secondary: Color::Rgb(235, 203, 139),
            tertiary: Color::Rgb(180, 142, 173),
            success: Color::Rgb(163, 190, 140),
        },
        ThemeKind::Gruvbox => Palette {
            bg: Color::Rgb(40, 40, 40),
            fg: Color::Rgb(235, 219, 178),
            muted: Color::Rgb(146, 131, 116),
            border: Color::Rgb(124, 111, 100),
            accent: Color::Rgb(131, 165, 152),
            secondary: Color::Rgb(250, 189, 47),
            tertiary: Color::Rgb(211, 134, 155),
            success: Color::Rgb(184, 187, 38),
        },
        ThemeKind::Dracula => Palette {
            bg: Color::Rgb(40, 42, 54),
            fg: Color::Rgb(248, 248, 242),
            muted: Color::Rgb(98, 114, 164),
            border: Color::Rgb(98, 114, 164),
            accent: Color::Rgb(139, 233, 253),
            secondary: Color::Rgb(241, 250, 140),
            tertiary: Color::Rgb(255, 121, 198),
            success: Color::Rgb(80, 250, 123),
        },
        ThemeKind::Amber => Palette {
            bg: Color::Rgb(26, 20, 0),
            fg: Color::Rgb(255, 216, 102),
            muted: Color::Rgb(138, 106, 31),
            border: Color::Rgb(138, 106, 31),
            accent: Color::Rgb(255, 176, 0),
            secondary: Color::Rgb(255, 216, 102),
            tertiary: Color::Rgb(255, 143, 0),
            success: Color::Rgb(255, 232, 115),
        },
        ThemeKind::Compact => Palette {
            bg: Color::Black,
            fg: Color::Rgb(206, 255, 206),
            muted: Color::Rgb(82, 142, 82),
            border: Color::Rgb(60, 140, 60),
            accent: Color::Rgb(124, 255, 124),
            secondary: Color::Rgb(220, 255, 124),
            tertiary: Color::Rgb(80, 200, 160),
            success: Color::Rgb(160, 255, 160),
        },
        ThemeKind::Minimal => Palette {
            bg: Color::Black,
            fg: Color::Rgb(230, 230, 230),
            muted: Color::Rgb(110, 110, 110),
            border: Color::Rgb(60, 60, 60),
            accent: Color::White,
            secondary: Color::Rgb(180, 180, 180),
            tertiary: Color::Rgb(140, 140, 140),
            success: Color::Rgb(230, 230, 230),
        },
        ThemeKind::Focus => Palette {
            bg: Color::Rgb(8, 8, 16),
            fg: Color::Rgb(240, 240, 255),
            muted: Color::Rgb(120, 120, 160),
            border: Color::Rgb(80, 80, 140),
            accent: Color::Rgb(160, 160, 255),
            secondary: Color::White,
            tertiary: Color::Rgb(120, 200, 255),
            success: Color::Rgb(200, 255, 200),
        },
        ThemeKind::Grid => Palette {
            bg: Color::Rgb(12, 12, 18),
            fg: Color::Rgb(230, 230, 240),
            muted: Color::Rgb(110, 110, 140),
            border: Color::Rgb(140, 120, 200),
            accent: Color::Rgb(190, 160, 255),
            secondary: Color::Rgb(120, 220, 255),
            tertiary: Color::Rgb(255, 160, 220),
            success: Color::Rgb(160, 255, 180),
        },
    }
}

pub(crate) fn ui_style(theme: ThemeKind) -> UiStyle {
    match theme {
        ThemeKind::Compact => UiStyle::Compact,
        ThemeKind::Minimal => UiStyle::Minimal,
        ThemeKind::Focus => UiStyle::Focus,
        ThemeKind::Grid => UiStyle::Grid,
        _ => UiStyle::Classic,
    }
}
