//! Settings → Shortcuts: a row for every action, alternates on one row, what a shared key
//! says, and the filter by name and by key.
use super::*;
use crate::app::{
    actions,
    panel::SettingsSection,
    settings_view::SHORTCUT_CATEGORIES,
    shortcuts_page::{Row, listing, rows},
};
use rex_linux::keymap::{Overrides, Resolved};
use rexrs::api::client::{Binding, InspectResponseAction};
use serde_json::{Value, json};

/// The keymap of this client alone, with `overrides` as its shortcuts.json.
fn local(overrides: Value) -> Resolved {
    actions::resolve(&Overrides::parse(&overrides))
}

fn default_rows() -> Vec<Row> {
    rows(&local(json!([])), &[], &[])
}

fn titled<'a>(rows: &'a [Row], title: &str) -> &'a Row {
    let found: Vec<_> = rows.iter().filter(|row| row.title == title).collect();
    assert_eq!(found.len(), 1, "one row titled {title:?}");
    found[0]
}

fn titles(rows: &[Row]) -> Vec<&str> {
    rows.iter().map(|row| row.title.as_str()).collect()
}

fn caps(keys: &[&[&str]]) -> Vec<Vec<String>> {
    keys.iter()
        .map(|chord| chord.iter().map(|cap| (*cap).to_owned()).collect())
        .collect()
}

#[test]
fn every_registered_action_has_a_row_bound_or_not() {
    let rows = default_rows();
    for spec in actions::SPECS {
        let own: Vec<_> = rows.iter().filter(|row| row.action == spec.name).collect();
        assert!(!own.is_empty(), "{}: no row", spec.name);
        if spec.argument().is_none() {
            assert_eq!(own.len(), 1, "{}: one row", spec.name);
            assert_eq!(own[0].title, spec.title());
        }
        for row in own {
            assert_eq!(row.category, spec.category(), "{}", spec.name);
        }
    }
    // Nothing else is listed without a server, and no two rows are the same.
    assert!(rows.iter().all(|row| actions::find(&row.action).is_some()));
    let ids: std::collections::HashSet<String> = rows.iter().map(Row::id).collect();
    assert_eq!(ids.len(), rows.len());
    // An action with no default key is there, with no keys and nothing to explain.
    let close = titled(&rows, "Close tab");
    assert!(close.keys.is_empty() && close.notes.is_empty());
    assert!(
        rows.iter().filter(|row| row.keys.is_empty()).count() > 20,
        "the unbound actions are listed"
    );
}

#[test]
fn an_action_with_a_direction_has_a_row_a_direction() {
    let rows = default_rows();
    let split: Vec<_> = rows
        .iter()
        .filter(|row| row.action == "pane.split")
        .map(|row| (row.title.as_str(), row.keys.clone()))
        .collect();
    assert_eq!(
        split,
        [
            ("Split right", vec![caps(&[&["Ctrl", "Shift", "D"]])]),
            ("Split left", vec![]),
            ("Split down", vec![caps(&[&["Ctrl", "Alt", "D"]])]),
            ("Split up", vec![]),
            ("Split best fit", vec![]),
        ]
    );
    // The four pane-focus defaults are four rows, each found by its own key.
    for direction in ["left", "right", "up", "down"] {
        let row = titled(&rows, &format!("Focus pane {direction}"));
        let mut key = direction.to_owned();
        key[..1].make_ascii_uppercase();
        assert_eq!(row.keys, [caps(&[&["Ctrl", "Alt", &key]])]);
        assert_eq!(row.id(), format!("pane.focus:{direction}"));
    }
    assert_eq!(
        titles(&rows)
            .iter()
            .filter(|title| title.starts_with("Move tab "))
            .count(),
        2
    );
}

#[test]
fn an_action_with_a_number_or_a_key_has_a_row_for_each_bound_value() {
    let rows = default_rows();
    // Bound by default with two values: two rows, by value.
    let scroll: Vec<_> = rows
        .iter()
        .filter(|row| row.action == "pane.scroll")
        .map(|row| row.title.as_str())
        .collect();
    assert_eq!(scroll, ["Scroll up · 20 lines", "Scroll down · 20 lines"]);
    // Bound by default with nine: the numbered tabs in order, then the last.
    let goto: Vec<_> = rows
        .iter()
        .filter(|row| row.action == "client.tab.goto")
        .map(|row| (row.title.as_str(), row.keys.clone()))
        .collect();
    assert_eq!(goto.len(), 9);
    assert_eq!(
        goto[0],
        ("Go to tab 1", vec![caps(&[&["Ctrl", "Alt", "1"]])])
    );
    assert_eq!(
        goto[7],
        ("Go to tab 8", vec![caps(&[&["Ctrl", "Alt", "8"]])])
    );
    assert_eq!(
        goto[8],
        ("Go to last tab", vec![caps(&[&["Ctrl", "Alt", "9"]])])
    );
    // Bound with nothing: one row that says what it needs.
    assert_eq!(
        titled(&rows, "Send key").notes,
        ["Bound in shortcuts.json with an argument: key"]
    );

    let rows = self::rows(
        &local(json!([
            {"keys": "alt+2", "action": "client.tab.goto", "args": {"index": 2}},
            {"keys": "alt+1", "action": "client.tab.goto", "args": {"index": 1}},
            {"keys": "alt+9", "action": "client.tab.goto", "args": {"index": -1}},
        ])),
        &[],
        &[],
    );
    let goto: Vec<_> = rows
        .iter()
        .filter(|row| row.action == "client.tab.goto")
        .map(|row| (row.title.as_str(), row.keys.clone(), row.notes.clone()))
        .collect();
    assert_eq!(
        goto[..2],
        [
            (
                "Go to tab 1",
                vec![caps(&[&["Alt", "1"]]), caps(&[&["Ctrl", "Alt", "1"]])],
                vec!["Alt 1 from shortcuts.json".to_owned()]
            ),
            (
                "Go to tab 2",
                vec![caps(&[&["Alt", "2"]]), caps(&[&["Ctrl", "Alt", "2"]])],
                vec!["Alt 2 from shortcuts.json".to_owned()]
            ),
        ]
    );
    // The last tab's row follows the numbered ones, whatever its keys.
    assert_eq!(
        goto[8],
        (
            "Go to last tab",
            vec![caps(&[&["Alt", "9"]]), caps(&[&["Ctrl", "Alt", "9"]])],
            vec!["Alt 9 from shortcuts.json".to_owned()]
        )
    );
}

#[test]
fn an_actions_bindings_share_one_row_in_the_order_keys_are_tried() {
    let rows = default_rows();
    let zoom = titled(&rows, "Increase interface zoom");
    assert_eq!(
        zoom.keys,
        [
            caps(&[&["Ctrl", "Plus"]]),
            caps(&[&["Ctrl", "Shift", "="]]),
            caps(&[&["Ctrl", "="]]),
        ],
        "the main block's keys, a literal plus spelled out"
    );
    assert_eq!(zoom.notes, ["Also on the numpad"]);
    assert_eq!(
        titled(&rows, "Decrease interface zoom").keys,
        [caps(&[&["Ctrl", "-"]])]
    );
    // The first is the one Commands and the menus show.
    let ws = workspace();
    assert_eq!(
        ws.shortcut_chords("client.zoom.in", &Value::Null),
        zoom.keys[0]
    );
    // The numpad's key still finds the row.
    let found = listing(rows, "ctrl+[NumpadAdd]");
    let (keys, on_key) = found.key.expect("a key");
    assert_eq!(keys, "Ctrl NumpadAdd");
    assert_eq!(titles(&on_key), ["Increase interface zoom"]);
    std::fs::remove_file(&ws.settings_path).ok();

    // A key from shortcuts.json joins the default's row and is named there.
    let rows = self::rows(
        &local(json!([{"keys": "ctrl+j", "action": "client.tab.new"}])),
        &[],
        &[],
    );
    let new_tab = titled(&rows, "New tab");
    assert_eq!(
        new_tab.keys,
        [caps(&[&["Ctrl", "J"]]), caps(&[&["Ctrl", "Shift", "T"]])]
    );
    assert_eq!(new_tab.notes, ["Ctrl J from shortcuts.json"]);
    // A sequence is one binding of two chords.
    let rows = self::rows(
        &local(json!([{"keys": "ctrl+b>c", "action": "client.tab.close"}])),
        &[],
        &[],
    );
    let close = titled(&rows, "Close tab");
    assert_eq!(close.keys, [caps(&[&["Ctrl", "B"], &["C"]])]);
    assert_eq!(close.notes, ["shortcuts.json"]);
}

#[test]
fn a_key_given_to_two_actions_says_so_on_both_rows() {
    let rows = self::rows(
        &local(json!([{"keys": "ctrl+k", "action": "client.tab.new"}])),
        &[],
        &[],
    );
    let new_tab = titled(&rows, "New tab");
    assert_eq!(
        new_tab.notes,
        [
            "Ctrl K from shortcuts.json",
            "Ctrl K is also bound to Switch session"
        ]
    );
    // The default it took never runs: the row has no key and says who has it.
    let switch = titled(&rows, "Switch session");
    assert!(switch.keys.is_empty());
    assert_eq!(switch.notes, ["Ctrl K is taken by New tab"]);

    // The key lists both, the one it performs first, under any spelling of it.
    for query in ["ctrl+k", " Control+K ", "ctrl + k"] {
        let found = listing(rows.clone(), query);
        let (keys, on_key) = found.key.expect("a key");
        assert_eq!(keys, "Ctrl K");
        assert_eq!(titles(&on_key), ["New tab", "Switch session"], "{query}");
        assert!(found.tie);
        assert!(found.rows.is_empty());
    }

    // A sequence takes its first chord from the key bound alone, which is not a shared key.
    let rows = self::rows(
        &local(json!([{"keys": "ctrl+k>t", "action": "client.tab.new"}])),
        &[],
        &[],
    );
    assert_eq!(
        titled(&rows, "New tab").notes,
        ["Ctrl K, T from shortcuts.json"]
    );
    assert_eq!(
        titled(&rows, "Switch session").notes,
        ["Ctrl K is taken by New tab"]
    );
    let found = listing(rows, "ctrl+k");
    assert_eq!(
        titles(&found.key.as_ref().unwrap().1),
        ["New tab", "Switch session"]
    );
    assert!(!found.tie, "one starts a sequence, one is the key alone");
}

/// The helper text's promise, for every key of a keymap with shared keys in it.
#[test]
fn the_first_result_of_a_key_is_the_action_the_key_performs() {
    let map = local(json!([
        {"keys": "ctrl+k", "action": "client.tab.new"},
        {"keys": "ctrl+shift+d", "action": "pane.zoom"},
        {"keys": "ctrl+shift+t", "action": "pane.split", "args": {"direction": "up"}},
        {"keys": "ctrl+b>c", "action": "client.tab.close"},
        {"keys": "ctrl+b>n", "action": "client.tab.next"},
    ]));
    let rows = rows(&map, &[], &[]);
    let effective = actions::effective(&map, &[]);
    assert!(effective.len() > 30);
    for binding in effective {
        let query = binding.binding.keys.join(">");
        let found = listing(rows.clone(), &query);
        let (_, on_key) = found.key.unwrap_or_else(|| panic!("{query} is a key"));
        let first = on_key.first().unwrap_or_else(|| panic!("{query}: no row"));
        assert_eq!(first.action, binding.binding.action, "{query}");
        // A row's arguments are what its bindings run; when they run is in its helper line.
        let runs = binding.binding.args.clone().unwrap_or(Value::Null);
        assert_eq!(
            first.args.clone().unwrap_or(Value::Null),
            actions::without_when(&runs),
            "{query}"
        );
    }
    // Every key that runs something is on a row, and no row shows one that does not run.
    let shown: usize = rows.iter().map(|row| row.keys.len()).sum();
    let folded = rows
        .iter()
        .filter(|row| row.notes.iter().any(|note| note == "Also on the numpad"))
        .count();
    assert_eq!(shown + folded, actions::effective(&map, &[]).len());
}

#[test]
fn the_filter_takes_a_key_or_a_name() {
    let rows = default_rows();
    // The roadmap's check: the key lists Split right.
    let found = listing(rows.clone(), "ctrl+shift+d");
    let (keys, on_key) = found.key.clone().expect("a key");
    assert_eq!(keys, "Ctrl Shift D");
    assert_eq!(titles(&on_key), ["Split right"]);
    assert!(!found.tie && found.rows.is_empty());

    // A key nothing is bound to is still a key, and says so.
    let found = listing(rows.clone(), "ctrl+shift+j");
    assert_eq!(found.key, Some(("Ctrl Shift J".into(), vec![])));
    assert!(found.is_empty());

    // A prefix of a chord is not a key: it is looked for in the keycaps.
    let found = listing(rows.clone(), "ctrl+shift");
    assert!(found.key.is_none());
    assert!(found.rows.len() > 10);
    assert!(
        found.rows.iter().all(|row| row
            .keys
            .iter()
            .flatten()
            .any(|chord| chord.starts_with(&["Ctrl".to_owned(), "Shift".to_owned()]))),
        "{:?}",
        titles(&found.rows)
    );

    // A name, in any case; rows keep the page's order.
    let found = listing(rows.clone(), "SPLIT");
    assert!(found.key.is_none());
    assert_eq!(
        titles(&found.rows),
        [
            "Split right",
            "Split left",
            "Split down",
            "Split up",
            "Split best fit"
        ]
    );
    // `tab` is a key's name too, but nothing is bound to Tab alone: it finds names.
    let found = listing(rows.clone(), "tab");
    assert!(found.key.is_none());
    assert!(titles(&found.rows).contains(&"New tab"));
    assert!(titles(&found.rows).contains(&"Next tab across hosts"));
    // An action's name as shortcuts.json writes it.
    assert_eq!(
        titles(&listing(rows.clone(), "client.tab.close").rows),
        ["Close tab", "Close other tabs"]
    );
    // What is not set.
    let unset = listing(rows.clone(), "not set");
    assert!(unset.rows.len() > 20);
    assert!(unset.rows.iter().all(|row| row.keys.is_empty()));
    // Nothing.
    assert!(listing(rows.clone(), "zzzz").is_empty());
    // No query is every row.
    assert_eq!(listing(rows.clone(), "  ").rows, rows);
}

#[test]
fn the_hosts_actions_are_listed_under_server_bound_or_not() {
    let mut map = local(json!([]));
    // As a host's reply has them: its own bindings carry no source.
    map.bindings.insert(
        0,
        Binding::new(["ctrl+shift+l"], "layout_tall")
            .args(json!({}))
            .into(),
    );
    map.bindings
        .insert(1, Binding::new(["ctrl+shift+y"], "untitled").into());
    let action = |name: &str, title: Option<&str>| {
        let mut action = InspectResponseAction::new(name).kind("lua");
        action.title = title.map(str::to_owned);
        action
    };
    let server = [
        action("layout_tall", Some("Tall layout")),
        action("layout_wide", Some("Wide layout")),
        action("untitled", None),
    ];
    let offered = [
        ("layout_tall".to_owned(), "Tall layout".to_owned()),
        ("layout_wide".to_owned(), "Wide layout".to_owned()),
    ];
    let rows = rows(&map, &server, &offered);
    let listed: Vec<_> = rows
        .iter()
        .filter(|row| row.category == "Server")
        .map(|row| (row.title.as_str(), row.keys.clone(), row.notes.clone()))
        .collect();
    assert_eq!(
        listed,
        [
            (
                "Tall layout",
                vec![caps(&[&["Ctrl", "Shift", "L"]])],
                vec!["Server".to_owned()]
            ),
            ("Wide layout", vec![], vec![]),
            (
                "untitled",
                vec![caps(&[&["Ctrl", "Shift", "Y"]])],
                vec!["Server".to_owned()]
            ),
        ]
    );
    // An action the server does not offer cannot run: its binding is not a row.
    let mut map = local(json!([]));
    map.bindings
        .insert(0, Binding::new(["ctrl+shift+l"], "gone").into());
    assert!(
        self::rows(&map, &[], &[])
            .iter()
            .all(|row| row.category != "Server")
    );
}

/// `overrides` as the workspace's shortcuts.json, without a file. Settings reads the file
/// again when it opens, so this comes after.
fn set_overrides(ws: &mut Workspace, overrides: Value) {
    ws.settings.shortcut_file.overrides = Overrides::parse(&overrides);
    ws.local_keymap = actions::resolve(&ws.settings.shortcut_file.overrides);
    ws.keymap_generation += 1;
}

#[test]
fn a_default_key_taken_away_in_shortcuts_json_is_named_on_its_row() {
    let mut ws = workspace();
    set_overrides(
        &mut ws,
        json!([
            {"keys": "ctrl+k", "action": null},
            {"keys": "ctrl+shift+d", "action": null},
            {"keys": "ctrl+shift+t", "action": null},
            {"keys": "ctrl+shift+t", "action": "client.tab.new"},
        ]),
    );
    let rows = ws.shortcut_rows();
    let switch = titled(&rows, "Switch session");
    assert!(switch.keys.is_empty());
    assert_eq!(switch.notes, ["Ctrl K removed in shortcuts.json"]);
    assert_eq!(
        titled(&rows, "Split right").notes,
        ["Ctrl Shift D removed in shortcuts.json"]
    );
    // Taken away and given back: the row has the key, so nothing was lost.
    let new_tab = titled(&rows, "New tab");
    assert_eq!(new_tab.keys, [caps(&[&["Ctrl", "Shift", "T"]])]);
    assert_eq!(new_tab.notes, ["shortcuts.json"]);
    std::fs::remove_file(&ws.settings_path).ok();
}

/// Settings open on Shortcuts in a window large enough for the whole dialog, at rest.
fn shortcuts_open(ws: Workspace) -> Runtime<Workspace> {
    let mut ws = ws;
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Shortcuts);
    let mut rt = runtime_at(ws, (1440., 1000.));
    settle(&mut rt);
    rt
}

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

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.
}

/// The drawn text `words`, and where.
fn text_rects(rt: &Runtime<Workspace>, words: &str) -> Vec<[f32; 4]> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting && e.text.as_deref() == Some(words))
        .map(|e| e.rect)
        .collect()
}

/// The keys of the action rows on the page, in order.
fn row_keys(rt: &Runtime<Workspace>) -> Vec<String> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting)
        .filter_map(|e| e.key)
        .filter(|key| key.starts_with("shortcut:"))
        .collect()
}

fn type_in(rt: &mut Runtime<Workspace>, words: &str) {
    for c in words.chars() {
        press(rt, Key::Text(c.to_string()));
    }
    settle(rt);
}

#[test]
fn the_page_draws_a_row_for_every_action_with_its_keys_or_not_set() {
    let rt = shortcuts_open(workspace_hosts(2));
    let expected: Vec<String> = rt
        .state
        .shortcut_rows()
        .iter()
        .map(|row| format!("shortcut:{}", row.id()))
        .collect();
    assert!(expected.len() >= actions::SPECS.len());
    let mut drawn = row_keys(&rt);
    let mut sorted = expected.clone();
    drawn.sort();
    sorted.sort();
    assert_eq!(drawn, sorted, "one row an action, each drawn once");

    for row in rt.state.shortcut_rows() {
        let frame = rect(&rt, &format!("shortcut:{}", row.id()));
        let keys = rect(&rt, &format!("shortcut-keys:{}", row.id()));
        assert!(
            close(keys[0] + keys[2], frame[0] + frame[2] - 12.),
            "{}: its keys end at the card's inset: {keys:?} in {frame:?}",
            row.title
        );
        assert!(
            keys[0] > frame[0] + frame[2] / 2.,
            "{}: keys on the right",
            row.title
        );
        assert!(
            keys[1] >= frame[1] && keys[1] + keys[3] <= frame[1] + frame[3],
            "{}: keys inside the row: {keys:?} in {frame:?}",
            row.title
        );
    }
    // A name starts at the inset and ends before its keys.
    let frame = rect(&rt, "shortcut:client.tab.next");
    let title = text_rects(&rt, "Next tab")[0];
    let keys = rect(&rt, "shortcut-keys:client.tab.next");
    assert!(close(title[0], frame[0] + 12.) && title[0] + title[2] < keys[0]);
    // The keys are the keycaps menus and Commands draw: 17 pt chips on a 7% white fill.
    let chip = crate::app::theme::white(crate::app::theme::KEYCAP_FILL);
    let caps = rt
        .elements()
        .iter()
        .filter(|e| !e.exiting && e.fill == chip && close(e.rect[3], 17.))
        .count();
    let bound: usize = rt
        .state
        .shortcut_rows()
        .iter()
        .flat_map(|row| row.keys.iter().flatten())
        .map(Vec::len)
        .sum();
    assert_eq!(caps, bound, "a chip a key");
    // An unbound action has Record shortcut, or reads Not set when it needs an argument no
    // key recorded here gives; the pointer's two say how the pointer runs them.
    let rows = rt.state.shortcut_rows();
    let unbound = |recordable: bool| {
        rows.iter()
            .filter(|row| row.keys.is_empty() && row.recordable == recordable)
            .count()
    };
    assert!(unbound(true) > 20 && unbound(false) >= 1);
    assert_eq!(text_rects(&rt, "Record shortcut").len(), unbound(true));
    assert_eq!(text_rects(&rt, "Not set").len(), unbound(false));
    assert_eq!(text_rects(&rt, "Mouse Back").len(), 1);
    assert_eq!(text_rects(&rt, "Mouse Forward").len(), 1);
    // Named as Commands and the menus name them, each once.
    for title in [
        "Split right",
        "Split down",
        "Move tab left",
        "Move tab right",
        "Close pane or tab",
        "Focus pane left",
    ] {
        assert_eq!(text_rects(&rt, title).len(), 1, "{title}");
    }
    assert_eq!(text_rects(&rt, "Also on the numpad").len(), 3);

    // Alternates are stacked on one row, which grows for them; every line ends on the inset.
    let zoom = rect(&rt, "shortcut:client.zoom.in");
    let plain = rect(&rt, "shortcut:client.settings");
    assert!(zoom[3] > plain[3] + 30., "{zoom:?} against {plain:?}");
    assert!(close(plain[3], 40.), "a plain row: {plain:?}");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_filter_stays_above_the_rows_and_narrows_them_by_key() {
    let mut rt = shortcuts_open(workspace_hosts(2));
    let field = rect(&rt, "shortcut-filter");
    let scroll = rect(&rt, "settings-scroll:shortcuts");
    // The Configuration menu's button ends the filter's row.
    let menu = rect(&rt, crate::app::recorder::MENU);
    assert!(
        field[1] + field[3] <= scroll[1]
            && close(field[0], scroll[0] + 20.)
            && close(field[0] + field[2], menu[0] - 8.)
            && close(menu[0] + menu[2], scroll[0] + scroll[2] - 20.)
            && close(menu[1], field[1])
            && close(menu[3], field[3]),
        "the filter is above the scroll region, inset as the page is: {field:?} and {menu:?} over {scroll:?}"
    );
    // Scrolled, the rows move and the filter does not.
    rt.wheel(
        scroll[0] + scroll[2] / 2.,
        scroll[1] + scroll[3] / 2.,
        -900.,
    );
    settle(&mut rt);
    assert_eq!(rect(&rt, "shortcut-filter"), field);
    assert!(
        rect(&rt, "shortcut:client.tab.next")[1] < scroll[1],
        "the first row went under it"
    );

    click(&mut rt, "shortcut-filter");
    type_in(&mut rt, "ctrl+shift+d");
    assert_eq!(rt.state.shortcut_filter.text, "ctrl+shift+d");
    assert_eq!(row_keys(&rt), ["shortcut:pane.split:right"]);
    assert_eq!(text_rects(&rt, "Bound to Ctrl Shift D").len(), 1);
    // The results are at the top of the page again, under the filter.
    let row = rect(&rt, "shortcut:pane.split:right");
    assert!(
        row[1] > scroll[1] && row[1] < scroll[1] + 80.,
        "{row:?} in {scroll:?}"
    );
    // The page's own explanations make way for results.
    assert!(!has(&rt, "shortcuts-help") && !has(&rt, "shortcuts-path"));

    // Escape empties the filter and keeps Settings; a second Escape closes Settings.
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    assert_eq!(rt.state.shortcut_filter.text, "");
    assert!(rt.state.panel.is_some());
    assert!(row_keys(&rt).len() >= actions::SPECS.len());
    assert!(has(&rt, "shortcuts-help") && has(&rt, "shortcuts-path"));
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    assert!(rt.state.panel.is_none());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_filter_says_when_nothing_matches_and_starts_empty_each_time_settings_opens() {
    let mut rt = shortcuts_open(workspace_hosts(2));
    click(&mut rt, "shortcut-filter");
    type_in(&mut rt, "zzzz");
    assert!(row_keys(&rt).is_empty());
    assert_eq!(text_rects(&rt, "No action or key matches “zzzz”.").len(), 1);
    // The clear button empties it.
    let clear = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.role == "button" && e.label.as_deref() == Some("Clear"))
        .expect("a clear button while there is a query")
        .rect;
    rt.pointer_down(clear[0] + clear[2] / 2., clear[1] + clear[3] / 2.);
    rt.pointer_up(clear[0] + clear[2] / 2., clear[1] + clear[3] / 2.);
    settle(&mut rt);
    assert_eq!(rt.state.shortcut_filter.text, "");
    assert!(row_keys(&rt).len() >= actions::SPECS.len());

    // A key nothing has.
    click(&mut rt, "shortcut-filter");
    type_in(&mut rt, "ctrl+shift+j");
    assert!(row_keys(&rt).is_empty());
    assert_eq!(
        text_rects(
            &rt,
            "Nothing is bound to Ctrl Shift J; the key goes to the terminal."
        )
        .len(),
        1
    );
    // Closed and opened again, Settings does not keep the query.
    rt.update(|s, tx| {
        s.close_panel(tx);
        s.perform(Intent::Settings, tx);
    });
    settle(&mut rt);
    assert_eq!(rt.state.settings_section, SettingsSection::Shortcuts);
    assert_eq!(rt.state.shortcut_filter.text, "");
    assert!(row_keys(&rt).len() >= actions::SPECS.len());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// The keymap host 0's server answered with: its bindings and the actions it offers.
fn server_map(ws: &mut Workspace, bindings: Vec<Binding>, actions: Vec<InspectResponseAction>) {
    let reply = rexrs::api::client::KeymapResponse {
        bindings,
        actions,
        ..Default::default()
    };
    ws.hosts[0].model.keymap = Some(std::sync::Arc::new(
        rex_linux::connection::keymap::Snapshot {
            generation: ws.keymap_generation,
            resolved: Resolved::from_server(&reply, &ws.settings.shortcut_file.overrides),
            reply,
        },
    ));
    ws.hosts[0].model.keymap_revision += 1;
}

#[test]
fn a_connected_hosts_actions_are_rows_under_server() {
    let (mut ws, _host) = tabbed_workspace("split");
    let mut bindings: Vec<Binding> = rex_linux::keymap::defaults()
        .bindings
        .into_iter()
        .map(|mut binding| {
            binding.source = Some("linux".into());
            binding
        })
        .collect();
    bindings.insert(0, Binding::new(["ctrl+shift+l"], "layout_tall"));
    let mut actions = actions::declarations();
    for (name, title) in [
        ("layout_tall", "Tall layout"),
        ("layout_wide", "Wide layout"),
    ] {
        actions.push(InspectResponseAction::new(name).kind("lua").title(title));
    }
    server_map(&mut ws, bindings, actions);
    let rows = ws.shortcut_rows();
    let server: Vec<_> = rows
        .iter()
        .filter(|row| row.category == "Server")
        .map(|row| (row.title.as_str(), row.keys.len()))
        .collect();
    assert_eq!(server, [("Tall layout", 1), ("Wide layout", 0)]);

    let rt = shortcuts_open(ws);
    assert_eq!(
        text_rects(&rt, "Server").len(),
        2,
        "the heading and a source"
    );
    assert!(has(&rt, "shortcut:layout_tall") && has(&rt, "shortcut:layout_wide"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// The page as drawn, for a look (see `Camera`): its top, its end, a key with two actions,
/// a name, a key nothing has, and a narrow window. Nothing is asserted.
#[test]
fn the_shortcuts_page_draws() {
    use super::pane_verb_tests::{Camera, framed_terminal};
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let scene = |size: (f32, f32), overrides: Value| {
        let (mut ws, _host) = tabbed_workspace("split");
        let (sender, _sent) = flume::unbounded();
        framed_terminal(&mut ws, "block:b", &sender, b"$ cargo test");
        ws.perform(Intent::Settings, &mut rune::Tx::default());
        ws.open_section(SettingsSection::Shortcuts);
        // After Settings opened: opening it reads shortcuts.json again, and there is none.
        set_overrides(&mut ws, overrides);
        let mut rt = Runtime::new(ws, view, size, Env::new(true, 2.));
        rt.on_key = Some(crate::app::input::on_key);
        settle(&mut rt);
        rt
    };
    let shared = json!([
        {"keys": "ctrl+k", "action": "client.tab.new"},
        {"keys": "ctrl+b>c", "action": "client.tab.close"},
        {"keys": "alt+1", "action": "client.tab.goto", "args": {"index": 1}},
    ]);
    let wheel = |rt: &mut Runtime<Workspace>, by: f32| {
        let scroll = rect(rt, "settings-scroll:shortcuts");
        rt.wheel(scroll[0] + scroll[2] / 2., scroll[1] + scroll[3] / 2., by);
        settle(rt);
    };
    let done = |rt: Runtime<Workspace>| {
        std::fs::remove_file(&rt.state.settings_path).ok();
    };

    let mut rt = scene((1024., 700.), json!([]));
    camera.snap(&mut rt, "shortcuts-top");
    wheel(&mut rt, -1500.);
    camera.snap(&mut rt, "shortcuts-middle");
    wheel(&mut rt, -750.);
    camera.snap(&mut rt, "shortcuts-interface");
    wheel(&mut rt, -9000.);
    camera.snap(&mut rt, "shortcuts-end");
    done(rt);

    let mut rt = scene((1024., 700.), shared.clone());
    camera.snap(&mut rt, "shortcuts-overrides");
    click(&mut rt, "shortcut-filter");
    type_in(&mut rt, "ctrl+k");
    camera.snap(&mut rt, "shortcuts-filter-key");
    done(rt);

    let mut rt = scene((1024., 700.), shared);
    click(&mut rt, "shortcut-filter");
    type_in(&mut rt, "tab");
    camera.snap(&mut rt, "shortcuts-filter-name");
    done(rt);

    let mut rt = scene((1024., 700.), json!([]));
    click(&mut rt, "shortcut-filter");
    type_in(&mut rt, "ctrl+shift+j");
    camera.snap(&mut rt, "shortcuts-filter-free-key");
    done(rt);

    let mut rt = scene((700., 520.), json!([]));
    camera.snap(&mut rt, "shortcuts-narrow");
    done(rt);
}

#[test]
fn the_helper_names_recording_and_the_filter_and_the_layers_are_under_files() {
    let rt = shortcuts_open(workspace_hosts(2));
    let words_of = |key: &str| {
        rt.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .and_then(|e| e.text)
            .unwrap_or_else(|| panic!("{key}"))
    };
    // Two short sentences open the page: what a row's keys do, and what the filter takes.
    let help = words_of("shortcuts-help");
    for words in ["Record shortcut", "ctrl+k"] {
        assert!(help.contains(words), "{words}: {help}");
    }
    assert!(
        help.matches(". ").count() < 2 && help.len() < 140,
        "the opening stays short: {help}"
    );
    // Which layer wins, and the way here from Commands, are with the files they are about.
    let layers = words_of("shortcuts-layers");
    for words in ["recorded here wins", "shortcuts.json", "Ctrl Enter"] {
        assert!(layers.contains(words), "{words}: {layers}");
    }
    assert!(rect(&rt, "shortcuts-layers")[1] > rect(&rt, "shortcuts-path")[1]);
    // It is the first thing on the page, above the first heading.
    let top = rect(&rt, "shortcuts-help");
    let heading = text_rects(&rt, SHORTCUT_CATEGORIES[0])[0];
    assert!(top[1] + top[3] < heading[1]);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_filter_is_drawn_as_the_other_search_fields_of_settings_are() {
    let rt = shortcuts_open(workspace_hosts(2));
    let fill = |key: &str| {
        rt.elements()
            .into_iter()
            .find(|e| !e.exiting && e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("missing {key}"))
            .fill
    };
    // One raised surface for Search Settings and the filter beside it, not a sunken field.
    assert_eq!(fill("shortcut-filter-box"), fill("settings-search-box"));
    assert!(fill("shortcut-filter-box").a > 0.);
    let (surface, field) = (
        rect(&rt, "shortcut-filter-box"),
        rect(&rt, "shortcut-filter"),
    );
    assert!(
        close(surface[0], field[0]) && close(surface[2], field[2]) && close(surface[3], field[3]),
        "the field fills its surface: {field:?} in {surface:?}"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_files_card_names_each_file_and_keeps_its_path_on_one_line() {
    let (mut ws, _host) = tabbed_workspace("split");
    server_map(&mut ws, vec![], vec![]);
    let snapshot = std::sync::Arc::make_mut(ws.hosts[0].model.keymap.as_mut().unwrap());
    snapshot.reply.config.path =
        "/home/someone-with-a-long-name/.config/rex/profiles/workstation/init.lua".into();
    snapshot.reply.config.exists = false;
    let mut rt = shortcuts_open(ws);
    let scroll = rect(&rt, "settings-scroll:shortcuts");
    rt.pointer_move(scroll[0] + 40., scroll[1] + 40.);
    rt.wheel(scroll[0] + 40., scroll[1] + 40., -20000.);
    settle(&mut rt);
    // Both rows are a label and a path: neither is a bare path, and nothing is in brackets.
    let (server, own) = (
        rect(&rt, "server-shortcuts-path"),
        rect(&rt, "shortcuts-path"),
    );
    let labels = (
        text_rects(&rt, "Server configuration"),
        text_rects(&rt, "shortcuts.json"),
    );
    assert_eq!((labels.0.len(), labels.1.len()), (1, 1));
    assert!(
        close(labels.0[0][0], labels.1[0][0]),
        "the labels share an edge"
    );
    assert!(
        close(server[0] + server[2], own[0] + own[2]),
        "and the paths end on one: {server:?} {own:?}"
    );
    assert!(
        !rt.elements()
            .iter()
            .any(|e| e.text.as_deref().is_some_and(|t| t.contains("(not"))),
        "a file that is not there says so in a chip"
    );
    let chip = rect(&rt, "server-shortcuts-missing");
    assert!(chip[0] + chip[2] <= server[0], "before the path: {chip:?}");
    // A long path is one line, cut in its middle inside the card, never wrapped.
    assert!(
        server[3] < 20. && labels.0[0][0] + labels.0[0][2] <= chip[0],
        "one line that leaves the label whole: {server:?}"
    );
    assert!(server[0] + server[2] <= scroll[0] + scroll[2] - 20.);
    std::fs::remove_file(&rt.state.settings_path).ok();
}
