use super::*;
use serde_json::json;
use std::net::{Ipv4Addr, Ipv6Addr};

#[test]
fn status_parsing_and_unsupported_forces_empty_list() {
    assert_eq!(PortsStatus::parse(Some("ok")), PortsStatus::Ok);
    assert_eq!(PortsStatus::parse(Some("partial")), PortsStatus::Partial);
    assert_eq!(
        PortsStatus::parse(Some("unsupported")),
        PortsStatus::Unsupported
    );
    assert_eq!(
        PortsStatus::parse(Some("future_status")),
        PortsStatus::Partial
    );
    assert_eq!(PortsStatus::parse(None), PortsStatus::Partial);

    let snap = PortSnapshot::parse(&json!({
        "status": "unsupported",
        "ports": [
            {"port": 8080, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z", "pid": 42}
        ],
        "processes": [
            {"pid": 42, "user_id": 1000, "name": "node"}
        ],
        "truncated": true
    }));
    assert_eq!(snap.status, PortsStatus::Unsupported);
    assert!(snap.ports.is_empty());
    assert!(snap.processes.is_empty());
    assert!(!snap.truncated);
    assert!(!snap.is_complete());
}

#[test]
fn filters_invalid_ports_protocols_dedups_and_sorts_deterministically() {
    let snap = PortSnapshot::parse(&json!({
        "status": "ok",
        "ports": [
            {"port": 0, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z"},
            {"port": -1, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z"},
            {"port": 65536, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z"},
            {"port": "8080", "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z"},
            {"port": 53, "address": "127.0.0.1", "protocol": "udp", "first_seen_at": "2026-11-24T12:00:00Z"},
            {"port": 8080, "address": "::1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z", "pid": 20},
            // Duplicate without pid followed by duplicate with pid: richer entry wins deterministically.
            {"port": 3000, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "not-a-date"},
            {"port": 3000, "address": "::ffff:127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:01Z", "pid": 10},
            {"port": 3000, "address": "0.0.0.0", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:02Z", "pid": -5}
        ],
        "processes": [
            {"pid": 10, "user_id": 1000, "name": "vite"},
            {"pid": 20, "user_id": 1000, "name": "python3"},
            {"pid": 999, "user_id": 1000, "name": "unreferenced"}
        ]
    }));

    assert!(snap.is_complete());
    assert_eq!(snap.ports.len(), 3);
    // Sorted by (port, canonical address): (3000, 0.0.0.0), (3000, 127.0.0.1), (8080, ::1)
    assert_eq!(snap.ports[0].port, 3000);
    assert_eq!(snap.ports[0].address.canonical(), "0.0.0.0");
    assert_eq!(snap.ports[0].pid, None);

    assert_eq!(snap.ports[1].port, 3000);
    assert_eq!(snap.ports[1].address.canonical(), "127.0.0.1");
    assert_eq!(snap.ports[1].pid, Some(10));
    assert_eq!(
        snap.ports[1].first_seen_at.as_deref(),
        Some("2026-11-24T12:00:01Z")
    );

    assert_eq!(snap.ports[2].port, 8080);
    assert_eq!(snap.ports[2].address.canonical(), "::1");
    assert_eq!(snap.ports[2].pid, Some(20));

    // Only referenced processes are retained.
    assert_eq!(snap.processes.len(), 2);
    assert!(snap.processes.contains_key(&10));
    assert!(snap.processes.contains_key(&20));
    assert!(!snap.processes.contains_key(&999));
}

#[test]
fn ten_thousand_entries_capped_at_256_and_unreferenced_processes_pruned() {
    let mut ports = Vec::with_capacity(10_000);
    let mut processes = Vec::with_capacity(10_000);
    for i in 1..=10_000u32 {
        let port = ((i - 1) % 60_000) + 1;
        ports.push(json!({
            "port": port,
            "address": "127.0.0.1",
            "protocol": "tcp",
            "first_seen_at": "2026-11-24T12:00:00Z",
            "pid": i
        }));
        processes.push(json!({
            "pid": i,
            "user_id": 1000,
            "name": format!("srv-{i}")
        }));
    }

    let snap = PortSnapshot::parse(&json!({
        "status": "ok",
        "ports": ports,
        "processes": processes
    }));

    assert_eq!(snap.ports.len(), MAX_PORTS_PER_BLOCK);
    assert!(snap.capped);
    assert!(!snap.is_complete());
    assert_eq!(snap.ports.first().unwrap().port, 1);
    assert_eq!(snap.ports.last().unwrap().port, MAX_PORTS_PER_BLOCK as u16);
    assert_eq!(snap.processes.len(), MAX_PORTS_PER_BLOCK);
    assert!(snap.processes.contains_key(&1));
    assert!(snap.processes.contains_key(&(MAX_PORTS_PER_BLOCK as u32)));
    assert!(
        !snap
            .processes
            .contains_key(&(MAX_PORTS_PER_BLOCK as u32 + 1))
    );
}

#[test]
fn host_budget_trims_from_end_sets_capped_and_prunes_processes() {
    let mut snap = PortSnapshot::parse(&json!({
        "status": "ok",
        "ports": [
            {"port": 8001, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z", "pid": 1},
            {"port": 8002, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z", "pid": 2},
            {"port": 8003, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z", "pid": 3}
        ],
        "processes": [
            {"pid": 1, "user_id": 1000, "name": "a"},
            {"pid": 2, "user_id": 1000, "name": "b"},
            {"pid": 3, "user_id": 1000, "name": "c"}
        ]
    }));
    assert!(snap.is_complete());
    snap.apply_host_budget(2);
    assert!(snap.capped);
    assert!(!snap.is_complete());
    assert_eq!(snap.ports.len(), 2);
    assert_eq!(snap.ports[0].port, 8001);
    assert_eq!(snap.ports[1].port, 8002);
    assert_eq!(snap.processes.len(), 2);
    assert!(!snap.processes.contains_key(&3));
}

#[test]
fn sanitizes_bidi_controls_enforces_caps_and_derives_labels() {
    let hostile = "foo\n\r\t\u{001b}\u{202e}\u{2066}bar";
    let long_script = format!("/opt/{}/server.py", "a".repeat(1200));
    let snap = PortSnapshot::parse(&json!({
        "status": "ok",
        "ports": [
            {"port": 8000, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z", "pid": 1},
            {"port": 8001, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z", "pid": 2},
            {"port": 8002, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z", "pid": 3},
            {"port": 8003, "address": "127.0.0.1", "protocol": "tcp", "first_seen_at": "2026-11-24T12:00:00Z", "pid": 4}
        ],
        "processes": [
            {
                "pid": 1,
                "user_id": 1000,
                "user": hostile,
                "name": "python3.14",
                "path": "/usr/bin/python3.14",
                "invoked_path": "/usr/bin/python3",
                "argv0": "python3",
                "script": "/srv/app/dev_server.py///",
                "cwd": hostile
            },
            {
                "pid": 2,
                "user_id": 0,
                "name": "fallback_name",
                "argv0": "C:\\Program Files\\Node\\node.exe\\\\",
                "script": "   ///  "
            },
            {
                "pid": 3,
                "user_id": 1000,
                "name": format!("{}x", "n".repeat(80)),
                "argv0": "",
                "script": ""
            },
            {
                "pid": 4,
                "user_id": 1000,
                "name": "python3",
                "script": long_script
            }
        ]
    }));

    let p1 = snap.process(Some(1)).unwrap();
    assert_eq!(p1.user.as_deref(), Some("foo bar"));
    assert_eq!(p1.cwd.as_deref(), Some("foo bar"));
    assert_eq!(p1.label, "dev_server.py");

    let p2 = snap.process(Some(2)).unwrap();
    assert_eq!(p2.user_id, Some(0));
    assert_eq!(p2.label, "node.exe");

    let p3 = snap.process(Some(3)).unwrap();
    assert_eq!(p3.name.chars().count(), MAX_NAME_CHARS);
    assert_eq!(p3.label.chars().count(), MAX_LABEL_CHARS);

    let p4 = snap.process(Some(4)).unwrap();
    assert_eq!(p4.script.as_ref().unwrap().chars().count(), MAX_PATH_CHARS);
    // Script was truncated at 1024 chars before label derivation, so label is capped cleanly at 48.
    assert_eq!(p4.label.chars().count(), MAX_LABEL_CHARS);
}

#[test]
fn zone_validation_and_timestamps() {
    // Valid zones: [A-Za-z0-9._-] up to 16 chars on IPv6.
    assert_eq!(
        PortAddress::parse("fe80::1%eth0.1_a-2"),
        PortAddress::Ip {
            ip: IpAddr::V6("fe80::1".parse::<Ipv6Addr>().unwrap()),
            zone: Some("eth0.1_a-2".into()),
        }
    );
    assert_eq!(
        PortAddress::parse("fe80::1%0123456789abcdef"),
        PortAddress::Ip {
            ip: IpAddr::V6("fe80::1".parse::<Ipv6Addr>().unwrap()),
            zone: Some("0123456789abcdef".into()),
        }
    );

    // Invalid zones -> Unparsable.
    for bad in [
        "fe80::1%",
        "fe80::1%0123456789abcdefg",
        "fe80::1%eth0;id",
        "fe80::1%eth 0",
        "fe80::1%eth/0",
        "127.0.0.1%eth0",
        "[::1]",
        "localhost",
        "127.0.0.1:8080",
    ] {
        assert!(
            matches!(PortAddress::parse(bad), PortAddress::Unparsable(_)),
            "expected Unparsable for {bad:?}"
        );
    }

    // Valid vs invalid RFC 3339 timestamps.
    assert_eq!(
        parse_timestamp("2026-11-24T12:34:56Z").as_deref(),
        Some("2026-11-24T12:34:56Z")
    );
    assert_eq!(
        parse_timestamp("2026-11-24T12:34:56.123456789Z").as_deref(),
        Some("2026-11-24T12:34:56.123456789Z")
    );
    for bad in [
        "",
        "2026-13-01T00:00:00Z",
        "2026-02-29T00:00:00Z",
        "2026-11-24 12:00:00Z",
        "2026-11-24T12:00:00",
        "2026-11-24T12:00:00Z\nrm -rf /",
        "2026-11-24T12:00:00.1234567890123456789012345Z",
    ] {
        assert_eq!(parse_timestamp(bad), None, "expected None for {bad:?}");
    }
}

#[test]
fn host_locality_only_unix_is_local() {
    assert_eq!(
        HostLocality::from_endpoint("unix:///run/user/1000/rex/server.sock"),
        HostLocality::Local
    );
    assert_eq!(
        HostLocality::from_endpoint("ssh://alice@localhost"),
        HostLocality::Ssh
    );
    for remote in [
        "https://localhost:7297",
        "http://127.0.0.1:7297",
        "wss://127.0.0.1:7297",
        "ws://localhost:7297",
        "https://box.tail.ts.net",
        "",
    ] {
        assert_eq!(
            HostLocality::from_endpoint(remote),
            HostLocality::Remote,
            "expected Remote for {remote:?}"
        );
    }
}

#[test]
fn exhaustive_locality_and_reach_matrix() {
    let local = "unix:///run/user/1000/rex/server.sock";
    let ssh = "ssh://alice@devbox";
    let remote_local_url = "https://127.0.0.1:7297";

    let loopback_v4 = PortAddress::parse("127.0.0.1");
    let loopback_v4_alt = PortAddress::parse("127.0.1.5");
    let loopback_v6 = PortAddress::parse("::1");
    let wildcard_v4 = PortAddress::parse("0.0.0.0");
    let wildcard_v6 = PortAddress::parse("::");
    let specific_v4 = PortAddress::parse("192.168.1.42");
    let specific_v6 = PortAddress::parse("2001:db8::42");
    let zoned_v6 = PortAddress::parse("fe80::1%eth0");
    let unparsable = PortAddress::parse("evil.example;rm -rf /");

    assert_eq!(loopback_v4.scope(), AddressScope::Loopback);
    assert_eq!(loopback_v4_alt.scope(), AddressScope::Loopback);
    assert_eq!(loopback_v6.scope(), AddressScope::Loopback);
    assert_eq!(wildcard_v4.scope(), AddressScope::Wildcard);
    assert_eq!(wildcard_v6.scope(), AddressScope::Wildcard);
    assert_eq!(specific_v4.scope(), AddressScope::Specific);
    assert_eq!(specific_v6.scope(), AddressScope::Specific);
    assert_eq!(zoned_v6.scope(), AddressScope::LinkLocalZoned);
    assert_eq!(unparsable.scope(), AddressScope::Unparsable);

    struct Row<'a> {
        name: &'a str,
        addr: &'a PortAddress,
        port: u16,
        local_open: Option<&'a str>,
        copy: &'a str,
        ssh_cmd: Option<&'a str>,
    }

    let rows = [
        Row {
            name: "loopback v4",
            addr: &loopback_v4,
            port: 3000,
            local_open: Some("http://127.0.0.1:3000"),
            copy: "127.0.0.1:3000",
            ssh_cmd: Some("ssh -N -L 3000:127.0.0.1:3000 -- alice@devbox"),
        },
        Row {
            name: "loopback v4 /8",
            addr: &loopback_v4_alt,
            port: 8080,
            local_open: Some("http://127.0.1.5:8080"),
            copy: "127.0.1.5:8080",
            ssh_cmd: Some("ssh -N -L 8080:127.0.1.5:8080 -- alice@devbox"),
        },
        Row {
            name: "loopback v6",
            addr: &loopback_v6,
            port: 5173,
            local_open: Some("http://[::1]:5173"),
            copy: "[::1]:5173",
            ssh_cmd: Some("ssh -N -L '5173:[::1]:5173' -- alice@devbox"),
        },
        Row {
            name: "wildcard v4",
            addr: &wildcard_v4,
            port: 8000,
            local_open: Some("http://127.0.0.1:8000"),
            copy: "0.0.0.0:8000",
            ssh_cmd: Some("ssh -N -L 8000:127.0.0.1:8000 -- alice@devbox"),
        },
        Row {
            name: "wildcard v6",
            addr: &wildcard_v6,
            port: 9000,
            local_open: Some("http://[::1]:9000"),
            copy: "[::]:9000",
            ssh_cmd: Some("ssh -N -L '9000:[::1]:9000' -- alice@devbox"),
        },
        Row {
            name: "specific v4",
            addr: &specific_v4,
            port: 4000,
            local_open: Some("http://192.168.1.42:4000"),
            copy: "192.168.1.42:4000",
            ssh_cmd: Some("ssh -N -L 4000:192.168.1.42:4000 -- alice@devbox"),
        },
        Row {
            name: "specific v6",
            addr: &specific_v6,
            port: 4000,
            local_open: Some("http://[2001:db8::42]:4000"),
            copy: "[2001:db8::42]:4000",
            ssh_cmd: Some("ssh -N -L '4000:[2001:db8::42]:4000' -- alice@devbox"),
        },
        Row {
            name: "link-local zoned",
            addr: &zoned_v6,
            port: 8080,
            local_open: None,
            copy: "[fe80::1%eth0]:8080",
            ssh_cmd: None,
        },
        Row {
            name: "unparsable",
            addr: &unparsable,
            port: 8080,
            local_open: None,
            copy: "8080",
            ssh_cmd: None,
        },
    ];

    for row in rows {
        // Local host
        assert_eq!(
            open_url(HostLocality::from_endpoint(local), row.addr, row.port).as_deref(),
            row.local_open,
            "local open_url for {}",
            row.name
        );
        assert_eq!(
            ssh_forward_command(local, row.addr, row.port),
            None,
            "local ssh_forward_command for {}",
            row.name
        );

        // SSH host: NEVER open in browser, ssh -L only when allowed
        assert_eq!(
            open_url(HostLocality::from_endpoint(ssh), row.addr, row.port),
            None,
            "ssh open_url for {}",
            row.name
        );
        assert_eq!(
            ssh_forward_command(ssh, row.addr, row.port).as_deref(),
            row.ssh_cmd,
            "ssh forward for {}",
            row.name
        );

        // Remote host (even https://127.0.0.1): NEVER open in browser, NEVER ssh -L
        assert_eq!(
            open_url(
                HostLocality::from_endpoint(remote_local_url),
                row.addr,
                row.port
            ),
            None,
            "remote open_url for {}",
            row.name
        );
        assert_eq!(
            ssh_forward_command(remote_local_url, row.addr, row.port),
            None,
            "remote ssh_forward_command for {}",
            row.name
        );

        // Copy address is identical across all localities and never leaks raw unparsable text
        assert_eq!(
            copy_address(row.addr, row.port),
            row.copy,
            "copy_address for {}",
            row.name
        );
    }
}

#[test]
fn ssh_forward_privileged_ports_custom_port_and_hostile_destination_quoting() {
    let loopback = PortAddress::Ip {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        zone: None,
    };

    // Privileged remote port < 1024 maps to 10000 + port.
    assert_eq!(
        ssh_forward_command("ssh://alice@web:2222/run/rex.sock", &loopback, 80).as_deref(),
        Some("ssh -N -L 10080:127.0.0.1:80 -p 2222 -- alice@web")
    );
    assert_eq!(
        ssh_forward_command("ssh://web", &loopback, 443).as_deref(),
        Some("ssh -N -L 10443:127.0.0.1:443 -- web")
    );
    assert_eq!(
        ssh_forward_command("ssh://web", &loopback, 1023).as_deref(),
        Some("ssh -N -L 11023:127.0.0.1:1023 -- web")
    );
    assert_eq!(
        ssh_forward_command("ssh://web", &loopback, 1024).as_deref(),
        Some("ssh -N -L 1024:127.0.0.1:1024 -- web")
    );

    // Hostile shell metacharacters in user/host MUST be POSIX single-quoted cleanly.
    let hostile = "ssh://user@host'$(id);rm";
    assert_eq!(
        ssh_forward_command(hostile, &loopback, 8080).as_deref(),
        Some("ssh -N -L 8080:127.0.0.1:8080 -- 'user@host'\\''$(id);rm'")
    );
    let backticks = "ssh://`whoami`@box;touch";
    assert_eq!(
        ssh_forward_command(backticks, &loopback, 3000).as_deref(),
        Some("ssh -N -L 3000:127.0.0.1:3000 -- '`whoami`@box;touch'")
    );
}
