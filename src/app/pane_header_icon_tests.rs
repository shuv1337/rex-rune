//! PN02: every pane header draws its own program's glyph before its name, as Rex for macOS
//! does, under the same count and colour settings as tabs, without moving the terminal.
use super::pane_verb_tests::{Camera, framed_terminal, rect};
use super::*;
use crate::app::{activity::PaneProgram, pane_header, tab_icons, theme::*};
use rex_rune::settings::interface::TabIconColor;
use rex_rune::settings::{PaneDensity, PaneHeader};

const BUTTONS: [&str; 4] = [
    "pane-split-right",
    "pane-split-down",
    "pane-zoom",
    "pane-close",
];

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

fn element(rt: &Runtime<Workspace>, key: &str) -> rune::ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
}

/// The drawn glyph inside the icon cell keyed `key`.
fn glyph(rt: &Runtime<Workspace>, key: &str) -> rune::ElementInfo {
    let path = format!("{}/", element(rt, key).path);
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.path.starts_with(&path) && e.text_color.is_some())
        .unwrap_or_else(|| panic!("no glyph under {key}"))
}

/// `nvim` and `git` are distinct glyphs, so a header showing its neighbour's would show.
fn programmes(ws: &mut Workspace) {
    let model = &mut ws.hosts[0].model;
    model.programs.insert("block:b".into(), "nvim".into());
    model.programs.insert("block:c".into(), "git".into());
}

/// The glyph cells inside one header, whatever their keys.
fn glyph_cells(rt: &Runtime<Workspace>, header: &str) -> usize {
    let path = format!("{}/", element(rt, header).path);
    rt.elements()
        .into_iter()
        .filter(|e| {
            !e.exiting
                && e.path.starts_with(&path)
                && e.key
                    .as_deref()
                    .is_some_and(|key| key.starts_with("pane-icon:"))
        })
        .count()
}

#[test]
fn split_headers_draw_their_own_panes_program_glyph() {
    let _families = rune::text::with_families("Inter", "Inter");
    let (mut ws, host) = tabbed_workspace("split");
    programmes(&mut ws);
    let (sender, sent) = flume::unbounded();
    for block in ["block:b", "block:c"] {
        framed_terminal(&mut ws, block, &sender, b"\x1b[31m$ prompt\x1b[0m");
    }
    let mut rt = runtime(ws);
    settle(&mut rt);
    for (block, program) in [("block:b", "nvim"), ("block:c", "git")] {
        let key = format!("pane-icon:{block}");
        let cell = element(&rt, &key);
        assert_eq!(
            cell.help.as_deref(),
            Some(program),
            "the glyph names the program it was drawn for"
        );
        assert_eq!(glyph_cells(&rt, &format!("pane-header:{block}")), 1);
        let header = rect(&rt, &format!("pane-header:{block}"));
        let name = element(&rt, &format!("pane-name:{block}"));
        let [x, y, w, h] = cell.rect;
        assert_eq!((w, h), (15., 15.), "a program glyph's slot");
        assert!(
            x >= header[0] && x + w <= name.rect[0],
            "{key} sits before the name, inside the row"
        );
        assert!(
            y >= header[1] && y + h <= header[1] + header[3],
            "{key} is inside the header row"
        );
        assert!(
            (y + h / 2. - (name.rect[1] + name.rect[3] / 2.)).abs() < 0.5,
            "{key} is centred with the name"
        );
        // Never colour: the glyph is lit with its own pane, as the name is.
        let drawn = glyph(&rt, &key).text_color;
        if block == "block:b" {
            assert_eq!(drawn, Some(rgb(ink())));
        } else {
            assert_eq!(drawn, Some(rgb(muted())));
        }
    }
    // Use colour for tab icons tints a header only for the pane with the keyboard, as it
    // tints only the first glyph of an active tab.
    rt.update(|s, _| s.settings.interface.tab_icon_color = TabIconColor::ActivePane);
    settle(&mut rt);
    let tint = tab_icons::icons_of(&[PaneProgram::of(rt.state.model(), "block:b")], 1)[0]
        .icon
        .tint;
    assert_eq!(glyph(&rt, "pane-icon:block:b").text_color, Some(rgb(tint)));
    assert_eq!(
        glyph(&rt, "pane-icon:block:c").text_color,
        Some(rgb(muted()))
    );
    assert!(sent.is_empty() && host.is_empty() && rt.state.claimed.is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn every_header_placement_draws_the_glyph_and_hidden_ones_do_not() {
    let _families = rune::text::with_families("Inter", "Inter");
    for (tab, header, density, pinned, expected) in [
        (
            "single",
            PaneHeader::Auto,
            PaneDensity::Comfortable,
            false,
            true,
        ),
        (
            "single",
            PaneHeader::Always,
            PaneDensity::Comfortable,
            false,
            true,
        ),
        (
            "single",
            PaneHeader::OnHover,
            PaneDensity::Comfortable,
            true,
            true,
        ),
        (
            "single",
            PaneHeader::Hidden,
            PaneDensity::Comfortable,
            true,
            true,
        ),
        (
            "single",
            PaneHeader::Hidden,
            PaneDensity::Comfortable,
            false,
            false,
        ),
        (
            "split",
            PaneHeader::Auto,
            PaneDensity::Comfortable,
            false,
            true,
        ),
        (
            "split",
            PaneHeader::Auto,
            PaneDensity::Compact,
            false,
            false,
        ),
        (
            "split",
            PaneHeader::Always,
            PaneDensity::Compact,
            false,
            true,
        ),
        (
            "split",
            PaneHeader::OnHover,
            PaneDensity::Compact,
            true,
            true,
        ),
        (
            "split",
            PaneHeader::Hidden,
            PaneDensity::Compact,
            false,
            false,
        ),
    ] {
        let (mut ws, _host) = tabbed_workspace(tab);
        ws.settings.pane_header = header;
        ws.settings.pane_density = density;
        ws.header_reveal.pinned = pinned;
        programmes(&mut ws);
        let (sender, _sent) = flume::unbounded();
        let blocks: &[&str] = if tab == "single" {
            &["block:a"]
        } else {
            &["block:b", "block:c"]
        };
        for block in blocks {
            framed_terminal(&mut ws, block, &sender, b"$ prompt");
        }
        let mut rt = runtime(ws);
        settle(&mut rt);
        for block in blocks {
            let header_key = format!("pane-header:{block}");
            let icon_key = format!("pane-icon:{block}");
            assert_eq!(
                has(&rt, &header_key),
                expected,
                "{tab} {header:?} {density:?}: header presence is unchanged"
            );
            assert_eq!(
                has(&rt, &icon_key),
                expected,
                "{tab} {header:?} {density:?}: the header's glyph follows it"
            );
            if expected {
                let header = rect(&rt, &header_key);
                let icon = element(&rt, &icon_key);
                assert!(
                    icon.rect[0] >= header[0]
                        && icon.rect[0] + icon.rect[2] <= header[0] + header[2],
                    "{tab} {header:?} {density:?}: the glyph is inside its own row"
                );
            }
        }
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

#[test]
fn tab_icon_count_governs_the_glyph_and_never_moves_the_grid() {
    let _families = rune::text::with_families("Inter", "Inter");
    let (mut ws, host) = tabbed_workspace("split");
    programmes(&mut ws);
    let (sender, sent) = flume::unbounded();
    for block in ["block:b", "block:c"] {
        framed_terminal(&mut ws, block, &sender, b"$ prompt");
    }
    let mut rt = runtime(ws);
    settle(&mut rt);
    let before: Vec<_> = ["block:b", "block:c"]
        .iter()
        .map(|block| {
            (
                rect(&rt, &format!("grid:{block}")),
                rect(&rt, &format!("pane-header:{block}")),
            )
        })
        .collect();
    for count in [1, 0, 3, 0, 1] {
        rt.update(|s, _| s.settings.interface.tab_icon_count = count);
        settle(&mut rt);
        for (index, block) in ["block:b", "block:c"].iter().enumerate() {
            let grid = rect(&rt, &format!("grid:{block}"));
            let header = rect(&rt, &format!("pane-header:{block}"));
            assert_eq!((grid, header), before[index], "the grid and row stay put");
            let shown = count > 0;
            assert_eq!(
                has(&rt, &format!("pane-icon:{block}")),
                shown,
                "count {count}: 0 hides the glyph, any count draws one"
            );
            assert_eq!(
                glyph_cells(&rt, &format!("pane-header:{block}")),
                usize::from(shown),
                "count {count}: a header shows one glyph, not the tab's count"
            );
            let name = rect(&rt, &format!("pane-name:{block}"));
            let expected = header[0] + 14. + if shown { 23. } else { 0. };
            assert!(
                (name[0] - expected).abs() < 0.01,
                "count {count}: the name starts after the glyph and its 8 pt gap, or at the card's edge: {}",
                name[0]
            );
        }
    }
    assert!(sent.is_empty() && host.is_empty() && rt.state.claimed.is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_long_pane_name_truncates_between_its_glyph_and_its_buttons() {
    let _families = rune::text::with_families("Inter", "Inter");
    let (mut ws, host) = tabbed_workspace("split");
    programmes(&mut ws);
    let (sender, sent) = flume::unbounded();
    for block in ["block:b", "block:c"] {
        framed_terminal(&mut ws, block, &sender, b"$ prompt");
    }
    let mut rt = runtime(ws);
    settle(&mut rt);
    let grid = rect(&rt, "grid:block:b");
    let long = "a pane name that keeps going past every bit of room its header has ".repeat(8);
    rt.update(|s, _| {
        s.hosts[0].model.views.get_mut("s").unwrap().windows[1].layers[0].blocks[0].label =
            Some(long.clone());
    });
    settle(&mut rt);
    assert_eq!(
        rect(&rt, "grid:block:b"),
        grid,
        "a longer name never reaches the terminal"
    );
    let name = element(&rt, "pane-name:block:b");
    assert_eq!(name.text.as_deref(), Some(long.as_str()));
    assert_eq!(
        name.help.as_deref(),
        Some(long.as_str()),
        "cut short, the name keeps its whole text as the tooltip"
    );
    let first = rect(&rt, "pane-split-right:block:b");
    assert!(
        name.rect[0] + name.rect[2] <= first[0],
        "the name gives way before the buttons: {name:?} vs {first:?}"
    );
    for button in BUTTONS {
        assert!(
            has(&rt, &format!("{button}:block:b")),
            "{button} stays in the row"
        );
    }
    assert!(has(&rt, "pane-icon:block:b"), "the glyph stays too");
    assert!(sent.is_empty() && host.is_empty() && rt.state.claimed.is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_narrow_header_keeps_its_buttons_and_drops_the_glyph() {
    let _families = rune::text::with_families("Inter", "Inter");
    let (mut ws, _host) = tabbed_workspace("split");
    programmes(&mut ws);
    // A 15% left pane of a 640 pt window is under 120 pt wide.
    let view = ws.hosts[0].model.views.get_mut("s").unwrap();
    let layout = serde_json::json!({"split": {"direction": "horizontal", "ratio": 0.15,
        "before": {"block_id": "block:b"}, "after": {"block_id": "block:c"}}});
    view.windows[1].layers[0].layout = serde_json::from_value(layout).unwrap();
    let mut rt = runtime_at(ws, (640., 480.));
    settle(&mut rt);
    let pane = rect(&rt, "pane:block:b");
    assert!(pane[2] < 120., "{pane:?}");
    assert!(
        !pane_header::glyph_fits(
            pane[2],
            pane_header::HEADER_LEAD,
            pane_header::buttons(pane[2]),
            0.
        ),
        "the glyph has no room beside the name and Zoom and Close: {pane:?}"
    );
    assert!(
        !has(&rt, "pane-icon:block:b"),
        "the glyph gives way before the buttons do"
    );
    assert!(has(&rt, "pane-zoom:block:b") && has(&rt, "pane-close:block:b"));
    let close = rect(&rt, "pane-close:block:b");
    assert!(
        close[0] + close[2] <= pane[0] + pane[2],
        "Close stays inside the pane: {close:?} vs {pane:?}"
    );
    assert!(
        has(&rt, "pane-icon:block:c"),
        "the wide pane keeps its glyph"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// The same rule for the longest words a header draws: “Needs sign-in” at 29% (about 170 pt,
/// two buttons) and “Paused 100%” at 36% (about 219 pt, four), both wider than the row without
/// the glyph needs and narrower than the measured word lets it keep.
#[test]
fn the_longest_activity_words_drop_the_glyph_and_leave_close_where_it_was() {
    use rex_rune::connection::activity::{BlockedKind, ProgramState, TerminalProgress};
    let _families = rune::text::with_families("Inter", "Inter");
    for (ratio, word) in [(0.29, "Needs sign-in"), (0.36, "Paused 100%")] {
        let (mut ws, _host) = tabbed_workspace("split");
        programmes(&mut ws);
        if word == "Needs sign-in" {
            let mut blocked = record(ProgramState::Blocked, true);
            blocked.kind = Some(BlockedKind::Auth);
            set_activity(&mut ws, "block:b", &[("", blocked)]);
        } else {
            ws.hosts[0]
                .model
                .progress
                .insert("block:b".into(), TerminalProgress::Paused(Some(100)));
        }
        let view = ws.hosts[0].model.views.get_mut("s").unwrap();
        let layout = serde_json::json!({"split": {"direction": "horizontal", "ratio": ratio,
            "before": {"block_id": "block:b"}, "after": {"block_id": "block:c"}}});
        view.windows[1].layers[0].layout = serde_json::from_value(layout).unwrap();
        let (sender, _sent) = flume::unbounded();
        framed_terminal(&mut ws, "block:b", &sender, b"$ prompt");
        framed_terminal(&mut ws, "block:c", &sender, b"$ prompt");
        let mut rt = runtime_at(ws, (640., 480.));
        settle(&mut rt);
        let pane = rect(&rt, "pane:block:b");
        let close = rect(&rt, "pane-close:block:b");
        assert!(has(&rt, "pane-word:block:b"), "{word}: the word is drawn");
        assert!(
            !has(&rt, "pane-icon:block:b"),
            "{word}: no room for the glyph beside the word: {pane:?}"
        );
        assert!(
            close[0] + close[2] <= pane[0] + pane[2],
            "{word}: Close ends inside the pane: {pane:?}"
        );
        // Count 0 draws the same row without a glyph: the glyph never moves Close.
        rt.update(|s, _| s.settings.interface.tab_icon_count = 0);
        settle(&mut rt);
        assert_eq!(rect(&rt, "pane-close:block:b"), close);
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

/// A header's activity word and the buttons it draws decide the glyph's room. With a Blocked
/// record drawing “Needs input” on a 25% and a 33% pane of a 640×480 split — about 151 pt and
/// 200 pt, where the word and the buttons alone just fit — the glyph gives way instead of
/// pushing Close past the pane's edge, and Close stays where a header without a glyph has it.
#[test]
fn an_activity_word_drops_the_glyph_before_it_pushes_close_out() {
    use rex_rune::connection::activity::ProgramState;
    let _families = rune::text::with_families("Inter", "Inter");
    for ratio in [0.25, 0.33] {
        let (mut ws, _host) = tabbed_workspace("split");
        programmes(&mut ws);
        // More than one record decides the pane's program, and the newest is Blocked.
        set_activity(
            &mut ws,
            "block:b",
            &[("", record(ProgramState::Blocked, true))],
        );
        let view = ws.hosts[0].model.views.get_mut("s").unwrap();
        let layout = serde_json::json!({"split": {"direction": "horizontal", "ratio": ratio,
            "before": {"block_id": "block:b"}, "after": {"block_id": "block:c"}}});
        view.windows[1].layers[0].layout = serde_json::from_value(layout).unwrap();
        let (sender, _sent) = flume::unbounded();
        framed_terminal(&mut ws, "block:b", &sender, b"$ prompt");
        let mut rt = runtime_at(ws, (640., 480.));
        settle(&mut rt);
        let pane = rect(&rt, "pane:block:b");
        assert!(
            has(&rt, "pane-word:block:b"),
            "{ratio}: the whole word is drawn beside the buttons"
        );
        let inside = |rt: &Runtime<Workspace>| {
            let close = rect(rt, "pane-close:block:b");
            close[0] + close[2] <= pane[0] + pane[2]
        };
        assert!(
            !has(&rt, "pane-icon:block:b"),
            "{ratio}: no room for the glyph beside the word: {pane:?}"
        );
        assert!(inside(&rt), "{ratio}: Close ends inside the pane");
        // Count 0 draws the same row without a glyph: the glyph never makes the row worse.
        rt.update(|s, _| s.settings.interface.tab_icon_count = 0);
        settle(&mut rt);
        assert!(!has(&rt, "pane-icon:block:b"));
        assert!(
            inside(&rt),
            "{ratio}: the header without a glyph keeps Close inside too"
        );
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

/// The reconnecting strip's Retry pill cannot shrink, so the glyph gives way for it too. On a
/// 31% and a 40% pane of a 640×480 split — about 188 pt with two buttons and 244 pt with four,
/// where Retry just clears the buttons without a glyph — Retry still clears them.
#[test]
fn the_reconnecting_strip_drops_the_glyph_before_retry_meets_the_buttons() {
    let _families = rune::text::with_families("Inter", "Inter");
    for ratio in [0.31, 0.4] {
        let (mut ws, _host) = tabbed_workspace("split");
        programmes(&mut ws);
        let view = ws.hosts[0].model.views.get_mut("s").unwrap();
        let layout = serde_json::json!({"split": {"direction": "horizontal", "ratio": ratio,
            "before": {"block_id": "block:b"}, "after": {"block_id": "block:c"}}});
        view.windows[1].layers[0].layout = serde_json::from_value(layout).unwrap();
        let (sender, _sent) = flume::unbounded();
        framed_terminal(&mut ws, "block:b", &sender, b"$ prompt");
        framed_terminal(&mut ws, "block:c", &sender, b"$ prompt");
        ws.terminals[&ws.term_key("block:b")]
            .connection
            .model
            .lock()
            .unwrap()
            .connected = false;
        let mut rt = runtime_at(ws, (640., 480.));
        settle(&mut rt);
        let clear = |rt: &Runtime<Workspace>| {
            let retry = rect(rt, "strip-action:block:b");
            let first = if has(rt, "pane-split-right:block:b") {
                rect(rt, "pane-split-right:block:b")
            } else {
                rect(rt, "pane-zoom:block:b")
            };
            retry[0] + retry[2] <= first[0]
        };
        let pane = rect(&rt, "pane:block:b");
        assert!(
            !has(&rt, "pane-icon:block:b"),
            "{ratio}: no room for the glyph beside Retry: {pane:?}"
        );
        assert!(clear(&rt), "{ratio}: Retry ends before the buttons");
        rt.update(|s, _| s.settings.interface.tab_icon_count = 0);
        settle(&mut rt);
        assert!(
            clear(&rt),
            "{ratio}: the header without a glyph clears them too"
        );
        std::fs::remove_file(&rt.state.settings_path).ok();
    }
}

/// Real-font evidence: run each individually with `REX_TEST_SNAP_DIR`.
#[test]
fn pane_header_icon_draw_large() {
    draw((1024., 700.), "pane-header-icon");
}

#[test]
fn pane_header_icon_draw_minimum() {
    draw((640., 480.), "pane-header-icon-minimum");
}

/// Real-font evidence of the room rule: run individually with `REX_TEST_SNAP_DIR`.
#[test]
fn pane_header_icon_draw_reconnect() {
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let (mut ws, _host) = tabbed_workspace("split");
    programmes(&mut ws);
    let layout = serde_json::json!({"split": {"direction": "horizontal", "ratio": 0.31,
        "before": {"block_id": "block:b"}, "after": {"block_id": "block:c"}}});
    ws.hosts[0].model.views.get_mut("s").unwrap().windows[1].layers[0].layout =
        serde_json::from_value(layout).unwrap();
    let (sender, _sent) = flume::unbounded();
    framed_terminal(&mut ws, "block:b", &sender, b"$ nvim src/main.rs");
    framed_terminal(&mut ws, "block:c", &sender, b"$ git log --oneline");
    ws.terminals[&ws.term_key("block:b")]
        .connection
        .model
        .lock()
        .unwrap()
        .connected = false;
    let mut rt = Runtime::with_text(
        ws,
        view,
        (640., 480.),
        Env::new(true, 1.),
        rune::text::TextSystem::new(fonts.files, fonts.families),
    );
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    camera.snap(&mut rt, "pane-header-icon-reconnect");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

fn draw(size: (f32, f32), prefix: &str) {
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let (mut ws, _host) = tabbed_workspace("split");
    programmes(&mut ws);
    let (sender, _sent) = flume::unbounded();
    framed_terminal(
        &mut ws,
        "block:a",
        &sender,
        b"$ tail -f server.log\r\nlistening on :8080",
    );
    framed_terminal(
        &mut ws,
        "block:b",
        &sender,
        b"$ nvim src/main.rs\r\n 1 use std::io;\r\n~",
    );
    framed_terminal(&mut ws, "block:c", &sender, b"$ git log --oneline");
    let mut rt = Runtime::with_text(
        ws,
        view,
        size,
        Env::new(true, 1.),
        rune::text::TextSystem::new(fonts.files, fonts.families),
    );
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    camera.snap(&mut rt, &format!("{prefix}-split"));
    rt.update(|s, _| {
        s.hosts[0]
            .model
            .views
            .get_mut("s")
            .unwrap()
            .active_window_id = Some("single".into());
    });
    settle(&mut rt);
    camera.snap(&mut rt, &format!("{prefix}-single"));
    rt.update(|s, _| {
        s.hosts[0]
            .model
            .views
            .get_mut("s")
            .unwrap()
            .active_window_id = Some("split".into());
        s.settings.pane_header = PaneHeader::OnHover;
        s.header_reveal.pinned = true;
    });
    settle(&mut rt);
    camera.snap(&mut rt, &format!("{prefix}-floating"));
    rt.update(|s, _| {
        s.header_reveal.pinned = false;
        s.settings.pane_header = PaneHeader::Auto;
        s.settings.interface.tab_icon_count = 0;
    });
    settle(&mut rt);
    camera.snap(&mut rt, &format!("{prefix}-off"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}
