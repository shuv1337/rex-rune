//! Settings → Remote Hosts → SSH: pick which SSH agent `ssh` uses for `ssh://` hosts,
//! inspect its live key count off the UI thread, and wake auth-failed SSH hosts when a
//! usable agent is selected or refreshed.
use super::{
    panel::{Entry, Form, Intent, PanelKind},
    settings_kit::{self, ChipTone, button, card, footnote, setting, state_chip, value_popup},
    workspace::Workspace,
};
use rex_rune::{
    connection::HostCommand,
    settings::ssh::{SshAgent, parse_socket},
    ssh::agent::{AgentHealth, Report},
};
use rune::{El, row};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProbeState {
    #[default]
    Idle,
    Checking,
    Ready(Report),
}

pub struct SshAgentState {
    pub generation: u64,
    pub pending_wake: bool,
    pub state: ProbeState,
    /// Mirrors `platform::local::ssh_available()`; mutable in tests to exercise Windows omission.
    pub available: bool,
    #[cfg(test)]
    pub probe_requests: Vec<(u64, SshAgent, bool)>,
    #[cfg(test)]
    pub sent_host_updates: Vec<(usize, Option<PathBuf>, bool)>,
}

impl Default for SshAgentState {
    fn default() -> Self {
        Self {
            generation: 0,
            pending_wake: false,
            state: ProbeState::Idle,
            available: rex_rune::platform::local::ssh_available(),
            #[cfg(test)]
            probe_requests: Vec::new(),
            #[cfg(test)]
            sent_host_updates: Vec::new(),
        }
    }
}

fn short_socket_label(socket: &str, max_chars: usize) -> String {
    let chars: Vec<char> = socket.chars().collect();
    if chars.len() <= max_chars {
        return socket.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    let head = keep.div_ceil(2);
    let tail = keep - head;
    chars[..head]
        .iter()
        .chain(std::iter::once(&'…'))
        .chain(&chars[chars.len() - tail..])
        .collect()
}

fn keys_word(keys: u32) -> String {
    if keys == 1 {
        "1 key".into()
    } else {
        format!("{keys} keys")
    }
}

fn health_short(health: &AgentHealth) -> String {
    match health {
        AgentHealth::NotSet => "Not set".into(),
        AgentHealth::NotFound => "Not found".into(),
        AgentHealth::NotAnswering(_) => "Not answering".into(),
        AgentHealth::Answering { keys } => keys_word(*keys),
    }
}

impl Workspace {
    /// Resolved `SSH_AUTH_SOCK` path for the saved SSH agent, or `None` for System default.
    pub fn ssh_agent_socket(&self) -> Option<PathBuf> {
        self.settings.ssh.agent.resolve_default()
    }

    /// Start an off-thread SSH agent discovery/probe pass. Never called on app startup;
    /// triggered only when opening Remote Hosts settings, picking an agent, or pressing Refresh.
    pub fn refresh_ssh_agents(&mut self, wake_on_ready: bool) {
        if !self.ssh_agent.available {
            return;
        }
        self.ssh_agent.generation += 1;
        let generation = self.ssh_agent.generation;
        self.ssh_agent.pending_wake |= wake_on_ready;
        self.ssh_agent.state = ProbeState::Checking;
        let configured = self.settings.ssh.agent.clone();

        #[cfg(test)]
        {
            self.ssh_agent
                .probe_requests
                .push((generation, configured, wake_on_ready));
        }

        #[cfg(not(test))]
        {
            let handle = self.handle.clone();
            let spawned = std::thread::Builder::new()
                .name("rex-ssh-agent".into())
                .spawn(move || {
                    let report = rex_rune::ssh::agent::discover(&configured);
                    handle.update(move |s, _| s.apply_ssh_agent_probe(generation, report));
                });
            if let Err(error) = spawned {
                let reason = format!("Couldn't check SSH agents: {error}");
                self.apply_ssh_agent_probe(
                    generation,
                    Report {
                        system_health: AgentHealth::NotAnswering(reason.clone()),
                        system_name: None,
                        selected_health: AgentHealth::NotAnswering(reason),
                        selected_name: None,
                        detected: Vec::new(),
                    },
                );
            }
        }
    }

    /// Receive a probe result; stale generations are dropped cleanly.
    pub fn apply_ssh_agent_probe(&mut self, generation: u64, report: Report) {
        if generation != self.ssh_agent.generation {
            return;
        }
        let should_wake = self.ssh_agent.pending_wake && report.selected_health.has_keys();
        self.ssh_agent.pending_wake = false;
        self.ssh_agent.state = ProbeState::Ready(report);
        if should_wake {
            self.push_ssh_agent(true);
        }
    }

    /// Save the user's chosen SSH agent, update all host workers without dropping connected
    /// hosts, and probe it immediately (waking auth-failed SSH hosts if it answers with keys).
    pub fn select_ssh_agent(&mut self, agent: SshAgent) {
        self.settings.ssh.agent = agent;
        self.save();
        self.push_ssh_agent(false);
        self.refresh_ssh_agents(true);
    }

    fn push_ssh_agent(&mut self, wake_auth_retry: bool) {
        let socket = self.ssh_agent_socket();
        for index in 0..self.hosts.len() {
            #[cfg(test)]
            self.ssh_agent
                .sent_host_updates
                .push((index, socket.clone(), wake_auth_retry));
            self.send(
                index,
                HostCommand::SshAgent {
                    socket: socket.clone(),
                    wake_auth_retry,
                },
            );
        }
    }

    pub fn open_ssh_agents(&mut self) {
        self.open_panel(PanelKind::SshAgents, "SSH agent", &[]);
        if let Some(current) = self.entries().iter().position(|e| e.current) {
            self.panel.as_mut().unwrap().selected = current;
        }
    }

    pub fn open_ssh_agent_custom(&mut self) {
        let current = self.settings.ssh.agent.socket().unwrap_or("").to_owned();
        self.open_panel(
            PanelKind::Form(Form::SshAgentSocket),
            "Custom SSH agent socket",
            &[("Socket path", "/path/to/agent.sock or ~/…", &current)],
        );
    }

    pub fn ssh_agent_entries(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        let report = match &self.ssh_agent.state {
            ProbeState::Ready(report) => Some(report),
            ProbeState::Idle | ProbeState::Checking => None,
        };

        let system_detail = match report {
            Some(report) => match report.system_name {
                Some(name) => format!("{name} · {}", health_short(&report.system_health)),
                None => health_short(&report.system_health),
            },
            None if matches!(self.ssh_agent.state, ProbeState::Checking) => "Checking…".into(),
            None => String::new(),
        };
        let mut system = Entry::new(
            "System default",
            system_detail,
            "",
            Intent::SshAgent(SshAgent::System),
        )
        .in_section(0);
        system.current = self.settings.ssh.agent.is_system();
        entries.push(system);

        let saved_resolved = self.ssh_agent_socket();
        let mut matched_saved = self.settings.ssh.agent.is_system();
        if let Some(report) = report {
            for agent in &report.detected {
                let duplicate_name = report
                    .detected
                    .iter()
                    .filter(|other| other.name == agent.name)
                    .count()
                    > 1;
                let label = if duplicate_name {
                    format!("{} ({})", agent.name, short_socket_label(&agent.saved, 22))
                } else {
                    agent.name.to_owned()
                };
                let current = matches!(&self.settings.ssh.agent, SshAgent::Socket(s) if s == &agent.saved)
                    || saved_resolved.as_deref().is_some_and(|resolved| {
                        rex_rune::ssh::agent::same_path(resolved, &agent.path)
                    });
                matched_saved |= current;
                let mut entry = Entry::new(
                    label,
                    health_short(&agent.health),
                    "",
                    Intent::SshAgent(SshAgent::Socket(agent.saved.clone())),
                )
                .in_section(1);
                entry.current = current;
                entries.push(entry);
            }
        }

        if !matched_saved && let SshAgent::Socket(saved) = &self.settings.ssh.agent {
            let detail = match report {
                Some(report) => health_short(&report.selected_health),
                None => "Custom".into(),
            };
            let mut custom_current = Entry::new(
                short_socket_label(saved, 28),
                detail,
                "",
                Intent::SshAgent(SshAgent::Socket(saved.clone())),
            )
            .in_section(1);
            custom_current.current = true;
            entries.push(custom_current);
        }

        let mut custom = Entry::new("Custom socket…", "", "", Intent::SshAgentCustom).in_section(2);
        custom.followup = true;
        entries.push(custom);
        entries
    }
}

fn selected_label(s: &Workspace) -> String {
    match &s.settings.ssh.agent {
        SshAgent::System => "System default".into(),
        SshAgent::Socket(raw) => {
            if let ProbeState::Ready(report) = &s.ssh_agent.state
                && let Some(name) = report.selected_name
            {
                return name.to_owned();
            }
            short_socket_label(raw, 24)
        }
    }
}

fn status_presentation(s: &Workspace) -> (String, String, ChipTone, String) {
    match &s.ssh_agent.state {
        ProbeState::Idle => (
            "Status has not been checked yet.".into(),
            "Not checked".into(),
            ChipTone::Off,
            String::new(),
        ),
        ProbeState::Checking => (
            "Checking configured and detected SSH agents…".into(),
            "Checking…".into(),
            ChipTone::Off,
            String::new(),
        ),
        ProbeState::Ready(report) => {
            let subject = match (&s.settings.ssh.agent, report.selected_name) {
                (SshAgent::System, Some(name)) => format!("System default ({name})"),
                (SshAgent::System, None) => "System default".into(),
                (SshAgent::Socket(_), Some(name)) => name.to_owned(),
                (SshAgent::Socket(path), None) => short_socket_label(path, 36),
            };
            match &report.selected_health {
                AgentHealth::Answering { keys } if *keys > 0 => (
                    format!(
                        "{subject} is answering with {} public {}.",
                        keys,
                        if *keys == 1 { "key" } else { "keys" }
                    ),
                    keys_word(*keys),
                    ChipTone::Good,
                    String::new(),
                ),
                AgentHealth::Answering { .. } => (
                    format!("{subject} answered with no public keys loaded."),
                    "0 keys".into(),
                    ChipTone::Attention,
                    "Agent answered, but has no public keys loaded".into(),
                ),
                AgentHealth::NotSet => (
                    "SSH_AUTH_SOCK is not set in this app's environment.".into(),
                    "Not set".into(),
                    ChipTone::Off,
                    "SSH_AUTH_SOCK is not set in this app's environment".into(),
                ),
                AgentHealth::NotFound => (
                    format!("Socket not found for {subject}."),
                    "Not found".into(),
                    ChipTone::Attention,
                    "Socket path does not exist".into(),
                ),
                AgentHealth::NotAnswering(reason) => (
                    format!("{subject}: {reason}."),
                    "Not answering".into(),
                    ChipTone::Attention,
                    reason.clone(),
                ),
            }
        }
    }
}

/// Validate a custom socket form value.
pub fn validate_socket_form(input: &str) -> Result<String, String> {
    parse_socket(input).map_err(str::to_owned)
}

/// Settings → Remote Hosts → SSH group. Returns `None` on platforms without SSH (Windows).
pub fn section(s: &Workspace) -> Option<El> {
    if !s.ssh_agent.available {
        return None;
    }
    let choice = selected_label(s);
    let (detail, chip_label, chip_tone, chip_why) = status_presentation(s);
    Some(settings_kit::section(
        "SSH",
        [
            card([
                setting(
                    "SSH agent",
                    "Agent used by ssh:// hosts, or a custom socket path.",
                    value_popup("ssh-agent", &choice, Intent::ChooseSshAgent),
                ),
                setting(
                    "Agent status",
                    &detail,
                    row()
                        .gap(8.)
                        .center_y()
                        .child(state_chip(
                            "ssh-agent-status",
                            &chip_label,
                            chip_tone,
                            &chip_why,
                        ))
                        .child(button(
                            "ssh-agent-refresh",
                            "Refresh",
                            super::widgets::Pill::Neutral,
                            |s, _| s.refresh_ssh_agents(true),
                        )),
                ),
            ]),
            footnote("IdentityAgent in ~/.ssh/config still chooses the agent for hosts it names."),
        ],
    ))
}
