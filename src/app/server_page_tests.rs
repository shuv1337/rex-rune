//! Cached Server facts, privacy, navigation and read-only behavior.
use super::pane_verb_tests::{Camera, framed_terminal, rect};
use super::*;
use crate::app::{panel::SettingsSection, server_page};
use rex_rune::connection::server_info::{Answer, Build, Process, ServerInfo};

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

fn reported(version: &str) -> ServerInfo {
    ServerInfo {
        api_revisions: Some((10, 13)),
        build: Answer::Reported(Build {
            rex_version: Some(version.into()),
            platform: Some("linux_amd64".into()),
            commit: Some("abc1234".into()),
            dirty: Some(false),
            built_at: Some("2026-11-18T10:00:00-08:00".into()),
        }),
        process: Answer::Reported(Process {
            pid: Some(4242),
            instance_id: Some("private-instance".into()),
            host_id: Some("private-host-id".into()),
            started_at: Some("2026-11-18T11:00:00-08:00".into()),
            session_count: Some(7),
            listeners: vec![rexrs::api::server::Listener {
                endpoint: "unix:///run/rex/server.sock".into(),
                addresses: vec!["private-bound-address".into()],
                ..Default::default()
            }],
            tailscale: Some(rexrs::api::server::Tailscale {
                state: "running".into(),
                auth_url: Some("https://login.example/secret-auth".into()),
                error: Some("secret-error".into()),
                https: Some(rexrs::api::server::Http {
                    state: "available".into(),
                    url: Some("https://user:secret-password@build.tail1234.ts.net?token=secret-query#secret-fragment".into()),
                }),
                ..Default::default()
            }),
        }),
    }
}

fn fixture() -> Workspace {
    let mut ws = workspace();
    ws.settings.hosts[0].label = "Build machine".into();
    ws.settings.hosts[0].endpoint = "ssh://user@build.example/run/rex.sock".into();
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.protocol.api_revision = 13;
    ws.hosts[0].model.server_info = reported("0.1.0-preview.15");
    ws
}

// The workspace always retains a default host and cannot remove its last one. Exercise
// the page's defensive empty state in the real Settings shell without unrelated chrome
// (whose existing invariant requires a host).
fn page_view(s: &Workspace, cx: &mut rune::Cx<Workspace>) -> rune::El {
    if s.hosts.is_empty() {
        rune::el()
            .w_full()
            .h_full()
            .fill(crate::app::theme::rgb(crate::app::theme::ground()))
            .child(crate::app::settings_view::settings(s, s.viewport))
    } else {
        view(s, cx)
    }
}

fn prepare(ws: &mut Workspace, size: (f32, f32)) {
    ws.panel = Some(crate::app::panel::Panel::new(
        PanelKind::Settings,
        "Settings",
        &[],
    ));
    ws.viewport = [size.0, size.1];
    ws.open_section(SettingsSection::Server);
}

fn open(mut ws: Workspace, size: (f32, f32)) -> Runtime<Workspace> {
    prepare(&mut ws, size);
    let mut rt = Runtime::new(ws, page_view, size, Env::new(true, 1.));
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    rt
}

fn page_text(rt: &Runtime<Workspace>) -> String {
    rt.elements()
        .iter()
        .filter(|e| !e.exiting && e.path.contains("settings-scroll:server"))
        .filter_map(|e| e.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n")
}

fn shows(rt: &Runtime<Workspace>, value: &str) -> bool {
    rt.elements().iter().any(|e| {
        !e.exiting && e.path.contains("settings-scroll:server") && e.text.as_deref() == Some(value)
    })
}

fn tap(rt: &mut Runtime<Workspace>, key: &str) {
    rt.update(|_, tx| tx.scroll_center(key));
    settle(rt);
    click(rt, key);
    settle(rt);
}

#[test]
fn reported_facts_are_cached_not_live_and_sensitive_members_are_not_rendered() {
    let rt = open(fixture(), (1024., 700.));
    for value in [
        "Build machine",
        "connected",
        "ssh://build.example/run/rex.sock",
        "0.1.0-preview.15",
        "13",
        "10–13",
        "linux_amd64",
        "abc1234",
        "Clean",
        "2026-11-18T10:00:00-08:00",
        "4242",
        "2026-11-18T11:00:00-08:00",
        "7",
        "1 reported",
        "unix:///run/rex/server.sock",
        "Running",
        "Available",
        "https://build.tail1234.ts.net/",
    ] {
        assert!(shows(&rt, value), "missing {value}: {}", page_text(&rt));
    }
    let text = page_text(&rt);
    assert!(text.contains("Last reported server facts, not live monitoring"));
    assert!(text.contains("not this client's local Tailscale"));
    for secret in ["private-", "secret-", "user@", "token="] {
        assert!(!text.contains(secret), "{secret}: {text}");
        assert!(
            !rt.elements()
                .iter()
                .any(|e| e.path.contains("settings-scroll:server")
                    && e.help.as_deref().is_some_and(|help| help.contains(secret)))
        );
    }
}

#[test]
fn pending_partial_refused_and_unknown_fields_remain_distinct() {
    let mut ws = fixture();
    ws.hosts[0].model.server_info = ServerInfo::default();
    ws.hosts[0].model.protocol.api_revision = 0;
    let mut rt = open(ws, (1024., 700.));
    assert!(shows(&rt, "Waiting for report"));
    for label in [
        "Built-in Tailscale",
        "Tailscale HTTPS",
        "Tailscale address · Pair device",
    ] {
        assert!(page_text(&rt).contains(&format!("{label}\nWaiting for report")));
    }
    assert!(shows(&rt, "Not reported") && !shows(&rt, "Not connected"));
    assert!(!has(&rt, "server-copy-tailnet"));
    rt.update(|s, _| {
        s.hosts[0].model.server_info = ServerInfo {
            build: Answer::Unreported,
            process: Answer::Unreported,
            ..Default::default()
        };
    });
    settle(&mut rt);
    assert!(!shows(&rt, "Waiting for report"));
    assert!(shows(&rt, "Not reported"));
    assert!(!shows(&rt, "0") && !shows(&rt, "Disabled"));

    rt.update(|s, _| {
        s.hosts[0].model.protocol.api_revision = 10;
        s.hosts[0].model.server_info = ServerInfo {
            build: Answer::Reported(Build {
                rex_version: Some("old-server".into()),
                ..Default::default()
            }),
            process: Answer::Reported(Process::default()),
            ..Default::default()
        };
    });
    settle(&mut rt);
    assert!(shows(&rt, "10") && shows(&rt, "old-server"));
    assert!(!shows(&rt, "Waiting for report") && !shows(&rt, "0 reported"));

    rt.update(|s, _| {
        if let Answer::Reported(p) = &mut s.hosts[0].model.server_info.process {
            p.tailscale = Some(rexrs::api::server::Tailscale {
                state: "future-state-secret".into(),
                https: Some(rexrs::api::server::Http {
                    state: "future-state".into(),
                    url: Some("https://not-ready.example".into()),
                }),
                ..Default::default()
            });
        }
    });
    settle(&mut rt);
    assert!(shows(&rt, "Unknown (starting)") && shows(&rt, "Pending"));
    assert!(!has(&rt, "server-copy-tailnet"));
    assert!(!page_text(&rt).contains("future-state"));
    for (state, label) in [
        ("disabled", "Disabled"),
        ("starting", "Starting"),
        ("needs_login", "Needs sign-in"),
        ("needs_machine_approval", "Needs machine approval"),
    ] {
        rt.update(|s, _| {
            if let Answer::Reported(p) = &mut s.hosts[0].model.server_info.process {
                p.tailscale.as_mut().unwrap().state = state.into();
            }
        });
        settle(&mut rt);
        assert!(shows(&rt, label));
        assert!(!has(&rt, "server-copy-tailnet"));
    }
    rt.update(|s, _| {
        s.hosts[0].model.connected = false;
        s.hosts[0].model.server_info = ServerInfo::default();
    });
    settle(&mut rt);
    assert!(!shows(&rt, "Waiting for report"));
    assert!(shows(&rt, "Not reported"));
}

#[test]
fn disconnected_reconnecting_and_new_connection_never_mix_reports() {
    let mut ws = fixture();
    ws.hosts[0].model.connected = false;
    ws.hosts[0].model.status = "Disconnected: password=secret-status".into();
    let mut rt = open(ws, (1024., 700.));
    assert!(shows(&rt, "disconnected"));
    assert!(shows(&rt, "Running (last reported)") && shows(&rt, "Available (last reported)"));
    assert!(!shows(&rt, "Running") && !shows(&rt, "Available"));
    assert!(shows(&rt, "Not connected") && shows(&rt, "0.1.0-preview.15"));
    assert!(page_text(&rt).contains("Disconnected: last reported"));
    assert!(!page_text(&rt).contains("secret-status"));
    let before = page_text(&rt);
    settle(&mut rt);
    assert_eq!(page_text(&rt), before, "no invented ticking uptime");

    rt.update(|s, _| s.hosts[0].model.status = "Reconnecting…".into());
    settle(&mut rt);
    assert!(shows(&rt, "reconnecting"));
    rt.update(|s, _| {
        // The existing welcome path replaces the cache on a new connection.
        let h = &mut s.hosts[0].model;
        h.connected = true;
        h.connection_epoch += 1;
        h.server_info = ServerInfo::default();
    });
    settle(&mut rt);
    assert!(shows(&rt, "Waiting for report"));
    assert!(!shows(&rt, "0.1.0-preview.15") && !shows(&rt, "4242"));
    rt.update(|s, _| s.hosts[0].model.server_info = reported("new-server"));
    settle(&mut rt);
    assert!(shows(&rt, "new-server"));
}

#[test]
fn active_hosts_with_same_server_ids_never_share_facts_or_copy_payloads() {
    let mut ws = workspace_hosts(2);
    for (i, name) in ["first", "second"].into_iter().enumerate() {
        ws.settings.hosts[i].label = name.into();
        ws.settings.hosts[i].endpoint =
            format!("https://user:password@{name}.example?token=secret");
        ws.hosts[i].model.connected = true;
        ws.hosts[i].model.protocol.api_revision = 13;
        ws.hosts[i].model.server_info = reported(name);
    }
    let mut rt = open(ws, (1024., 700.));
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    assert!(clipboard.text().is_none(), "opening does not copy");
    for (index, here, other) in [
        (0, "first", "second"),
        (1, "second", "first"),
        (0, "first", "second"),
    ] {
        rt.update(|s, _| s.active = index);
        settle(&mut rt);
        assert!(shows(&rt, here) && !shows(&rt, other));
        tap(&mut rt, "server-copy-endpoint");
        assert_eq!(
            clipboard.text().as_ref().map(|v| v.as_str()),
            Some(format!("https://{here}.example/").as_str())
        );
    }
    tap(&mut rt, "server-copy-tailnet");
    assert_eq!(
        clipboard.text().as_ref().map(|v| v.as_str()),
        Some("https://build.tail1234.ts.net/")
    );
}

#[test]
fn address_forms_strip_credentials_and_never_echo_unknown_syntax() {
    for (input, expected) in [
        (
            "https://u:p@example.org:443/path?token=secret#secret",
            Some("https://example.org/path"),
        ),
        (
            "wss://u:p@example.org:9000/ws?key=x",
            Some("wss://example.org:9000/ws"),
        ),
        (
            "ssh://u:p@box.example/run/rex.sock?password=x",
            Some("ssh://box.example/run/rex.sock"),
        ),
        (
            "unix:///run/rex.sock?token=x#x",
            Some("unix:///run/rex.sock?token=x#x"),
        ),
        (
            "unix:///tmp/rex?a/server.sock",
            Some("unix:///tmp/rex?a/server.sock"),
        ),
        (
            "unix:///tmp/rex#1/server.sock",
            Some("unix:///tmp/rex#1/server.sock"),
        ),
        (
            "unix:///home/me/My Sockets/réx.sock",
            Some("unix:///home/me/My Sockets/réx.sock"),
        ),
        ("unix://relative.sock", None),
        ("127.0.0.1:7297", Some("127.0.0.1:7297")),
        ("[::1]:7297", Some("[::1]:7297")),
        ("javascript:password", None),
        ("data:text/plain,secret", None),
        ("future://u:p@example?token=x", None),
        ("not an address: secret", None),
        ("https://example.org/\npassword", None),
    ] {
        assert_eq!(server_page::address(input).as_deref(), expected, "{input}");
    }
    let mut ws = fixture();
    ws.settings.hosts[0].endpoint = "future://secret-configured".into();
    if let Answer::Reported(p) = &mut ws.hosts[0].model.server_info.process {
        p.listeners[0].endpoint = "https://u:secret-listener@example.org?secret-query".into();
        for endpoint in ["tailnet://:7297", "localhost:7297"] {
            p.listeners.push(rexrs::api::server::Listener {
                endpoint: endpoint.into(),
                ..Default::default()
            });
        }
        p.tailscale.as_mut().unwrap().https.as_mut().unwrap().url =
            Some("javascript:secret-link".into());
    }
    let rt = open(ws, (1024., 700.));
    assert!(!has(&rt, "server-copy-endpoint") && !has(&rt, "server-copy-tailnet"));
    assert!(shows(&rt, "https://example.org/") && shows(&rt, "Address unavailable"));
    assert_eq!(
        page_text(&rt).matches("Endpoint format not shown").count(),
        2
    );
    assert!(!page_text(&rt).contains("secret"));

    // Copy a Unix endpoint exactly as transport interprets it, not as a URL.
    let literal = "unix:///home/me/My Sockets/réx?1#2.sock";
    let mut ws = fixture();
    ws.settings.hosts[0].endpoint = literal.into();
    let mut rt = open(ws, (640., 480.));
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    assert!(shows(&rt, literal));
    tap(&mut rt, "server-copy-endpoint");
    assert_eq!(clipboard.text().as_ref().map(|v| v.as_str()), Some(literal));
}

#[test]
fn no_hosts_has_no_fabricated_facts_or_copy_controls() {
    let mut ws = fixture();
    ws.hosts.clear();
    ws.settings.hosts.clear();
    let rt = open(ws, (640., 480.));
    assert!(has(&rt, "server-empty"));
    assert!(!has(&rt, "server-copy-endpoint") && !has(&rt, "server-copy-tailnet"));
    assert!(!shows(&rt, "connected") && !shows(&rt, "0"));
}

#[test]
fn settings_search_history_keyboard_and_copy_are_local_even_over_a_live_pane() {
    let _families = rune::text::with_families("Inter", "Inter");
    for size in [(1024., 700.), (640., 480.)] {
        let (mut ws, host) = tabbed_workspace("single");
        ws.hosts[0].model.server_info = reported("read-only");
        let (sender, terminal) = flume::unbounded();
        framed_terminal(&mut ws, "block:a", &sender, b"unchanged terminal");
        let mut rt = runtime_at(ws, size);
        settle(&mut rt);
        host.drain().for_each(drop);
        terminal.drain().for_each(drop);
        let claimed = rt.state.claimed.clone();
        let frame = rt.state.terminals[&rt.state.term_key("block:a")]
            .frame
            .clone();
        let clipboard = rune::clipboard::memory();
        rt.use_clipboard(&clipboard);
        rt.update(|s, tx| s.perform(Intent::Settings, tx));
        settle(&mut rt);
        click(&mut rt, "settings-section:server");
        settle(&mut rt);
        assert_eq!(rt.state.settings_page(), SettingsSection::Server);
        assert!(focused(&rt).is_some_and(|f| f.contains("settings-section:server")));
        for label in [
            "Configured address",
            "Server version",
            "Started at",
            "Session count",
            "Listeners",
            "Built-in Tailscale",
            "Tailscale address · Pair device",
        ] {
            click(&mut rt, "settings-search");
            rt.update(|s, _| {
                s.settings_nav.search = rune::TextEdit::new(label);
                s.settings_search_changed();
            });
            let hits = rt.state.settings_nav.hits();
            assert_eq!(hits[0].label, label);
            assert_eq!(hits[0].page, SettingsSection::Server);
            press(&mut rt, Key::Enter);
            settle(&mut rt);
            let found = rect(&rt, "settings-found");
            let scroll = rect(&rt, "settings-scroll:server");
            assert!(
                found[1] >= scroll[1] - 1. && found[1] + found[3] <= scroll[1] + scroll[3] + 1.
            );
        }
        press(&mut rt, Key::Escape); // Clear the search, returning the section list.
        let offset = rt.state.settings_nav.offset().get();
        assert!(offset > 0.);
        click(&mut rt, "settings-section:terminal");
        rt.key(KeyEvent {
            key: Key::Left,
            mods: Mods {
                alt: true,
                ..Default::default()
            },
        });
        settle(&mut rt);
        assert_eq!(rt.state.settings_page(), SettingsSection::Server);
        assert!((rt.state.settings_nav.offset().get() - offset).abs() < 1.);
        rt.key(KeyEvent {
            key: Key::Right,
            mods: Mods {
                alt: true,
                ..Default::default()
            },
        });
        settle(&mut rt);
        assert_eq!(rt.state.settings_page(), SettingsSection::Terminal);
        click(&mut rt, "settings-back");
        settle(&mut rt);
        rt.update(|_, tx| {
            tx.scroll_center("server-copy-tailnet");
            tx.focus("server-copy-tailnet", true);
        });
        settle(&mut rt);
        press(&mut rt, Key::Enter);
        assert_eq!(
            clipboard.text().as_ref().map(|v| v.as_str()),
            Some("https://build.tail1234.ts.net/")
        );
        press(&mut rt, Key::Tab);
        press(&mut rt, Key::Escape);
        settle(&mut rt);
        assert!(rt.state.panel.is_none());
        assert!(
            host.is_empty() && terminal.is_empty(),
            "no requests, mutations, PTY input or resizes"
        );
        assert_eq!(rt.state.claimed, claimed);
        assert!(std::sync::Arc::ptr_eq(
            frame.as_ref().unwrap(),
            rt.state.terminals[&rt.state.term_key("block:a")]
                .frame
                .as_ref()
                .unwrap()
        ));
    }
}

#[test]
fn long_values_fit_both_page_widths_and_keep_safe_tooltips() {
    let _families = rune::text::with_families("Inter", "Inter");
    for size in [(1024., 700.), (640., 480.)] {
        let mut ws = fixture();
        ws.settings.hosts[0].label = "Very long host ".repeat(30);
        ws.settings.hosts[0].endpoint = format!(
            "https://box.example/{}?password=secret",
            "folder/".repeat(40)
        );
        let rt = open(ws, size);
        let scroll = rect(&rt, "settings-scroll:server");
        for e in rt.elements().iter().filter(|e| {
            !e.exiting
                && e.path.contains("settings-scroll:server")
                && matches!(e.role, "text" | "button")
        }) {
            assert!(
                e.rect[0] >= scroll[0] - 0.5
                    && e.rect[0] + e.rect[2] <= scroll[0] + scroll[2] + 0.5,
                "{:?} outside {scroll:?}",
                e.rect
            );
            assert!(
                !e.help
                    .as_deref()
                    .is_some_and(|h| h.contains("password") || h.contains("secret"))
            );
        }
    }
}

fn draw(case: &str) {
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let mut ws = fixture();
    match case {
        "older" => {
            ws.hosts[0].model.protocol.api_revision = 10;
            ws.hosts[0].model.server_info = ServerInfo {
                build: Answer::Unreported,
                process: Answer::Reported(Process::default()),
                ..Default::default()
            };
        }
        "disconnected" => {
            ws.hosts[0].model.connected = false;
            ws.hosts[0].model.status = "Disconnected".into();
        }
        "nohost" => {
            ws.hosts.clear();
            ws.settings.hosts.clear();
        }
        _ => {}
    }
    let size = if std::env::var_os("REX_M33_MINIMUM").is_some() {
        (640., 480.)
    } else {
        (1024., 700.)
    };
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
    camera.snap(&mut rt, &format!("{case}-top"));
    if case != "nohost" {
        let scroll = rect(&rt, "settings-scroll:server");
        rt.wheel(scroll[0] + 20., scroll[1] + 30., -400.);
        settle(&mut rt);
        camera.snap(&mut rt, &format!("{case}-build"));
        rt.wheel(scroll[0] + 20., scroll[1] + 30., -4000.);
        settle(&mut rt);
        camera.snap(&mut rt, &format!("{case}-bottom"));
    }
}

#[test]
fn server_connected_draw() {
    draw("connected");
}
#[test]
fn server_older_draw() {
    draw("older");
}
#[test]
fn server_disconnected_draw() {
    draw("disconnected");
}
#[test]
fn server_nohost_draw() {
    draw("nohost");
}

fn pair_report(ws: &mut Workspace, host: usize, state: &str, url: &str) {
    if let Answer::Reported(p) = &mut ws.hosts[host].model.server_info.process {
        let h = p.tailscale.as_mut().unwrap().https.as_mut().unwrap();
        h.state = state.into();
        h.url = Some(url.into());
    }
}

fn pair_data(ws: &Workspace) -> &crate::app::pair_device::Dialog {
    let PanelKind::PairDevice(d) = &ws.panel.as_ref().unwrap().kind else {
        panic!("pair dialog")
    };
    d
}

#[test]
fn pair_button_and_command_require_connected_available_https_report() {
    let _families = rune::text::with_families("Inter", "Inter");
    for (connected, state, url, available) in [
        (true, "available", "https://build.tail1234.ts.net", true),
        (false, "available", "https://build.tail1234.ts.net", false),
        (true, "pending", "https://build.tail1234.ts.net", false),
        (true, "off", "https://build.tail1234.ts.net", false),
        (true, "available", "http://build.tail1234.ts.net", false),
        (true, "available", "https://", false),
        (true, "available", "unix:///private/tunnel.sock", false),
        (true, "available", "https://bad\n.example", false),
    ] {
        let mut ws = fixture();
        ws.hosts[0].model.connected = connected;
        pair_report(&mut ws, 0, state, url);
        let mut rt = open(ws, (640., 480.));
        assert_eq!(
            has(&rt, "server-pair"),
            available,
            "{connected} {state} {url}"
        );
        assert_eq!(rt.state.pair_address().is_some(), available);
        rt.update(|s, _| s.picker(false));
        let entry = rt
            .state
            .entries()
            .into_iter()
            .find(|e| e.label == "Pair device…")
            .unwrap();
        assert_eq!(entry.enabled, available);
        assert!(entry.keys.is_empty(), "unbound by default");
        rt.update(|s, tx| {
            crate::app::actions::run(
                s,
                crate::app::pair_device::ACTION,
                &serde_json::Value::Null,
                tx,
            );
        });
        assert_eq!(
            matches!(
                rt.state.panel.as_ref().unwrap().kind,
                PanelKind::PairDevice(_)
            ),
            available
        );
    }
    let mut ws = fixture();
    ws.hosts[0].model.server_info = ServerInfo::default();
    assert!(
        ws.pair_address().is_none(),
        "no inference from the saved SSH destination or local Tailscale"
    );
}

#[test]
fn pair_code_copy_and_text_hold_exactly_the_same_sanitized_address_and_return() {
    let mut rt = open(fixture(), (1024., 700.));
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    tap(&mut rt, "server-copy-tailnet");
    let expected = "https://build.tail1234.ts.net/";
    assert_eq!(
        clipboard.text().as_ref().map(|s| s.as_str()),
        Some(expected)
    );
    tap(&mut rt, "server-pair");
    assert_eq!(pair_data(&rt.state).address, expected);
    assert_eq!(
        pair_data(&rt.state).symbol,
        rex_rune::qr::encode(expected.as_bytes())
    );
    assert!(has(&rt, "pair-code"));
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.text.as_deref() == Some(expected))
    );
    assert!(!rt.elements().iter().filter(|e| !e.exiting).any(|e| {
        e.text
            .as_deref()
            .is_some_and(|t| t.contains("secret-") || t.contains("user@") || t.contains("token="))
    }));
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().unwrap().kind,
        PanelKind::Settings
    ));
    assert_eq!(rt.state.settings_page(), SettingsSection::Server);
    tap(&mut rt, "server-pair");
    click(&mut rt, "pair-done");
    settle(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().unwrap().kind,
        PanelKind::Settings
    ));
    assert_eq!(rt.state.settings_page(), SettingsSection::Server);
}

#[test]
fn pair_too_long_keeps_full_selectable_text_without_a_code() {
    let _families = rune::text::with_families("Inter", "Inter");
    let address = format!("https://build.tail1234.ts.net/{}", "a".repeat(220));
    let mut ws = fixture();
    pair_report(&mut ws, 0, "available", &address);
    let mut rt = open(ws, (640., 480.));
    assert!(has(&rt, "server-pair"));
    tap(&mut rt, "server-pair");
    assert_eq!(pair_data(&rt.state).address, address);
    assert_eq!(pair_data(&rt.state).symbol, Err(rex_rune::qr::TooLong));
    assert!(!has(&rt, "pair-code"));
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.text.as_deref() == Some("This address is too long to show as a code."))
    );
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.text.as_deref() == Some(&address))
    );
    let dialog = rect(&rt, "dialog");
    let done = rect(&rt, "pair-done");
    assert!(done[1] + done[3] <= dialog[1] + dialog[3]);
    assert!(dialog[1] >= 0. && dialog[1] + dialog[3] <= 480.);
}

#[test]
fn pair_host_switch_disconnect_epoch_or_changed_report_hides_old_code() {
    for change in 0..4 {
        let mut ws = workspace_hosts(2);
        for i in 0..2 {
            ws.hosts[i].model.connected = true;
            ws.hosts[i].model.server_info = reported("test");
            pair_report(
                &mut ws,
                i,
                "available",
                &format!("https://host{i}-rex.tail1234.ts.net/"),
            );
        }
        let mut rt = open(ws, (1024., 700.));
        tap(&mut rt, "server-pair");
        assert_eq!(
            pair_data(&rt.state).address,
            "https://host0-rex.tail1234.ts.net/"
        );
        rt.update(|s, _| match change {
            0 => s.active = 1,
            1 => s.hosts[0].model.connected = false,
            2 => s.hosts[0].model.connection_epoch += 1,
            _ => pair_report(s, 0, "pending", "https://host0-rex.tail1234.ts.net/"),
        });
        settle(&mut rt);
        assert!(!has(&rt, "pair-code") && !has(&rt, "pair-address"));
        click(&mut rt, "pair-done");
        settle(&mut rt);
        if change == 0 {
            tap(&mut rt, "server-pair");
            let d = pair_data(&rt.state);
            assert_eq!(d.address, "https://host1-rex.tail1234.ts.net/");
            assert_eq!(d.symbol, rex_rune::qr::encode(d.address.as_bytes()));
        }
    }
}

#[test]
fn pair_search_and_command_use_the_same_dialog_without_host_commands() {
    let mut ws = fixture();
    let host = capture_host(&mut ws, 0);
    let mut rt = open(ws, (1024., 700.));
    rt.update(|s, _| {
        s.settings_nav.search = rune::TextEdit::new("pair");
        s.settings_search_changed();
    });
    assert!(
        rt.state
            .settings_nav
            .hits()
            .iter()
            .any(|h| h.page == SettingsSection::Server
                && h.label == "Tailscale address · Pair device")
    );
    rt.update(|s, _| s.picker(false));
    let entry = rt
        .state
        .entries()
        .into_iter()
        .find(|e| e.label == "Pair device…")
        .unwrap();
    rt.update(|s, tx| s.perform(entry.action, tx));
    settle(&mut rt);
    let expected = pair_data(&rt.state).symbol.clone();
    click(&mut rt, "pair-done");
    rt.update(|s, tx| {
        s.perform(Intent::Settings, tx);
        s.open_section(SettingsSection::Server);
    });
    settle(&mut rt);
    tap(&mut rt, "server-pair");
    assert_eq!(pair_data(&rt.state).symbol, expected);
    click(&mut rt, "pair-done");
    assert!(
        host.is_empty(),
        "no Identify, status, session or terminal commands"
    );
    let spec = crate::app::actions::find(crate::app::pair_device::ACTION).unwrap();
    assert!(spec.requires_person());
    assert!(matches!(
        spec.remote(),
        crate::app::actions::Remote::Never(_)
    ));
}

#[test]
fn pair_largest_symbol_fits_both_sizes_and_keeps_black_white_in_every_theme() {
    let _families = rune::text::with_families("Inter", "Inter");
    let prefix = "https://build.tail1234.ts.net/";
    let address = format!("{prefix}{}", "a".repeat(213 - prefix.len()));
    for theme in rex_rune::settings::ChromeTheme::ALL {
        for size in [(1024., 700.), (640., 480.)] {
            for scale in [1., 2.] {
                let mut ws = fixture();
                ws.settings.chrome_theme = theme;
                pair_report(&mut ws, 0, "available", &address);
                prepare(&mut ws, size);
                ws.pair_device();
                let mut rt = Runtime::new(ws, page_view, size, Env::new(true, scale));
                settle(&mut rt);
                assert_eq!(pair_data(&rt.state).symbol.as_ref().unwrap().size(), 57);
                let code = rect(&rt, "pair-code");
                let dialog = rect(&rt, "dialog");
                let done = rect(&rt, "pair-done");
                assert!(dialog[1] >= 0. && dialog[1] + dialog[3] <= size.1);
                assert!(done[1] + done[3] <= dialog[1] + dialog[3]);
                let elements = rt.elements();
                let card = elements
                    .iter()
                    .find(|e| e.key.as_deref() == Some("pair-code") && !e.exiting)
                    .unwrap();
                assert_eq!(card.fill, rune::Color::WHITE);
                let runs: Vec<_> = elements
                    .iter()
                    .filter(|e| !e.exiting && e.path.starts_with(&format!("{}/", card.path)))
                    .collect();
                assert!(
                    !runs.is_empty() && runs.len() < 1000,
                    "merged runs, not one element per module"
                );
                let unit = code[2] / 65.;
                assert!((unit * scale - (unit * scale).round()).abs() < 0.001);
                for run in runs {
                    assert_eq!(run.fill, rune::Color::BLACK);
                    assert!(
                        run.rect[0] >= code[0] + 4. * unit - 0.01
                            && run.rect[1] >= code[1] + 4. * unit - 0.01
                    );
                    assert!(run.rect[0] + run.rect[2] <= code[0] + code[2] - 4. * unit + 0.01);
                    assert!(run.rect[1] + run.rect[3] <= code[1] + code[3] - 4. * unit + 0.01);
                }
            }
        }
    }
}

#[test]
fn pair_device_draw() {
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
    let scale = if std::env::var_os("REX_M33_SCALE2").is_some() {
        2.
    } else {
        1.
    };
    let mut ws = fixture();
    prepare(&mut ws, size);
    let mut rt = Runtime::with_text(
        ws,
        page_view,
        size,
        Env::new(true, scale),
        rune::text::TextSystem::new(fonts.files, fonts.families),
    );
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    rt.update(|_, tx| tx.scroll_center("server-pair"));
    settle(&mut rt);
    camera.snap(&mut rt, "pair-row");
    tap(&mut rt, "server-pair");
    let code = rect(&rt, "pair-code");
    let done = rect(&rt, "pair-done");
    let dialog = rect(&rt, "dialog");
    assert!(dialog[1] >= 0. && dialog[1] + dialog[3] <= size.1);
    assert!(done[1] + done[3] <= dialog[1] + dialog[3]);
    assert!((code[0] + code[2] / 2. - size.0 / 2.).abs() < 1.);
    camera.snap(&mut rt, "pair-dialog");
}
