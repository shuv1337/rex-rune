use super::*;
use crate::app::ports::{API_15, PARTIAL, Reach};
use rex_rune::{
    connection::{Command, activity::TERMINAL},
    ports::PortSnapshot,
};
use rune::{Ime, Tx};
use serde_json::json;

const UNIX: &str = "unix:///nonexistent/rex-test.sock";
const SSH: &str = "ssh://dev@buildbox:2222/private/rex.sock";

fn fixture(endpoint: &str) -> (Workspace, flume::Receiver<HostCommand>) {
    let (mut ws, rx) = tabbed_workspace("split");
    ws.settings.hosts[0].endpoint = endpoint.into();
    ws.settings.hosts[0].label = "Build box".into();
    let model = &mut ws.hosts[0].model;
    model.protocol.api_revision = 21;
    model.connection_epoch = 3;
    model.sessions[0].label = "Workbench".into();
    model.views.get_mut("s").unwrap().label = Some("Workbench".into());
    for (block, label) in model
        .views
        .get_mut("s")
        .unwrap()
        .windows
        .iter_mut()
        .flat_map(|window| &mut window.layers)
        .flat_map(|layer| &mut layer.blocks)
        .zip(["Docs", "Frontend", "Build API"])
    {
        block.creator_name = Some(TERMINAL.into());
        block.label = Some(label.into());
    }
    (ws, rx)
}

fn report(status: &str, ports: &[(u16, &str)], process: &str) -> PortSnapshot {
    PortSnapshot::parse(&json!({
        "status": status,
        "ports": ports.iter().map(|(port, address)| json!({
            "protocol": "tcp", "port": port, "address": address, "pid": 2,
        })).collect::<Vec<_>>(),
        "processes": [{"pid": 2, "script": process}],
    }))
}

fn fill(ws: &mut Workspace) {
    ws.hosts[0].model.ports.extend([
        ("block:a".into(), report("ok", &[(8080, "::1")], "docs.py")),
        (
            "block:b".into(),
            report(
                "ok",
                &[(3000, "127.0.0.1"), (4173, "0.0.0.0")],
                "development-server-with-a-very-long-process-label.py",
            ),
        ),
        (
            "block:c".into(),
            report("ok", &[(9000, "192.0.2.8")], "api-worker"),
        ),
    ]);
}

fn watches(rx: &flume::Receiver<HostCommand>) -> Vec<(u64, Option<String>)> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::WatchPorts { epoch, session } => Some((epoch, session)),
            HostCommand::ReadPorts { .. } => panic!("the Ports UI must not issue one-shot reads"),
            _ => None,
        })
        .collect()
}

fn publish(ws: &mut Workspace, tx: &mut Tx) {
    *ws.hosts[0].connection.model.lock().unwrap() = ws.model().clone();
    ws.sync(0, tx);
}

fn modified_enter(rt: &mut Runtime<Workspace>) {
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    });
    rt.frame(1. / 60.);
}

fn clipboard(rt: &mut Runtime<Workspace>) -> rune::clipboard::MemoryClipboard {
    let clipboard = rune::clipboard::memory();
    clipboard.set_text("untouched");
    rt.use_clipboard(&clipboard);
    clipboard
}

#[test]
fn groups_follow_the_invoking_pane_then_server_order_and_filter_all_fields() {
    let (mut ws, _rx) = fixture(UNIX);
    fill(&mut ws);
    ws.pane_menu("block:c".into(), None);
    let menu = ws.entries();
    let index = menu
        .iter()
        .position(|e| e.label == "Listening ports…")
        .unwrap();
    assert_eq!(menu[index - 1].label, "Download file…");
    assert_eq!(menu[index].section, 2);
    ws.perform(menu[index].action.clone(), &mut Tx::default());
    assert_eq!(
        ws.panel.as_ref().unwrap().title,
        "Listening ports · Workbench"
    );
    let entries = ws.entries();
    assert_eq!(
        entries
            .iter()
            .map(|e| (e.group.as_str(), e.label.as_str(), e.detail.as_str()))
            .collect::<Vec<_>>(),
        [
            ("dev · Build API", ":9000", "192.0.2.8"),
            ("server · Docs", ":8080", "this computer only"),
            ("dev · Frontend", ":3000", "this computer only"),
            ("dev · Frontend", ":4173", "all interfaces"),
        ]
    );
    for (query, labels) in [
        ("080", vec![":8080"]),
        ("API-WORKER", vec![":9000"]),
        ("::1", vec![":8080"]),
        ("Frontend", vec![":3000", ":4173"]),
        ("server · docs", vec![":8080"]),
    ] {
        ws.panel.as_mut().unwrap().inputs[0].text = query.into();
        ws.query_changed();
        assert_eq!(
            ws.entries()
                .iter()
                .map(|e| e.label.as_str())
                .collect::<Vec<_>>(),
            labels
        );
    }
}

#[test]
fn watch_starts_only_on_open_closes_on_escape_replacement_and_session_exit_and_rearms() {
    let (mut ws, rx) = fixture(UNIX);
    publish(&mut ws, &mut Tx::default());
    assert!(watches(&rx).is_empty());
    ws.show_ports(None);
    assert_eq!(watches(&rx), [(3, Some("s".into()))]);
    let mut rt = runtime(ws);
    press(&mut rt, Key::Escape);
    assert_eq!(watches(&rx), [(3, None)]);
    rt.update(|ws, _| ws.show_ports(None));
    rt.update(|ws, _| ws.picker(false));
    assert_eq!(watches(&rx), [(3, Some("s".into())), (3, None)]);
    rt.update(|ws, _| ws.show_ports(None));
    assert_eq!(watches(&rx), [(3, Some("s".into()))]);
    rt.update(|ws, tx| {
        ws.hosts[0].model.connected = false;
        publish(ws, tx);
        assert!(ws.ports_target().is_some());
    });
    assert!(
        watches(&rx).is_empty(),
        "the disconnected worker has already cancelled reads"
    );
    rt.update(|ws, tx| {
        ws.hosts[0].model.connected = true;
        ws.hosts[0].model.connection_epoch = 4;
        publish(ws, tx);
        publish(ws, tx);
    });
    assert_eq!(watches(&rx), [(4, Some("s".into()))]);
    rt.update(|ws, tx| {
        ws.hosts[0].model.selected = Some("different-session".into());
        publish(ws, tx);
    });
    assert!(rt.state.panel.is_none());
    assert_eq!(watches(&rx), [(4, None)]);
}

#[test]
fn outside_press_and_switching_hosts_stop_the_original_watch() {
    let (populated, _rx) = fixture(UNIX);
    let mut ws = workspace_hosts(2);
    ws.hosts[0].model = populated.model().clone();
    let rx = capture_host(&mut ws, 0);
    let other = capture_host(&mut ws, 1);
    ws.show_ports(None);
    let mut rt = runtime(ws);
    rt.pointer_down(8., 400.);
    rt.pointer_up(8., 400.);
    rt.frame(1. / 60.);
    assert!(rt.state.panel.is_none());
    assert_eq!(watches(&rx), [(3, Some("s".into())), (3, None)]);
    rt.update(|ws, tx| {
        ws.show_ports(None);
        ws.activate_host(1, tx);
    });
    assert!(rt.state.panel.is_none());
    assert_eq!(watches(&rx), [(3, Some("s".into())), (3, None)]);
    assert!(watches(&other).is_empty());
}

#[test]
fn coverage_distinguishes_disconnected_older_reading_unsupported_empty_and_limits() {
    let (mut ws, rx) = fixture(SSH);
    ws.hosts[0].model.connected = false;
    ws.show_ports(None);
    assert_eq!(ws.ports_coverage().empty, "Host is disconnected.");
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.protocol.api_revision = 14;
    ws.show_ports(None);
    assert_eq!(
        ws.ports_coverage().empty,
        "This Rex server can't list ports. Update Rex on the host."
    );
    assert!(watches(&rx).is_empty());
    ws.hosts[0].model.protocol.api_revision = 15;
    ws.show_ports(None);
    assert_eq!(watches(&rx), [(3, Some("s".into()))]);
    ws.hosts[0].model.ports_reading.insert("block:a".into());
    let coverage = ws.ports_coverage();
    assert!(coverage.api15);
    assert_eq!(coverage.empty, "Reading ports…");
    assert_eq!(
        coverage.remote.as_deref(),
        Some("These ports are open on Build box, not on this computer.")
    );
    let rt = runtime(ws);
    assert!(
        rt.elements()
            .iter()
            .any(|el| el.text.as_deref() == Some(API_15))
    );
    let mut ws = rt.state;
    ws.hosts[0].model.ports_reading.clear();
    for id in ["block:a", "block:b", "block:c"] {
        ws.hosts[0]
            .model
            .ports
            .insert(id.into(), PortSnapshot::unsupported());
    }
    assert_eq!(
        ws.ports_coverage().empty,
        "Rex can't list ports for this terminal."
    );
    for snapshot in ws.hosts[0].model.ports.values_mut() {
        *snapshot = report("ok", &[], "");
    }
    assert_eq!(
        ws.ports_coverage().empty,
        "No listening ports in this session."
    );
    fill(&mut ws);
    ws.panel.as_mut().unwrap().inputs[0].text = "none".into();
    assert!(ws.entries().is_empty());
    assert_eq!(ws.ports_coverage().empty, "No matching ports.");
    let snap = ws.hosts[0].model.ports.get_mut("block:b").unwrap();
    snap.status = rex_rune::ports::PortsStatus::Partial;
    snap.truncated = true;
    assert!(ws.ports_coverage().limits.unwrap().contains("server"));
    ws.hosts[0].model.ports.get_mut("block:b").unwrap().capped = true;
    let coverage = ws.ports_coverage();
    assert!(coverage.partial);
    assert!(coverage.limits.unwrap().contains("display limit"));
}

#[test]
fn the_selected_identity_survives_insertions_and_does_not_retarget_on_removal() {
    let (mut ws, _rx) = fixture(UNIX);
    let set = |ws: &mut Workspace, ports: &[(u16, &str)]| {
        ws.hosts[0]
            .model
            .ports
            .insert("block:b".into(), report("ok", ports, "dev"));
    };
    set(&mut ws, &[(4001, "::1"), (4001, "127.0.0.1")]);
    ws.show_ports(None);
    let mut rt = runtime(ws);
    let board = clipboard(&mut rt);
    press(&mut rt, Key::Down);
    let selected = rt.state.panel.as_ref().unwrap().row.clone().unwrap();
    rt.update(|ws, tx| {
        set(
            ws,
            &[
                (3000, "127.0.0.1"),
                (4001, "127.0.0.1"),
                (4001, "::1"),
                (6000, "::1"),
            ],
        );
        publish(ws, tx);
    });
    assert_eq!(rt.state.panel.as_ref().unwrap().row, Some((2, selected.1)));
    press(&mut rt, Key::Enter);
    assert_eq!(board.text().unwrap().as_str(), "[::1]:4001");
    modified_enter(&mut rt);
    assert_eq!(rt.state.opened_links, ["http://[::1]:4001"]);
    rt.update(|ws, tx| {
        set(ws, &[(4001, "127.0.0.1"), (6000, "::1"), (7001, "0.0.0.0")]);
        publish(ws, tx);
    });
    board.set_text("untouched");
    press(&mut rt, Key::Enter);
    modified_enter(&mut rt);
    assert_eq!(board.text().unwrap().as_str(), "untouched");
    assert_eq!(rt.state.opened_links.len(), 1);
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Enter);
    assert_eq!(board.text().unwrap().as_str(), "127.0.0.1:4001");
}

#[test]
fn clipboard_and_browser_actions_follow_only_saved_endpoint_and_validated_address() {
    for (endpoint, bind, copy, reach, expected) in [
        (
            UNIX,
            "0.0.0.0",
            "0.0.0.0:80",
            Some(Reach::Open),
            "http://127.0.0.1:80",
        ),
        (
            SSH,
            "::",
            "[::]:80",
            Some(Reach::Forward),
            "ssh -N -L '10080:[::1]:80' -p 2222 -- dev@buildbox",
        ),
        (
            "https://localhost:9000",
            "127.0.0.4",
            "127.0.0.4:80",
            None,
            "",
        ),
        ("ws://127.0.0.1:9000", "::1", "[::1]:80", None, ""),
        (UNIX, "fe80::7%eth0", "[fe80::7%eth0]:80", None, ""),
        (SSH, "fe80::7%eth0", "[fe80::7%eth0]:80", None, ""),
        (UNIX, "bad/address", "80", None, ""),
        (SSH, "bad/address", "80", None, ""),
    ] {
        let (mut ws, _rx) = fixture(endpoint);
        ws.hosts[0]
            .model
            .ports
            .insert("block:b".into(), report("ok", &[(80, bind)], "worker"));
        ws.show_ports(None);
        let entry = ws.entries().pop().unwrap();
        assert_eq!(entry.port.unwrap().reach, reach);
        if endpoint != UNIX && (bind.starts_with("127.") || bind == "::1") {
            assert_eq!(entry.detail, "loopback on Build box");
        }
        let mut rt = runtime(ws);
        let board = clipboard(&mut rt);
        press(&mut rt, Key::Enter);
        assert_eq!(board.text().unwrap().as_str(), copy);
        if endpoint != UNIX {
            assert!(
                rt.state
                    .notices
                    .last()
                    .unwrap()
                    .text
                    .ends_with("(on Build box)")
            );
        }
        board.set_text("untouched");
        modified_enter(&mut rt);
        match reach {
            Some(Reach::Open) => assert_eq!(rt.state.opened_links, [expected]),
            Some(Reach::Forward) => assert_eq!(board.text().unwrap().as_str(), expected),
            None => assert_eq!(board.text().unwrap().as_str(), "untouched"),
        }
        if endpoint != UNIX {
            assert!(rt.state.opened_links.is_empty());
        }
    }
}

#[test]
fn inline_actions_use_their_row_and_input_stays_out_of_the_pty() {
    let (mut ws, _rx) = fixture(UNIX);
    let (sender, sent) = flume::unbounded();
    super::pane_verb_tests::framed_terminal(&mut ws, "block:b", &sender, b"$ listener");
    fill(&mut ws);
    ws.show_ports(Some("block:b".into()));
    let mut rt = runtime(ws);
    let board = clipboard(&mut rt);
    for _ in 0..30 {
        rt.frame(1. / 60.);
    }
    click(&mut rt, "Open");
    assert_eq!(rt.state.opened_links, ["http://127.0.0.1:3000"]);
    assert_eq!(
        board.text().unwrap().as_str(),
        "untouched",
        "Open does not press the row"
    );
    click(&mut rt, "Copy");
    assert_eq!(board.text().unwrap().as_str(), "127.0.0.1:3000");
    rt.ime(Ime::Commit("frontend".into()));
    rt.frame(1. / 60.);
    assert_eq!(rt.state.query(), "frontend");
    assert!(crate::app::input::on_ime(
        &mut rt.state,
        &Ime::Commit("no fallback paste".into()),
        &mut Tx::default(),
    ));
    press(&mut rt, Key::Down);
    press(&mut rt, Key::Up);
    press(&mut rt, Key::Enter);
    modified_enter(&mut rt);
    assert!(
        !rt.state
            .active_terminal_mut()
            .unwrap()
            .try_send(Command::Paste("no".into()))
    );
    press(&mut rt, Key::Escape);
    assert!(
        rt.state
            .active_terminal_mut()
            .unwrap()
            .try_send(Command::Paste("after".into()))
    );
    let inputs: Vec<_> = sent
        .try_iter()
        .filter_map(|command| match command {
            HostCommand::Terminal { command, .. }
                if matches!(
                    command,
                    Command::Key(_) | Command::KeyWithPrefix { .. } | Command::Paste(_)
                ) =>
            {
                Some(command)
            }
            _ => None,
        })
        .collect();
    assert!(matches!(inputs.as_slice(), [Command::Paste(text)] if text == "after"));
}

fn draw(name: &str, endpoint: &str, api: u32) {
    use super::pane_verb_tests::{Camera, framed_terminal};
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let (size, tag) = if std::env::var("REX_PORTS_MINIMUM").as_deref() == Ok("1") {
        ((640., 480.), "640")
    } else {
        ((1024., 700.), "1024")
    };
    let (mut ws, _rx) = fixture(endpoint);
    let (tx, _commands) = flume::unbounded();
    framed_terminal(&mut ws, "block:b", &tx, b"$ npm run dev\r\nFrontend ready");
    framed_terminal(
        &mut ws,
        "block:c",
        &tx,
        b"$ ./api-worker\r\nListening on port 9000",
    );
    ws.hosts[0].model.protocol.api_revision = api;
    if api >= 15 {
        fill(&mut ws);
    }
    if endpoint == SSH {
        ws.hosts[0].model.ports.get_mut("block:b").unwrap().status =
            rex_rune::ports::PortsStatus::Partial;
    }
    ws.show_ports(Some("block:b".into()));
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
    assert!(has(&rt, "menu-surface"));
    if api >= 15 {
        assert!(has(&rt, "Copy"));
        let rows = rt.elements();
        let last = rows
            .iter()
            .find(|el| el.key.as_deref() == Some("entry:3"))
            .unwrap();
        assert!(
            last.visible[3] >= last.rect[3] - 1.,
            "the four fixture rows fit"
        );
    }
    if endpoint == SSH {
        assert!(
            rt.elements()
                .iter()
                .any(|el| el.text.as_deref() == Some(PARTIAL))
        );
    }
    camera.snap(&mut rt, &format!("ports-{name}-{tag}"));
}

#[test]
fn ports_local_draw() {
    draw("local", UNIX, 21);
}

#[test]
fn ports_remote_ssh_draw() {
    draw("remote-ssh", SSH, 21);
}

#[test]
fn ports_empty_api14_draw() {
    draw("empty-api14", UNIX, 14);
}
