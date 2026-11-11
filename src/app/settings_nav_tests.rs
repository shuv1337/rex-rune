//! Settings navigation: the sidebar and its search, sub-pages, and Back and Forward.
use super::*;
use crate::app::{
    panel::SettingsSection,
    settings_nav::{Page, Status, SubPage},
};

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

/// Settings as the toolbar's button opens it, at rest.
fn settings(hosts: usize, size: (f32, f32)) -> Runtime<Workspace> {
    let mut ws = workspace_hosts(hosts);
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

fn type_text(rt: &mut Runtime<Workspace>, words: &str) {
    for c in words.chars() {
        press(rt, Key::Text(c.to_string()));
    }
}

fn alt(rt: &mut Runtime<Workspace>, key: Key) {
    rt.key(KeyEvent {
        key,
        mods: Mods {
            alt: true,
            ..Mods::default()
        },
    });
    for _ in 0..4 {
        rt.frame(1. / 60.);
    }
}

fn hits(rt: &Runtime<Workspace>) -> Vec<(String, String)> {
    rt.state
        .settings_nav
        .hits()
        .iter()
        .map(|hit| (hit.label.clone(), hit.place.clone()))
        .collect()
}

fn title(rt: &Runtime<Workspace>) -> String {
    element_text(rt, "settings-title").join("")
}

/// Whether the header's Back or Forward can be pressed.
fn enabled(rt: &Runtime<Workspace>, key: &str) -> bool {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
        .focusable
}

#[test]
fn the_sidebar_lists_the_sections_in_groups_behind_their_tiles() {
    let rt = settings(1, (1440., 1000.));
    let elements = rt.elements();
    let rows: Vec<_> = elements
        .iter()
        .filter(|e| {
            !e.exiting
                && e.key
                    .as_deref()
                    .is_some_and(|k| k.starts_with("settings-section:"))
        })
        .collect();
    let keys: Vec<&str> = rows
        .iter()
        .map(|e| {
            e.key
                .as_deref()
                .unwrap()
                .trim_start_matches("settings-section:")
        })
        .collect();
    assert_eq!(
        keys,
        [
            "interface",
            "themes",
            "terminal",
            "shortcuts",
            "integrations",
            "notifications",
            "hosts",
            "updates",
            "about"
        ]
    );
    // Three groups: a gap before Terminal and before Updates, and nowhere else.
    let gaps: Vec<f32> = rows
        .windows(2)
        .map(|pair| pair[1].rect[1] - (pair[0].rect[1] + pair[0].rect[3]))
        .collect();
    for (index, gap) in gaps.iter().enumerate() {
        let wide = matches!(keys[index + 1], "terminal" | "updates");
        assert_eq!(
            *gap > 10.,
            wide,
            "the gap before {}: {gap}",
            keys[index + 1]
        );
    }
    // Every row is a 22 pt tile and a label, on one left edge, inside the sidebar.
    let sidebar = rect(&rt, "settings-sidebar");
    let search = rect(&rt, "settings-search-box");
    for row in &rows {
        assert!(
            (row.rect[0] - search[0]).abs() < 0.5 && (row.rect[2] - search[2]).abs() < 0.5,
            "{:?} lines up with the search field {search:?}",
            row.rect
        );
        assert!(row.rect[0] + row.rect[2] <= sidebar[0] + sidebar[2]);
        assert!((row.rect[3] - 32.).abs() < 0.5);
    }
    let tiles = elements
        .iter()
        .filter(|e| {
            !e.exiting
                && e.path.contains("settings-sections")
                && (e.rect[2] - 22.).abs() < 0.5
                && (e.rect[3] - 22.).abs() < 0.5
        })
        .count();
    assert_eq!(tiles, rows.len(), "a tile a section");
    // The search field's band is the page header's: both are centred on one line.
    let header = rect(&rt, "settings-header");
    assert!(
        ((search[1] + search[3] / 2.) - (header[1] + header[3] / 2.)).abs() < 0.5,
        "{search:?} and {header:?}"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn settings_opens_on_its_section_list_and_a_typed_letter_starts_a_search() {
    let mut rt = settings(1, (1440., 1000.));
    assert!(
        focused(&rt).is_some_and(|path| path.ends_with("settings-section:interface")),
        "focus starts on the section list, not {:?}",
        focused(&rt)
    );
    type_text(&mut rt, "fo");
    assert_eq!(rt.state.settings_nav.search.text, "fo");
    assert!(
        focused(&rt).is_some_and(|path| path.contains("settings-search")),
        "the field has the keyboard: {:?}",
        focused(&rt)
    );
    assert!(hits(&rt).iter().any(|(label, _)| label == "Font size"));
    assert!(
        !has(&rt, "settings-section:interface"),
        "the hits take the list's place"
    );
    // Escape ends the search first, and closes Settings only once there is none.
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.settings_nav.search.text, "");
    assert!(has(&rt, "settings-section:interface"));
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    // From the empty field, Down goes back to the section list.
    press(&mut rt, Key::Down);
    assert!(
        focused(&rt).is_some_and(|path| path.ends_with("settings-section:interface")),
        "{:?}",
        focused(&rt)
    );
    press(&mut rt, Key::Escape);
    assert!(rt.state.panel.is_none(), "Escape with no search closes");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn typing_sound_lists_notifications_and_enter_opens_it() {
    let mut rt = settings(1, (1440., 1000.));
    click(&mut rt, "settings-search");
    type_text(&mut rt, "sound");
    assert_eq!(hits(&rt), [("Notifications".to_owned(), String::new())]);
    assert!(has(&rt, "settings-hit:0"));
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Notifications)
    );
    assert_eq!(title(&rt), "Notifications");
    assert!(has(&rt, "notify-tabs"), "its page is on screen");
    assert!(!has(&rt, "settings-found"), "a page has no row to mark");
    // The search stays, with the keyboard still in it.
    assert_eq!(rt.state.settings_nav.search.text, "sound");
    assert!(focused(&rt).is_some_and(|path| path.contains("settings-search")));

    // Nothing found says so, and Enter does nothing.
    press(&mut rt, Key::Escape);
    type_text(&mut rt, "zebra");
    assert!(hits(&rt).is_empty());
    assert!(has(&rt, "settings-no-hits"));
    press(&mut rt, Key::Enter);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Notifications)
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn typing_padding_lists_the_padding_rows_and_enter_marks_the_first() {
    let mut rt = settings(1, (1440., 1000.));
    click(&mut rt, "settings-search");
    type_text(&mut rt, "padding");
    assert_eq!(
        hits(&rt)[0],
        (
            "Horizontal padding".to_owned(),
            "Interface › Panes".to_owned()
        )
    );
    assert_eq!(
        hits(&rt)[1],
        (
            "Vertical padding".to_owned(),
            "Interface › Panes".to_owned()
        )
    );
    // The arrows walk the hits, around the ends.
    let count = hits(&rt).len();
    assert!(count > 1, "the Panes page is a hit too");
    press(&mut rt, Key::Up);
    assert_eq!(rt.state.settings_nav.selected(), count - 1);
    press(&mut rt, Key::Down);
    assert_eq!(rt.state.settings_nav.selected(), 0);

    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!(rt.state.settings_page(), Page::Sub(SubPage::Panes));
    assert_eq!(rt.state.settings_section, SettingsSection::Interface);
    assert_eq!(title(&rt), "Panes");
    // The row is marked: the tint and its outline, around the row that holds the stepper.
    let mark = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("settings-found"))
        .expect("the found row's mark");
    let (fill, _) = crate::app::theme::selection(false);
    assert_eq!(mark.fill, fill);
    let stepper = rect(&rt, "pane-padding-x-larger");
    assert!(
        mark.rect[1] < stepper[1] && mark.rect[1] + mark.rect[3] > stepper[1] + stepper[3],
        "{:?} is around the row of {stepper:?}",
        mark.rect
    );
    // The mark leaves by itself.
    rt.update(|s, tx| {
        let later = std::time::Instant::now() + std::time::Duration::from_secs(5);
        assert!(s.poll_settings(later, tx));
    });
    settle(&mut rt);
    assert!(!has(&rt, "settings-found"));
    assert_eq!(rt.state.settings_page(), Page::Sub(SubPage::Panes));

    // A pointer picks a hit as Enter does.
    press(&mut rt, Key::Escape);
    type_text(&mut rt, "font size");
    click(&mut rt, "settings-hit:0");
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Terminal)
    );
    let stepper = rect(&rt, "font-larger");
    let mark = rect(&rt, "settings-found");
    assert!(mark[1] < stepper[1] && mark[1] + mark[3] > stepper[1] + stepper[3]);
    assert!(
        focused(&rt).is_some_and(|path| path.contains("settings-search")),
        "the press leaves the keyboard in the field"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn search_reads_the_rows_the_pages_are_built_from() {
    let mut rt = settings(2, (1440., 1000.));
    click(&mut rt, "settings-search");
    // A row from each kind of page: a section's, another module's, a sub-page's.
    for (query, label, place) in [
        ("font size", "Font size", "Terminal"),
        ("zoom", "Interface zoom", "Interface"),
        ("window col", "Window colours", "Themes"),
        ("terminal theme", "Terminal theme", "Themes"),
        ("client label", "Client label", "Remote Hosts"),
        (
            "desktop notifications",
            "Desktop notifications",
            "Notifications",
        ),
        ("customize", "Customize panes", "Interface"),
    ] {
        rt.update(|s, _| {
            s.settings_nav.search = rune::TextEdit::new(query);
            s.settings_search_changed();
        });
        assert!(
            hits(&rt).contains(&(label.to_owned(), place.to_owned())),
            "{query} finds {label} on {place}: {:?}",
            hits(&rt)
        );
    }
    // A label a page repeats, one per host, is one hit.
    rt.update(|s, _| {
        s.settings_nav.search = rune::TextEdit::new("remote control");
        s.settings_search_changed();
    });
    let found = hits(&rt);
    assert_eq!(
        found
            .iter()
            .filter(|(label, _)| label == "Remote control")
            .count(),
        1,
        "{found:?}"
    );
    // A helper's words find its row, after the rows named for them.
    rt.update(|s, _| {
        s.settings_nav.search = rune::TextEdit::new("osc");
        s.settings_search_changed();
    });
    assert_eq!(
        hits(&rt),
        [(
            "Messages from programs".to_owned(),
            "Notifications".to_owned()
        )]
    );
    // Reading the pages leaves the one on screen as it was.
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Interface)
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_status_page_goes_back_to_notifications_where_it_was_scrolled() {
    // A window short enough that Notifications scrolls.
    let mut rt = settings(1, (900., 440.));
    assert!(!enabled(&rt, "settings-back") && !enabled(&rt, "settings-forward"));
    click(&mut rt, "settings-section:notifications");
    settle(&mut rt);
    let page = rect(&rt, "settings-scroll:notifications");
    let (x, y) = (page[0] + page[2] / 2., page[1] + page[3] / 2.);
    rt.wheel(x, y, -64.);
    settle(&mut rt);
    let scrolled = rt.state.settings_nav.offset().get();
    assert!(scrolled > 20., "the page scrolled: {scrolled}");
    let row_before = rect(&rt, "notify-status:blocked");

    click(&mut rt, "notify-status:blocked");
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Sub(SubPage::Status(Status::Blocked))
    );
    assert_eq!(title(&rt), "Needs you");
    assert_eq!(element_text(&rt, "settings-crumb"), ["Notifications"]);
    assert_eq!(
        rt.state.settings_section,
        SettingsSection::Notifications,
        "the sidebar keeps Notifications selected"
    );
    assert!(has(&rt, "notify-blocked") && has(&rt, "status-sample"));
    assert_eq!(
        rt.state.settings_nav.offset().get(),
        0.,
        "a new page opens at its top"
    );
    assert!(enabled(&rt, "settings-back") && !enabled(&rt, "settings-forward"));

    // Back, by its button: Notifications is where it was left.
    click(&mut rt, "settings-back");
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Notifications)
    );
    assert!((rt.state.settings_nav.offset().get() - scrolled).abs() < 0.5);
    assert_eq!(rect(&rt, "notify-status:blocked"), row_before);
    assert!(enabled(&rt, "settings-back") && enabled(&rt, "settings-forward"));

    // Forward and Back from the keyboard, then with the pointer's own buttons.
    alt(&mut rt, Key::Right);
    assert_eq!(
        rt.state.settings_page(),
        Page::Sub(SubPage::Status(Status::Blocked))
    );
    alt(&mut rt, Key::Left);
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Notifications)
    );
    assert!((rt.state.settings_nav.offset().get() - scrolled).abs() < 0.5);
    let dialog = rect(&rt, "dialog");
    let (x, y) = (dialog[0] + dialog[2] / 2., dialog[1] + dialog[3] / 2.);
    for (button, page) in [
        (
            rune::PointerButton::Forward,
            Page::Sub(SubPage::Status(Status::Blocked)),
        ),
        (
            rune::PointerButton::Back,
            Page::Section(SettingsSection::Notifications),
        ),
        (
            rune::PointerButton::Back,
            Page::Section(SettingsSection::Interface),
        ),
        // Nowhere further back: it stays, and Settings stays open.
        (
            rune::PointerButton::Back,
            Page::Section(SettingsSection::Interface),
        ),
    ] {
        rt.button_down(x, y, button);
        rt.button_up(x, y, button);
        settle(&mut rt);
        assert_eq!(rt.state.settings_page(), page, "after {button:?}");
    }
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));

    // The crumb goes to the section as a visit, so Back returns to the sub-page.
    alt(&mut rt, Key::Right);
    alt(&mut rt, Key::Right);
    click(&mut rt, "settings-crumb");
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Notifications)
    );
    alt(&mut rt, Key::Left);
    assert_eq!(
        rt.state.settings_page(),
        Page::Sub(SubPage::Status(Status::Blocked))
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn history_starts_again_each_time_settings_opens() {
    let mut rt = settings(1, (1440., 1000.));
    click(&mut rt, "customize-panes");
    assert_eq!(rt.state.settings_page(), Page::Sub(SubPage::Panes));
    assert!(enabled(&rt, "settings-back"));
    // Alt with the arrows is Settings' own: it does nothing once Settings is closed.
    click(&mut rt, "close-settings");
    settle(&mut rt);
    assert!(rt.state.panel.is_none());
    alt(&mut rt, Key::Left);
    assert_eq!(rt.state.settings_page(), Page::Sub(SubPage::Panes));

    // It reopens on the page it was left on, with nothing to go back to.
    rt.update(|s, tx| s.perform(Intent::Settings, tx));
    settle(&mut rt);
    assert_eq!(title(&rt), "Panes");
    assert!(!enabled(&rt, "settings-back") && !enabled(&rt, "settings-forward"));
    // Opening on a section (About, from Commands) is not a visit either.
    rt.update(|s, tx| {
        s.close_panel(tx);
        s.perform(Intent::About, tx);
    });
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::About)
    );
    assert!(!enabled(&rt, "settings-back"));
    // Nor is opening on Updates, which also starts without the last search.
    click(&mut rt, "settings-search");
    type_text(&mut rt, "font");
    rt.update(|s, tx| {
        s.close_panel(tx);
        s.perform(Intent::Updates, tx);
    });
    settle(&mut rt);
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::Updates)
    );
    assert_eq!(rt.state.settings_nav.search.text, "");
    assert!(has(&rt, "settings-section:updates") && !enabled(&rt, "settings-back"));
    rt.update(|s, tx| {
        s.close_panel(tx);
        s.perform(Intent::About, tx);
    });
    settle(&mut rt);
    // The first visit from there can come back to it.
    click(&mut rt, "settings-section:terminal");
    assert!(enabled(&rt, "settings-back"));
    click(&mut rt, "settings-back");
    assert_eq!(
        rt.state.settings_page(),
        Page::Section(SettingsSection::About)
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_status_page_shows_the_state_in_a_tab_and_its_notification_switch() {
    let mut rt = settings(1, (1440., 1000.));
    click(&mut rt, "settings-section:notifications");
    // Each state's row says where it shows.
    let summary = |rt: &Runtime<Workspace>, status: &str| {
        element_text(rt, &format!("notify-status:{status}"))
            .last()
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(summary(&rt, "blocked"), "Tabs, Notification");
    assert_eq!(summary(&rt, "working"), "Tabs", "off by default");

    click(&mut rt, "notify-status:working");
    assert!(has(&rt, "status-sample-progress"), "working is a bar");
    assert!(!rt.state.settings.notifications.working);
    click(&mut rt, "notify-working");
    assert!(rt.state.settings.notifications.working);
    assert!(
        Settings::load(&rt.state.settings_path)
            .unwrap()
            .notifications
            .working,
        "saved"
    );
    click(&mut rt, "settings-back");
    assert_eq!(summary(&rt, "working"), "Tabs, Notification");

    // With both channels off the row says Off, and the page says why its switch is dimmed.
    click(&mut rt, "notify-tabs");
    click(&mut rt, "notify-system");
    assert_eq!(summary(&rt, "error"), "Off");
    click(&mut rt, "notify-status:error");
    assert!(!has(&rt, "status-sample"));
    let words: Vec<String> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.path.contains("settings-scroll:status-error"))
        .filter_map(|e| e.text)
        .collect();
    assert!(
        words.contains(&"Desktop notifications are off.".to_owned()),
        "{words:?}"
    );
    let before = rt.state.settings.notifications.error;
    click(&mut rt, "notify-error");
    assert_eq!(rt.state.settings.notifications.error, before, "dimmed");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Settings as drawn, for a look at the sidebar, the header and a search (see `Camera` in
/// `pane_verb_tests.rs`).
#[test]
fn the_settings_shell_draws_its_sidebar_header_and_search() {
    use pane_verb_tests::Camera;
    let mut ws = workspace_hosts(2);
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    let mut rt = Runtime::new(ws, view, (1024., 700.), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    let mut camera = Camera::new();
    let mut snap = |rt: &mut Runtime<Workspace>, name: &str| {
        if let Some(camera) = &mut camera {
            camera.snap(rt, name);
        }
    };
    snap(&mut rt, "settings-interface");
    click(&mut rt, "settings-section:themes");
    settle(&mut rt);
    snap(&mut rt, "settings-themes");
    click(&mut rt, "settings-section:notifications");
    settle(&mut rt);
    snap(&mut rt, "settings-notifications");
    click(&mut rt, "notify-status:blocked");
    settle(&mut rt);
    snap(&mut rt, "settings-status-blocked");
    click(&mut rt, "settings-back");
    click(&mut rt, "notify-status:working");
    settle(&mut rt);
    snap(&mut rt, "settings-status-working");
    click(&mut rt, "settings-search");
    type_text(&mut rt, "pa");
    settle(&mut rt);
    snap(&mut rt, "settings-search");
    type_text(&mut rt, "dding");
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    snap(&mut rt, "settings-found");
    press(&mut rt, Key::Escape);
    type_text(&mut rt, "zebra");
    settle(&mut rt);
    snap(&mut rt, "settings-no-hits");
    assert!(has(&rt, "settings-no-hits"));
    std::fs::remove_file(&rt.state.settings_path).ok();

    // Where room is shortest: a zoomed window, and the smallest one. The sidebar is at its
    // narrowest and the header's title has the least width.
    for (name, scale, size) in [
        ("settings-zoomed", 1.3, (1024., 700.)),
        ("settings-smallest", 1., (640., 480.)),
    ] {
        let mut ws = workspace_hosts(2);
        ws.settings.ui_scale = scale;
        ws.perform(Intent::Settings, &mut rune::Tx::default());
        let mut rt = Runtime::new(ws, view, size, Env::new(true, 2.));
        rt.on_key = Some(crate::app::input::on_key);
        settle(&mut rt);
        rt.update(|s, _| s.navigate(Page::Sub(SubPage::Status(Status::Working))));
        click(&mut rt, "settings-search");
        type_text(&mut rt, "desktop");
        settle(&mut rt);
        snap(&mut rt, name);
        // Nothing in the header or the sidebar is cut by the dialog's edges.
        let dialog = rect(&rt, "dialog");
        for e in rt.elements().iter().filter(|e| {
            !e.exiting
                && (e.path.contains("settings-header") || e.path.contains("settings-sidebar"))
                && e.rect[2] > 0.
        }) {
            assert!(
                e.rect[0] >= dialog[0] - 0.5
                    && e.rect[0] + e.rect[2] <= dialog[0] + dialog[2] + 0.5,
                "{name}: {:?} {:?} leaves the dialog {dialog:?}",
                e.key.as_deref().or(e.text.as_deref()),
                e.rect
            );
        }
        let (close, title) = (rect(&rt, "close-settings"), rect(&rt, "settings-title"));
        assert!(
            title[0] + title[2] <= close[0],
            "{name}: the title {title:?} ends before Close {close:?}"
        );
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}
