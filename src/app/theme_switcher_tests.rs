//! The theme switcher (its search, size and check), the themes a Commands search finds, and
//! the forms that open over Settings against their row.
use super::*;
use crate::app::{commands, panel::SettingsSection, theme_list};
use rex_linux::settings::ChromeTheme;
use serde_json::json;

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

fn rect(rt: &Runtime<Workspace>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
        .rect
}

fn kind(rt: &Runtime<Workspace>) -> Option<&PanelKind> {
    rt.state.panel.as_ref().map(|p| &p.kind)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.
}

fn type_text(rt: &mut Runtime<Workspace>, words: &str) {
    for c in words.chars() {
        press(rt, Key::Text(c.to_string()));
    }
}

/// A directory of Ghostty themes named `dark` and `light`, removed by the caller.
fn theme_dir(tag: &str, dark: &[&str], light: &[&str]) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("rex-linux-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for name in dark {
        std::fs::write(
            root.join(name),
            "background = #1e1e2e\nforeground = #cdd6f4\n",
        )
        .unwrap();
    }
    for name in light {
        std::fs::write(
            root.join(name),
            "background = #f2eede\nforeground = #222222\n",
        )
        .unwrap();
    }
    root
}

fn with_themes(ws: &mut Workspace, root: &std::path::Path) {
    ws.theme_dirs = ThemeDirs {
        user: Some(root.to_owned()),
        system: vec![],
    };
}

/// (label, group, the parts of the label that are marked).
fn found(ws: &Workspace) -> Vec<(String, String, Vec<String>)> {
    ws.entries()
        .into_iter()
        .map(|e| {
            let marks = e.marks.iter().map(|m| e.label[m.clone()].to_owned());
            (e.label.clone(), e.group.clone(), marks.collect())
        })
        .collect()
}

fn hit(label: &str, group: &str, marks: &[&str]) -> (String, String, Vec<String>) {
    (
        label.to_owned(),
        group.to_owned(),
        marks.iter().map(|m| (*m).to_owned()).collect(),
    )
}

fn query(ws: &mut Workspace, text: &str) {
    ws.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new(text);
    ws.query_changed();
}

#[test]
fn the_switcher_finds_names_without_their_accents_and_marks_what_matched() {
    let root = theme_dir(
        "switcher-search",
        &["Rose Pine", "Catppuccin Mocha", "Tokyo Night Moon"],
        &["Rose Pine Dawn"],
    );
    let mut ws = workspace();
    with_themes(&mut ws, &root);
    ws.perform(Intent::ChooseTheme, &mut rune::Tx::default());
    assert_eq!(
        ws.panel.as_ref().unwrap().fields[0].1,
        "Search terminal themes…"
    );
    assert!(
        ws.entries().iter().all(|e| e.marks.is_empty()),
        "nothing is marked until something is typed"
    );

    // The built-in Rosé Pine, which `rose` used to miss, beside Ghostty's Rose Pine.
    query(&mut ws, "rose");
    assert_eq!(
        found(&ws),
        [
            hit("Rosé Pine", "Built in", &["Rosé"]),
            hit("Rosé Pine Moon", "Built in", &["Rosé"]),
            hit("Rosé Pine Dawn", "Built in", &["Rosé"]),
            hit("Rose Pine", "Dark", &["Rose"]),
            hit("Rose Pine Dawn", "Light", &["Rose"]),
        ]
    );
    assert_eq!(ws.panel.as_ref().unwrap().selected, 0);
    // Typed with the accent, it finds the names without one too; case never matters.
    query(&mut ws, "ROSÉ PINE D");
    assert_eq!(
        found(&ws),
        [
            hit("Rosé Pine Dawn", "Built in", &["Rosé", "Pine", "D"]),
            hit("Rose Pine Dawn", "Light", &["Rose", "Pine", "D"]),
        ]
    );
    // Words in any order, each marked where it is; every word must be there.
    query(&mut ws, "moon rose");
    assert_eq!(
        found(&ws),
        [hit("Rosé Pine Moon", "Built in", &["Rosé", "Moon"])]
    );
    query(&mut ws, "moon");
    assert_eq!(
        found(&ws),
        [
            hit("Rosé Pine Moon", "Built in", &["Moon"]),
            hit("Tokyo Night Moon", "Dark", &["Moon"]),
        ]
    );
    // The switcher finds the middle of a word, and the choices that are not theme names.
    query(&mut ws, "ccin mo");
    assert_eq!(
        found(&ws),
        [
            hit("Catppuccin Mocha", "Built in", &["ccin", "Mo"]),
            hit("Catppuccin Mocha", "Dark", &["ccin", "Mo"]),
        ]
    );
    query(&mut ws, "follow");
    assert_eq!(
        found(&ws),
        [
            hit("Follow server", "Server", &["Follow"]),
            hit("Follow chrome", "Built in", &["Follow"]),
        ]
    );
    query(&mut ws, "rose zebra");
    assert!(found(&ws).is_empty());
    std::fs::remove_file(&ws.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

/// Sixty dark themes: more than the switcher shows at once.
fn many_themes(tag: &str) -> std::path::PathBuf {
    let names: Vec<String> = (0..60).map(|i| format!("Theme {i:02}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    theme_dir(tag, &names, &[])
}

#[test]
fn the_switcher_is_340_wide_at_most_420_tall_and_checks_the_theme_in_use_at_the_end() {
    let root = many_themes("switcher-size");
    for scale in [1., 1.3] {
        let mut ws = workspace();
        with_themes(&mut ws, &root);
        ws.settings.ui_scale = scale;
        ws.settings.terminal_theme = TerminalThemeChoice::Named("rex/midnight".into());
        let mut rt = runtime_at(ws, (1440., 1000.));
        rt.update(|s, tx| {
            assert!(crate::app::actions::run(
                s,
                commands::CHOOSE_THEME,
                &serde_json::Value::Null,
                tx
            ));
        });
        settle(&mut rt);
        assert!(matches!(kind(&rt), Some(PanelKind::Themes)));
        let surface = rect(&rt, "menu-surface");
        assert!(
            close(surface[2], 340. * scale),
            "at {scale}: 340 pt wide, not {surface:?}"
        );
        assert!(
            surface[3] <= 420. * scale + 0.5 && surface[3] > 380. * scale,
            "at {scale}: a full list is close to 420 pt tall and never more: {surface:?}"
        );
        // Centred near the top, as Commands is.
        assert!(close(surface[0] + surface[2] / 2., 720.), "{surface:?}");
        // The list scrolls to the rest, and says there is more.
        let results = rect(&rt, "results");
        assert!(results[3] > surface[3] * 2., "{results:?}");

        // One check, in the row of the theme in use, at the row's end; no row keeps a
        // column for it before its name, so every name starts on one line.
        let index = rt
            .state
            .entries()
            .iter()
            .position(|e| e.current)
            .expect("the theme in use is listed");
        assert_eq!(rt.state.entries()[index].label, "Midnight");
        let checks: Vec<_> = rt
            .elements()
            .into_iter()
            .filter(|e| !e.exiting && e.key.as_deref() == Some("check"))
            .collect();
        assert_eq!(checks.len(), 1);
        let row = rect(&rt, &format!("entry:{index}"));
        let check = checks[0].rect;
        assert!(
            checks[0].path.contains(&format!("entry:{index}")),
            "{}",
            checks[0].path
        );
        assert!(
            check[0] > row[0] + row[2] / 2. && check[0] + check[2] <= row[0] + row[2],
            "at {scale}: the check {check:?} ends the row {row:?}"
        );
        let label_x = |rt: &Runtime<Workspace>, i: usize| {
            let key = format!("entry:{i}");
            rt.elements()
                .into_iter()
                .find(|e| !e.exiting && e.text.is_some() && e.path.contains(&key))
                .unwrap_or_else(|| panic!("no text in {key}"))
                .rect[0]
        };
        assert!(
            close(label_x(&rt, index), label_x(&rt, index + 1)),
            "at {scale}: names start on one line"
        );

        // Escape closes it: it was not opened from Settings.
        press(&mut rt, Key::Escape);
        assert!(rt.state.panel.is_none());
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn the_switcher_under_its_settings_row_keeps_the_same_size() {
    let root = many_themes("switcher-anchored");
    let mut ws = workspace_hosts(2);
    with_themes(&mut ws, &root);
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Themes);
    let mut rt = runtime_at(ws, (1440., 1000.));
    settle(&mut rt);
    click(&mut rt, "terminal-theme");
    settle(&mut rt);
    let (control, surface) = (rect(&rt, "terminal-theme"), rect(&rt, "menu-surface"));
    assert!(close(surface[2], 340.), "{surface:?}");
    assert!(surface[3] <= 420.5, "{surface:?}");
    assert!(
        close(surface[0] + surface[2], control[0] + control[2])
            && close(surface[1], control[1] + control[3] + 4.),
        "under its control, trailing edges together: {surface:?} {control:?}"
    );
    // Choosing keeps Settings open.
    type_text(&mut rt, "theme 07");
    assert_eq!(
        found(&rt.state),
        [hit("Theme 07", "Dark", &["Theme", "07"])]
    );
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(matches!(kind(&rt), Some(PanelKind::Settings)));
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::Named("Theme 07".into())
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

fn ctrl_shift_p(rt: &mut Runtime<Workspace>) {
    ctrl_shift(rt, 'p');
}

#[test]
fn a_commands_search_lists_window_colours_and_themes_under_appearance() {
    let root = theme_dir(
        "commands-themes",
        &["Rose Pine", "Aurora Rose", "Primrose"],
        &["Rose Pine Dawn"],
    );
    let mut ws = workspace_hosts(2);
    with_themes(&mut ws, &root);
    ws.settings.terminal_theme = TerminalThemeChoice::Named("Rose Pine".into());
    let hosts = [capture_host(&mut ws, 0), capture_host(&mut ws, 1)];
    let mut rt = runtime(ws);
    ctrl_shift_p(&mut rt);
    assert!(matches!(kind(&rt), Some(PanelKind::Picker(false))));
    assert_eq!(
        rt.state.panel.as_ref().unwrap().fields[0].1,
        "Search commands, sessions, tabs and themes…"
    );
    // Until something is typed the group is its commands.
    let appearance: Vec<String> = rt
        .state
        .entries()
        .into_iter()
        .filter(|e| e.group == "Appearance")
        .map(|e| e.label)
        .collect();
    assert_eq!(
        appearance,
        [
            "Change theme…",
            "Change window colours…",
            "Use compact panes",
            "Increase interface zoom",
            "Decrease interface zoom",
            "Reset interface zoom",
            "Increase terminal font size",
            "Decrease terminal font size",
            "Reset terminal font size",
        ]
    );

    type_text(&mut rt, "rose");
    let rows: Vec<(String, String, String, bool)> = rt
        .state
        .entries()
        .into_iter()
        .map(|e| (e.label, e.detail, e.group, e.current))
        .collect();
    let row = |label: &str, detail: &str, current: bool| {
        (
            label.to_owned(),
            detail.to_owned(),
            theme_list::APPEARANCE.to_owned(),
            current,
        )
    };
    assert_eq!(
        rows,
        [
            row("Rosé Pine", "Window colours", false),
            row("Rosé Pine Moon", "Window colours", false),
            row("Rosé Pine", "Terminal theme", false),
            row("Rosé Pine Moon", "Terminal theme", false),
            row("Rosé Pine Dawn", "Terminal theme", false),
            // A word of the name starts with it: Primrose does not.
            row("Aurora Rose", "Terminal theme", false),
            row("Rose Pine", "Terminal theme", true),
            row("Rose Pine Dawn", "Terminal theme", false),
        ],
        "no command says rose; the palettes and themes named so are listed"
    );
    assert_eq!(
        found(&rt.state)[0],
        hit("Rosé Pine", "Appearance", &["Rosé"])
    );
    // The theme in use is checked at the end of its row, beside what the row is.
    settle(&mut rt);
    let checks: Vec<_> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.key.as_deref() == Some("check"))
        .collect();
    assert_eq!(checks.len(), 1);
    assert!(checks[0].path.contains("entry:6"), "{}", checks[0].path);

    // Return on a window colour draws the chrome in it and closes Commands.
    press(&mut rt, Key::Enter);
    assert!(rt.state.panel.is_none());
    assert_eq!(rt.state.settings.chrome_theme, ChromeTheme::RosePine);
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::Named("Rose Pine".into()),
        "a terminal theme someone chose is left alone"
    );
    for host in &hosts {
        assert!(host.try_iter().all(|c| !matches!(c, HostCommand::Theme(_))));
    }

    // A theme is declared to every host, as from the switcher.
    ctrl_shift_p(&mut rt);
    type_text(&mut rt, "pine dawn");
    assert_eq!(
        found(&rt.state),
        [
            hit("Rosé Pine Dawn", "Appearance", &["Pine", "Dawn"]),
            hit("Rose Pine Dawn", "Appearance", &["Pine", "Dawn"]),
        ]
    );
    // Ghostty's, the second row.
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Enter);
    assert!(rt.state.panel.is_none());
    assert_eq!(
        rt.state.settings.terminal_theme,
        TerminalThemeChoice::Named("Rose Pine Dawn".into())
    );
    let theme = rt.state.term_theme.clone().expect("a declared theme");
    assert_eq!(theme.background, Some(0xf2eede));
    for host in &hosts {
        let declared: Vec<_> = host
            .try_iter()
            .filter_map(|c| match c {
                HostCommand::Theme(args) => Some(args),
                _ => None,
            })
            .collect();
        assert_eq!(declared, [Some(theme.args())]);
    }
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_commands_search_names_themes_from_two_letters_and_never_the_follow_choices() {
    let root = theme_dir("commands-quiet", &["Relaxed", "Solarized Dark"], &[]);
    let mut ws = workspace();
    with_themes(&mut ws, &root);
    let appearance = |ws: &mut Workspace, text: &str| -> Vec<String> {
        ws.picker(false);
        query(ws, text);
        ws.entries()
            .into_iter()
            .filter(|e| matches!(e.action, Intent::Theme(_) | Intent::Chrome(_)))
            .map(|e| e.label)
            .collect()
    };
    assert!(appearance(&mut ws, "").is_empty());
    assert!(
        appearance(&mut ws, "r").is_empty(),
        "one letter is in most names"
    );
    assert_eq!(
        appearance(&mut ws, "re"),
        ["Relaxed"],
        "not Reset…, a command"
    );
    // Follow server, Rex default colours and Follow chrome are in the switcher only: a
    // search for a command about servers or defaults is not answered with a theme.
    for words in ["follow", "server", "default", "chrome", "rex"] {
        assert!(appearance(&mut ws, words).is_empty(), "{words}");
    }
    // Gruvbox Dark is the window colours, then the theme.
    assert_eq!(
        appearance(&mut ws, "gruvbox d"),
        ["Gruvbox Dark", "Gruvbox Dark"]
    );
    // The built-in Solarized Dark, then Ghostty's.
    assert_eq!(
        appearance(&mut ws, "sol dark"),
        ["Solarized Dark", "Solarized Dark"]
    );
    ws.picker(false);
    query(&mut ws, "gruv");
    let details: Vec<String> = ws.entries().into_iter().map(|e| e.detail).collect();
    assert_eq!(
        details,
        ["Window colours", "Terminal theme", "Terminal theme"],
        "the palette, then Gruvbox Dark and Gruvbox Light"
    );

    // A theme installed since is listed the next time Commands opens.
    std::fs::write(root.join("Renewed"), "background = #101010\n").unwrap();
    assert_eq!(appearance(&mut ws, "re"), ["Relaxed", "Renewed"]);
    std::fs::remove_file(&ws.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn change_theme_is_an_action_a_shortcut_can_open_and_a_remote_client_cannot() {
    let spec = crate::app::actions::find(commands::CHOOSE_THEME).expect("registered");
    assert_eq!(spec.title(), "Change theme");
    assert!(
        matches!(spec.remote(), crate::app::actions::Remote::Never(_)),
        "a popup is for the person at this computer"
    );
    let mut ws = workspace();
    ws.settings.shortcut_file.overrides = rex_linux::keymap::Overrides::parse(&json!([
        {"keys": "ctrl+alt+t", "action": "client.theme.choose"}
    ]));
    ws.local_keymap = crate::app::actions::resolve(&ws.settings.shortcut_file.overrides);
    ws.keymap_generation += 1;
    let mut rt = runtime(ws);
    rt.key(KeyEvent {
        key: Key::Text("t".into()),
        mods: Mods {
            ctrl: true,
            alt: true,
            ..Mods::default()
        },
    });
    settle(&mut rt);
    assert!(
        matches!(kind(&rt), Some(PanelKind::Themes)),
        "the key bound in shortcuts.json opens the switcher"
    );
    // Commands shows that key on its row.
    press(&mut rt, Key::Escape);
    rt.update(|s, _| s.picker(false));
    let row = rt
        .state
        .entries()
        .into_iter()
        .find(|e| e.label == "Change theme…")
        .expect("a Change theme… row");
    assert_eq!(row.detail, "Ctrl Alt T");
    assert_eq!(row.group, "Appearance");
    assert!(row.followup, "it opens the switcher");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Settings open on `section`, at rest.
fn settings_at(section: SettingsSection, scale: f32, size: (f32, f32)) -> Runtime<Workspace> {
    let mut ws = workspace_hosts(2);
    ws.settings.ui_scale = scale;
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(section);
    let mut rt = runtime_at(ws, size);
    settle(&mut rt);
    rt
}

/// Scroll the Terminal page to its foot, where the hosts' shells are.
fn foot_of_terminal(rt: &mut Runtime<Workspace>) {
    let page = rect(rt, "settings-scroll:terminal");
    rt.pointer_move(page[0] + 40., page[1] + 40.);
    rt.wheel(page[0] + 40., page[1] + 40., -8000.);
    settle(rt);
}

#[test]
fn a_rows_form_opens_over_settings_against_its_row() {
    for (scale, size) in [
        (1., (1440., 1000.)),
        (1.3, (1024., 700.)),
        (1., (640., 480.)),
    ] {
        let at = format!("at {scale} in {size:?}");
        let mut rt = settings_at(SettingsSection::Terminal, scale, size);
        foot_of_terminal(&mut rt);
        let page = rect(&rt, "settings-scroll:terminal");
        click(&mut rt, "host-shell:0");
        settle(&mut rt);
        assert!(matches!(
            kind(&rt),
            Some(PanelKind::Form(Form::HostShell(0)))
        ));
        // Settings stays where it was under the form, and takes no input.
        assert_eq!(rect(&rt, "settings-scroll:terminal"), page, "{at}");
        let dialog = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("dialog"))
            .unwrap();
        assert!(dialog.inert, "{at}: the page under the form is inert");
        let (row, form) = (rect(&rt, "host-shell:0"), rect(&rt, "form-popover"));
        let gap = 4. * scale;
        assert!(
            close(form[0] + form[2], row[0] + row[2]),
            "{at}: trailing edges {form:?} {row:?}"
        );
        // Under its row, or over it; in the smallest window, where neither side has the
        // room, against the window's bottom edge.
        assert!(
            close(form[1], row[1] + row[3] + gap)
                || close(form[1] + form[3], row[1] - gap)
                || (size.1 < 500. && close(form[1] + form[3], size.1 - 8.)),
            "{at}: the form {form:?} abuts its row {row:?}"
        );
        assert!(
            form[0] >= 0.
                && form[1] >= 0.
                && form[0] + form[2] <= size.0
                && form[1] + form[3] <= size.1,
            "{at}: the whole form is on screen: {form:?}"
        );
        // Nothing in it is cut by its edges.
        for e in rt
            .elements()
            .iter()
            .filter(|e| !e.exiting && e.path.contains("form-popover") && e.rect[2] > 0.)
        {
            assert!(
                e.rect[1] >= form[1] - 0.5 && e.rect[1] + e.rect[3] <= form[1] + form[3] + 0.5,
                "{at}: {:?} {:?} leaves the form {form:?}",
                e.key.as_deref().or(e.text.as_deref()),
                e.rect
            );
        }
        // Its first field has the keyboard.
        type_text(&mut rt, "X");
        assert!(
            rt.state.panel.as_ref().unwrap().inputs[0]
                .text
                .ends_with('X')
        );
        // Escape goes back to Settings, with the keyboard on the row again.
        press(&mut rt, Key::Escape);
        settle(&mut rt);
        assert!(matches!(kind(&rt), Some(PanelKind::Settings)), "{at}");
        assert!(
            focused(&rt).is_some_and(|path| path.ends_with("host-shell:0")),
            "{at}: focus returns to the row, not {:?}",
            focused(&rt)
        );
        assert_eq!(rt.state.settings.hosts[0].shell, None, "not saved");

        // Return opens it again; a press outside it goes back to Settings too.
        press(&mut rt, Key::Enter);
        settle(&mut rt);
        assert!(matches!(
            kind(&rt),
            Some(PanelKind::Form(Form::HostShell(0)))
        ));
        let dialog = rect(&rt, "dialog");
        rt.pointer_down(dialog[0] + 40., dialog[1] + dialog[3] - 20.);
        rt.pointer_up(dialog[0] + 40., dialog[1] + dialog[3] - 20.);
        settle(&mut rt);
        assert!(
            matches!(kind(&rt), Some(PanelKind::Settings)),
            "{at}: a press outside the form returns to Settings instead of closing it"
        );

        // Saving from it keeps Settings open.
        click(&mut rt, "host-shell:0");
        settle(&mut rt);
        rt.update(|s, _| {
            s.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("fish");
        });
        click(&mut rt, "save-form");
        settle(&mut rt);
        assert!(matches!(kind(&rt), Some(PanelKind::Settings)), "{at}");
        assert_eq!(rt.state.settings.hosts[0].shell.as_deref(), Some("fish"));
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

#[test]
fn a_hosts_shell_form_opens_against_its_row_and_shows_its_error_there() {
    let mut rt = settings_at(SettingsSection::Terminal, 1., (1024., 700.));
    foot_of_terminal(&mut rt);
    click(&mut rt, "host-shell:1");
    settle(&mut rt);
    assert!(matches!(
        kind(&rt),
        Some(PanelKind::Form(Form::HostShell(1)))
    ));
    let (row, form) = (rect(&rt, "host-shell:1"), rect(&rt, "form-popover"));
    assert!(
        close(form[0] + form[2], row[0] + row[2]),
        "{form:?} {row:?}"
    );
    assert!(
        close(form[1], row[1] + row[3] + 4.) || close(form[1] + form[3], row[1] - 4.),
        "the form {form:?} abuts its row {row:?}"
    );
    assert!(form[1] >= 0. && form[1] + form[3] <= 700., "{form:?}");
    type_text(&mut rt, "zsh -l");
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(
        has(&rt, "form-error"),
        "arguments are refused in the form, which stays"
    );
    let grown = rect(&rt, "form-popover");
    assert!(
        grown[1] >= 0. && grown[1] + grown[3] <= 700.,
        "still on screen with its error: {grown:?}"
    );
    assert!(
        close(grown[1], form[1]) || close(grown[1] + grown[3], form[1] + form[3]),
        "and against the same side of its row: {form:?} then {grown:?}"
    );
    rt.update(|s, _| s.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new("fish"));
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(matches!(kind(&rt), Some(PanelKind::Settings)));
    assert_eq!(rt.state.settings.hosts[1].shell.as_deref(), Some("fish"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_form_opened_away_from_settings_is_still_a_dialog() {
    // The host menu's Default shell… has no row to open against.
    let mut ws = workspace_hosts(2);
    ws.perform(Intent::HostShell(0), &mut rune::Tx::default());
    let mut rt = runtime(ws);
    settle(&mut rt);
    assert!(!has(&rt, "form-popover"));
    let dialog = rect(&rt, "dialog");
    assert!(
        close(dialog[0] + dialog[2] / 2., 512.),
        "centred: {dialog:?}"
    );
    // Nor do Settings forms that are not a row's own: the client label is one.
    let mut rt = settings_at(SettingsSection::Hosts, 1., (1024., 700.));
    rt.update(|s, tx| s.open_anchored("client-label", Intent::ClientLabel, tx));
    settle(&mut rt);
    assert!(matches!(
        kind(&rt),
        Some(PanelKind::Form(Form::ClientLabel))
    ));
    assert!(!has(&rt, "form-popover"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// The switcher, the Commands search and the forms as drawn (see `Camera` in
/// `pane_verb_tests.rs`).
#[test]
fn the_switcher_commands_and_forms_draw() {
    use pane_verb_tests::Camera;
    let mut names: Vec<String> = [
        "Aurora Rose",
        "Catppuccin Mocha",
        "Dracula",
        "Everforest Dark Hard",
        "Kanagawa Wave",
        "Nord",
        "Rose Pine",
        "Tokyo Night Moon",
    ]
    .map(str::to_owned)
    .into();
    names.extend((0..30).map(|i| format!("Zenburn {i:02}")));
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let root = theme_dir(
        "switcher-draw",
        &names,
        &["Rose Pine Dawn", "Solarized Light"],
    );
    let mut camera = Camera::new();
    let mut snap = |rt: &mut Runtime<Workspace>, name: &str| {
        if let Some(camera) = &mut camera {
            camera.snap(rt, name);
        }
    };
    let open = |section: Option<SettingsSection>| {
        let mut ws = workspace_hosts(2);
        with_themes(&mut ws, &root);
        ws.settings.terminal_theme = TerminalThemeChoice::Named("rex/rose-pine".into());
        if let Some(section) = section {
            ws.perform(Intent::Settings, &mut rune::Tx::default());
            ws.open_section(section);
        }
        let mut rt = Runtime::new(ws, view, (1024., 700.), Env::new(true, 2.));
        rt.on_key = Some(crate::app::input::on_key);
        settle(&mut rt);
        rt
    };

    let mut rt = open(None);
    rt.update(|s, tx| s.perform(Intent::ChooseTheme, tx));
    settle(&mut rt);
    snap(&mut rt, "switcher");
    type_text(&mut rt, "rose");
    settle(&mut rt);
    snap(&mut rt, "switcher-rose");
    press(&mut rt, Key::Down);
    settle(&mut rt);
    snap(&mut rt, "switcher-rose-second");
    press(&mut rt, Key::Escape);
    ctrl_shift_p(&mut rt);
    type_text(&mut rt, "rose");
    settle(&mut rt);
    snap(&mut rt, "commands-rose");
    press(&mut rt, Key::Escape);
    ctrl_shift_p(&mut rt);
    type_text(&mut rt, "appearance");
    settle(&mut rt);
    snap(&mut rt, "commands-appearance");
    press(&mut rt, Key::Escape);
    ctrl_shift_p(&mut rt);
    let row = rt
        .state
        .entries()
        .iter()
        .position(|e| e.label == "Reset terminal font size")
        .unwrap();
    for _ in 0..row {
        if rt.state.panel.as_ref().unwrap().selected >= row {
            break;
        }
        press(&mut rt, Key::Down);
        // The app's poll hook reveals the selection; the headless runtime has no hooks.
        rt.update(|s, tx| {
            crate::app::poll(s, 0., tx);
        });
    }
    settle(&mut rt);
    snap(&mut rt, "commands-groups");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut rt = open(Some(SettingsSection::Themes));
    click(&mut rt, "terminal-theme");
    settle(&mut rt);
    snap(&mut rt, "switcher-over-settings");
    type_text(&mut rt, "pine");
    settle(&mut rt);
    snap(&mut rt, "switcher-over-settings-pine");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut rt = open(Some(SettingsSection::Terminal));
    foot_of_terminal(&mut rt);
    click(&mut rt, "host-shell:0");
    settle(&mut rt);
    snap(&mut rt, "shell-form");
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    click(&mut rt, "host-shell:1");
    settle(&mut rt);
    type_text(&mut rt, "zsh -l");
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    snap(&mut rt, "shell-form-error");
    std::fs::remove_file(&rt.state.settings_path).ok();

    // The smallest window: the form over part of its row, whole.
    let mut ws = workspace_hosts(2);
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Terminal);
    let mut rt = Runtime::new(ws, view, (640., 480.), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    foot_of_terminal(&mut rt);
    click(&mut rt, "host-shell:0");
    settle(&mut rt);
    snap(&mut rt, "shell-form-smallest");
    std::fs::remove_file(&rt.state.settings_path).ok();
    std::fs::remove_dir_all(root).ok();
}
