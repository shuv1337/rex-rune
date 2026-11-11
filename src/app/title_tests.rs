//! Generated titles: a tab or pane without a name of its own called by its foreground program
//! and that program's directory, on the tab, the pane header, the window, the sidebar and
//! the tab's target, and the switch on Settings → Terminal that turns them on.
use super::pane_verb_tests::{Camera, framed_terminal, rect};
use super::*;
use crate::app::panel::SettingsSection;
use rex_linux::connection::{DEFAULT_TAB_LABEL, activity};
use serde_json::json;

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

/// Put `program` (pid 7) in the foreground of `block` in `cwd`, under the terminal's own zsh
/// (pid 1); none leaves that shell at its prompt. Home is `/home/u`.
fn run(ws: &mut Workspace, block: &str, program: Option<&str>, cwd: &str) {
    let shell = json!({"pid": 1, "name": "zsh", "argv0": "-zsh", "cwd": cwd});
    let process = match program {
        Some(name) => json!({"home": "/home/u", "ancestors": [shell],
            "foreground": {"pid": 7, "name": name, "cwd": cwd}}),
        None => json!({"home": "/home/u", "foreground": shell}),
    };
    ws.hosts[0]
        .model
        .apply_block_event("s", block.into(), activity::parse_process(&process));
}

/// Title `block` as the process `owner` would: 1 is its shell, 7 the program in front.
fn title(ws: &mut Workspace, block: &str, text: &str, owner: u64) {
    let event = activity::parse_title(&json!({"title": text, "owner": {"pid": owner}}));
    ws.hosts[0]
        .model
        .apply_block_event("s", block.into(), event);
}

/// Session "lunar-basin" with a tab still called Shell (one terminal, `block:a`) and a tab
/// the user named dev, split into a terminal with its default label (`block:b`) and one the
/// user named logs (`block:c`).
fn shells(active: &str) -> (Workspace, flume::Receiver<HostCommand>) {
    let (mut ws, rx) = tabbed_workspace(active);
    let view = ws.hosts[0].model.views.get_mut("s").unwrap();
    view.label = Some("lunar-basin".into());
    view.windows[0].label = Some(DEFAULT_TAB_LABEL.into());
    for block in view
        .windows
        .iter_mut()
        .flat_map(|w| &mut w.layers)
        .flat_map(|l| &mut l.blocks)
    {
        block.creator_name = Some(activity::TERMINAL.into());
        block.flavor = Some(format!("{}.shell", activity::TERMINAL));
        block.label = Some(
            match block.block_id.as_deref() {
                Some("block:c") => "logs",
                _ => "shell",
            }
            .into(),
        );
    }
    (ws, rx)
}

fn label(rt: &Runtime<Workspace>, key: &str) -> String {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .and_then(|e| e.label)
        .unwrap_or_else(|| panic!("missing {key}"))
}

/// Change the workspace, name the window again as a model update does, and draw.
fn change(rt: &mut Runtime<Workspace>, change: impl FnOnce(&mut Workspace)) {
    rt.update(|s, tx| {
        change(s);
        s.update_title(tx);
    });
    settle(rt);
}

#[test]
fn titles_stay_as_they_were_until_generated_titles_are_chosen() {
    let (mut ws, _rx) = shells("single");
    assert!(!ws.generated_titles(), "off unless chosen");
    run(&mut ws, "block:a", Some("vim"), "/home/u/rex");
    let mut rt = runtime(ws);
    change(&mut rt, |_| {});
    assert_eq!(label(&rt, "tab:single"), "Shell");
    assert_eq!(rt.state.title, "Shell - lunar-basin · test-0");
    change(&mut rt, |s| title(s, "block:a", "u@host:~/rex", 1));
    assert_eq!(label(&rt, "tab:single"), "u@host:~/rex");
    assert_eq!(rt.state.title, "u@host:~/rex - lunar-basin · test-0");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_tab_running_vim_in_rex_reads_vim_and_its_directory() {
    let (mut ws, _rx) = shells("single");
    ws.settings.terminal.generated_titles = true;
    run(&mut ws, "block:a", Some("vim"), "/home/u/rex");
    run(&mut ws, "block:b", Some("cargo"), "/home/u/rex");
    let mut rt = runtime(ws);
    change(&mut rt, |_| {});
    assert_eq!(label(&rt, "tab:single"), "vim - ~/rex");
    assert!(element_text(&rt, "tab:single").contains(&"vim - ~/rex".to_owned()));
    assert_eq!(rt.state.title, "vim - ~/rex - lunar-basin · test-0");
    // The name the user gave the other tab stays, whatever runs there.
    assert_eq!(label(&rt, "tab:split"), "dev");

    // A title the shell wrote, at its prompt or before it ran vim, gives way.
    change(&mut rt, |s| title(s, "block:a", "u@host:~/rex", 1));
    assert_eq!(label(&rt, "tab:single"), "vim - ~/rex");
    // A title vim set for itself shows.
    change(&mut rt, |s| title(s, "block:a", "notes.md - VIM", 7));
    assert_eq!(label(&rt, "tab:single"), "notes.md - VIM");
    assert_eq!(rt.state.title, "notes.md - VIM - lunar-basin · test-0");
    // Vim exits without clearing it: the shell is the program, where it now is.
    change(&mut rt, |s| run(s, "block:a", None, "/home/u/rex/docs"));
    assert_eq!(label(&rt, "tab:single"), "zsh - ~/rex/docs");

    // The tab's menu, its close confirmation and the sidebar call it the same.
    let target = rt.state.tab_target(0, "s", "single").unwrap();
    assert_eq!(target.name, "zsh - ~/rex/docs");
    change(&mut rt, |s| {
        s.set_vertical_tabs(true);
        if !s.sidebar.is_open() {
            s.sidebar.toggle();
        }
    });
    assert_eq!(label(&rt, "sidebar-tab:0:s:single"), "zsh - ~/rex/docs");
    assert_eq!(label(&rt, "sidebar-tab:0:s:split"), "dev");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_renamed_tab_keeps_its_name_and_takes_a_generated_one_with_its_default_title_back() {
    let (mut ws, rx) = shells("split");
    ws.settings.terminal.generated_titles = true;
    run(&mut ws, "block:b", Some("cargo"), "/home/u/rex");
    let mut rt = runtime(ws);
    change(&mut rt, |_| {});
    assert_eq!(label(&rt, "tab:split"), "dev");
    assert_eq!(rt.state.title, "dev - lunar-basin · test-0");
    // Use default title asks the server for its default label; nothing here invents one.
    let _ = requests(&rx);
    change(&mut rt, |s| {
        let view = s.hosts[0].model.views.get_mut("s").unwrap();
        view.windows[1].label = Some(DEFAULT_TAB_LABEL.into());
    });
    assert_eq!(label(&rt, "tab:split"), "cargo - ~/rex");
    assert!(requests(&rx).is_empty(), "naming a tab sends nothing");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_pane_with_its_default_label_takes_a_generated_title_and_a_named_one_keeps_its_name() {
    let (mut ws, _rx) = shells("split");
    run(&mut ws, "block:b", Some("cargo"), "/home/u/rex");
    run(&mut ws, "block:c", Some("tail"), "/var/log");
    let (sender, _sent) = flume::unbounded();
    framed_terminal(&mut ws, "block:b", &sender, b"$ cargo test");
    framed_terminal(&mut ws, "block:c", &sender, b"$ tail -f server.log");
    let mut rt = runtime(ws);
    settle(&mut rt);
    let header = |rt: &Runtime<Workspace>, block: &str| {
        element_text(rt, &format!("pane-header:{block}"))
            .first()
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(header(&rt, "block:b"), "shell", "off: its label, as before");
    change(&mut rt, |s| s.settings.terminal.generated_titles = true);
    assert_eq!(header(&rt, "block:b"), "cargo - ~/rex");
    assert_eq!(header(&rt, "block:c"), "logs");
    // Rename still starts from the server's label, and the menu names the pane as drawn.
    let target = rt.state.target("block:b").unwrap();
    assert_eq!(
        (target.label.as_str(), target.name.as_str()),
        ("shell", "cargo - ~/rex")
    );
    // Each pane is called by its own program, not the tab's focused one.
    change(&mut rt, |s| {
        let view = s.hosts[0].model.views.get_mut("s").unwrap();
        view.windows[1].layers[0].blocks[1].label = Some("shell".into());
    });
    assert_eq!(header(&rt, "block:c"), "tail - /var/log");
    assert_eq!(header(&rt, "block:b"), "cargo - ~/rex");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_long_directory_keeps_its_last_folders_on_the_tab() {
    let (mut ws, _rx) = shells("single");
    ws.settings.terminal.generated_titles = true;
    run(
        &mut ws,
        "block:a",
        Some("cargo"),
        "/home/u/.cache/agent-ws/rex-parity/c",
    );
    let mut rt = runtime(ws);
    settle(&mut rt);
    assert_eq!(label(&rt, "tab:single"), "cargo - …/agent-ws/rex-parity/c");
    // Whole on the tab: nothing is cut at the pill's widest.
    let (tab, words) = (rect(&rt, "tab:single"), element_text(&rt, "tab:single"));
    assert!(tab[2] <= 240.5, "{tab:?}");
    assert_eq!(words, ["cargo - …/agent-ws/rex-parity/c"]);
    let text = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.text.as_deref() == Some("cargo - …/agent-ws/rex-parity/c"))
        .expect("the tab's words");
    assert!(
        text.rect[0] + text.rect[2] <= tab[0] + tab[2] - 13.,
        "the words end inside the pill's padding: {:?} in {tab:?}",
        text.rect
    );
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Settings open on Terminal over the tabs of `shells`.
fn titles_settings() -> Runtime<Workspace> {
    let (mut ws, _rx) = shells("single");
    run(&mut ws, "block:a", Some("vim"), "/home/u/rex");
    title(&mut ws, "block:a", "u@host:~/rex", 1);
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Terminal);
    let mut rt = runtime_at(ws, (1440., 1000.));
    change(&mut rt, |_| {});
    rt
}

/// Scroll the Terminal page until the element keyed `key` is well inside it.
fn reveal(rt: &mut Runtime<Workspace>, key: &str) {
    let (page, control) = (rect(rt, "settings-scroll:terminal"), rect(rt, key));
    let (x, middle) = (page[0] + page[2] / 2., page[1] + page[3] / 2.);
    let off = control[1] + control[3] / 2. - middle;
    if off.abs() > page[3] / 2. - 60. {
        rt.pointer_move(x, middle);
        rt.wheel(x, middle, -off);
        settle(rt);
    }
}

#[test]
fn the_titles_switch_saves_and_renames_the_tab_and_the_window_at_once() {
    let mut rt = titles_settings();
    assert_eq!(label(&rt, "tab:single"), "u@host:~/rex");
    assert_eq!(rt.state.title, "u@host:~/rex - lunar-basin · test-0");
    let switch = |rt: &Runtime<Workspace>| {
        rt.elements()
            .into_iter()
            .find(|e| !e.exiting && e.key.as_deref() == Some("generated-titles"))
            .expect("the switch")
    };
    assert_eq!(
        switch(&rt).label.as_deref(),
        Some("Prefer generated titles")
    );

    reveal(&mut rt, "generated-titles");
    click(&mut rt, "generated-titles");
    settle(&mut rt);
    assert!(rt.state.generated_titles());
    assert_eq!(label(&rt, "tab:single"), "vim - ~/rex");
    assert_eq!(rt.state.title, "vim - ~/rex - lunar-basin · test-0");
    assert!(rt.state.panel.is_some(), "Settings stay open");
    let saved = Settings::load(&rt.state.settings_path).unwrap();
    assert!(saved.terminal.generated_titles, "saved as it flips");

    click(&mut rt, "generated-titles");
    settle(&mut rt);
    assert!(!rt.state.generated_titles());
    assert_eq!(label(&rt, "tab:single"), "u@host:~/rex");
    assert_eq!(rt.state.title, "u@host:~/rex - lunar-basin · test-0");
    let saved = Settings::load(&rt.state.settings_path).unwrap();
    assert!(!saved.terminal.generated_titles);
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn the_titles_card_sits_between_selection_and_default_shell_with_its_footnote() {
    let mut rt = titles_settings();
    reveal(&mut rt, "generated-titles");
    let headings: Vec<(f32, String)> = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.role == "heading" && e.path.contains("settings-scroll:"))
        .map(|e| (e.rect[1], e.text.unwrap_or_default()))
        .collect();
    let top = |name: &str| headings.iter().find(|(_, n)| n == name).unwrap().0;
    let switch = rect(&rt, "generated-titles");
    assert!(top("Selection") < top("Titles") && top("Titles") < switch[1]);
    assert!(switch[1] < top("Default shell"));
    let note = rt
        .elements()
        .into_iter()
        .find(|e| {
            !e.exiting
                && e.text
                    .as_deref()
                    .is_some_and(|t| t.starts_with("Show the running program and its directory"))
        })
        .expect("the footnote");
    assert!(note.rect[1] > switch[1] + switch[3] && note.rect[1] < top("Default shell"));
    // The switch works from the keyboard too.
    rt.update(|_, tx| tx.focus("generated-titles", true));
    settle(&mut rt);
    press(&mut rt, Key::Text(" ".into()));
    settle(&mut rt);
    assert!(rt.state.generated_titles(), "Space flips it");
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_notification_names_the_tab_by_its_generated_title() {
    use rex_linux::connection::activity::DesktopNotice;
    // The split tab is in front; the Shell tab, out of sight, runs vim.
    let (mut ws, _rx) = shells("split");
    ws.settings.terminal.generated_titles = true;
    run(&mut ws, "block:a", Some("vim"), "/home/u/rex");
    assert_eq!(ws.tab_target(0, "s", "single").unwrap().name, "vim - ~/rex");
    // A message a program sends without a title of its own is headed by its tab's name.
    ws.deliver_program_notices(
        0,
        vec![DesktopNotice {
            session: "s".into(),
            block: "block:a".into(),
            title: String::new(),
            body: "Saved".into(),
        }],
    );
    assert_eq!(ws.outbox.len(), 1);
    assert_eq!(ws.outbox[0].summary, "vim - ~/rex");
    std::fs::remove_file(&ws.settings_path).ok();
}

/// Generated titles as drawn, for a look (see `Camera`): a tab and split panes named by
/// their programs beside ones the user named, a long directory, and the Titles card.
#[test]
fn generated_titles_draw_on_tabs_panes_and_the_terminal_page() {
    let Some(mut camera) = Camera::new() else {
        return;
    };
    // The host's receiver is kept, so nothing sent to it reads as a stopped worker.
    let mut hosts = Vec::new();
    let mut scene = |active: &str| {
        let (mut ws, host) = shells(active);
        hosts.push(host);
        ws.settings.terminal.generated_titles = true;
        run(&mut ws, "block:a", Some("vim"), "/home/u/rex");
        run(&mut ws, "block:b", Some("cargo"), "/home/u/rex");
        run(&mut ws, "block:c", Some("tail"), "/var/log");
        let (sender, _sent) = flume::unbounded();
        framed_terminal(&mut ws, "block:a", &sender, b"~/rex $ vim notes.md");
        framed_terminal(
            &mut ws,
            "block:b",
            &sender,
            b"$ cargo test\r\nrunning 12 tests\r\ntest result: ok",
        );
        framed_terminal(&mut ws, "block:c", &sender, b"$ tail -f server.log");
        ws
    };
    let studio = |ws: Workspace| {
        let mut rt = Runtime::new(ws, view, (1024., 700.), Env::new(true, 2.));
        rt.on_key = Some(crate::app::input::on_key);
        settle(&mut rt);
        rt
    };

    let mut rt = studio(scene("single"));
    camera.snap(&mut rt, "titles-tab");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut ws = scene("split");
    ws.hosts[0].model.views.get_mut("s").unwrap().windows[1].label = Some(DEFAULT_TAB_LABEL.into());
    let mut rt = studio(ws);
    camera.snap(&mut rt, "titles-split");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut ws = scene("single");
    run(
        &mut ws,
        "block:a",
        Some("cargo"),
        "/home/u/.cache/agent-ws/rex-parity/c",
    );
    ws.set_vertical_tabs(true);
    if !ws.sidebar.is_open() {
        ws.sidebar.toggle();
    }
    let mut rt = studio(ws);
    camera.snap(&mut rt, "titles-long-sidebar");
    std::fs::remove_file(&rt.state.settings_path).ok();

    let mut ws = scene("single");
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Terminal);
    let mut rt = studio(ws);
    reveal(&mut rt, "generated-titles");
    camera.snap(&mut rt, "titles-settings");
    click(&mut rt, "generated-titles");
    settle(&mut rt);
    camera.snap(&mut rt, "titles-settings-off");
    std::fs::remove_file(&rt.state.settings_path).ok();
}
