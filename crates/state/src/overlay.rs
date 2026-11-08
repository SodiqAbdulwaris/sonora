//! The mini player overlay: where it stands, what shape it takes and how it is summoned.
//!
//! These are the choices only. The window itself is opened and styled by the app binary, and the
//! views draw what is inside it.

use std::fmt;
use std::str::FromStr;

/// Where on the screen the overlay stands, as a preset along the top edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    #[default]
    TopCenter,
    TopRight,
}

impl Corner {
    pub const ALL: [Self; 3] = [Self::TopLeft, Self::TopCenter, Self::TopRight];

    /// The name settings store the preset by.
    pub fn id(self) -> &'static str {
        match self {
            Self::TopLeft => "top-left",
            Self::TopCenter => "top-center",
            Self::TopRight => "top-right",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|corner| corner.id() == id)
    }

    /// The Fluent key of the preset's name.
    pub fn key(self) -> &'static str {
        match self {
            Self::TopLeft => "settings-overlay-corner-top-left",
            Self::TopCenter => "settings-overlay-corner-top-center",
            Self::TopRight => "settings-overlay-corner-top-right",
        }
    }
}

/// What the overlay looks like: the full pill, or the small disc it folds into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    #[default]
    Pill,
    Disc,
}

impl Shape {
    pub fn id(self) -> &'static str {
        match self {
            Self::Pill => "pill",
            Self::Disc => "disc",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "pill" => Some(Self::Pill),
            "disc" => Some(Self::Disc),
            _ => None,
        }
    }
}

/// The key of a global shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Letter(char),
    Digit(char),
    Function(u8),
    Space,
}

/// A shortcut that works whichever program has the focus, written like `Alt+Shift+O`. It always
/// carries a modifier, since a bare key would stop that key working anywhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: Key,
}

impl Shortcut {
    /// Show or hide the overlay.
    pub const TOGGLE: Self = Self::alt_shift('O');
    /// Let clicks pass through the overlay, or take them again.
    pub const CLICK_THROUGH: Self = Self::alt_shift('C');

    const fn alt_shift(letter: char) -> Self {
        Self {
            ctrl: false,
            alt: true,
            shift: true,
            win: false,
            key: Key::Letter(letter),
        }
    }
}

/// Why a shortcut was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Empty,
    /// A key this build does not know.
    Key,
    /// No modifier, or no key.
    Incomplete,
}

impl FromStr for Shortcut {
    type Err = Invalid;

    fn from_str(text: &str) -> Result<Self, Invalid> {
        if text.trim().is_empty() {
            return Err(Invalid::Empty);
        }
        let mut shortcut = Self {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
            key: Key::Space,
        };
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => shortcut.ctrl = true,
                "alt" | "option" => shortcut.alt = true,
                "shift" => shortcut.shift = true,
                "win" | "super" | "meta" | "cmd" => shortcut.win = true,
                "space" => key = Some(Key::Space),
                other => key = Some(named(other).ok_or(Invalid::Key)?),
            }
        }
        shortcut.key = key.ok_or(Invalid::Incomplete)?;
        match shortcut.ctrl || shortcut.alt || shortcut.shift || shortcut.win {
            true => Ok(shortcut),
            false => Err(Invalid::Incomplete),
        }
    }
}

/// A letter, digit or function key by its lowercase name.
fn named(name: &str) -> Option<Key> {
    let mut chars = name.chars();
    match (chars.next()?, chars.as_str()) {
        (letter @ 'a'..='z', "") => Some(Key::Letter(letter.to_ascii_uppercase())),
        (digit @ '0'..='9', "") => Some(Key::Digit(digit)),
        ('f', number) => number
            .parse::<u8>()
            .ok()
            .filter(|number| (1..=12).contains(number))
            .map(Key::Function),
        _ => None,
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (held, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.win, "Win"),
        ] {
            if held {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            Key::Letter(letter) | Key::Digit(letter) => write!(f, "{letter}"),
            Key::Function(number) => write!(f, "F{number}"),
            Key::Space => f.write_str("Space"),
        }
    }
}
