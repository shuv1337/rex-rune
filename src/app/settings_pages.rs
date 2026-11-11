//! Settings pages that are not a section's first: Themes, and the pages pushed over a
//! section (Interface → Panes, a notification state). Built from `settings_kit` like the rest.
use super::{
    activity::progress_bar,
    panel::Intent,
    settings_kit::{card, footnote, page, section, setting, value, value_popup},
    settings_nav::Status,
    settings_view::notify_switch,
    tab_icons::icons_of,
    theme::*,
    theme_pair,
    toolbar::{PILL, sample_pill},
    workspace::Workspace,
};
use rex_linux::settings::NotificationsConfig;
use rune::{El, text};

/// The colours of the window and of its terminals, over the library of terminal themes.
pub fn themes(s: &Workspace) -> El {
    page([pickers(s), super::themes_view::library(s)])
}

/// What the window and its terminals are drawn in now, each with its list: one terminal
/// theme, or a light and a dark one with the appearance they follow.
fn pickers(s: &Workspace) -> El {
    let mut rows = vec![
        setting(
            "Window colours",
            "Terminals take the matching theme until you choose one.",
            value_popup(
                "chrome-theme",
                s.settings.chrome_theme.name(),
                Intent::ChooseChrome,
            ),
        ),
        theme_pair::pair_switch(s),
    ];
    let note = if s.pairs_themes() {
        rows.extend(theme_pair::pair_rows(s));
        theme_pair::PAIR_FOOTNOTE
    } else {
        rows.push(setting(
            "Terminal theme",
            "The colours programs draw in.",
            value_popup(
                "terminal-theme",
                Workspace::theme_label(&s.settings.terminal_theme),
                Intent::ChooseTheme,
            ),
        ));
        "A terminal theme is declared to Rex for panes this client owns or nobody owns. Follow server declares nothing until this client has declared a theme once; from then on it resets those panes to Rex's colours, once each per connection."
    };
    section("", [card(rows), footnote(note)])
}

impl Status {
    /// When a program is in this state, in a sentence for its row on Notifications.
    pub fn detail(self) -> &'static str {
        match self {
            Status::Blocked => "Waiting for approval, an answer or a sign-in.",
            Status::Error => "Reports an error.",
            Status::Done => "Stops working, or reports that it's done.",
            Status::Working => "",
        }
    }

    /// Whether this state sends a desktop notification.
    pub fn notifies(self, n: &NotificationsConfig) -> bool {
        match self {
            Status::Blocked => n.blocked,
            Status::Error => n.error,
            Status::Done => n.done,
            Status::Working => n.working,
        }
    }

    fn field(self) -> fn(&mut NotificationsConfig) -> &mut bool {
        match self {
            Status::Blocked => |n| &mut n.blocked,
            Status::Error => |n| &mut n.error,
            Status::Done => |n| &mut n.done,
            Status::Working => |n| &mut n.working,
        }
    }

    /// What a tab shows for this state: the word after its name and its colour, or for a
    /// working program nothing but the progress it reports.
    fn word(self) -> Option<(&'static str, u32)> {
        match self {
            Status::Blocked => Some(("Needs approval", warning())),
            Status::Error => Some(("Error", danger())),
            Status::Done => Some(("Done", accent())),
            Status::Working => None,
        }
    }

    fn in_tabs(self) -> &'static str {
        match self {
            Status::Blocked => "What the program waits for, after the tab's name.",
            Status::Error => "Error after the tab's name, until the program moves on.",
            Status::Done => "Done after the tab's name, until you look at the tab.",
            Status::Working => "A bar under the tab's name, when the program reports progress.",
        }
    }

    /// Where this state shows, for its row on Notifications: `Tabs, Notification`.
    pub fn summary(self, n: &NotificationsConfig) -> String {
        let mut shown = Vec::new();
        if n.show_in_tabs {
            shown.push("Tabs");
        }
        if n.system_notifications && self.notifies(n) {
            shown.push("Notification");
        }
        if shown.is_empty() {
            "Off".into()
        } else {
            shown.join(", ")
        }
    }
}

/// A tab as the strip draws it in this state. A picture: it takes no input.
fn sample_tab(s: &Workspace, status: Status) -> El {
    let tab = sample_pill(
        "status-sample",
        &icons_of(&[]),
        "build",
        true,
        None,
        &s.tab_icon_look(),
    )
    .role("img")
    .label(match status.word() {
        Some((word, _)) => format!("A tab named build that reads {word}"),
        None => "A tab named build with a progress bar".to_owned(),
    })
    .h(PILL)
    .inert();
    match status.word() {
        Some((word, tone)) => tab.child(text(word).font(medium(11.5)).color(rgb(tone))),
        None => tab.child(progress_bar(
            "status-sample-progress".into(),
            (Some(60), accent()),
            14.,
        )),
    }
}

/// Notifications → one program state: how it shows in tabs, and whether it sends a desktop
/// notification.
pub fn status(s: &Workspace, status: Status) -> El {
    let n = &s.settings.notifications;
    let off = !n.system_notifications;
    let tabs = if n.show_in_tabs {
        setting("In tabs", status.in_tabs(), sample_tab(s, status))
    } else {
        setting(
            "In tabs",
            "Nothing, while Activity in tabs is off.",
            value("Off"),
        )
    };
    let label = format!(
        "Desktop notification when a program {}",
        lower(status.title())
    );
    page([section(
        "",
        [card([
            tabs,
            setting(
                "Desktop notification",
                if off {
                    "Desktop notifications are off."
                } else {
                    "For tabs you aren't looking at."
                },
                notify_switch(
                    &format!("notify-{}", status.key()),
                    &label,
                    status.notifies(n),
                    off,
                    status.field(),
                ),
            ),
        ])],
    )])
}

/// `Needs you` as it reads in the middle of a sentence.
fn lower(title: &str) -> String {
    let mut chars = title.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}
