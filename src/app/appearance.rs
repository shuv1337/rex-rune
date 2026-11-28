//! Local chrome choice and Rune's native appearance signal.
use super::{
    panel::PanelKind,
    theme::{Palette, native_scheme, resolved_palette, set_palette},
    workspace::Workspace,
};
use rex_rune::{
    chrome_theme::{ResolvedTheme, ThemeLibrary},
    platform::dirs::themes_dir_for,
    settings::{ChromeTheme, Settings, TerminalThemeChoice},
    terminal_theme::ThemeDirs,
};
use rune::Tx;
use std::{path::Path, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeSlot {
    Manual,
    Light,
    Dark,
}

impl ChromeSlot {
    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Theme",
            Self::Light => "Light theme",
            Self::Dark => "Dark theme",
        }
    }

    pub fn chosen(self, settings: &Settings) -> &ChromeTheme {
        match self {
            Self::Manual => &settings.chrome_theme,
            Self::Light => &settings.light_chrome_theme,
            Self::Dark => &settings.dark_chrome_theme,
        }
    }
}

pub struct ChromeState {
    pub library: ThemeLibrary,
    pub palette: Arc<Palette>,
    system_dark: Option<bool>,
}

impl ChromeState {
    pub fn new(settings: &Settings, path: &Path, dirs: &ThemeDirs) -> Self {
        let library = ThemeLibrary::selected(
            &themes_dir_for(path),
            dirs,
            &[
                &settings.chrome_theme,
                &settings.dark_chrome_theme,
                &settings.light_chrome_theme,
            ],
        );
        let (id, theme) = library.resolve(active_slot(settings, true).chosen(settings));
        let palette = resolved_palette(id, theme);
        Self {
            library,
            palette,
            system_dark: None,
        }
    }

    pub fn active_theme(&self, settings: &Settings) -> &ResolvedTheme {
        self.library
            .resolve(active_slot(settings, self.system_dark.unwrap_or(true)).chosen(settings))
            .1
    }

    pub fn waiting_for_system(&self, settings: &Settings) -> bool {
        settings.match_system_appearance && self.system_dark.is_none()
    }
}

fn active_slot(settings: &Settings, system_dark: bool) -> ChromeSlot {
    if !settings.match_system_appearance {
        ChromeSlot::Manual
    } else if system_dark {
        ChromeSlot::Dark
    } else {
        ChromeSlot::Light
    }
}

impl Workspace {
    pub fn current_chrome_slot(&self) -> ChromeSlot {
        active_slot(&self.settings, self.chrome.system_dark.unwrap_or(true))
    }

    pub fn chrome_choice(&self, slot: ChromeSlot) -> (&ChromeTheme, &ResolvedTheme) {
        self.chrome.library.resolve(slot.chosen(&self.settings))
    }

    /// Called from Rune's existing frame callback. Rune wakes it on a native appearance
    /// change, including Linux portal updates. Rex schedules no wake or platform query.
    pub fn system_appearance(&mut self, tx: &mut Tx) -> bool {
        let Some(dark) = tx.system_dark() else {
            return false;
        };
        if self.chrome.system_dark == Some(dark) {
            return false;
        }
        self.chrome.system_dark = Some(dark);
        self.settings.match_system_appearance && self.sync_chrome(tx)
    }

    pub fn toggle_system_appearance(&mut self, tx: &mut Tx) {
        if let Some(dark) = tx.system_dark() {
            self.chrome.system_dark = Some(dark);
        }
        let follow = !self.settings.match_system_appearance;
        if follow && !self.settings.appearance_slots_seeded {
            let manual = self.settings.chrome_theme.clone();
            let dark = self.chrome_choice(ChromeSlot::Manual).1.palette.dark;
            if dark && self.settings.dark_chrome_theme.is_default() {
                self.settings.dark_chrome_theme = manual;
            } else if !dark && self.settings.light_chrome_theme.is_light_default() {
                self.settings.light_chrome_theme = manual;
            }
        }
        self.settings.appearance_slots_seeded = true;
        self.settings.match_system_appearance = follow;
        self.sync_chrome(tx);
        self.handle
            .set_color_scheme(native_scheme(follow, &self.chrome.palette));
        self.save();
    }

    /// Update chrome and the terminal declaration together, without recolouring server output.
    pub(super) fn sync_chrome(&mut self, tx: &mut Tx) -> bool {
        let (id, theme) = self.chrome_choice(self.current_chrome_slot());
        let next_palette =
            (**self.chrome.palette != theme.palette).then(|| resolved_palette(id, theme));
        let next_terminal = (self.settings.terminal_theme == TerminalThemeChoice::FollowChrome
            && !self.chrome.waiting_for_system(&self.settings)
            && self.term_theme.as_ref() != Some(&theme.terminal))
        .then(|| theme.terminal.clone());
        let changed = next_palette.is_some() || next_terminal.is_some();
        if changed {
            tx.instant();
        }
        if let Some(palette) = next_palette {
            if !self.settings.match_system_appearance && self.chrome.palette.dark != palette.dark {
                self.handle.set_color_scheme(native_scheme(false, &palette));
            }
            set_palette(palette.clone());
            self.chrome.palette = palette;
        }
        if let Some(terminal) = next_terminal {
            self.term_theme = Some(terminal);
            self.declare_theme();
        }
        changed
    }

    pub fn set_chrome(&mut self, chrome: ChromeTheme, tx: &mut Tx) {
        let slot = match self.panel.as_ref().map(|p| &p.kind) {
            Some(PanelKind::Chromes(slot)) => *slot,
            _ => self.current_chrome_slot(),
        };
        let selected = match slot {
            ChromeSlot::Manual => &mut self.settings.chrome_theme,
            ChromeSlot::Light => &mut self.settings.light_chrome_theme,
            ChromeSlot::Dark => &mut self.settings.dark_chrome_theme,
        };
        if *selected == chrome {
            return;
        }
        *selected = chrome;
        if slot == self.current_chrome_slot()
            && !self.settings.terminal_theme_chosen
            && self.settings.terminal_theme == TerminalThemeChoice::FollowServer
        {
            self.settings.terminal_theme = TerminalThemeChoice::FollowChrome;
        }
        self.sync_chrome(tx);
        self.save();
    }

    pub fn open_chromes(&mut self, slot: ChromeSlot, tx: &mut Tx) {
        self.chrome.library =
            ThemeLibrary::scan(&themes_dir_for(&self.settings_path), &self.theme_dirs);
        self.sync_chrome(tx);
        self.open_panel(
            PanelKind::Chromes(slot),
            slot.label(),
            &[("", "Filter themes…", "")],
        );
        if let Some(current) = self.entries().iter().position(|e| e.current) {
            let panel = self.panel.as_mut().unwrap();
            panel.selected = current;
            panel.reveal = true;
        }
    }
}
