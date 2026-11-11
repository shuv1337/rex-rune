//! Recording shortcuts in Settings → Shortcuts: the recorder's keys, the layer it writes
//! and where that is saved, what a taken key says, Remove and Reset all, the Configuration
//! menu, and Ctrl Enter in Commands.
use super::*;
use crate::app::{
    actions,
    panel::SettingsSection,
    recorder::{self, FIELD, MENU, Verdict},
    shortcuts_page::Row,
};
use rex_linux::{
    keymap::{Chord, RECORDED},
    settings::shortcuts::Shortcuts,
};
use serde_json::{Value, json};

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..40 {
        rt.frame(1. / 60.);
    }
}

/// Settings open on Shortcuts, at rest, in a window that shows the whole dialog.
fn open(ws: Workspace) -> Runtime<Workspace> {
    let mut ws = ws;
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Shortcuts);
    let mut rt = runtime_at(ws, (1440., 1000.));
    settle(&mut rt);
    rt
}

/// Press a control of the page, scrolled into view first.
fn reach(rt: &mut Runtime<Workspace>, key: &str) {
    let row = key.to_owned();
    rt.update(move |_, tx| tx.scroll_into_view(row));
    settle(rt);
    click(rt, key);
}

fn chord(rt: &mut Runtime<Workspace>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    settle(rt);
}

fn ctrl_shift_mods() -> Mods {
    Mods {
        ctrl: true,
        cmd: true,
        shift: true,
        ..Mods::default()
    }
}

/// Ctrl Shift and a letter, as a native keyboard sends it.
fn ctrl_shift_key(rt: &mut Runtime<Workspace>, c: char) {
    chord(rt, Key::Command(c), ctrl_shift_mods());
}

fn ctrl_key(rt: &mut Runtime<Workspace>, c: char) {
    chord(
        rt,
        Key::Command(c),
        Mods {
            ctrl: true,
            cmd: true,
            ..Mods::default()
        },
    );
}

fn row<'a>(rows: &'a [Row], title: &str) -> &'a Row {
    rows.iter()
        .find(|row| row.title == title)
        .unwrap_or_else(|| panic!("no row {title}"))
}

fn spelled(row: &Row) -> Vec<String> {
    row.keys
        .iter()
        .map(|chords| crate::app::keycaps::spell(chords))
        .collect()
}

fn status(rt: &Runtime<Workspace>) -> String {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("shortcut-recording-status"))
        .and_then(|e| e.text)
        .expect("the recorder's status")
}

fn typed(rt: &Runtime<Workspace>) -> Vec<String> {
    rt.state
        .recorder
        .as_ref()
        .expect("recording")
        .chords
        .iter()
        .map(Chord::spelling)
        .collect()
}

/// What the local keymap runs for `keys`: the action and its arguments.
fn runs(ws: &Workspace, keys: &[&str]) -> Option<(String, Value)> {
    let server = [];
    actions::effective(&ws.local_keymap, &server)
        .into_iter()
        .find(|bound| bound.binding.keys == keys)
        .map(|bound| {
            (
                bound.binding.action.clone(),
                bound.binding.args.clone().unwrap_or(Value::Null),
            )
        })
}

/// A workspace in its own directory, beside a `shortcuts.json` with `contents`.
fn with_shortcuts_file(contents: &str) -> (Workspace, std::path::PathBuf) {
    let mut ws = workspace();
    let directory = ws.settings_path.with_extension("recorder-test");
    std::fs::create_dir_all(&directory).unwrap();
    ws.settings_path = directory.join("workspace.json");
    ws.settings.save(&ws.settings_path).unwrap();
    std::fs::write(Shortcuts::path(&ws.settings_path), contents).unwrap();
    (relaunch(&ws), directory)
}

#[test]
fn a_recorded_key_works_at_once_survives_a_restart_and_never_touches_shortcuts_json() {
    // A file with its own spacing and a comment-like key: any rewrite would change it.
    let file = "[\n  {\"keys\":   \"ctrl+alt+u\", \"action\": \"pane.zoom\"}\n]\n\n";
    let (mut ws, directory) = with_shortcuts_file(file);
    let shortcuts = Shortcuts::path(&ws.settings_path);
    let rx = capture_host(&mut ws, 0);
    // Nothing recorded: the keymap is declared as earlier versions declared it.
    let layers: Vec<String> = ws
        .keymap_declaration()
        .request
        .overrides
        .iter()
        .map(|layer| layer.name.clone())
        .collect();
    assert_eq!(layers, ["settings"]);

    let mut rt = open(ws);
    reach(&mut rt, "shortcut-key:pane.split:right:0");
    settle(&mut rt);
    assert!(rt.state.recorder.is_some(), "the row is recording");
    assert!(
        focused(&rt).is_some_and(|path| path.contains(FIELD)),
        "the recorder has the keyboard: {:?}",
        focused(&rt)
    );
    assert!(has(&rt, "shortcut-remove") && has(&rt, "shortcut-cancel"));
    assert!(!has(&rt, "shortcut-save"), "nothing to save yet");
    assert!(status(&rt).contains("Backspace removes Ctrl Shift D;"));

    ctrl_shift_key(&mut rt, 'j');
    assert_eq!(typed(&rt), ["ctrl+shift+j"]);
    assert!(status(&rt).starts_with("Enter saves"), "{}", status(&rt));
    assert!(has(&rt, "shortcut-save"));
    // Nothing is in effect until it is saved.
    assert!(rt.state.settings.recorded_shortcuts.is_empty());
    assert!(runs(&rt.state, &["ctrl+shift+j"]).is_none());

    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(rt.state.recorder.is_none());
    assert_eq!(
        runs(&rt.state, &["ctrl+shift+j"]),
        Some(("pane.split".into(), json!({"direction": "right"})))
    );
    assert!(runs(&rt.state, &["ctrl+shift+d"]).is_none());
    // The file's own binding is still under it.
    assert_eq!(
        runs(&rt.state, &["ctrl+alt+u"]).map(|(action, _)| action),
        Some("pane.zoom".into())
    );
    let rows = rt.state.shortcut_rows();
    let split = row(&rows, "Split right");
    assert_eq!(spelled(split), ["Ctrl Shift J"]);
    assert_eq!(
        split.notes,
        ["Recorded here", "Ctrl Shift D removed here"],
        "the row says where its key is from and what it replaced"
    );
    // Every host is told, with the recorded layer above the file's.
    let declared: Vec<_> = rx
        .try_iter()
        .filter_map(|command| match command {
            HostCommand::Keymap(declaration) => Some(declaration),
            _ => None,
        })
        .collect();
    let last = declared.last().expect("the keymap was declared again");
    assert_eq!(last.generation, rt.state.keymap_generation);
    let layers: Vec<&str> = last
        .request
        .overrides
        .iter()
        .map(|layer| layer.name.as_str())
        .collect();
    assert_eq!(layers, ["settings", RECORDED]);
    let recorded = &last.request.overrides[1];
    assert_eq!(recorded.bindings[0].keys, ["ctrl+shift+j"]);
    assert_eq!(recorded.removals[0].keys, ["ctrl+shift+d"]);
    assert_eq!(recorded.removals[0].action.as_deref(), Some("pane.split"));

    // A second launch reads it from the app's settings file.
    let again = relaunch(&rt.state);
    assert_eq!(
        again.settings.recorded_shortcuts,
        rt.state.settings.recorded_shortcuts
    );
    assert_eq!(
        runs(&again, &["ctrl+shift+j"]).map(|(action, _)| action),
        Some("pane.split".into())
    );
    assert!(runs(&again, &["ctrl+shift+d"]).is_none());
    assert_eq!(
        std::fs::read_to_string(&shortcuts).unwrap(),
        file,
        "shortcuts.json is byte for byte what the user wrote"
    );

    // Reset all, from the Configuration menu and after asking, restores the defaults.
    click(&mut rt, MENU);
    settle(&mut rt);
    let labels: Vec<(String, bool)> = rt
        .state
        .entries()
        .into_iter()
        .map(|entry| (entry.label, entry.enabled))
        .collect();
    assert_eq!(
        labels,
        [
            ("2 recorded changes".to_owned(), false),
            ("Copy shortcuts.json path".to_owned(), true),
            ("Copy recorded shortcuts as JSON".to_owned(), true),
            ("Reset all…".to_owned(), true),
        ]
    );
    rt.update(|s, tx| s.perform(Intent::Shortcuts(recorder::Menu::ResetAll), tx));
    settle(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::ResetShortcuts)
    ));
    assert!(
        !rt.state.settings.recorded_shortcuts.is_empty(),
        "asked first"
    );
    // Return keeps them: the default button is Keep shortcuts.
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    assert_eq!(rt.state.settings.recorded_shortcuts.len(), 2);
    click(&mut rt, MENU);
    settle(&mut rt);
    rt.update(|s, tx| s.perform(Intent::Shortcuts(recorder::Menu::ResetAll), tx));
    settle(&mut rt);
    click(&mut rt, "confirm-close");
    settle(&mut rt);
    assert!(rt.state.settings.recorded_shortcuts.is_empty());
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    assert_eq!(
        runs(&rt.state, &["ctrl+shift+d"]),
        Some(("pane.split".into(), json!({"direction": "right"})))
    );
    assert!(runs(&rt.state, &["ctrl+shift+j"]).is_none());
    let saved: Value =
        serde_json::from_slice(&std::fs::read(&rt.state.settings_path).unwrap()).unwrap();
    assert!(saved.get("recorded_shortcuts").is_none());
    assert_eq!(std::fs::read_to_string(&shortcuts).unwrap(), file);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_recorded_key_runs_its_action_in_the_workspace_and_the_old_key_does_not() {
    let (ws, rx) = tabbed_workspace("split");
    let mut rt = open(ws);
    reach(&mut rt, "shortcut-key:pane.split:right:0");
    settle(&mut rt);
    ctrl_shift_key(&mut rt, 'j');
    click(&mut rt, "shortcut-save");
    settle(&mut rt);
    assert!(rt.state.recorder.is_none());
    rt.update(|s, tx| s.close_panel(tx));
    settle(&mut rt);
    let _ = requests(&rx);

    ctrl_shift_key(&mut rt, 'd');
    assert!(requests(&rx).is_empty(), "the old key is the terminal's");
    ctrl_shift_key(&mut rt, 'j');
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "session.new_split");
    // Hints follow at once: the pane menu and Commands show the new key.
    assert_eq!(
        rt.state
            .shortcut_detail("pane.split", &json!({"direction": "right"})),
        "Ctrl Shift J"
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_key_another_action_has_is_said_before_it_is_saved() {
    let mut rt = open(workspace());
    reach(&mut rt, "shortcut-key:pane.split:right:0");
    settle(&mut rt);
    ctrl_key(&mut rt, 'k');
    assert_eq!(typed(&rt), ["ctrl+k"]);
    assert_eq!(
        status(&rt),
        "Also bound to Switch session, which loses the key if you save."
    );
    let rows = rt.state.shortcut_rows();
    assert_eq!(
        rt.state.recorder.as_ref().unwrap().verdict(&rows),
        Verdict::Taken {
            title: "Switch session".into(),
            keys: "Ctrl K".into(),
            exact: true
        }
    );
    // Said, not done: the key is still the switcher's.
    assert_eq!(
        runs(&rt.state, &["ctrl+k"]).map(|(action, _)| action),
        Some("session.switch".into())
    );
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!(
        runs(&rt.state, &["ctrl+k"]).map(|(action, _)| action),
        Some("pane.split".into())
    );
    let rows = rt.state.shortcut_rows();
    assert_eq!(spelled(row(&rows, "Split right")), ["Ctrl K"]);
    assert!(
        row(&rows, "Split right")
            .notes
            .contains(&"Also bound to Switch session".to_owned())
    );
    let switch = row(&rows, "Switch session");
    assert!(switch.keys.is_empty());
    assert_eq!(switch.notes, ["Ctrl K is taken by Split right"]);

    // The keys a row has already are no change, and a sequence that starts with a bound
    // key says which binding stops working.
    reach(&mut rt, "shortcut-key:pane.split:right:0");
    settle(&mut rt);
    ctrl_key(&mut rt, 'k');
    assert_eq!(status(&rt), "Already its shortcut.");
    press(&mut rt, Key::Backspace);
    settle(&mut rt);
    ctrl_shift_key(&mut rt, 'n');
    press(&mut rt, Key::Text("c".into()));
    settle(&mut rt);
    assert_eq!(typed(&rt), ["ctrl+shift+n", "c"]);
    assert_eq!(
        status(&rt),
        "Overlaps Ctrl Shift N (New session), which stops working if you save."
    );
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    assert!(rt.state.recorder.is_none());
    assert_eq!(rt.state.settings.recorded_shortcuts.len(), 2);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_recorder_takes_every_key_and_refuses_one_that_types() {
    let mut rt = open(workspace());
    // An action with no key: Record shortcut, and nothing to remove.
    reach(&mut rt, "shortcut-key:client.tab.close:0");
    settle(&mut rt);
    assert!(rt.state.recorder.is_some());
    assert!(!has(&rt, "shortcut-remove"));
    assert_eq!(status(&rt), "Type the keys. Esc cancels.");

    // A plain character, Enter, Tab and an arrow are the terminal's.
    for key in [Key::Text("a".into()), Key::Enter, Key::Tab, Key::Left] {
        press(&mut rt, key.clone());
        settle(&mut rt);
        assert!(typed(&rt).is_empty(), "{key:?}");
        assert!(status(&rt).starts_with("Start with Ctrl, Alt or Super"));
        assert!(
            rt.state.recorder.is_some() && rt.state.panel.is_some(),
            "{key:?}"
        );
    }
    // A workspace shortcut pressed while recording is recorded, not run: Ctrl , would
    // otherwise be Settings and Ctrl = the interface zoom, which works in every dialog.
    let zoom = rt.state.settings.ui_scale;
    ctrl_key(&mut rt, '=');
    assert_eq!(typed(&rt), ["ctrl+="]);
    assert_eq!(rt.state.settings.ui_scale, zoom);
    press(&mut rt, Key::Backspace);
    settle(&mut rt);
    assert!(typed(&rt).is_empty());
    // A held key is one press; modifiers alone are nothing.
    rt.repeat_key(KeyEvent {
        key: Key::Command('b'),
        mods: Mods {
            ctrl: true,
            cmd: true,
            ..Mods::default()
        },
    });
    settle(&mut rt);
    assert!(typed(&rt).is_empty());

    // A function key starts a shortcut on its own, and so does Shift with a named key.
    press(&mut rt, Key::Function(5));
    settle(&mut rt);
    assert_eq!(typed(&rt), ["f5"]);
    press(&mut rt, Key::Backspace);
    settle(&mut rt);
    chord(
        &mut rt,
        Key::PageUp,
        Mods {
            shift: true,
            ..Mods::default()
        },
    );
    assert_eq!(typed(&rt), ["shift+pageup"]);
    press(&mut rt, Key::Backspace);
    settle(&mut rt);

    // A sequence: the keys after the first may be plain, up to four.
    ctrl_key(&mut rt, 'b');
    for c in ["c", "d", "e"] {
        press(&mut rt, Key::Text(c.into()));
        settle(&mut rt);
    }
    assert_eq!(typed(&rt), ["ctrl+b", "c", "d", "e"]);
    press(&mut rt, Key::Text("f".into()));
    settle(&mut rt);
    assert_eq!(typed(&rt).len(), 4);
    assert!(status(&rt).starts_with("Four keys at most"));
    press(&mut rt, Key::Backspace);
    press(&mut rt, Key::Backspace);
    settle(&mut rt);
    assert_eq!(typed(&rt), ["ctrl+b", "c"]);
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(rt.state.recorder.is_none());
    assert_eq!(
        runs(&rt.state, &["ctrl+b", "c"]).map(|(action, _)| action),
        Some("client.tab.close".into())
    );
    let rows = rt.state.shortcut_rows();
    assert_eq!(spelled(row(&rows, "Close tab")), ["Ctrl B, C"]);
    assert_eq!(row(&rows, "Close tab").notes, ["Recorded here"]);
    // Focus is back on the row's key, so the keyboard can go on from there.
    assert!(
        focused(&rt).is_some_and(|path| path.contains("shortcut-key:client.tab.close:0")),
        "{:?}",
        focused(&rt)
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn escape_cancel_and_a_press_elsewhere_leave_the_keymap_as_it_was() {
    let mut rt = open(workspace());
    let generation = rt.state.keymap_generation;
    type Leave<'a> = &'a dyn Fn(&mut Runtime<Workspace>);
    let leave: [Leave; 3] = [
        &|rt| press(rt, Key::Escape),
        &|rt| click(rt, "shortcut-cancel"),
        // Anywhere outside the recorder: here the filter above the page.
        &|rt| click(rt, "shortcut-filter"),
    ];
    for leave in leave {
        reach(&mut rt, "shortcut-key:pane.zoom:0");
        settle(&mut rt);
        ctrl_shift_key(&mut rt, 'j');
        assert_eq!(typed(&rt), ["ctrl+shift+j"]);
        leave(&mut rt);
        settle(&mut rt);
        assert!(rt.state.recorder.is_none());
        assert!(rt.state.panel.is_some(), "Settings stays open");
        assert!(rt.state.settings.recorded_shortcuts.is_empty());
        assert_eq!(rt.state.keymap_generation, generation);
    }
    // Another section, or Settings opened again, never finds a recorder left behind.
    reach(&mut rt, "shortcut-key:pane.zoom:0");
    settle(&mut rt);
    rt.update(|s, _| s.open_section(SettingsSection::About));
    assert!(rt.state.recorder.is_none());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn remove_takes_a_key_off_its_action_and_recording_it_again_brings_the_default_back() {
    let mut rt = open(workspace());
    reach(&mut rt, "shortcut-key:pane.zoom:0");
    settle(&mut rt);
    click(&mut rt, "shortcut-remove");
    settle(&mut rt);
    assert!(rt.state.recorder.is_none());
    assert!(runs(&rt.state, &["ctrl+shift+enter"]).is_none());
    let rows = rt.state.shortcut_rows();
    let zoom = row(&rows, "Zoom pane");
    assert!(zoom.keys.is_empty());
    assert_eq!(zoom.notes, ["Ctrl Shift Enter removed here"]);
    assert_eq!(rt.state.settings.recorded_shortcuts.len(), 1);
    assert!(has(&rt, "shortcut-key:pane.zoom:0"), "Record shortcut");

    // The same keys again: the removal is forgotten, and nothing is left recorded.
    reach(&mut rt, "shortcut-key:pane.zoom:0");
    settle(&mut rt);
    chord(&mut rt, Key::Enter, ctrl_shift_mods());
    assert_eq!(typed(&rt), ["ctrl+shift+enter"]);
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(rt.state.settings.recorded_shortcuts.is_empty());
    assert_eq!(
        runs(&rt.state, &["ctrl+shift+enter"]).map(|(action, _)| action),
        Some("pane.zoom".into())
    );
    assert!(row(&rt.state.shortcut_rows(), "Zoom pane").notes.is_empty());

    // Backspace with nothing typed removes too, and a key recorded here is forgotten
    // rather than removed: the layer holds no trace of it.
    reach(&mut rt, "shortcut-key:client.tab.close:0");
    settle(&mut rt);
    ctrl_shift_key(&mut rt, 'j');
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!(rt.state.settings.recorded_shortcuts.len(), 1);
    reach(&mut rt, "shortcut-key:client.tab.close:0");
    settle(&mut rt);
    press(&mut rt, Key::Backspace);
    settle(&mut rt);
    assert!(rt.state.recorder.is_none());
    assert!(rt.state.settings.recorded_shortcuts.is_empty());

    // A numbered row keeps its place once its key is gone, so it can take another, and
    // the key it gets waits for the switcher as the default did.
    reach(&mut rt, "shortcut-key:session.goto:1:0");
    settle(&mut rt);
    press(&mut rt, Key::Backspace);
    settle(&mut rt);
    let rows = rt.state.shortcut_rows();
    let first = row(&rows, "Go to session 1");
    assert!(first.keys.is_empty() && first.recordable);
    reach(&mut rt, "shortcut-key:session.goto:1:0");
    settle(&mut rt);
    chord(
        &mut rt,
        Key::Function(1),
        Mods {
            alt: true,
            ..Mods::default()
        },
    );
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!(
        runs(&rt.state, &["alt+f1"]),
        Some((
            "session.goto".into(),
            json!({"index": 1, "when": "switching"})
        ))
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn an_action_that_needs_an_argument_has_no_recorder() {
    let mut rt = open(workspace());
    let rows = rt.state.shortcut_rows();
    let send = row(&rows, "Send key");
    assert!(!send.recordable && send.keys.is_empty());
    assert!(!has(&rt, "shortcut-key:pane.send_key:0"));
    rt.update(|s, tx| s.record_shortcut("pane.send_key", None, tx));
    assert!(rt.state.recorder.is_none());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_key_an_input_method_is_composing_with_is_not_a_shortcut() {
    let mut rt = open(workspace());
    reach(&mut rt, "shortcut-key:pane.zoom:0");
    settle(&mut rt);
    rt.update(|s, tx| {
        assert!(crate::app::input::on_ime(
            s,
            &rune::Ime::Preedit("に".into(), None),
            tx
        ));
    });
    ctrl_shift_key(&mut rt, 'j');
    assert!(typed(&rt).is_empty(), "the input method had the key");
    rt.update(|s, tx| {
        assert!(crate::app::input::on_ime(
            s,
            &rune::Ime::Commit("に".into()),
            tx
        ));
    });
    ctrl_shift_key(&mut rt, 'j');
    assert_eq!(typed(&rt), ["ctrl+shift+j"]);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_configuration_menu_copies_the_path_and_the_recordings_and_starts_empty() {
    let ws = workspace();
    let path = Shortcuts::path(&ws.settings_path).display().to_string();
    let mut rt = open(ws);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    click(&mut rt, MENU);
    settle(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::ShortcutMenu)
    ));
    let entries = rt.state.entries();
    let rows: Vec<(&str, bool, bool)> = entries
        .iter()
        .map(|entry| (entry.label.as_str(), entry.enabled, entry.danger))
        .collect();
    assert_eq!(
        rows,
        [
            ("No recorded shortcuts", false, false),
            ("Copy shortcuts.json path", true, false),
            ("Copy recorded shortcuts as JSON", false, false),
            ("Reset all…", false, true),
        ]
    );
    // The list is over Settings, against its button, and the first row it lights is one
    // that does something.
    let menu = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("menu-surface"))
        .expect("the menu")
        .rect;
    let button = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(MENU))
        .expect("its button")
        .rect;
    assert!(
        menu[1] >= button[1] + button[3] && (menu[0] + menu[2] - button[0] - button[2]).abs() < 1.,
        "{menu:?} under {button:?}"
    );
    assert_eq!(rt.state.panel.as_ref().unwrap().selected, 1);
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!(clipboard.text().map(|text| text.to_string()), Some(path));
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    // Escape closes the menu and leaves Settings.
    click(&mut rt, MENU);
    settle(&mut rt);
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));

    // With something recorded, the JSON is a shortcuts.json file.
    reach(&mut rt, "shortcut-key:pane.split:right:0");
    settle(&mut rt);
    ctrl_shift_key(&mut rt, 'j');
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    click(&mut rt, MENU);
    settle(&mut rt);
    rt.update(|s, tx| s.perform(Intent::Shortcuts(recorder::Menu::CopyJson), tx));
    settle(&mut rt);
    let copied: Value = serde_json::from_str(&clipboard.text().expect("copied")).unwrap();
    assert_eq!(
        copied,
        json!({"shortcuts": [
            {"keys": "ctrl+shift+d", "action": null},
            {"keys": "ctrl+shift+j", "action": "pane.split", "args": {"direction": "right"}},
        ]})
    );
    let parsed = rex_linux::keymap::Overrides::parse(&copied["shortcuts"]);
    assert!(parsed.diagnostics.is_empty());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn ctrl_enter_in_commands_opens_the_highlighted_commands_recorder() {
    let (ws, _rx) = tabbed_workspace("split");
    let mut rt = runtime_at(ws, (1440., 1000.));
    let highlight = |rt: &mut Runtime<Workspace>, label: &str| {
        rt.update(|s, _| s.picker(false));
        settle(rt);
        let index = rt
            .state
            .entries()
            .iter()
            .position(|entry| entry.label == label)
            .unwrap_or_else(|| panic!("no command {label}"));
        rt.update(move |s, _| s.panel.as_mut().unwrap().selected = index);
        settle(rt);
    };
    let ctrl_enter = |rt: &mut Runtime<Workspace>| {
        chord(
            rt,
            Key::Enter,
            Mods {
                ctrl: true,
                cmd: true,
                ..Mods::default()
            },
        );
    };

    highlight(&mut rt, "Split down");
    ctrl_enter(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    assert_eq!(rt.state.settings_section, SettingsSection::Shortcuts);
    assert_eq!(
        rt.state.recorder.as_ref().map(|r| r.row.as_str()),
        Some("pane.split:down")
    );
    // Its row was brought into view, with the recorder holding the keyboard.
    let scroll = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("settings-scroll:shortcuts"))
        .unwrap()
        .rect;
    let field = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(FIELD))
        .expect("the recorder")
        .rect;
    assert!(
        field[1] >= scroll[1] && field[1] + field[3] <= scroll[1] + scroll[3],
        "{field:?} in {scroll:?}"
    );
    assert!(focused(&rt).is_some_and(|path| path.contains(FIELD)));
    chord(
        &mut rt,
        Key::Command('j'),
        Mods {
            ctrl: true,
            cmd: true,
            alt: true,
            ..Mods::default()
        },
    );
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!(
        runs(&rt.state, &["ctrl+alt+j"]),
        Some(("pane.split".into(), json!({"direction": "down"})))
    );
    rt.update(|s, tx| s.close_panel(tx));

    // An unbound command opens on Record shortcut; the action that opens the page is one.
    highlight(&mut rt, "Change shortcuts…");
    ctrl_enter(&mut rt);
    assert_eq!(
        rt.state.recorder.as_ref().map(|r| r.row.as_str()),
        Some(recorder::EDIT)
    );
    rt.update(|s, tx| s.close_panel(tx));

    // A command with no action stays where it is and says why.
    highlight(&mut rt, "Check for updates");
    ctrl_enter(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Picker(false))
    ));
    assert!(rt.state.recorder.is_none());
    assert!(
        rt.state
            .notices
            .iter()
            .any(|n| n.text == "Check for updates has no action a shortcut could run.")
    );
    // Plain Enter still runs the command.
    highlight(&mut rt, "Change shortcuts…");
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    assert_eq!(rt.state.settings_section, SettingsSection::Shortcuts);
    assert!(rt.state.recorder.is_none());
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn opening_the_shortcuts_page_is_a_local_action_no_other_client_can_ask_for() {
    let spec = actions::find(recorder::EDIT).expect("registered");
    assert!(matches!(spec.remote(), actions::Remote::Never(_)));
    assert_eq!(spec.title(), "Change shortcuts");
    assert!(!spec.global(), "not from inside a dialog or a field");
}

/// The recorder as drawn, for a look (see `Camera`): at rest, listening, with a key, with
/// a key something else has, a refusal, the Configuration menu and the Reset confirmation.
/// Nothing is asserted.
#[test]
fn the_recorder_draws() {
    use super::pane_verb_tests::{Camera, framed_terminal};
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let (mut ws, _host) = tabbed_workspace("split");
    let (sender, _sent) = flume::unbounded();
    framed_terminal(&mut ws, "block:b", &sender, b"$ cargo test");
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Shortcuts);
    let mut rt = Runtime::new(ws, view, (1024., 700.), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    let settle = |rt: &mut Runtime<Workspace>| {
        for _ in 0..90 {
            rt.frame(1. / 60.);
        }
    };
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-rest");
    reach(&mut rt, "shortcut-key:client.tab.next:0");
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-listening");
    press(&mut rt, Key::Text("a".into()));
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-refused");
    ctrl_shift_key(&mut rt, 'j');
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-typed");
    press(&mut rt, Key::Backspace);
    ctrl_key(&mut rt, 'k');
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-taken");
    press(&mut rt, Key::Backspace);
    ctrl_key(&mut rt, 'b');
    press(&mut rt, Key::Text("n".into()));
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-sequence");
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-saved");
    reach(&mut rt, "shortcut-key:client.history.back:0");
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-unbound");
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    click(&mut rt, MENU);
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-menu");
    rt.update(|s, tx| s.perform(Intent::Shortcuts(recorder::Menu::ResetAll), tx));
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-reset");
    std::fs::remove_file(&rt.state.settings_path).ok();

    // The smallest window the app allows, on a row with a long name.
    let (mut ws, _host) = tabbed_workspace("split");
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Shortcuts);
    let mut rt = Runtime::new(ws, view, (640., 480.), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    reach(&mut rt, "shortcut-key:client.tab.previous_global:0");
    settle(&mut rt);
    ctrl_key(&mut rt, 'k');
    settle(&mut rt);
    camera.snap(&mut rt, "recorder-narrow");
    std::fs::remove_file(&rt.state.settings_path).ok();
}
