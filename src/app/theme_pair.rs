//! A light and a dark terminal theme, and which of the two terminals take.
//!
//! Under `TerminalThemeChoice::LightDark` this client declares `light_theme` while the
//! appearance is light and `dark_theme` while it is dark. The appearance is the desktop's
//! light or dark setting unless `color_scheme` says Light or Dark. Rune follows the desktop
//! (the XDG portal's `color-scheme`) and tells the view; [`desktop_probe`] passes it on to the
//! workspace, at the first frame and at every change.
//!
//! When the appearance changes, the other theme is resolved and declared: one
//! `HostCommand::Theme` to each host, whose connection declares it once to each pane this
//! client may theme. Nothing is declared before the desktop's setting is known, so a launch
//! on a light desktop declares the light theme and never the dark one first.
//!
//! The chrome stays dark in either appearance (DESIGN.md): only the terminal theme follows.
use super::{
    panel::{Intent, PanelKind},
    settings_kit::{
        ChipTone, Segment, found, popup_button, row_frame, segmented_fixed, setting, state_chip,
        switch,
    },
    theme::*,
    themes_view::{Picture, swatch_strip},
    workspace::Workspace,
};
use rex_linux::{
    settings::{ColorScheme, Settings, TerminalThemeChoice},
    terminal_theme::{TerminalTheme, builtin, paired},
};
use rune::{El, Tx, el, row, text};

/// The popup buttons of the two themes on Settings → Themes. A theme switcher opened from
/// one lists that appearance's themes.
pub const LIGHT_CONTROL: &str = "light-theme";
pub const DARK_CONTROL: &str = "dark-theme";

const SCHEMES: [ColorScheme; 3] = [ColorScheme::Auto, ColorScheme::Light, ColorScheme::Dark];

/// The theme choice to resolve as the workspace is built. None for a pair that follows the
/// desktop: which of its themes is known once the window has said what the desktop asks for.
pub fn at_launch(settings: &Settings) -> Option<TerminalThemeChoice> {
    match (&settings.terminal_theme, settings.color_scheme) {
        (TerminalThemeChoice::LightDark, ColorScheme::Auto) => None,
        (TerminalThemeChoice::LightDark, scheme) => {
            Some(settings.theme_in_use(scheme == ColorScheme::Light))
        }
        (choice, _) => Some(choice.clone()),
    }
}

/// Tells the workspace what the desktop asks for, `dark` or light, as the view learns it
/// from Rune: an element of no size whose key is the answer, so it is laid out once at the
/// first frame and once more each time the answer changes.
pub fn desktop_probe(dark: bool) -> El {
    el().key(if dark {
        "desktop:dark"
    } else {
        "desktop:light"
    })
    .absolute(0., 0.)
    .size(0., 0.)
    .on_layout(move |s: &mut Workspace, _| s.set_desktop_dark(dark))
}

impl Workspace {
    /// Whether terminals take a light and a dark theme.
    pub fn pairs_themes(&self) -> bool {
        self.settings.terminal_theme == TerminalThemeChoice::LightDark
    }

    /// Whether the appearance terminal themes follow is light. None under Auto until the
    /// window has said what the desktop asks for.
    pub fn light_appearance(&self) -> Option<bool> {
        match self.settings.color_scheme {
            ColorScheme::Light => Some(true),
            ColorScheme::Dark => Some(false),
            ColorScheme::Auto => self.desktop_dark.map(|dark| !dark),
        }
    }

    /// A pair is waiting to learn which of its themes to declare.
    pub(super) fn theme_waits_for_desktop(&self) -> bool {
        self.pairs_themes() && self.light_appearance().is_none()
    }

    /// The desktop asks for dark, or for light.
    pub fn set_desktop_dark(&mut self, dark: bool) {
        if self.desktop_dark == Some(dark) {
            return;
        }
        let before = self.light_appearance();
        self.desktop_dark = Some(dark);
        self.appearance_changed(before);
    }

    /// Follow the desktop, or keep to the light or the dark theme whatever it asks for.
    pub fn set_color_scheme(&mut self, scheme: ColorScheme) {
        if self.settings.color_scheme == scheme {
            return;
        }
        let before = self.light_appearance();
        self.settings.color_scheme = scheme;
        self.save();
        self.appearance_changed(before);
    }

    /// The appearance was `before`: when it is another now, declare its theme, once.
    fn appearance_changed(&mut self, before: Option<bool>) {
        if !self.pairs_themes() || self.light_appearance() == before {
            return;
        }
        if let Err(error) = self.use_theme_of_appearance() {
            if before.is_none() {
                // At launch there is no theme to keep, as when one theme can't be read.
                self.notify(format!("{error:#}; following the server's colours"));
                self.declare_theme();
            } else {
                self.notify(format!("{error:#}; keeping the theme in use"));
            }
        }
    }

    /// Resolve the theme of the appearance and, when it isn't the one in use, declare it to
    /// every host. Nothing changes when it can't be read.
    fn use_theme_of_appearance(&mut self) -> anyhow::Result<()> {
        let Some(light) = self.light_appearance() else {
            return Ok(());
        };
        let theme = self.theme_dirs.resolve(
            &self.settings.theme_in_use(light),
            self.settings.chrome_theme,
        )?;
        if theme != self.term_theme {
            self.term_theme = theme;
            self.declare_theme();
        }
        Ok(())
    }

    /// The part of `apply_theme` that is about the pair, or none when `choice` is one theme
    /// for both appearances. Choosing the pair turns it on. With it on, a theme by name
    /// becomes the light or the dark theme, by its background, and is declared only when
    /// that is the appearance now; Follow server, Rex's colours and Follow chrome are one
    /// theme again.
    pub(super) fn apply_to_pair(
        &mut self,
        choice: &TerminalThemeChoice,
    ) -> Option<anyhow::Result<()>> {
        match (self.pairs_themes(), choice) {
            (false, TerminalThemeChoice::LightDark) => Some(self.pair_themes(true)),
            (true, TerminalThemeChoice::LightDark) => Some(Ok(())),
            (true, TerminalThemeChoice::Named(id)) => Some(self.set_pair_theme(id)),
            _ => None,
        }
    }

    /// Take a light and a dark theme (`on`), or one theme again. Either way terminals keep
    /// the theme they have where that is possible: turning the pair on files a theme chosen
    /// by name (or the chrome's, under Follow chrome) as the light or the dark theme by its
    /// background, and turning it off leaves the theme of the appearance as the one theme.
    pub fn pair_themes(&mut self, on: bool) -> anyhow::Result<()> {
        if on == self.pairs_themes() {
            return Ok(());
        }
        if on {
            let kept = match &self.settings.terminal_theme {
                TerminalThemeChoice::Named(id) => Some(id.clone()),
                TerminalThemeChoice::FollowChrome => {
                    Some(paired(self.settings.chrome_theme).choice_id())
                }
                _ => None,
            };
            if let (Some(id), Some(theme)) = (kept, &self.term_theme) {
                *self.pair_slot(theme.is_light()) = id;
            }
            self.settings.terminal_theme = TerminalThemeChoice::LightDark;
        } else {
            let light = self.light_appearance().unwrap_or(false);
            self.settings.terminal_theme = self.settings.theme_in_use(light);
        }
        self.settings.terminal_theme_chosen = true;
        self.save();
        if on {
            return self.use_theme_of_appearance();
        }
        let theme = self
            .theme_dirs
            .resolve(&self.settings.terminal_theme, self.settings.chrome_theme)?;
        if theme != self.term_theme {
            self.term_theme = theme;
            self.declare_theme();
        }
        Ok(())
    }

    fn pair_slot(&mut self, light: bool) -> &mut String {
        if light {
            &mut self.settings.light_theme
        } else {
            &mut self.settings.dark_theme
        }
    }

    /// Make the theme `id` the light or the dark theme, by its background, and declare it
    /// when that is the appearance now.
    fn set_pair_theme(&mut self, id: &str) -> anyhow::Result<()> {
        let theme = self
            .theme_dirs
            .resolve(
                &TerminalThemeChoice::Named(id.to_owned()),
                self.settings.chrome_theme,
            )?
            .unwrap_or_default();
        let light = theme.is_light();
        *self.pair_slot(light) = id.to_owned();
        self.settings.terminal_theme_chosen = true;
        self.save();
        if self.light_appearance() == Some(light) && self.term_theme.as_ref() != Some(&theme) {
            self.term_theme = Some(theme);
            self.declare_theme();
        }
        Ok(())
    }

    /// Which of the pair a list of themes is for, light (`true`) or dark: the one whose
    /// popup button the open theme switcher was opened from, else the one of the appearance
    /// now. None while terminals take one theme, which every list offers whole.
    pub(super) fn listed_appearance(&self) -> Option<bool> {
        if !self.pairs_themes() {
            return None;
        }
        let control = self
            .panel
            .as_ref()
            .filter(|panel| matches!(panel.kind, PanelKind::Themes))
            .and_then(|panel| panel.anchor_to.as_deref());
        Some(match control {
            Some(LIGHT_CONTROL) => true,
            Some(DARK_CONTROL) => false,
            _ => self.light_appearance().unwrap_or(false),
        })
    }

    /// The colours of the theme `id`, when it is a built-in theme or one of the Ghostty
    /// themes that were listed: never read from disk for a picture.
    fn listed_theme(&self, id: &str) -> Option<TerminalTheme> {
        match builtin(id) {
            Some(builtin) => Some(builtin.theme()),
            None => self
                .themes
                .iter()
                .find(|theme| theme.name == id)
                .map(|theme| theme.theme.clone()),
        }
    }

    /// The theme switcher for the light or the dark theme, against its popup button, with
    /// the theme it is set to selected.
    fn open_pair_theme(&mut self, light: bool, tx: &mut Tx) {
        let control = if light { LIGHT_CONTROL } else { DARK_CONTROL };
        self.open_anchored(control, Intent::ChooseTheme, tx);
        // The list is this appearance's only now that it knows its control.
        let current = self.entries().iter().position(|entry| entry.current);
        if let Some(panel) = &mut self.panel {
            panel.selected = current.unwrap_or(0);
            panel.reveal = true;
            if let Some((_, placeholder)) = panel.fields.first_mut() {
                *placeholder = if light {
                    "Search light themes…"
                } else {
                    "Search dark themes…"
                }
                .into();
            }
        }
    }
}

/// The name a theme id is shown by: a built-in theme's, else the Ghostty file's own.
pub fn theme_name(id: &str) -> &str {
    builtin(id).map_or(id, |builtin| builtin.name)
}

/// The switch between one terminal theme and a light and dark pair.
pub fn pair_switch(s: &Workspace) -> El {
    setting(
        "Light and dark themes",
        "One terminal theme while the desktop is light and another while it is dark.",
        switch(
            "theme-pair",
            "Light and dark themes",
            s.pairs_themes(),
            |s| {
                let on = !s.pairs_themes();
                if let Err(error) = s.pair_themes(on) {
                    s.notify(format!("{error:#}"));
                }
            },
        ),
    )
}

/// The rows of the pair: which appearance terminals follow, then the theme of each.
pub fn pair_rows(s: &Workspace) -> [El; 3] {
    let scheme = s.settings.color_scheme;
    let following = match (scheme, s.desktop_dark) {
        (ColorScheme::Auto, Some(true)) => "Auto follows the desktop, which is dark now.",
        (ColorScheme::Auto, Some(false)) => "Auto follows the desktop, which is light now.",
        _ => "Auto follows the desktop's light or dark setting.",
    };
    let appearance = setting(
        "Appearance",
        following,
        segmented_fixed(
            "color-scheme",
            216.,
            &[
                Segment::new("Auto"),
                Segment::new("Light"),
                Segment::new("Dark"),
            ],
            SCHEMES.iter().position(|each| *each == scheme),
            |s, index, _| s.set_color_scheme(SCHEMES[index]),
        ),
    );
    [appearance, pair_row(s, true), pair_row(s, false)]
}

/// One theme of the pair: which it is, whether terminals have it now, its colours and the
/// popup button that changes it.
fn pair_row(s: &Workspace, light: bool) -> El {
    let (label, control, id) = if light {
        ("Light theme", LIGHT_CONTROL, &s.settings.light_theme)
    } else {
        ("Dark theme", DARK_CONTROL, &s.settings.dark_theme)
    };
    let mut words = row().grow(1.).shrink().min_w(0.).gap(8.).center_y().child(
        text(label)
            .font(ui(13.))
            .color(rgb(ink()))
            .truncate()
            .shrink(),
    );
    if s.light_appearance() == Some(light) {
        words = words.child(state_chip(
            &format!("{control}-in-use"),
            "In use",
            ChipTone::Good,
            "Terminals have this theme now.",
        ));
    }
    let mut trailing = row().gap(6.).center_y();
    if let Some(theme) = s.listed_theme(id) {
        trailing = trailing.child(swatch_strip(&Picture::Theme(s.swatch(&theme))));
    }
    let trailing = trailing.child(popup_button(
        control,
        theme_name(id),
        move |s: &mut Workspace, tx: &mut Tx| s.open_pair_theme(light, tx),
    ));
    found(label, "", row_frame(false))
        .child(words)
        .child(trailing)
}

/// What the pair does, under its rows.
pub const PAIR_FOOTNOTE: &str = "Each theme is declared to Rex for panes this client owns or nobody owns, the other one once when the appearance changes. In the library, a light theme you choose becomes the light theme and a dark one the dark theme. The window's own colours stay dark.";
