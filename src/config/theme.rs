use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeKind {
    #[default]
    Classic,
    Monochrome,
    Nord,
    Gruvbox,
    Dracula,
    Amber,
    Compact,
    Minimal,
    Focus,
    Grid,
}

impl ThemeKind {
    pub const ALL: [Self; 10] = [
        Self::Classic,
        Self::Monochrome,
        Self::Nord,
        Self::Gruvbox,
        Self::Dracula,
        Self::Amber,
        Self::Compact,
        Self::Minimal,
        Self::Focus,
        Self::Grid,
    ];

    pub fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|theme| *theme == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

impl fmt::Display for ThemeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Classic => write!(formatter, "Classic"),
            Self::Monochrome => write!(formatter, "Monochrome"),
            Self::Nord => write!(formatter, "Nord"),
            Self::Gruvbox => write!(formatter, "Gruvbox"),
            Self::Dracula => write!(formatter, "Dracula"),
            Self::Amber => write!(formatter, "Amber"),
            Self::Compact => write!(formatter, "Compact"),
            Self::Minimal => write!(formatter, "Minimal"),
            Self::Focus => write!(formatter, "Focus"),
            Self::Grid => write!(formatter, "Grid"),
        }
    }
}
