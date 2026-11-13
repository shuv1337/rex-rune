use super::*;
use std::time::{Duration, Instant, UNIX_EPOCH};

fn started(ws: &mut Workspace, value: Option<&str>) {
    let Answer::Reported(p) = &mut ws.hosts[0].model.server_info.process else {
        panic!("process");
    };
    p.started_at = value.map(str::to_owned);
}

#[test]
fn uptime_bounds_units_and_missing_or_disconnected_are_honest() {
    let mut ws = fixture();
    started(&mut ws, Some("1970-01-01T00:00:00Z"));
    for (seconds, expected) in [
        (0, "now"),
        (59, "now"),
        (60, "1m"),
        (3599, "59m"),
        (3600, "1h"),
        (86399, "23h"),
        (86400, "1d"),
        (365 * 86400, "365d"),
    ] {
        assert_eq!(
            server_page::uptime(
                &ws.hosts[0].model,
                UNIX_EPOCH + Duration::from_secs(seconds)
            )
            .as_deref(),
            Some(expected)
        );
    }
    assert_eq!(
        server_page::uptime(
            &ws.hosts[0].model,
            UNIX_EPOCH + Duration::from_secs(365 * 86400 + 1)
        ),
        None
    );
    for bad in [
        None,
        Some("bad"),
        Some("1970-01-01T00:01:01Z"),
        Some("9999-12-31T23:59:59Z"),
    ] {
        started(&mut ws, bad);
        assert_eq!(server_page::uptime(&ws.hosts[0].model, UNIX_EPOCH), None);
    }
    started(&mut ws, Some("1970-01-01T00:01:00Z"));
    assert_eq!(
        server_page::uptime(&ws.hosts[0].model, UNIX_EPOCH).as_deref(),
        Some("now")
    );
    ws.hosts[0].model.connected = false;
    assert_eq!(server_page::uptime(&ws.hosts[0].model, UNIX_EPOCH), None);
    let rt = open(ws, (640., 480.));
    assert!(!shows(&rt, "Running for"));
}

#[test]
fn minute_refresh_is_only_for_visible_server_settings_and_sends_nothing() {
    let mut ws = fixture();
    let sent = capture_host(&mut ws, 0);
    let now = Instant::now();
    assert_eq!(ws.refresh_server_at(now), (false, None));
    prepare(&mut ws, (640., 480.));
    assert_eq!(
        ws.refresh_server_at(now),
        (false, Some(Duration::from_secs(60)))
    );
    assert_eq!(
        ws.refresh_server_at(now + Duration::from_secs(59)),
        (false, Some(Duration::from_secs(1)))
    );
    assert_eq!(
        ws.refresh_server_at(now + Duration::from_secs(60)),
        (true, Some(Duration::from_secs(60)))
    );
    ws.open_section(SettingsSection::Interface);
    assert_eq!(ws.refresh_server_at(now), (false, None));
    ws.open_section(SettingsSection::Server);
    ws.stop_server();
    assert_eq!(
        ws.refresh_server_at(now),
        (false, None),
        "not under the confirmation"
    );
    ws.cancel_panel_quiet();
    assert!(ws.refresh_server_at(now).1.is_some());
    ws.cancel_panel_quiet();
    assert_eq!(ws.refresh_server_at(now), (false, None));
    assert!(sent.is_empty());
}

#[test]
fn stop_button_commands_and_person_gate_agree() {
    let _families = rune::text::with_families("Inter", "Inter");
    for endpoint in [
        "unix:///private/server.sock",
        "ssh://test/private/server.sock",
        "https://test",
    ] {
        for connected in [false, true] {
            let mut ws = fixture();
            ws.settings.hosts[0].endpoint = endpoint.into();
            ws.hosts[0].model.connected = connected;
            let sent = capture_host(&mut ws, 0);
            let mut rt = open(ws, (640., 480.));
            let available = connected && !endpoint.starts_with("https:");
            assert_eq!(rt.state.can_stop_server(), available);
            assert_eq!(has(&rt, "server-stop"), available);
            if connected && !available {
                assert!(shows(
                    &rt,
                    "A server can be stopped only from its own machine or over SSH."
                ));
            }
            rt.update(|s, _| s.picker(false));
            let row = rt
                .state
                .entries()
                .into_iter()
                .find(|e| e.label == "Stop server…")
                .unwrap();
            assert_eq!(row.enabled, available);
            assert!(row.keys.is_empty());
            assert!(row.danger);
            rt.update(|s, tx| {
                crate::app::actions::run(s, server_page::STOP, &serde_json::Value::Null, tx);
            });
            assert_eq!(
                matches!(
                    rt.state.panel.as_ref().unwrap().kind,
                    PanelKind::Confirm { .. }
                ),
                available
            );
            assert!(sent.is_empty(), "opening never sends");
        }
    }
    let spec = crate::app::actions::find(server_page::STOP).unwrap();
    assert!(spec.requires_person());
    assert!(matches!(
        spec.remote(),
        crate::app::actions::Remote::Never(_)
    ));
}

fn description(ws: &Workspace) -> &str {
    let PanelKind::Confirm {
        method,
        payload,
        description,
    } = &ws.panel.as_ref().unwrap().kind
    else {
        panic!("confirm");
    };
    assert_eq!(method, "server.stop");
    assert_eq!(*payload, serde_json::json!({}));
    description
}

#[test]
fn stop_confirmation_names_consequences_and_cancel_and_escape_restore_server() {
    let _families = rune::text::with_families("Inter", "Inter");
    for reported in [true, false] {
        let mut ws = fixture();
        if !reported {
            ws.hosts[0].model.server_info.process = Answer::Unreported;
        }
        let sent = capture_host(&mut ws, 0);
        let mut rt = open(ws, (640., 480.));
        for cancel in [true, false] {
            tap(&mut rt, "server-stop");
            let words = description(&rt.state);
            for word in [
                "Build machine",
                "Every program running in its sessions ends.",
                "Other clients are disconnected.",
                "Rex Rune will not start it again; a service manager on that machine may.",
            ] {
                assert!(words.contains(word), "{words}");
            }
            assert!(words.contains(if reported {
                "Last reported session count: 7."
            } else {
                "The session count was not reported."
            }));
            if cancel {
                click(&mut rt, "cancel-close");
            } else {
                press(&mut rt, Key::Escape);
            }
            settle(&mut rt);
            assert!(matches!(
                rt.state.panel.as_ref().unwrap().kind,
                PanelKind::Settings
            ));
            assert_eq!(rt.state.settings_page(), SettingsSection::Server);
            assert!(sent.is_empty());
        }
        // The default Enter action is Cancel, never destructive.
        tap(&mut rt, "server-stop");
        press(&mut rt, Key::Enter);
        settle(&mut rt);
        assert!(sent.is_empty());
        assert!(matches!(
            rt.state.panel.as_ref().unwrap().kind,
            PanelKind::Settings
        ));
    }
}

#[test]
fn confirmed_stop_targets_only_one_host_once_and_rejects_changed_target() {
    for change in 0..5 {
        let mut ws = workspace_hosts(2);
        for i in 0..2 {
            ws.settings.hosts[i].endpoint = format!("unix:///private/{i}.sock");
            ws.hosts[i].model.connected = true;
            ws.hosts[i].model.connection_epoch = 9;
        }
        let a = capture_host(&mut ws, 0);
        let b = capture_host(&mut ws, 1);
        prepare(&mut ws, (640., 480.));
        ws.stop_server();
        match change {
            1 => ws.active = 1,
            2 => ws.hosts[0].model.connection_epoch += 1,
            3 => ws.hosts[0].model.connected = false,
            4 => ws.settings.hosts[0].endpoint = "unix:///private/replaced.sock".into(),
            _ => {}
        }
        ws.submit(&mut rune::Tx::default());
        if change == 0 {
            assert!(matches!(
                a.try_recv().unwrap(),
                HostCommand::StopServer { epoch: 9 }
            ));
        }
        assert!(a.is_empty() && b.is_empty());
        ws.submit(&mut rune::Tx::default());
        assert!(a.is_empty() && b.is_empty(), "confirmation consumed");
    }
}

#[test]
fn pressing_the_stop_button_in_the_confirmation_sends_one_stop_to_that_host() {
    let _families = rune::text::with_families("Inter", "Inter");
    let mut ws = workspace_hosts(2);
    for i in 0..2 {
        ws.settings.hosts[i].endpoint = format!("unix:///private/{i}.sock");
        ws.hosts[i].model.connected = true;
        ws.hosts[i].model.connection_epoch = 9;
    }
    let a = capture_host(&mut ws, 0);
    let b = capture_host(&mut ws, 1);
    let mut rt = open(ws, (1024., 700.));
    tap(&mut rt, "server-stop");
    // The real destructive button, not the default one.
    click(&mut rt, "confirm-close");
    settle(&mut rt);
    let stops: Vec<u64> = a
        .try_iter()
        .filter_map(|c| match c {
            HostCommand::StopServer { epoch } => Some(epoch),
            _ => None,
        })
        .collect();
    assert_eq!(
        stops,
        [9],
        "one stop, for the connection it was confirmed on"
    );
    assert!(
        !b.try_iter()
            .any(|c| matches!(c, HostCommand::StopServer { .. })),
        "the other host is not asked to stop"
    );
    assert!(!matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Confirm { .. })
    ));
    // Nothing is left to press a second time.
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(
        !a.try_iter()
            .chain(b.try_iter())
            .any(|c| matches!(c, HostCommand::StopServer { .. }))
    );
}

#[test]
fn stop_settings_search_indexes_the_actual_row() {
    let mut rt = open(fixture(), (1024., 700.));
    rt.update(|s, _| {
        s.settings_nav.search = rune::TextEdit::new("stop");
        s.settings_search_changed();
    });
    assert!(
        rt.state
            .settings_nav
            .hits()
            .iter()
            .any(|h| h.page == SettingsSection::Server && h.label == "Stop server…")
    );
}

#[test]
fn a_host_cannot_request_stop_through_perform_or_returned_actions() {
    let mut ws = fixture();
    let sent = capture_host(&mut ws, 0);
    ws.settings.hosts[0].remote_control = true;
    ws.remote_perform(
        0,
        crate::app::tests::remote_tests::pending(server_page::STOP, serde_json::json!({})),
        &mut rune::Tx::default(),
    );
    crate::app::tests::remote_tests::code(
        crate::app::tests::remote_tests::reply(&sent),
        "not_supported",
    );
    assert!(ws.panel.is_none());
    let mut model = ws.hosts[0].model.clone();
    model
        .client_actions
        .push(rex_rune::connection::actions::Queued {
            generation: ws.keymap_generation,
            session: None,
            block: None,
            action: rexrs::api::client::ClientAction {
                action: server_page::STOP.into(),
                args: None,
            },
        });
    *ws.hosts[0].connection.model.lock().unwrap() = model;
    ws.sync(0, &mut rune::Tx::default());
    assert!(ws.panel.is_none());
    assert!(sent.try_iter().all(|c| !matches!(
        c,
        HostCommand::StopServer { .. } | HostCommand::Request { .. }
    )));
}

#[test]
fn stop_outcomes_are_readable_on_the_page_not_only_behind_the_modal() {
    let _families = rune::text::with_families("Inter", "Inter");
    for result in [
        "Could not stop server: forbidden: private refusal. Nothing was retried.",
        "Could not stop server: not_supported: private refusal. Nothing was retried.",
        "Could not stop server: conflict: private refusal. Nothing was retried.",
        "Could not confirm server stop: the connection dropped or did not reply. Check the host before trying again. Nothing was retried.",
    ] {
        let mut ws = fixture();
        let sent = capture_host(&mut ws, 0);
        ws.hosts[0].model.server_stop_result = Some(result.into());
        let rt = open(ws, (640., 480.));
        assert!(shows(&rt, &format!("Last stop request: {result}")));
        assert!(sent.is_empty());
    }
}

#[test]
fn server_controls_draw() {
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let size = if std::env::var_os("REX_M33_MINIMUM").is_some() {
        (640., 480.)
    } else {
        (1024., 700.)
    };
    let mut ws = fixture();
    // Capture runner supplies a recent RFC 3339 time; no wall-clock assumption in the suite.
    let start = std::env::var("REX_TEST_SERVER_START").expect("capture needs a recent start time");
    started(&mut ws, Some(&start));
    prepare(&mut ws, size);
    let mut rt = Runtime::with_text(
        ws,
        page_view,
        size,
        Env::new(true, 1.),
        rune::text::TextSystem::new(fonts.files, fonts.families),
    );
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    rt.update(|_, tx| tx.scroll_center("server-stop"));
    settle(&mut rt);
    assert!(shows(&rt, "Running for"));
    camera.snap(&mut rt, "server-controls");
    tap(&mut rt, "server-stop");
    let dialog = rect(&rt, "dialog");
    let stop = rect(&rt, "confirm-close");
    assert!(dialog[1] >= 0. && dialog[1] + dialog[3] <= size.1);
    assert!(stop[1] + stop[3] <= dialog[1] + dialog[3]);
    camera.snap(&mut rt, "server-stop-confirm");
}
