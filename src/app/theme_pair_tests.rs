//! A light and a dark terminal theme (`theme_pair`): the appearance they follow, what is
//! declared when it changes, the two lists and what the library does with a card.
use super::*;
use crate::app::{panel::SettingsSection, theme_pair};
use rex_linux::{
    settings::{ChromeTheme, ColorScheme},
    terminal_theme::{TerminalTheme, builtin},
};
use rexrs::api::terminal::SetTheme;
use rune::ElementInfo;

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..40 {
        rt.frame(1. / 60.);
    }
}

/// The desktop turns dark, or light, as Rune reports it from the portal.
fn desktop(rt: &mut Runtime<Workspace>, dark: bool) {
    rt.set_system_dark(dark);
    settle(rt);
}

fn named(id: &str) -> TerminalThemeChoice {
    TerminalThemeChoice::Named(id.to_owned())
}

/// What a built-in theme declares.
fn args(id: &str) -> Option<SetTheme> {
    Some(
        builtin(id)
            .unwrap_or_else(|| panic!("no built-in {id}"))
            .theme()
            .args(),
    )
}

/// A directory with one dark Ghostty theme, Mocha, and one light one, Paper; removed by the
/// caller.
fn theme_dirs(tag: &str) -> (std::path::PathBuf, ThemeDirs) {
    let root = std::env::temp_dir().join(format!("rex-linux-pair-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("Mocha"),
        "background = #1e1e2e\nforeground = #cdd6f4\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Paper"),
        "background = #f2eede\nforeground = #222222\n",
    )
    .unwrap();
    let dirs = ThemeDirs {
        user: None,
        system: vec![root.clone()],
    };
    (root, dirs)
}

/// Settings on Themes with two captured hosts, at rest on a dark desktop.
fn themes_page(mut ws: Workspace) -> (Runtime<Workspace>, [flume::Receiver<HostCommand>; 2]) {
    let hosts = [capture_host(&mut ws, 0), capture_host(&mut ws, 1)];
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Themes);
    let mut rt = runtime_at(ws, (1440., 1000.));
    settle(&mut rt);
    (rt, hosts)
}

fn find(rt: &Runtime<Workspace>, key: &str) -> Option<ElementInfo> {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
}

/// The labels of the rows the open list shows, and which is checked.
fn rows(ws: &Workspace) -> Vec<(String, bool)> {
    ws.entries()
        .into_iter()
        .map(|e| (e.label, e.current))
        .collect()
}

/// Bring the library's card of theme `id` into view.
fn reveal(rt: &mut Runtime<Workspace>, id: &str) -> String {
    let key = format!("theme-card:{id}");
    rt.update(|_, tx| {
        tx.scroll_into_view(key.clone());
        tx.instant();
    });
    settle(rt);
    key
}

/// Whether the library marks the card of theme `id`, and what its tooltip says.
fn marked(rt: &mut Runtime<Workspace>, id: &str) -> Option<String> {
    let key = reveal(rt, id);
    let elements = rt.elements();
    let card = elements
        .iter()
        .position(|e| !e.exiting && e.key.as_deref() == Some(key.as_str()))
        .unwrap_or_else(|| panic!("missing {key}"));
    let mut inside = vec![card];
    let mut current = false;
    for (index, e) in elements.iter().enumerate().skip(card + 1) {
        if !e.exiting && e.parent.is_some_and(|parent| inside.contains(&parent)) {
            inside.push(index);
            current |= e.key.as_deref() == Some("theme-current");
        }
    }
    current.then(|| elements[card].help.clone().unwrap_or_default())
}

#[test]
fn one_theme_is_the_default_and_the_desktop_changes_nothing() {
    let (mut rt, hosts) = themes_page(workspace_hosts(2));
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::FollowServer
    );
    assert!(!rt.state.pairs_themes());
    assert_eq!(rt.state.desktop_dark, Some(true), "the window said so");
    assert!(find(&rt, "terminal-theme").is_some());
    assert!(find(&rt, "theme-pair").is_some(), "the switch is offered");
    for key in ["color-scheme", "light-theme", "dark-theme"] {
        assert!(find(&rt, key).is_none(), "{key} belongs to the pair");
    }
    desktop(&mut rt, false);
    assert_eq!(rt.state.desktop_dark, Some(false));
    assert_eq!(rt.state.light_appearance(), Some(true));
    desktop(&mut rt, true);
    assert!(
        hosts.iter().all(|host| host.is_empty()),
        "one theme never follows the desktop"
    );
    assert_eq!(rt.state.term_theme, None);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_pair_declares_the_theme_of_the_appearance_once_each_time_it_changes() {
    let (mut rt, hosts) = themes_page(workspace_hosts(2));
    click(&mut rt, "theme-pair");
    assert!(rt.state.pairs_themes());
    assert!(rt.state.settings.terminal_theme_chosen);
    for host in &hosts {
        assert_eq!(
            declared(host),
            [args("rex/merino-dark")],
            "the dark theme, on a dark desktop"
        );
    }
    // The page shows the pair instead of the one theme, and which of the two is in use.
    assert!(find(&rt, "terminal-theme").is_none());
    assert_eq!(
        find(&rt, "light-theme").unwrap().label.as_deref(),
        Some("Merino Light")
    );
    assert_eq!(
        find(&rt, "dark-theme").unwrap().label.as_deref(),
        Some("Merino Dark")
    );
    assert!(find(&rt, "dark-theme-in-use").is_some());
    assert!(find(&rt, "light-theme-in-use").is_none());
    assert!(
        element_text(&rt, "settings-scroll:themes")
            .iter()
            .any(|words| words == "Auto follows the desktop, which is dark now.")
    );

    // The desktop turns light: the light theme, once to each host, and nothing after.
    desktop(&mut rt, false);
    for host in &hosts {
        assert_eq!(declared(host), [args("rex/merino-light")]);
    }
    assert_eq!(
        rt.state.term_theme,
        Some(builtin("rex/merino-light").unwrap().theme())
    );
    assert!(find(&rt, "light-theme-in-use").is_some());
    assert!(find(&rt, "dark-theme-in-use").is_none());
    assert!(
        element_text(&rt, "settings-scroll:themes")
            .iter()
            .any(|words| words == "Auto follows the desktop, which is light now.")
    );
    settle(&mut rt);
    desktop(&mut rt, false);
    assert!(
        hosts.iter().all(|host| host.is_empty()),
        "the same appearance again declares nothing"
    );
    desktop(&mut rt, true);
    for host in &hosts {
        assert_eq!(declared(host), [args("rex/merino-dark")]);
    }

    // The same theme for both appearances is never declared again.
    rt.update(|s, _| s.settings.light_theme = "rex/merino-dark".into());
    desktop(&mut rt, false);
    assert_eq!(rt.state.light_appearance(), Some(true));
    assert!(hosts.iter().all(|host| host.is_empty()));

    // Saved as it is used.
    let saved = Settings::load(&rt.state.settings_path).unwrap();
    assert_eq!(saved.terminal_theme, TerminalThemeChoice::LightDark);
    assert_eq!(saved.dark_theme, "rex/merino-dark");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_appearance_can_be_kept_light_or_dark_whatever_the_desktop_asks_for() {
    let (mut rt, hosts) = themes_page(workspace_hosts(2));
    click(&mut rt, "theme-pair");
    hosts.iter().for_each(|host| drop(declared(host)));

    // Light, on a dark desktop.
    click(&mut rt, "segment1");
    assert_eq!(rt.state.settings.color_scheme, ColorScheme::Light);
    for host in &hosts {
        assert_eq!(declared(host), [args("rex/merino-light")]);
    }
    assert!(find(&rt, "light-theme-in-use").is_some());
    assert_eq!(
        Settings::load(&rt.state.settings_path)
            .unwrap()
            .color_scheme,
        ColorScheme::Light
    );
    // The desktop no longer matters.
    desktop(&mut rt, false);
    desktop(&mut rt, true);
    click(&mut rt, "segment1");
    assert!(hosts.iter().all(|host| host.is_empty()));

    // Dark, then Auto on a dark desktop: one declaration for the change, none for Auto.
    click(&mut rt, "segment2");
    assert_eq!(rt.state.settings.color_scheme, ColorScheme::Dark);
    for host in &hosts {
        assert_eq!(declared(host), [args("rex/merino-dark")]);
    }
    click(&mut rt, "segment0");
    assert_eq!(rt.state.settings.color_scheme, ColorScheme::Auto);
    assert!(hosts.iter().all(|host| host.is_empty()));
    desktop(&mut rt, false);
    for host in &hosts {
        assert_eq!(declared(host), [args("rex/merino-light")]);
    }
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_launch_waits_for_the_desktop_and_declares_one_theme() {
    let mut first = workspace();
    first.set_desktop_dark(true);
    first.pair_themes(true).unwrap();
    // This client has declared before, so one theme with nothing chosen would reset panes.
    first.settings.terminal_theme_declared = true;
    first.save();

    for dark in [true, false] {
        let mut ws = relaunch(&first);
        assert!(ws.pairs_themes());
        assert_eq!(ws.desktop_dark, None, "not known before the window says");
        assert_eq!(ws.term_theme, None);
        assert_eq!(
            ws.declared_theme(),
            None,
            "nothing is declared, and nothing reset, until it is known"
        );
        assert!(ws.notices.is_empty());
        let host = capture_host(&mut ws, 0);
        let mut rt = Runtime::new(ws, view, (1024., 700.), Env::new(dark, 1.));
        settle(&mut rt);
        let id = if dark {
            "rex/merino-dark"
        } else {
            "rex/merino-light"
        };
        assert_eq!(
            declared(&host),
            [args(id)],
            "the theme of the desktop, and never the other one first"
        );
        assert_eq!(rt.state.declared_theme(), args(id));
    }

    // An appearance that doesn't follow the desktop is known at once.
    first.set_color_scheme(ColorScheme::Light);
    let ws = relaunch(&first);
    assert_eq!(ws.declared_theme(), args("rex/merino-light"));
    assert_eq!(ws.light_appearance(), Some(true));
    assert_eq!(
        theme_pair::at_launch(&ws.settings),
        Some(named("rex/merino-light"))
    );
    std::fs::remove_file(&first.settings_path).ok();
}

#[test]
fn each_theme_of_the_pair_has_its_own_list_and_only_the_one_in_use_is_declared() {
    let (root, dirs) = theme_dirs("lists");
    let mut ws = workspace_hosts(2);
    ws.theme_dirs = dirs;
    let (mut rt, hosts) = themes_page(ws);
    click(&mut rt, "theme-pair");
    hosts.iter().for_each(|host| drop(declared(host)));

    // The light theme's list: the light themes by name, Merino Light checked and selected.
    click(&mut rt, "light-theme");
    let panel = rt.state.panel.as_ref().expect("the list");
    assert!(matches!(panel.kind, PanelKind::Themes));
    assert_eq!(panel.anchor_to.as_deref(), Some(theme_pair::LIGHT_CONTROL));
    assert_eq!(panel.fields[0].1, "Search light themes…");
    let listed = rows(&rt.state);
    let mut light: Vec<(String, bool)> = rex_linux::terminal_theme::BUILTINS
        .iter()
        .filter(|b| b.light())
        .map(|b| (b.name.to_owned(), b.id == "merino-light"))
        .collect();
    light.push(("Paper".to_owned(), false));
    assert_eq!(listed, light, "no dark theme, and no Follow server");
    assert_eq!(
        rt.state.entries()[panel.selected].label,
        "Merino Light",
        "the list opens on the theme it is set to"
    );
    // Choosing one changes the light theme only: terminals have the dark one.
    rt.update(|s, _| {
        s.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("paper");
        s.query_changed();
    });
    press(&mut rt, Key::Enter);
    assert!(
        matches!(
            rt.state.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Settings)
        ),
        "back to Settings"
    );
    assert_eq!(rt.state.settings.light_theme, "Paper");
    assert_eq!(rt.state.settings.dark_theme, "rex/merino-dark");
    assert!(rt.state.pairs_themes());
    assert!(hosts.iter().all(|host| host.is_empty()));
    settle(&mut rt);
    assert_eq!(
        find(&rt, "light-theme").unwrap().label.as_deref(),
        Some("Paper")
    );

    // The dark theme's list, and a choice that terminals take at once.
    click(&mut rt, "dark-theme");
    let listed = rows(&rt.state);
    assert_eq!(listed.len(), 17 + 1, "the dark built-in themes and Mocha");
    assert_eq!(listed[0], ("Merino Dark".to_owned(), true));
    assert!(listed.iter().all(|(label, _)| label != "Paper"));
    rt.update(|s, _| {
        s.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("mocha");
        s.query_changed();
    });
    assert_eq!(
        rows(&rt.state),
        [
            ("Catppuccin Mocha".to_owned(), false),
            ("Mocha".to_owned(), false)
        ]
    );
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.settings.dark_theme, "Mocha");
    let mocha = rt.state.term_theme.clone().expect("the dark theme");
    assert_eq!(mocha.background, Some(0x1e1e2e));
    for host in &hosts {
        assert_eq!(declared(host), [Some(mocha.args())]);
    }

    // Change theme… and a Commands search are for the appearance now: dark.
    rt.update(|s, tx| {
        s.close_panel(tx);
        s.perform(Intent::ChooseTheme, tx);
    });
    let listed = rows(&rt.state);
    assert_eq!(listed.len(), 18);
    assert_eq!(
        listed.iter().find(|(_, current)| *current),
        Some(&("Mocha".to_owned(), true))
    );
    rt.update(|s, _| s.picker(false));
    let themes = |rt: &mut Runtime<Workspace>, words: &str| -> Vec<String> {
        rt.update(|s, _| {
            s.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new(words);
            s.query_changed();
        });
        rt.state
            .entries()
            .into_iter()
            .filter(|e| matches!(e.action, Intent::Theme(_)))
            .map(|e| e.label)
            .collect()
    };
    assert!(themes(&mut rt, "paper").is_empty());
    assert_eq!(themes(&mut rt, "merino"), ["Merino Dark"]);
    desktop(&mut rt, false);
    assert_eq!(themes(&mut rt, "merino"), ["Merino Light"]);
    assert_eq!(themes(&mut rt, "paper"), ["Paper"]);

    let saved = Settings::load(&rt.state.settings_path).unwrap();
    assert_eq!(
        (saved.light_theme.as_str(), saved.dark_theme.as_str()),
        ("Paper", "Mocha")
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn in_the_library_a_card_becomes_the_theme_its_background_makes_it() {
    let (root, dirs) = theme_dirs("library");
    let mut ws = workspace_hosts(2);
    ws.theme_dirs = dirs;
    let (mut rt, hosts) = themes_page(ws);
    let headings = |rt: &Runtime<Workspace>| -> Vec<String> {
        rt.elements()
            .into_iter()
            .filter(|e| !e.exiting)
            .filter_map(|e| e.key?.strip_prefix("theme-group:").map(str::to_owned))
            .collect()
    };
    assert_eq!(headings(&rt)[0], "Default");
    click(&mut rt, "theme-pair");
    hosts.iter().for_each(|host| drop(declared(host)));
    assert_eq!(
        headings(&rt)[0],
        "Built in dark",
        "one theme for both appearances is not offered beside the pair"
    );
    assert!(find(&rt, "theme-card:follow_server").is_none());
    assert_eq!(element_text(&rt, "theme-count"), ["29 themes"]);

    // Both themes are marked, and each says which it is.
    assert_eq!(
        marked(&mut rt, "named:rex/merino-dark").as_deref(),
        Some("The dark theme, in use now.")
    );
    assert_eq!(
        marked(&mut rt, "named:rex/merino-light").as_deref(),
        Some("The light theme.")
    );
    assert_eq!(marked(&mut rt, "named:rex/dracula"), None);

    // A light card is the light theme from now on; terminals keep the dark one.
    let paper = reveal(&mut rt, "named:Paper");
    click(&mut rt, &paper);
    assert_eq!(rt.state.settings.light_theme, "Paper");
    assert!(rt.state.pairs_themes());
    assert!(hosts.iter().all(|host| host.is_empty()));
    assert_eq!(
        marked(&mut rt, "named:Paper").as_deref(),
        Some("The light theme.")
    );
    assert_eq!(marked(&mut rt, "named:rex/merino-light"), None);
    // A dark card is the dark theme, declared at once.
    let dracula = reveal(&mut rt, "named:rex/dracula");
    click(&mut rt, &dracula);
    assert_eq!(rt.state.settings.dark_theme, "rex/dracula");
    for host in &hosts {
        assert_eq!(declared(host), [args("rex/dracula")]);
    }
    assert_eq!(
        marked(&mut rt, "named:rex/dracula").as_deref(),
        Some("The dark theme, in use now.")
    );
    assert_eq!(
        find(&rt, "dark-theme").unwrap().label.as_deref(),
        Some("Dracula")
    );
    // Pressing the card in use again declares nothing.
    let dracula = reveal(&mut rt, "named:rex/dracula");
    click(&mut rt, &dracula);
    assert!(hosts.iter().all(|host| host.is_empty()));
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn turning_the_pair_on_and_off_keeps_the_theme_terminals_have() {
    let mut ws = workspace();
    ws.set_desktop_dark(true);
    let host = capture_host(&mut ws, 0);
    ws.apply_theme(named("rex/dracula")).unwrap();
    assert_eq!(declared(&host), [args("rex/dracula")]);

    // On: Dracula, a dark theme, is the dark theme, and still the one in use.
    ws.pair_themes(true).unwrap();
    assert_eq!(ws.settings.terminal_theme, TerminalThemeChoice::LightDark);
    assert_eq!(ws.settings.dark_theme, "rex/dracula");
    assert_eq!(ws.settings.light_theme, "rex/merino-light");
    assert!(host.is_empty(), "nothing changed for terminals");
    // Off: the theme in use is the one theme again.
    ws.pair_themes(false).unwrap();
    assert_eq!(ws.settings.terminal_theme, named("rex/dracula"));
    assert!(host.is_empty());
    assert_eq!(
        Settings::load(&ws.settings_path).unwrap().terminal_theme,
        named("rex/dracula")
    );

    // A light theme in use on a dark desktop is kept as the light theme; terminals take
    // the dark one, as the pair says.
    ws.apply_theme(named("rex/one-light")).unwrap();
    drop(declared(&host));
    ws.apply_theme(TerminalThemeChoice::LightDark).unwrap();
    assert_eq!(ws.settings.light_theme, "rex/one-light");
    assert_eq!(ws.settings.dark_theme, "rex/dracula");
    assert_eq!(declared(&host), [args("rex/dracula")]);

    // Follow server, chosen while the pair is on, is one theme again.
    ws.apply_theme(TerminalThemeChoice::FollowServer).unwrap();
    assert!(!ws.pairs_themes());
    assert_eq!(declared(&host), [None], "it stops declaring");
    assert_eq!(ws.settings.light_theme, "rex/one-light", "the pair is kept");

    // Follow chrome's theme is kept by its own name.
    let mut tx = rune::Tx::default();
    ws.set_chrome(ChromeTheme::RosePine, &mut tx);
    ws.apply_theme(TerminalThemeChoice::FollowChrome).unwrap();
    drop(declared(&host));
    ws.pair_themes(true).unwrap();
    assert_eq!(ws.settings.dark_theme, "rex/rose-pine");
    assert_eq!(ws.term_theme, Some(TerminalTheme::rose_pine()));
    assert!(host.is_empty());
    std::fs::remove_file(&ws.settings_path).ok();
}

#[test]
fn a_theme_that_cannot_be_read_keeps_the_one_in_use() {
    let (root, dirs) = theme_dirs("gone");
    let mut ws = workspace();
    ws.theme_dirs = dirs;
    ws.set_desktop_dark(true);
    let host = capture_host(&mut ws, 0);
    ws.pair_themes(true).unwrap();
    ws.apply_theme(named("Paper")).unwrap();
    assert_eq!(ws.settings.light_theme, "Paper");
    drop(declared(&host));

    // The light theme's file went away: the desktop turns light, the dark theme stays.
    std::fs::remove_file(root.join("Paper")).unwrap();
    let notices = ws.notices.len();
    ws.set_desktop_dark(false);
    assert_eq!(ws.notices.len(), notices + 1, "said once");
    assert_eq!(
        ws.term_theme,
        Some(builtin("rex/merino-dark").unwrap().theme())
    );
    assert!(host.is_empty());
    assert_eq!(
        ws.settings.light_theme, "Paper",
        "the setting is left alone"
    );
    // A theme that is not there can't be chosen either.
    assert!(ws.apply_theme(named("Nowhere")).is_err());
    assert_eq!(ws.settings.light_theme, "Paper");
    assert_eq!(ws.settings.dark_theme, "rex/merino-dark");
    // Back on a dark desktop nothing is declared: the dark theme never left.
    ws.set_desktop_dark(true);
    assert!(host.is_empty());
    std::fs::remove_file(&ws.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn the_pair_round_trips_through_the_workspace_file() {
    let mut ws = workspace();
    ws.set_desktop_dark(true);
    ws.pair_themes(true).unwrap();
    ws.apply_theme(named("rex/nord")).unwrap();
    ws.apply_theme(named("rex/catppuccin-latte")).unwrap();
    ws.set_color_scheme(ColorScheme::Light);

    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&ws.settings_path).unwrap()).unwrap();
    assert_eq!(json["terminal_theme"], "light_dark");
    assert_eq!(json["color_scheme"], "light");
    assert_eq!(json["light_theme"], "rex/catppuccin-latte");
    assert_eq!(json["dark_theme"], "rex/nord");

    let again = relaunch(&ws);
    assert!(again.pairs_themes());
    assert_eq!(again.settings.color_scheme, ColorScheme::Light);
    assert_eq!(again.settings.light_theme, "rex/catppuccin-latte");
    assert_eq!(again.settings.dark_theme, "rex/nord");
    assert_eq!(
        again.term_theme,
        Some(builtin("rex/catppuccin-latte").unwrap().theme()),
        "the theme of the appearance it was left in"
    );
    std::fs::remove_file(&ws.settings_path).ok();
}
