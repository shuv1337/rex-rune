use super::*;
use rex_rune::{
    connection::{Command, activity::TERMINAL},
    program_resume::ResumeSnapshot,
};
use rune::{Ime, Tx};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime};

const ACTION: &str = "client.program_resume";
const EMPTY: &str = "No program in this session has reported how to resume.";
const FOOTER: &str =
    "Trusted chosen programs resume after a server restart; others return to a shell.";

fn fixture() -> (Workspace, flume::Receiver<HostCommand>) {
    let (mut ws, rx) = tabbed_workspace("split");
    ws.settings.hosts[0].endpoint = "ssh://dev@buildbox/private/rex.sock".into();
    ws.settings.hosts[0].label = "Build box".into();
    let model = &mut ws.hosts[0].model;
    model.protocol.api_revision = 24;
    model.connection_epoch = 3;
    model.sessions[0].label = "Workbench".into();
    for (pane, name) in model
        .views
        .get_mut("s")
        .unwrap()
        .windows
        .iter_mut()
        .flat_map(|window| &mut window.layers)
        .flat_map(|layer| &mut layer.blocks)
        .zip(["Docs", "Frontend", "Build API"])
    {
        pane.creator_name = Some(TERMINAL.into());
        pane.label = Some(name.into());
    }
    (ws, rx)
}

fn record(
    pid: u64,
    program: &str,
    owner: &str,
    cwd: Option<&str>,
    chosen: bool,
    trusted: bool,
) -> Value {
    let mut record = json!({
        "owner": {"pid": pid, "name": owner},
        "program": program,
        "cwd": cwd,
        "chosen": chosen,
        "trusted": trusted,
        "reported_at": "2026-11-25T22:30:00Z",
        "command": ["secret-argument-sentinel"]
    });
    if trusted {
        record["rule"] = json!(program);
    } else {
        record["untrusted_reason"] = json!("no_rule");
    }
    record
}

fn populate(ws: &mut Workspace, block: &str, entries: &[Value], truncated: bool) {
    let mut snapshot = ResumeSnapshot::decode_metadata(
        &json!({"records": entries, "truncated": truncated}).to_string(),
    )
    .unwrap();
    for summary in &mut snapshot.records {
        summary.reported_at = Some(SystemTime::now() - Duration::from_secs(180));
    }
    ws.hosts[0]
        .model
        .program_resume
        .insert(block.into(), snapshot);
}

fn fill(ws: &mut Workspace) {
    populate(
        ws,
        "block:b",
        &[
            record(42, "claude", "claude", Some("/work/frontend"), true, true),
            record(53, "codex", "", None, false, true),
        ],
        false,
    );
    populate(
        ws,
        "block:c",
        &[record(
            71,
            "my\u{202e}prog",
            "worker\u{200b}",
            Some("/work/\u{3164}"),
            true,
            false,
        )],
        true,
    );
}

fn watches(rx: &flume::Receiver<HostCommand>) -> Vec<(u64, Option<String>)> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::WatchProgramResume { epoch, session } => Some((epoch, session)),
            _ => None,
        })
        .collect()
}

fn publish(ws: &mut Workspace, tx: &mut Tx) {
    *ws.hosts[0].connection.model.lock().unwrap() = ws.model().clone();
    ws.sync(0, tx);
}

#[test]
fn commands_and_pane_menu_gate_reads_and_show_only_cached_detail() {
    let (mut ws, rx) = fixture();
    ws.hosts[0].model.protocol.api_revision = 23;
    ws.picker(false);
    let commands = ws.entries();
    let command = commands
        .iter()
        .find(|e| e.id.as_deref() == Some(ACTION))
        .unwrap();
    assert_eq!(command.label, "Show program resume…");
    assert!(!command.enabled);
    assert_eq!(command.detail, "Needs Rex API 24 on Build box");
    ws.perform(command.action.clone(), &mut Tx::default());
    assert!(ws.program_resume_target().is_none());
    ws.pane_menu("block:c".into(), None);
    assert!(!ws.entries().iter().any(|e| e.label == "Program resume…"));
    assert!(watches(&rx).is_empty());

    ws.hosts[0].model.protocol.api_revision = 24;
    ws.pane_menu("block:c".into(), None);
    assert_eq!(
        ws.entries()
            .iter()
            .find(|e| e.label == "Program resume…")
            .unwrap()
            .detail,
        ""
    );
    fill(&mut ws);
    let menu = ws.entries();
    let i = menu
        .iter()
        .position(|e| e.label == "Program resume…")
        .unwrap();
    assert_eq!(menu[i - 1].label, "Listening ports…");
    assert_eq!(menu[i].detail, r"my\u{202E}prog");
    assert!(watches(&rx).is_empty());
    ws.perform(menu[i].action.clone(), &mut Tx::default());
    assert_eq!(
        ws.panel.as_ref().unwrap().title,
        "Program resume · Workbench"
    );
    assert_eq!(ws.panel.as_ref().unwrap().fields[0].1, "Filter programs…");
    assert_eq!(ws.entries()[0].group, "Build API");
    assert_eq!(watches(&rx), [(3, Some("s".into()))]);
}

#[test]
fn returned_client_action_is_refused_with_a_notice() {
    let (mut ws, rx) = fixture();
    let generation = ws.keymap_generation;
    ws.hosts[0]
        .model
        .client_actions
        .push(rex_rune::connection::actions::Queued {
            generation,
            session: Some("s".into()),
            block: None,
            action: serde_json::from_value(json!({"action": ACTION})).unwrap(),
        });
    publish(&mut ws, &mut Tx::default());
    assert!(ws.panel.is_none());
    assert!(watches(&rx).is_empty());
    assert!(
        ws.notices
            .iter()
            .any(|n| n.text.contains("can show program resume"))
    );
}

#[test]
fn foreground_view_arrival_opens_the_watch_and_every_reopen_reads_again() {
    let (mut ws, rx) = fixture();
    let view = ws.hosts[0].model.views.remove("s").unwrap();
    publish(&mut ws, &mut Tx::default());
    assert!(watches(&rx).is_empty());
    ws.show_program_resume(None);
    assert!(watches(&rx).is_empty());
    assert_eq!(
        ws.program_resume_coverage(ws.program_resume_target().unwrap())
            .empty,
        "Reading…"
    );
    ws.hosts[0].model.views.insert("s".into(), view);
    ws.hosts[0].model.background_views.insert("s".into());
    publish(&mut ws, &mut Tx::default());
    assert!(watches(&rx).is_empty());
    ws.hosts[0].model.background_views.remove("s");
    publish(&mut ws, &mut Tx::default());
    publish(&mut ws, &mut Tx::default());
    assert_eq!(watches(&rx), [(3, Some("s".into()))]);
    let mut rt = runtime(ws);
    press(&mut rt, Key::Escape);
    assert!(rt.state.panel.is_none());
    assert_eq!(watches(&rx), [(3, None)]);
    rt.update(|ws, _| ws.show_program_resume(None));
    rt.update(|ws, _| ws.picker(false));
    assert_eq!(watches(&rx), [(3, Some("s".into())), (3, None)]);
}

#[test]
fn host_session_and_epoch_changes_end_the_original_watch() {
    let (mut ws, rx) = fixture();
    ws.show_program_resume(None);
    ws.hosts[0].model.connection_epoch = 4;
    publish(&mut ws, &mut Tx::default());
    assert!(ws.panel.is_none());
    assert_eq!(watches(&rx), [(3, Some("s".into())), (3, None)]);
    ws.show_program_resume(None);
    ws.hosts[0].model.selected = Some("another".into());
    publish(&mut ws, &mut Tx::default());
    assert!(ws.panel.is_none());
    assert_eq!(watches(&rx), [(4, Some("s".into())), (4, None)]);

    let (populated, _) = fixture();
    let mut ws = workspace_hosts(2);
    ws.hosts[0].model = populated.model().clone();
    let rx = capture_host(&mut ws, 0);
    let other = capture_host(&mut ws, 1);
    ws.show_program_resume(None);
    ws.activate_host(1, &mut Tx::default());
    assert!(ws.panel.is_none());
    assert_eq!(watches(&rx), [(3, Some("s".into())), (3, None)]);
    assert!(watches(&other).is_empty());
}

#[test]
fn metadata_rows_escape_display_and_keep_server_order_and_footer_on_filter() {
    let (mut ws, _) = fixture();
    ws.show_program_resume(None);
    let coverage = |ws: &Workspace| ws.program_resume_coverage(ws.program_resume_target().unwrap());
    assert_eq!(coverage(&ws).empty, EMPTY);
    ws.hosts[0]
        .model
        .program_resume_reading
        .insert("block:b".into());
    assert_eq!(coverage(&ws).empty, "Reading…");
    fill(&mut ws);
    let entries = ws.entries();
    assert_eq!(
        entries
            .iter()
            .map(|e| (e.group.as_str(), e.label.as_str()))
            .collect::<Vec<_>>(),
        [
            ("Frontend", "claude"),
            ("Frontend", "codex"),
            ("Build API", r"my\u{202E}prog (worker\u{200B})")
        ]
    );
    let present: Vec<_> = entries
        .iter()
        .map(|e| e.program_resume.as_ref().unwrap())
        .collect();
    assert_eq!(
        (present[0].verdict, present[0].reason.as_ref()),
        ("Will resume", "Rule: claude")
    );
    assert_eq!(present[1].verdict, "Not chosen · Trusted");
    assert_eq!(
        (present[2].verdict, present[2].reason.as_ref()),
        ("Won't resume", "No trust rule names this command.")
    );
    assert_eq!(entries[1].detail, "reported 3m ago · pid 53");
    assert_eq!(
        entries[2].detail,
        r"/work/\u{3164} · reported 3m ago · pid 71"
    );
    ws.hosts[0]
        .model
        .program_resume
        .get_mut("block:c")
        .unwrap()
        .records[0]
        .chosen = false;
    assert_eq!(
        ws.entries()[2].program_resume.as_ref().unwrap().reason,
        present[2].reason
    );
    for (query, count) in [
        ("FRONTEND", 2),
        ("RULE: CLAUDE", 1),
        (r"\u{3164}", 1),
        ("secret-argument-sentinel", 0),
    ] {
        ws.panel.as_mut().unwrap().inputs[0].text = query.into();
        ws.query_changed();
        assert_eq!(ws.entries().len(), count);
    }
    assert_eq!(coverage(&ws).empty, "No matching programs.");
    assert_eq!(
        coverage(&ws).footer,
        [
            FOOTER,
            "These programs run on Build box.",
            "Some records aren't shown.",
            "Trust rules are set in init.lua on Build box (resume.trust).",
        ]
    );
    ws.hosts[0].model.program_resume_unsupported = true;
    assert!(ws.entries().is_empty());
    assert_eq!(
        coverage(&ws).empty,
        "Program resume needs Rex API 24 on Build box."
    );
}

#[test]
fn selection_tracks_the_block_and_record_across_reorders_without_activation() {
    let (mut ws, _) = fixture();
    let first = record(1, "first", "a", None, false, false);
    let second = record(1, "second", "b", None, false, false);
    populate(&mut ws, "block:b", &[first.clone(), second.clone()], false);
    populate(&mut ws, "block:c", std::slice::from_ref(&second), false);
    ws.show_program_resume(None);
    let mut rt = runtime(ws);
    press(&mut rt, Key::Down);
    let identity = rt.state.panel.as_ref().unwrap().row.clone().unwrap();
    assert_ne!(
        rt.state.entries()[2].id.as_deref(),
        Some(identity.1.as_str())
    );
    rt.update(|ws, tx| {
        populate(ws, "block:b", &[second.clone(), first.clone()], false);
        publish(ws, tx);
    });
    assert_eq!(
        rt.state.panel.as_ref().unwrap().row,
        Some((0, identity.1.clone()))
    );
    rt.update(|ws, tx| {
        populate(ws, "block:b", &[first], false);
        publish(ws, tx);
    });
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.panel.as_ref().unwrap().row, Some((0, identity.1)));
}

#[test]
fn minimum_popup_has_no_clipboard_or_pty_actions_even_on_repeat_ime_and_pointer() {
    let (mut ws, _) = fixture();
    let (sender, sent) = flume::unbounded();
    super::pane_verb_tests::framed_terminal(&mut ws, "block:b", &sender, b"$ fixture");
    fill(&mut ws);
    ws.show_program_resume(None);
    ws.viewport = [640., 480.];
    let mut rt = runtime_at(ws, (640., 480.));
    let clipboard = rune::clipboard::memory();
    clipboard.set_text("untouched");
    rt.use_clipboard(&clipboard);
    for _ in 0..30 {
        rt.frame(1. / 60.);
    }
    click(&mut rt, "entry:0");
    press(&mut rt, Key::Enter);
    rt.repeat_key(KeyEvent {
        key: Key::Enter,
        mods: Mods::default(),
    });
    rt.ime(Ime::Commit("claude".into()));
    rt.frame(1. / 60.);
    assert_eq!(rt.state.query(), "claude");
    assert!(crate::app::input::on_ime(
        &mut rt.state,
        &Ime::Commit("no paste".into()),
        &mut Tx::default()
    ));
    assert_eq!(clipboard.text().unwrap().as_str(), "untouched");
    assert!(rt.state.program_resume_target().is_some());
    let term = rt.state.active_terminal_mut().unwrap();
    for command in [
        Command::Paste("no".into()),
        Command::Mouse(json!({})),
        Command::Wheel(json!({})),
    ] {
        assert!(!term.try_send(command));
    }
    assert!(sent.try_iter().all(|cmd| !matches!(
        cmd,
        HostCommand::Terminal {
            command: Command::Paste(_)
                | Command::Key(_)
                | Command::KeyWithPrefix { .. }
                | Command::Mouse(_)
                | Command::Wheel(_),
            ..
        }
    )));
    press(&mut rt, Key::Escape);
    assert!(rt.state.panel.is_none());
    assert!(
        rt.state
            .active_terminal_mut()
            .unwrap()
            .try_send(Command::Paste("after".into()))
    );
}

fn draw(name: &str, populated: bool) {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let (size, tag) = if std::env::var("REX_RESUME_MINIMUM").as_deref() == Ok("1") {
        ((640., 480.), "640")
    } else {
        ((1024., 700.), "1024")
    };
    let (mut ws, _rx) = fixture();
    let (sender, _commands) = flume::unbounded();
    super::pane_verb_tests::framed_terminal(&mut ws, "block:b", &sender, b"$ coding session");
    super::pane_verb_tests::framed_terminal(&mut ws, "block:c", &sender, b"$ build logs");
    if populated {
        fill(&mut ws);
    }
    ws.show_program_resume(None);
    ws.viewport = [size.0, size.1];
    let mut rt = Runtime::with_text(
        ws,
        view,
        size,
        Env::new(true, 1.),
        rune::text::TextSystem::new(fonts.files, fonts.families),
    );
    for _ in 0..30 {
        rt.frame(1. / 60.);
    }
    let elements = rt.elements();
    for line in if populated {
        vec![
            FOOTER,
            "Will resume",
            "Won't resume",
            "Not chosen · Trusted",
        ]
    } else {
        vec![FOOTER, EMPTY]
    } {
        assert!(
            elements
                .iter()
                .any(|el| el.text.as_deref() == Some(line) && el.visible[3] >= el.rect[3] - 1.),
            "{line}"
        );
    }
    camera.snap(&mut rt, &format!("program-resume-{name}-{tag}"));
}

#[test]
#[ignore = "real-font capture; run alone with REX_TEST_SNAP_DIR"]
fn program_resume_populated_draw() {
    draw("populated", true);
}

#[test]
#[ignore = "real-font capture; run alone with REX_TEST_SNAP_DIR"]
fn program_resume_empty_draw() {
    draw("empty", false);
}
