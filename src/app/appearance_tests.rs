use super::*;
use crate::app::{appearance::ChromeSlot, panel::SettingsSection, theme::palette};
use rex_rune::{
    platform::dirs::themes_dir_for, settings::ChromeTheme, terminal_theme::TerminalTheme,
};
use serde_json::{Value, json, to_value};
use std::fs;

fn theme_messages(rx: &flume::Receiver<HostCommand>) -> Vec<Value> {
    rx.try_iter()
        .filter_map(|command| match command {
            HostCommand::Theme(Some(args)) => Some(to_value(args).unwrap()),
            _ => None,
        })
        .collect()
}

fn open_chromes(rt: &mut Runtime<Workspace>, slot: ChromeSlot) {
    rt.update(|s, tx| s.perform(Intent::ChooseChromeSlot(slot), tx));
    assert!(
        matches!(rt.state.panel.as_ref().unwrap().kind, PanelKind::Chromes(found) if found == slot)
    );
}

fn choose_chrome(rt: &mut Runtime<Workspace>, label: &str) {
    let row = rt
        .state
        .entries()
        .iter()
        .position(|e| e.label == label)
        .unwrap();
    rt.update(|s, tx| s.perform(s.entries()[row].action.clone(), tx));
}

fn checked(rt: &Runtime<Workspace>) -> String {
    rt.state
        .entries()
        .into_iter()
        .find(|e| e.current)
        .unwrap()
        .label
}

#[test]
fn chrome_picker_refreshes_both_file_sources_and_follows_their_terminal_colours() {
    let mut ws = workspace();
    let rx = capture_host(&mut ws, 0);
    // The directory follows the actual settings file, not the running user's settings.
    ws.settings_path.set_file_name("personal.json");
    let dir = themes_dir_for(&ws.settings_path);
    let ghostty = ws.settings_path.with_extension("ghostty");
    fs::create_dir(&dir).unwrap();
    fs::create_dir(&ghostty).unwrap();
    ws.theme_dirs = ThemeDirs {
        user: Some(ghostty.clone()),
        system: vec![],
    };
    ws.settings.terminal_theme = TerminalThemeChoice::FollowChrome;
    ws.term_theme = Some(TerminalTheme::midnight());
    let mut rt = runtime(ws);
    open_chromes(&mut rt, ChromeSlot::Manual);
    assert_eq!(rt.state.entries().len(), 50);

    let custom_file = dir.join("quiet.toml");
    fs::write(&custom_file, "name = 'Quiet'\nground = '#182433'\nink = '#efefef'\naccent = '#88cfff'\n[terminal]\ncursor = '#33accf'\n").unwrap();
    let ghostty_file = ghostty.join("Paper Moth");
    fs::write(
        &ghostty_file,
        "background = #f2efe8\nforeground = #233047\npalette = 4=#3456a3\n",
    )
    .unwrap();
    open_chromes(&mut rt, ChromeSlot::Manual);
    let rows = rt.state.entries();
    assert_eq!(rows.len(), 52);
    assert_eq!(
        (
            &*rows[0].group,
            &*rows[1].group,
            &*rows[36].group,
            &*rows[51].group
        ),
        ("Custom", "Dark", "Light", "Ghostty")
    );
    assert_eq!(rows[0].label, "Quiet");
    assert_eq!(rows[51].label, "Paper Moth");
    for (query, expected) in [
        ("custom dark", "Quiet"),
        ("ghostty light", "Paper Moth"),
        ("custom:quiet", "Quiet"),
    ] {
        rt.update(|s, _| s.panel.as_mut().unwrap().inputs[0] = rune::TextEdit::new(query));
        assert_eq!(
            rt.state
                .entries()
                .iter()
                .map(|e| e.label.as_str())
                .collect::<Vec<_>>(),
            [expected]
        );
    }
    open_chromes(&mut rt, ChromeSlot::Manual);
    choose_chrome(&mut rt, "Quiet");
    rt.frame(1. / 60.);
    assert_eq!(rt.state.settings.chrome_theme.id(), "custom:quiet");
    assert_eq!(palette().ground, 0x182433);
    assert_eq!(palette().kit().c.page, crate::app::theme::rgb(0x182433));
    assert_eq!(
        theme_messages(&rx),
        [json!({"background":"#182433","foreground":"#efefef","cursor":"#33accf","scheme":"dark"})]
    );

    open_chromes(&mut rt, ChromeSlot::Manual);
    assert_eq!(checked(&rt), "Quiet");
    choose_chrome(&mut rt, "Paper Moth");
    rt.frame(1. / 60.);
    assert_eq!(rt.state.settings.chrome_theme.id(), "ghostty:Paper Moth");
    assert_eq!(palette().ground, 0xf2efe8);
    assert_eq!(
        theme_messages(&rx),
        [json!({"background":"#f2efe8","foreground":"#233047","palette":["4=#3456a3"]})]
    );

    fs::remove_file(ghostty_file).unwrap();
    open_chromes(&mut rt, ChromeSlot::Manual);
    assert_eq!(checked(&rt), "Midnight");
    assert_eq!(rt.state.chrome.palette.ground, 0x001528);
    assert_eq!(
        theme_messages(&rx),
        [json!({"background":"#001528","scheme":"dark"})]
    );
    assert_eq!(
        Settings::load(&rt.state.settings_path)
            .unwrap()
            .chrome_theme
            .id(),
        "ghostty:Paper Moth"
    );
}

#[test]
fn system_appearance_switches_palette_and_pair_in_the_same_frame() {
    let mut ws = workspace();
    ws.theme_dirs = ThemeDirs {
        user: None,
        system: vec![],
    };
    let rx = capture_host(&mut ws, 0);
    ws.set_chrome(
        ChromeTheme::from_id("merino-light"),
        &mut rune::Tx::default(),
    );
    ws.set_chrome(
        ChromeTheme::from_id("catppuccin_latte"),
        &mut rune::Tx::default(),
    );
    theme_messages(&rx);
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = runtime_at(ws, (640., 480.));
    rt.poll = Some(crate::app::poll);
    rt.set_system_dark(false);
    rt.frame(1. / 60.);
    assert!(theme_messages(&rx).is_empty());
    assert!(!palette().dark, "manual theme stays unchanged");
    for _ in 0..40 {
        rt.frame(1. / 60.);
    }
    click(&mut rt, "match-system-appearance");
    assert!(rt.state.settings.match_system_appearance);
    assert_eq!(
        rt.state.settings.light_chrome_theme.id(),
        "catppuccin_latte"
    );
    assert_eq!(rt.state.settings.dark_chrome_theme, ChromeTheme::Midnight);
    assert!(theme_messages(&rx).is_empty());

    open_chromes(&mut rt, ChromeSlot::Dark);
    assert_eq!(checked(&rt), "Midnight");
    choose_chrome(&mut rt, "S2Code Dark");
    assert_eq!(rt.state.settings.dark_chrome_theme.id(), "s2-dark");
    assert_eq!(rt.state.settings.chrome_theme.id(), "catppuccin_latte");
    assert!(!rt.state.chrome.palette.dark);
    assert!(
        theme_messages(&rx).is_empty(),
        "editing the inactive slot sends no declaration"
    );

    rt.set_system_dark(true);
    rt.frame(1. / 60.);
    assert_eq!(palette().ground, 0x0c1119);
    assert_eq!(palette().kit().c.page, crate::app::theme::rgb(0x0c1119));
    let commands = theme_messages(&rx);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0]["background"], "#0c1119");
    assert_eq!(commands[0]["scheme"], "dark");

    rt.set_system_dark(false);
    rt.frame(1. / 60.);
    assert!(!palette().dark);
    assert_eq!(theme_messages(&rx)[0]["scheme"], "light");
    open_chromes(&mut rt, ChromeSlot::Light);
    assert_eq!(checked(&rt), "Catppuccin Latte");
    rt.update(|s, tx| s.toggle_system_appearance(tx));
    rt.set_system_dark(true);
    rt.frame(1. / 60.);
    assert!(!palette().dark, "turning it off restores the manual theme");
    assert!(theme_messages(&rx).is_empty());
    let saved = Settings::load(&rt.state.settings_path).unwrap();
    assert!(saved.appearance_slots_seeded);
    assert!(!saved.match_system_appearance);
    assert_eq!(saved.dark_chrome_theme.id(), "s2-dark");
    assert_eq!(saved.light_chrome_theme.id(), "catppuccin_latte");
}

#[test]
fn appearance_settings_are_searchable_even_when_a_slot_is_hidden() {
    let mut ws = workspace();
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = runtime(ws);
    for label in [
        "Match system appearance",
        "Light theme",
        "Dark theme",
        "Custom themes",
    ] {
        rt.update(|s, tx| {
            s.settings_nav.search = rune::TextEdit::new(label);
            s.settings_search_changed();
            let first = &s.settings_nav.hits()[0];
            assert_eq!(first.label, label);
            assert_eq!(
                first.page,
                crate::app::settings_nav::Page::Section(SettingsSection::Interface)
            );
            s.open_settings_hit(0, tx);
        });
        rt.frame(1. / 60.);
        assert!(
            has(&rt, "settings-found"),
            "{label} should land on a visible row"
        );
    }
}

#[test]
fn chrome_appearance_draws_at_normal_and_minimum_size() {
    if std::env::var_os("REX_TEST_SNAP_DIR").is_none() {
        return;
    }
    for (name, size) in [("normal", (1024., 700.)), ("minimum", (640., 480.))] {
        let mut ws = workspace();
        let dir = themes_dir_for(&ws.settings_path);
        let ghostty = ws.settings_path.with_extension("ghostty");
        fs::create_dir(&dir).unwrap();
        fs::create_dir(&ghostty).unwrap();
        fs::write(dir.join("quiet.toml"), "name = 'Quiet Evening over the Pacific Northwest'\nground = '#142133'\naccent = '#91d4ff'\n").unwrap();
        fs::write(
            ghostty.join("Paper Moth"),
            "background = #f2efe8\nforeground = #233047\npalette = 4=#3456a3\n",
        )
        .unwrap();
        ws.theme_dirs = ThemeDirs {
            user: Some(ghostty),
            system: vec![],
        };
        ws.chrome.library = rex_rune::chrome_theme::ThemeLibrary::scan(&dir, &ws.theme_dirs);
        ws.settings.chrome_theme = ChromeTheme::from_id("custom:quiet");
        ws.sync_chrome(&mut rune::Tx::default());
        ws.perform(Intent::Settings, &mut rune::Tx::default());
        let mut rt = crate::app::fonts::test_runtime(ws, view, size, Env::new(true, 2.));
        let mut camera = super::pane_verb_tests::Camera::new().unwrap();
        let settle = |rt: &mut Runtime<Workspace>| {
            for _ in 0..60 {
                rt.frame(1. / 60.);
            }
        };
        settle(&mut rt);
        camera.snap(&mut rt, &format!("chrome-manual-{name}"));
        click(&mut rt, "match-system-appearance");
        settle(&mut rt);
        camera.snap(&mut rt, &format!("chrome-auto-{name}"));
        click(&mut rt, "dark-chrome-theme");
        settle(&mut rt);
        assert!(matches!(
            rt.state.panel.as_ref().unwrap().kind,
            PanelKind::Chromes(ChromeSlot::Dark)
        ));
        camera.snap(&mut rt, &format!("chrome-picker-{name}"));
    }
}
