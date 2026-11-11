//! The rows of the theme switcher, and the themes and window colours a Commands search finds.
//!
//! Names are searched without case or accents (`fold.rs`), every word of the query must be in
//! the name, and each row carries where they matched so the list can mark it.
use super::{
    fold::{Name, fold},
    panel::{Entry, Intent},
    themes_view::Picture,
    workspace::Workspace,
};
use rex_linux::{
    settings::{ChromeTheme, TerminalThemeChoice},
    terminal_theme::{BUILTIN_PREFIX, BUILTINS, TerminalTheme, paired},
};

/// The heading themes and window colours are under in a Commands search.
pub const APPEARANCE: &str = "Appearance";
/// What a theme's row says it is, in Commands, where it is listed beside window colours.
pub const TERMINAL_THEME: &str = "Terminal theme";
pub const WINDOW_COLOURS: &str = "Window colours";
/// A Commands search lists themes from this many letters on: one letter is in most names.
const COMMANDS_MIN_QUERY: usize = 2;

impl Workspace {
    /// The server's own colours, the built-in themes, then the dark and the light Ghostty
    /// themes, each kept when its name holds every word of `query`. `named_only` leaves out
    /// the choices that are not a theme by name (Follow server, Rex default colours, Follow
    /// chrome) and has a word of the query count only where a word of the name begins.
    fn themes_matching(&self, query: &str, named_only: bool) -> Vec<Entry> {
        let query = fold(query);
        let current = &self.settings.terminal_theme;
        let mut entries = Vec::new();
        // Each row ends with its theme's colours as a strip of dots; Follow server, whose
        // colours are the server's, has none to show.
        let mut add = |label: &str,
                       detail: &str,
                       group: &str,
                       choice: TerminalThemeChoice,
                       theme: Option<&TerminalTheme>| {
            if named_only && !matches!(choice, TerminalThemeChoice::Named(_)) {
                return;
            }
            if let Some(marks) = Name::new(label).find(&query, named_only) {
                let mut e = Entry::new(label, detail, group, Intent::Theme(choice.clone()));
                e.current = *current == choice;
                e.marks = marks;
                e.picture = Some(match theme {
                    Some(theme) => Picture::Theme(self.swatch(theme)),
                    None => Picture::Server,
                });
                entries.push(e);
            }
        };
        add(
            "Follow server",
            if self.settings.terminal_theme_declared {
                "Resets to Rex's colours"
            } else {
                "Declares nothing"
            },
            "Server",
            TerminalThemeChoice::FollowServer,
            None,
        );
        add(
            "Rex default colours",
            "",
            "Server",
            TerminalThemeChoice::RexDefault,
            Some(&TerminalTheme::default()),
        );
        let chrome = paired(self.settings.chrome_theme);
        add(
            "Follow chrome",
            chrome.name,
            "Built in",
            TerminalThemeChoice::FollowChrome,
            Some(&(chrome.theme)()),
        );
        for builtin in BUILTINS {
            let id = format!("{BUILTIN_PREFIX}{}", builtin.id);
            add(
                builtin.name,
                "",
                "Built in",
                TerminalThemeChoice::Named(id),
                Some(&(builtin.theme)()),
            );
        }
        for (group, light) in [("Dark", false), ("Light", true)] {
            for theme in self.themes.iter().filter(|t| t.light == light) {
                add(
                    &theme.name,
                    if theme.custom { "Custom" } else { "" },
                    group,
                    TerminalThemeChoice::Named(theme.name.clone()),
                    Some(&theme.theme),
                );
            }
        }
        entries
    }

    /// The theme switcher's rows for what is typed in its field.
    pub(super) fn theme_entries(&self, query: &str) -> Vec<Entry> {
        self.themes_matching(query, false)
    }

    /// What a Commands search for `query` finds under Appearance: the window colours, then
    /// the terminal themes, whose names have a word starting with each word typed. Choosing
    /// one applies it at once; the one in use is checked. Follow server, Rex default colours
    /// and Follow chrome are not names to search for: they are in the switcher, which
    /// Change theme… opens.
    pub(super) fn appearance_matches(&self, query: &str) -> Vec<Entry> {
        let folded = fold(query);
        if folded.chars().filter(|c| !c.is_whitespace()).count() < COMMANDS_MIN_QUERY {
            return Vec::new();
        }
        let mut entries: Vec<Entry> = ChromeTheme::ALL
            .into_iter()
            .filter_map(|chrome| {
                let marks = Name::new(chrome.name()).find(&folded, true)?;
                let mut e = Entry::new(
                    chrome.name(),
                    WINDOW_COLOURS,
                    APPEARANCE,
                    Intent::Chrome(chrome),
                );
                e.current = chrome == self.settings.chrome_theme;
                e.marks = marks;
                e.picture = Some(Picture::Chrome(chrome));
                Some(e)
            })
            .collect();
        entries.extend(self.themes_matching(query, true).into_iter().map(|mut e| {
            e.detail = TERMINAL_THEME.into();
            e.group = APPEARANCE.into();
            e
        }));
        entries
    }

    /// Read the Ghostty themes again: the switcher and Commands list what is installed now.
    pub(super) fn read_themes(&mut self) {
        self.themes = self.theme_dirs.list();
    }
}
