//! Settings → Interface → Customize tabs: whether toolbar tabs share its width, and the icons
//! a tab shows for its panes' programs. Two sample tabs at the top are drawn by the toolbar's
//! own pieces (`toolbar::sample_pill`) with the settings as they are, so every control shows
//! what it does at once.
use super::{
    settings_kit::{
        INSET, card, footnote, page as kit_page, popup_button, section, select_row, setting,
        stepper, switch,
    },
    tab_icons::TabIcon,
    theme::*,
    toolbar::{PILL, TRACK, capsule, fill_floor, sample_pill},
    workspace::Workspace,
};
use rex_linux::settings::{ShellIconTheme, TabIconColor, TabsConfig};
use rune::{El, Icon, Tx, row};

/// The colour choices, in the order their list has them.
const COLORS: [TabIconColor; 4] = [
    TabIconColor::Always,
    TabIconColor::ActivePane,
    TabIconColor::ActiveTab,
    TabIconColor::Never,
];
const COLOR_NAMES: [&str; 4] = ["Always", "Active pane", "Active tab", "Never"];

const SHELLS: [ShellIconTheme; 3] = [
    ShellIconTheme::Default,
    ShellIconTheme::Theme,
    ShellIconTheme::Accent,
];
const SHELL_NAMES: [&str; 3] = ["Default", "Terminal theme", "Accent"];

/// The sample tabs: a name, and one program for each of five panes, the focused pane's
/// first, so every count from 0 to 5 draws differently.
const SAMPLES: [(&str, [(&Icon, &str); 5]); 2] = [
    (
        "Feature work",
        [
            (&TERMINAL, "zsh"),
            (&CLAUDE, "claude"),
            (&VIM, "nvim"),
            (&GIT, "lazygit"),
            (&CRATE, "cargo"),
        ],
    ),
    (
        "Code review",
        [
            (&CODEX, "codex"),
            (&TERMINAL, "zsh"),
            (&GIT, "git"),
            (&KEY, "ssh"),
            (&VIM, "nvim"),
        ],
    ),
];

/// Two tabs as the toolbar draws them now, the first one on screen. A picture: it takes no
/// input.
fn preview(s: &Workspace) -> El {
    let fill = s.settings.tabs.fill;
    let look = s.tab_icon_look();
    let mut names = row().h(PILL).gap(2.);
    if fill {
        names = names.grow(1.);
    }
    let floor = fill.then(|| fill_floor(look.count.min(5), None, false));
    for (index, (name, programs)) in SAMPLES.into_iter().enumerate() {
        let icons: Vec<TabIcon> = programs
            .into_iter()
            .enumerate()
            .map(|(pane, (icon, program))| TabIcon {
                icon,
                program: Some(program.to_owned()),
                focused: pane == 0,
            })
            .collect();
        names = names.child(sample_pill(
            &format!("tabs-preview-tab:{index}"),
            &icons,
            name,
            index == 0,
            floor,
            &look,
        ));
    }
    let mut track = capsule(row().px((TRACK - PILL) / 2.).center_y().child(names), TRACK);
    if fill {
        track = track.grow(1.);
    }
    row()
        .key("tabs-preview")
        .role("img")
        .label("Two sample tabs drawn with these settings")
        .w_full()
        .padding(INSET)
        .radius(CARD_RADIUS)
        .continuous_corners()
        .fill(black(0.22))
        .border(1., white(0.07))
        .inert()
        .child(track)
}

/// A setting picked from a list that has nothing to act on now: its value dimmed, out of
/// reach of the pointer and the Tab key, with the row's helper saying why.
fn unavailable_choice(key: &str, label: &str, why: &str, value: &str) -> El {
    setting(
        label,
        why,
        popup_button(key, value, |_: &mut Workspace, _: &mut Tx| {})
            .opacity(0.4)
            .inert(),
    )
}

pub fn page(s: &Workspace) -> El {
    let tabs = s.settings.tabs;
    let none = tabs.icons == 0;
    let color = COLORS
        .iter()
        .position(|c| *c == tabs.icon_color)
        .unwrap_or_default();
    let shell = SHELLS
        .iter()
        .position(|t| *t == tabs.shell_icon)
        .unwrap_or_default();
    let color_row = if none {
        unavailable_choice(
            "tab-icon-color",
            "Use colour for icons",
            "Tabs show no icons.",
            COLOR_NAMES[color],
        )
    } else {
        select_row(
            "tab-icon-color",
            "Use colour for icons",
            "Otherwise icons are drawn in the colour of the tab's name.",
            &COLOR_NAMES,
            color,
            |s, i, _| {
                s.settings.tabs.icon_color = COLORS[i.min(COLORS.len() - 1)];
                s.save();
            },
        )
    };
    let shell_row = if none || tabs.icon_color == TabIconColor::Never {
        unavailable_choice(
            "shell-icon",
            "Shell icon",
            if none {
                "Tabs show no icons."
            } else {
                "Icons are not in colour."
            },
            SHELL_NAMES[shell],
        )
    } else {
        select_row(
            "shell-icon",
            "Shell icon",
            "A shell's prompt on its tile, while it is in colour.",
            &SHELL_NAMES,
            shell,
            |s, i, _| {
                s.settings.tabs.shell_icon = SHELLS[i.min(SHELLS.len() - 1)];
                s.save();
            },
        )
    };
    kit_page([
        section("", [preview(s)]),
        section(
            "Width",
            [card([setting(
                "Tabs fill the toolbar",
                "Tabs share the toolbar's width equally instead of taking their names' width.",
                switch("tabs-fill", "Tabs fill the toolbar", tabs.fill, |s| {
                    s.settings.tabs.fill = !s.settings.tabs.fill;
                    s.save();
                }),
            )])],
        ),
        section(
            "Icons",
            [
                card([
                    setting(
                        "Icons per tab",
                        "One for each program in a tab's panes, the focused pane's first.",
                        stepper(
                            "tab-icons",
                            tabs.icons.to_string(),
                            |s, _| {
                                s.settings.tabs.icons = s.settings.tabs.icons.saturating_sub(1);
                                s.save();
                            },
                            |s, _| {
                                s.settings.tabs.icons =
                                    (s.settings.tabs.icons + 1).min(TabsConfig::MAX_ICONS);
                                s.save();
                            },
                        ),
                    ),
                    color_row,
                    shell_row,
                ]),
                footnote(
                    "Tabs listed in the sidebar show the same icons. Panes running the same program share one icon.",
                ),
            ],
        ),
    ])
}

/// Kept beside the page so the list and the setting can't drift apart.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_is_listed_once_under_its_name() {
        assert_eq!(COLORS.len(), COLOR_NAMES.len());
        assert_eq!(SHELLS.len(), SHELL_NAMES.len());
        assert_eq!(COLORS[3], TabIconColor::default());
        assert_eq!(SHELLS[0], ShellIconTheme::default());
        // Each sample has five different icons, so no two counts draw alike.
        for (_, programs) in SAMPLES {
            let icons: Vec<TabIcon> = programs
                .into_iter()
                .map(|(icon, program)| TabIcon {
                    icon,
                    program: Some(program.to_owned()),
                    focused: false,
                })
                .collect();
            for (index, icon) in icons.iter().enumerate() {
                assert!(
                    icons[..index]
                        .iter()
                        .all(|other| !std::ptr::eq(other.icon, icon.icon))
                );
            }
        }
    }
}
