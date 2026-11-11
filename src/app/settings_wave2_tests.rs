//! What the Settings work of several lanes shares: one way to open and leave a page of a
//! section's, one search over every page's rows, and forms placed against their rows on a
//! scrolled page.
use super::*;
use crate::app::{
    panel::SettingsSection,
    settings_nav::{Page, SubPage},
};

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

fn settings(size: (f32, f32)) -> Runtime<Workspace> {
    let mut ws = workspace_hosts(2);
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = runtime_at(ws, size);
    settle(&mut rt);
    rt
}

fn rect(rt: &Runtime<Workspace>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
        .rect
}

fn focus_is(rt: &Runtime<Workspace>, key: &str) -> bool {
    focused(rt).is_some_and(|path| path.ends_with(key))
}

fn on_settings(rt: &Runtime<Workspace>) -> bool {
    matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    )
}

#[test]
fn every_page_of_a_section_opens_and_leaves_the_same_way() {
    // In a window shorter than the dialog, so the rows that open the pages start under the
    // fold of their sections.
    for sub in SubPage::ALL {
        let (section, opener) = (Page::Section(sub.section()), sub.opener());
        let mut rt = settings((1100., 560.));
        rt.update(|s, _| s.navigate(section));
        settle(&mut rt);
        rt.update(|_, tx| {
            tx.scroll_into_view(opener.clone());
            tx.focus(opener.clone(), true);
        });
        settle(&mut rt);
        assert!(focus_is(&rt, &opener), "{sub:?}: {:?}", focused(&rt));
        let scroll = format!("settings-scroll:{}", section.key());
        let row = rect(&rt, &opener);
        let page = rect(&rt, &scroll);
        assert!(
            row[1] >= page[1] && row[1] + row[3] <= page[1] + page[3],
            "{sub:?}: the focused row {row:?} is in view of {page:?}"
        );

        // Return on the row opens its page, with the keyboard on the way back.
        press(&mut rt, Key::Enter);
        settle(&mut rt);
        assert_eq!(rt.state.settings_page(), Page::Sub(sub));
        assert_eq!(
            rt.state.settings_section,
            sub.section(),
            "the section stays selected"
        );
        assert!(
            focus_is(&rt, "settings-back"),
            "{sub:?}: {:?}",
            focused(&rt)
        );

        // Escape goes back to the section, where it was scrolled, with the keyboard on the
        // row again; Settings stays open.
        press(&mut rt, Key::Escape);
        settle(&mut rt);
        assert!(on_settings(&rt), "{sub:?}: Escape left Settings open");
        assert_eq!(rt.state.settings_page(), section);
        assert!(focus_is(&rt, &opener), "{sub:?}: {:?}", focused(&rt));
        let back = rect(&rt, &opener);
        assert!(
            (back[1] - row[1]).abs() < 1.,
            "{sub:?}: the row is where it was, {row:?} then {back:?}"
        );

        // Back, pressed with Return, does the same; so does the crumb.
        press(&mut rt, Key::Enter);
        settle(&mut rt);
        assert!(focus_is(&rt, "settings-back"));
        press(&mut rt, Key::Enter);
        settle(&mut rt);
        assert_eq!(rt.state.settings_page(), section);
        assert!(focus_is(&rt, &opener), "{sub:?}: {:?}", focused(&rt));
        press(&mut rt, Key::Enter);
        settle(&mut rt);
        click(&mut rt, "settings-crumb");
        settle(&mut rt);
        assert_eq!(rt.state.settings_page(), section);
        let back = rect(&rt, &opener);
        assert!(
            (back[1] - row[1]).abs() < 1.,
            "{sub:?}: the crumb returns there too, {row:?} then {back:?}"
        );

        // From a section's own page Escape closes Settings.
        rt.update(|_, tx| tx.blur());
        press(&mut rt, Key::Escape);
        settle(&mut rt);
        assert!(rt.state.panel.is_none(), "{sub:?}");
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

#[test]
fn a_page_reached_by_a_search_leaves_to_its_section() {
    let mut rt = settings((1440., 1000.));
    rt.update(|s, _| s.navigate(Page::Section(SettingsSection::About)));
    rt.update(|s, _| s.navigate(Page::Sub(SubPage::AdvancedFont)));
    settle(&mut rt);
    // Back would show About; Escape goes up to Terminal instead.
    rt.update(|s, tx| assert!(s.leave_settings_page(tx)));
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Terminal)
    );
    rt.update(|s, tx| assert!(!s.leave_settings_page(tx)));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn search_finds_the_rows_of_every_lanes_pages() {
    let mut rt = settings((1440., 1000.));
    click(&mut rt, "settings-search");
    for (query, label, place) in [
        ("terminal font", "Terminal font", "Terminal"),
        ("font style", "Font style", "Terminal"),
        ("cursor style", "Cursor style", "Terminal"),
        ("copy on", "Copy on selection", "Terminal"),
        ("generated", "Prefer generated titles", "Terminal"),
        (
            "line height",
            "Line height",
            "Terminal › Advanced font settings",
        ),
        (
            "fallback",
            "Fallback fonts",
            "Terminal › Advanced font settings",
        ),
        ("toolbar", "Toolbar", "Interface"),
        ("animation", "Animation style", "Interface"),
        ("launch", "Show window on launch", "Interface"),
        ("horizontal", "Horizontal padding", "Interface › Panes"),
        ("follows mouse", "Focus follows mouse", "Interface › Panes"),
        ("fill", "Tabs fill the toolbar", "Interface › Tabs"),
        ("light and dark", "Light and dark themes", "Themes"),
    ] {
        rt.update(|s, _| {
            s.settings_nav.search = rune::TextEdit::new(query);
            s.settings_search_changed();
        });
        let hits: Vec<(String, String)> = rt
            .state
            .settings_nav
            .hits()
            .iter()
            .map(|hit| (hit.label.clone(), hit.place.clone()))
            .collect();
        assert!(
            hits.contains(&(label.to_owned(), place.to_owned())),
            "{query} finds {label} on {place}: {hits:?}"
        );
    }
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn terminals_theme_row_names_the_theme_and_opens_themes() {
    let mut rt = settings((1440., 1000.));
    rt.update(|s, _| s.navigate(Page::Section(SettingsSection::Terminal)));
    settle(&mut rt);
    assert!(
        element_text(&rt, "open-themes")
            .iter()
            .any(|words| words == "Follow server"),
        "{:?}",
        element_text(&rt, "open-themes")
    );
    // The one control that chooses a terminal theme is on Themes.
    assert!(
        !rt.elements()
            .iter()
            .any(|e| e.key.as_deref() == Some("terminal-theme"))
    );
    click(&mut rt, "open-themes");
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Themes)
    );
    assert!(
        rt.elements()
            .iter()
            .any(|e| !e.exiting && e.key.as_deref() == Some("terminal-theme"))
    );
    // Back returns to Terminal.
    click(&mut rt, "settings-back");
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Terminal)
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Where the lanes' work meets, drawn for a look (see `Camera`); nothing is asserted.
#[test]
fn the_wave_two_surfaces_draw() {
    use super::pane_verb_tests::{Camera, framed_terminal};
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let scene = |prepare: &dyn Fn(&mut Workspace)| {
        let (mut ws, _host) = tabbed_workspace("split");
        let (sender, _sent) = flume::unbounded();
        framed_terminal(
            &mut ws,
            "block:b",
            &sender,
            b"$ cargo test\r\nrunning 12 tests\r\ntest result: ok",
        );
        framed_terminal(&mut ws, "block:c", &sender, b"$ tail -f server.log");
        prepare(&mut ws);
        let mut rt = Runtime::new(ws, view, (1024., 700.), Env::new(true, 2.));
        rt.on_key = Some(crate::app::input::on_key);
        settle(&mut rt);
        rt
    };
    let done = |rt: Runtime<Workspace>| {
        std::fs::remove_file(&rt.state.settings_path).ok();
    };
    let open = |page: Page| {
        scene(&move |ws| {
            ws.perform(Intent::Settings, &mut rune::Tx::default());
            ws.navigate(page);
        })
    };

    let mut rt = open(Page::Sub(SubPage::Tabs));
    camera.snap(&mut rt, "w2-tabs-page");
    done(rt);

    let mut rt = open(Page::Section(SettingsSection::Interface));
    click(&mut rt, "settings-search");
    for c in "padding".chars() {
        press(&mut rt, Key::Text(c.to_string()));
    }
    settle(&mut rt);
    camera.snap(&mut rt, "w2-search-padding");
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    camera.snap(&mut rt, "w2-search-padding-found");
    done(rt);

    let mut rt = open(Page::Section(SettingsSection::Shortcuts));
    rt.update(|s, _| s.shortcut_filter = rune::TextEdit::new("toolbar"));
    settle(&mut rt);
    camera.snap(&mut rt, "w2-shortcuts-toolbar");
    done(rt);

    let mut rt = open(Page::Section(SettingsSection::Themes));
    camera.snap(&mut rt, "w2-themes");
    done(rt);

    let mut rt = open(Page::Section(SettingsSection::Interface));
    let page = rect(&rt, "settings-scroll:interface");
    rt.pointer_move(page[0] + 40., page[1] + 40.);
    rt.wheel(page[0] + 40., page[1] + 40., -8000.);
    settle(&mut rt);
    camera.snap(&mut rt, "w2-interface-foot");
    done(rt);
}

#[test]
fn a_recording_takes_alt_left_before_settings_history_does() {
    let mut rt = settings((1440., 1000.));
    rt.update(|s, _| s.navigate(Page::Section(SettingsSection::Shortcuts)));
    settle(&mut rt);
    assert!(rt.state.settings_can_go(-1), "Back would leave the page");
    rt.update(|_, tx| tx.scroll_into_view("shortcut-key:pane.zoom:0"));
    settle(&mut rt);
    click(&mut rt, "shortcut-key:pane.zoom:0");
    settle(&mut rt);
    assert!(rt.state.recorder.is_some(), "the row is recording");
    rt.key(KeyEvent {
        key: Key::Left,
        mods: Mods {
            alt: true,
            ..Mods::default()
        },
    });
    settle(&mut rt);
    // The key is the recording's, not a step back through Settings.
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Shortcuts)
    );
    let typed: Vec<String> = rt
        .state
        .recorder
        .as_ref()
        .expect("still recording")
        .chords
        .iter()
        .map(rex_linux::keymap::Chord::spelling)
        .collect();
    assert_eq!(typed, ["alt+left"]);
    // Escape ends the recording and leaves Settings open on the page.
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    assert!(rt.state.recorder.is_none());
    assert!(on_settings(&rt));
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Shortcuts)
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}
