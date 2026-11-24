use super::*;
use crate::app::{panel::SettingsSection, ssh_agent::ProbeState};
use rex_rune::{
    settings::ssh::SshAgent,
    ssh::{
        Problem,
        agent::{AgentHealth, DetectedAgent, Report},
    },
};
use std::path::PathBuf;

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

fn sample_report(selected_health: AgentHealth, selected_name: Option<&'static str>) -> Report {
    Report {
        system_health: AgentHealth::Answering { keys: 2 },
        system_name: Some("1Password"),
        selected_health,
        selected_name,
        detected: vec![
            DetectedAgent {
                name: "1Password",
                saved: "~/.1password/agent.sock".into(),
                path: PathBuf::from("/fake/home/.1password/agent.sock"),
                health: AgentHealth::Answering { keys: 2 },
            },
            DetectedAgent {
                name: "OpenSSH agent",
                saved: "/run/user/1000/ssh-agent.socket".into(),
                path: PathBuf::from("/run/user/1000/ssh-agent.socket"),
                health: AgentHealth::Answering { keys: 0 },
            },
        ],
    }
}

#[test]
fn ssh_agent_never_probes_at_startup_and_drops_stale_generations() {
    let mut ws = workspace_hosts(2);
    ws.ssh_agent.available = true;
    assert_eq!(ws.ssh_agent.state, ProbeState::Idle);
    assert!(ws.ssh_agent.probe_requests.is_empty());

    // Opening Remote Hosts triggers a non-waking probe.
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Hosts);
    assert_eq!(ws.ssh_agent.probe_requests.len(), 1);
    assert_eq!(ws.ssh_agent.probe_requests[0], (1, SshAgent::System, false));
    assert_eq!(ws.ssh_agent.state, ProbeState::Checking);

    // Refresh bumps generation to 2 with wake_on_ready = true.
    ws.refresh_ssh_agents(true);
    assert_eq!(ws.ssh_agent.probe_requests.len(), 2);
    assert_eq!(ws.ssh_agent.probe_requests[1], (2, SshAgent::System, true));

    // A stale generation-1 reply is ignored completely (neither overwrites state nor wakes).
    ws.apply_ssh_agent_probe(1, sample_report(AgentHealth::NotFound, None));
    assert_eq!(ws.ssh_agent.state, ProbeState::Checking);
    assert!(ws.ssh_agent.sent_host_updates.is_empty());

    // Generation-2 reply with keys > 0 updates state and wakes hosts!
    let good = sample_report(AgentHealth::Answering { keys: 2 }, Some("1Password"));
    ws.apply_ssh_agent_probe(2, good.clone());
    assert_eq!(ws.ssh_agent.state, ProbeState::Ready(good.clone()));
    assert_eq!(ws.ssh_agent.sent_host_updates.len(), 2);
    assert!(
        ws.ssh_agent
            .sent_host_updates
            .iter()
            .all(|(_, sock, wake)| sock.is_none() && *wake)
    );

    // A non-waking re-entry while a waking Refresh is in flight must preserve pending_wake.
    ws.ssh_agent.sent_host_updates.clear();
    ws.refresh_ssh_agents(true);
    ws.refresh_ssh_agents(false);
    ws.apply_ssh_agent_probe(4, good);
    assert_eq!(ws.ssh_agent.sent_host_updates.len(), 2);
    assert!(
        ws.ssh_agent
            .sent_host_updates
            .iter()
            .all(|(_, _, wake)| *wake)
    );
}

#[test]
fn custom_socket_aliasing_detected_agent_marks_existing_row_without_duplicating() {
    let mut ws = workspace_hosts(1);
    ws.ssh_agent.available = true;
    ws.settings.ssh.agent = SshAgent::Socket("/fake/home/.1password/agent.sock".into());
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Hosts);
    ws.apply_ssh_agent_probe(
        1,
        sample_report(AgentHealth::Answering { keys: 2 }, Some("1Password")),
    );

    let entries = ws.ssh_agent_entries();
    // System default + 1Password + Bitwarden + Custom socket… (no duplicate custom row).
    assert_eq!(entries.len(), 4);
    assert_eq!(entries.iter().filter(|e| e.current).count(), 1);
    assert!(entries.iter().any(|e| e.label == "1Password" && e.current));
    assert!(!entries.iter().any(|e| e.label == "Custom socket"));
}

#[test]
fn picking_agent_and_refresh_wake_only_when_keys_greater_than_zero() {
    let mut ws = workspace_hosts(1);
    ws.ssh_agent.available = true;
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Hosts);
    ws.apply_ssh_agent_probe(
        1,
        sample_report(AgentHealth::Answering { keys: 2 }, Some("1Password")),
    );

    // Open picker and verify System default + detected agents + Custom socket…
    let mut tx = rune::Tx::default();
    ws.open_anchored("ssh-agent", Intent::ChooseSshAgent, &mut tx);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::SshAgents)
    ));
    let labels: Vec<(String, String, bool)> = ws
        .entries()
        .into_iter()
        .map(|e| (e.label, e.detail, e.current))
        .collect();
    assert_eq!(
        labels,
        vec![
            ("System default".into(), "1Password · 2 keys".into(), true),
            ("1Password".into(), "2 keys".into(), false),
            ("OpenSSH agent".into(), "0 keys".into(), false),
            ("Custom socket…".into(), "".into(), false),
        ]
    );

    // Pick OpenSSH agent (0 keys): saves setting, pushes socket with wake=false, probes, and
    // when probe finishes with 0 keys does NOT send wake=true.
    ws.ssh_agent.sent_host_updates.clear();
    ws.perform(
        Intent::SshAgent(SshAgent::Socket("/run/user/1000/ssh-agent.socket".into())),
        &mut tx,
    );
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    assert_eq!(
        ws.settings.ssh.agent,
        SshAgent::Socket("/run/user/1000/ssh-agent.socket".into())
    );
    assert_eq!(
        Settings::load(&ws.settings_path).unwrap().ssh.agent,
        SshAgent::Socket("/run/user/1000/ssh-agent.socket".into())
    );
    assert_eq!(
        ws.ssh_agent.sent_host_updates,
        vec![(
            0,
            Some(PathBuf::from("/run/user/1000/ssh-agent.socket")),
            false
        )]
    );
    let generation = ws.ssh_agent.generation;
    ws.apply_ssh_agent_probe(
        generation,
        sample_report(AgentHealth::Answering { keys: 0 }, Some("OpenSSH agent")),
    );
    assert_eq!(
        ws.ssh_agent.sent_host_updates.len(),
        1,
        "0 keys must not send wake_auth_retry=true"
    );

    // Refresh finds 3 keys now -> sends wake_auth_retry=true.
    ws.refresh_ssh_agents(true);
    let generation = ws.ssh_agent.generation;
    ws.apply_ssh_agent_probe(
        generation,
        sample_report(AgentHealth::Answering { keys: 3 }, Some("OpenSSH agent")),
    );
    assert_eq!(
        ws.ssh_agent.sent_host_updates.last(),
        Some(&(
            0,
            Some(PathBuf::from("/run/user/1000/ssh-agent.socket")),
            true
        ))
    );
}

#[test]
fn custom_socket_form_validates_saves_uncreated_path_and_returns_to_settings() {
    let mut ws = workspace();
    ws.ssh_agent.available = true;
    let mut tx = rune::Tx::default();
    ws.perform(Intent::Settings, &mut tx);
    ws.open_section(SettingsSection::Hosts);
    ws.open_anchored("ssh-agent", Intent::ChooseSshAgent, &mut tx);
    ws.perform(Intent::SshAgentCustom, &mut tx);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Form(Form::SshAgentSocket))
    ));

    // Invalid paths keep the form open with an inline error.
    for bad in [
        "",
        "relative/sock",
        "/tmp/dir/",
        "~/../bad.sock",
        "/tmp/a\nb",
    ] {
        ws.panel.as_mut().unwrap().inputs[0].text = bad.into();
        ws.submit(&mut tx);
        assert!(
            ws.panel.as_ref().unwrap().error.is_some(),
            "expected error for {bad:?}"
        );
        assert!(matches!(
            ws.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Form(Form::SshAgentSocket))
        ));
    }

    // Valid non-existent on-demand socket path saves cleanly, returns to Settings, and probes!
    ws.panel.as_mut().unwrap().inputs[0].text = "~/on-demand/hw-agent.sock".into();
    let probes_before = ws.ssh_agent.probe_requests.len();
    ws.submit(&mut tx);
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    assert_eq!(
        ws.settings.ssh.agent,
        SshAgent::Socket("~/on-demand/hw-agent.sock".into())
    );
    assert_eq!(
        Settings::load(&ws.settings_path).unwrap().ssh.agent,
        SshAgent::Socket("~/on-demand/hw-agent.sock".into())
    );
    assert_eq!(ws.ssh_agent.probe_requests.len(), probes_before + 1);
    assert_eq!(
        ws.ssh_agent.probe_requests.last().unwrap().1,
        SshAgent::Socket("~/on-demand/hw-agent.sock".into())
    );
}

#[test]
fn settings_search_finds_ssh_agent_terms_and_640x480_layout_fits() {
    let mut ws = workspace_hosts(2);
    ws.ssh_agent.available = true;
    ws.settings.ssh.agent =
        SshAgent::Socket("/var/folders/very/long/custom/path/to/a/hardware/ssh-agent.sock".into());
    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Hosts);
    let generation = ws.ssh_agent.generation;
    ws.apply_ssh_agent_probe(generation, sample_report(AgentHealth::NotFound, None));

    // Search indexing checks
    for query in ["ssh agent", "1password", "custom socket", "identityagent"] {
        ws.settings_nav.search.text = query.into();
        ws.settings_search_changed();
        assert!(
            ws.settings_nav
                .hits()
                .iter()
                .any(|hit| hit.page == SettingsSection::Hosts),
            "search {query:?} did not find Remote Hosts: {:?}",
            ws.settings_nav.hits()
        );
    }
    ws.clear_settings_search();

    // 640x480 minimum window layout check
    let mut rt = runtime_at(ws, (640., 480.));
    settle(&mut rt);
    let scroll = rect(&rt, "settings-scroll:hosts");
    for key in ["ssh-agent", "ssh-agent-status", "ssh-agent-refresh"] {
        let r = rect(&rt, key);
        assert!(
            r[0] >= scroll[0] - 0.5 && r[0] + r[2] <= scroll[0] + scroll[2] + 0.5,
            "{key} leaves 640x480 page: {r:?} not in {scroll:?}"
        );
    }
}

#[test]
fn ssh_section_is_omitted_when_unavailable_and_saved_agent_is_preserved() {
    let mut ws = workspace();
    ws.settings.ssh.agent = SshAgent::Socket("~/.1password/agent.sock".into());
    ws.save();
    ws.ssh_agent.available = false;

    ws.perform(Intent::Settings, &mut rune::Tx::default());
    ws.open_section(SettingsSection::Hosts);
    assert!(
        ws.ssh_agent.probe_requests.is_empty(),
        "unavailable platform must never probe"
    );

    // Search must also omit SSH-agent aliases when SSH is unavailable.
    for query in ["1password", "identityagent"] {
        ws.settings_nav.search.text = query.into();
        ws.settings_search_changed();
        assert!(
            ws.settings_nav.hits().is_empty(),
            "search {query:?} must not match when SSH is unavailable: {:?}",
            ws.settings_nav.hits()
        );
    }
    ws.clear_settings_search();

    let mut rt = runtime_at(ws, (1024., 700.));
    settle(&mut rt);
    assert!(
        !rt.elements()
            .iter()
            .any(|e| matches!(e.key.as_deref(), Some("ssh-agent" | "ssh-agent-refresh"))),
        "SSH group must be omitted when SSH is unavailable"
    );

    // Saving another preference must preserve ssh.agent untouched on disk.
    rt.state.settings.vertical_tabs = true;
    rt.state.save();
    assert_eq!(
        Settings::load(&rt.state.settings_path).unwrap().ssh.agent,
        SshAgent::Socket("~/.1password/agent.sock".into())
    );
}

#[test]
fn about_debug_info_redacts_ssh_auth_failure_and_never_leaks_socket_path() {
    let mut ws = workspace();
    let secret_socket = "/Users/alice/private-vault-1234/agent.sock";
    ws.settings.ssh.agent = SshAgent::Socket(secret_socket.into());
    ws.hosts[0].model.connected = false;
    ws.hosts[0].model.ssh_problem = Some(Problem::AuthFailed);
    ws.hosts[0].model.status =
        "Disconnected · ssh alice@secret-prod.internal: Permission denied (publickey).".into();

    assert_eq!(
        crate::app::about::state(&ws.hosts[0].model),
        "SSH authentication failed"
    );
    let debug = crate::app::about::debug_info(&ws);
    assert!(debug.contains("SSH authentication failed"), "{debug}");
    assert!(
        !debug.contains(secret_socket),
        "socket path leaked: {debug}"
    );
    assert!(
        !debug.contains("secret-prod.internal"),
        "hostname leaked: {debug}"
    );
}
