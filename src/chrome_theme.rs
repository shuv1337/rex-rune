//! Built-in chrome and terminal palettes share one catalogue. Saved chrome IDs and
//! terminal IDs are separate: the original Rosé Pine and Gruvbox IDs differ.
use crate::terminal_theme::TerminalTheme;
use serde::{Deserialize, Deserializer, Serialize};
use std::borrow::Cow;

mod catalog;
pub use catalog::BUILTINS;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ChromeTheme(Cow<'static, str>);

#[allow(non_upper_case_globals)]
impl ChromeTheme {
    const fn builtin(id: &'static str) -> Self {
        Self(Cow::Borrowed(id))
    }
    pub const Midnight: Self = Self::builtin("midnight");
    pub const RosePine: Self = Self::builtin("rose_pine");
    pub const RosePineMoon: Self = Self::builtin("rose_pine_moon");
    pub const GruvboxDark: Self = Self::builtin("gruvbox_dark");
    pub fn all() -> impl Iterator<Item = Self> {
        BUILTINS.iter().map(|b| b.chrome.clone())
    }
    pub fn id(&self) -> &str {
        &self.0
    }
    pub fn from_id(id: &str) -> Self {
        BUILTINS
            .iter()
            .find(|b| b.chrome.id() == id)
            .map(|b| b.chrome.clone())
            .unwrap_or_default()
    }
    pub fn preset(&self) -> &'static Builtin {
        BUILTINS
            .iter()
            .find(|b| b.chrome == *self)
            .expect("validated built-in chrome ID")
    }
    pub fn name(&self) -> &'static str {
        self.preset().name
    }
}
impl Default for ChromeTheme {
    fn default() -> Self {
        Self::Midnight
    }
}
impl<'de> Deserialize<'de> for ChromeTheme {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|id| Self::from_id(&id))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PaletteSpec {
    pub dark: bool,
    pub ground: u32,
    pub card: u32,
    pub sidebar: u32,
    pub popover: u32,
    pub ink: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent2: u32,
    pub live: u32,
    pub warning: u32,
    pub danger: u32,
    pub on_accent: u32,
    pub terminal_selection: u32,
    pub border_subtle: Option<u32>,
    pub border_strong: Option<u32>,
    pub surface_raised: Option<u32>,
}
pub struct Builtin {
    pub chrome: ChromeTheme,
    pub id: &'static str,
    pub name: &'static str,
    pub palette: PaletteSpec,
    // Trailing roles left out of a preset retain Rex's defaults.
    terminal: &'static [u32],
    ansi: &'static [u32],
}
impl Builtin {
    pub fn scheme(&self) -> &'static str {
        if self.palette.dark { "dark" } else { "light" }
    }
    pub fn theme(&self) -> TerminalTheme {
        TerminalTheme {
            background: Some(self.palette.ground),
            foreground: self.terminal.first().copied(),
            cursor: self.terminal.get(1).copied(),
            cursor_text: self.terminal.get(2).copied(),
            selection_background: self.terminal.get(3).copied(),
            selection_foreground: self.terminal.get(4).copied(),
            palette: (0..).zip(self.ansi.iter().copied()).collect(),
            scheme: Some(self.scheme()),
        }
    }
}
