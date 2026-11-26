//! On-demand listening ports for the current session. Reach decisions belong to the model.
use super::{
    activity::{pane_name, tab_name},
    keycaps::keycap,
    panel::{Entry, Intent, PanelKind},
    theme::*,
    widgets::highlight,
    workspace::{HostId, Workspace},
};
use rex_rune::{
    connection::{HostCommand, HostModel, activity::TERMINAL},
    ports::{
        self, AddressScope, HostLocality, PORTS_EVENTS_MIN_API_REVISION, PORTS_MIN_API_REVISION,
        PortSnapshot, PortsStatus,
    },
};
use rexrs::api::session::{LayerBlock, Window};
use rune::{Cursor, El, Tx, col, el, row, text};

pub const ACTION: &str = "client.ports";
pub const API_15: &str = "This server doesn't report port changes. Reopen to refresh.";
pub const PARTIAL: &str = "Some programs couldn't be inspected, so ports may be missing.";
const CAPPED: &str = "Some ports aren't shown because the client reached its display limit.";
const TRUNCATED: &str = "The server shortened its list, so ports may be missing.";

#[derive(Clone, Debug)]
pub struct Target {
    pub host: HostId,
    pub session: Option<String>,
    first: Option<String>,
}

#[derive(PartialEq, Eq)]
pub struct Watch {
    host: HostId,
    epoch: u64,
    session: String,
}

#[derive(Clone, Debug)]
pub struct PortKey {
    block: String,
    address: String,
    port: u16,
}

impl PortKey {
    fn id(&self) -> String {
        // Length-prefix the block, since a raw address may itself contain delimiters.
        format!(
            "ports:{}:{}:{}:{}",
            self.block.len(),
            self.block,
            self.port,
            self.address
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Open,
    Forward,
}

impl Reach {
    fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Forward => "Copy ssh -L",
        }
    }
}

pub struct Presentation {
    pub process: Option<String>,
    pub reach: Option<Reach>,
}

#[derive(Default)]
pub struct Coverage {
    pub remote: Option<String>,
    pub partial: bool,
    pub limits: Option<&'static str>,
    pub api15: bool,
    pub locality: Option<HostLocality>,
    pub empty: &'static str,
}

fn blocks(model: &HostModel) -> impl Iterator<Item = (&Window, &LayerBlock, &str)> {
    model
        .view()
        .into_iter()
        .flat_map(|v| &v.windows)
        .flat_map(|w| {
            w.layers
                .iter()
                .flat_map(|l| &l.blocks)
                .filter_map(move |b| {
                    if b.creator_name.as_deref() == Some(TERMINAL) {
                        Some((w, b, b.block_id.as_deref()?))
                    } else {
                        None
                    }
                })
        })
}

fn supported(model: &HostModel) -> bool {
    model.connected && model.protocol.api_revision_at_least(PORTS_MIN_API_REVISION)
}

fn reach(endpoint: &str, locality: HostLocality, entry: &ports::PortEntry) -> Option<Reach> {
    if ports::open_url(locality, &entry.address, entry.port).is_some() {
        Some(Reach::Open)
    } else if ports::ssh_forward_command(endpoint, &entry.address, entry.port).is_some() {
        Some(Reach::Forward)
    } else {
        None
    }
}

impl Workspace {
    pub fn show_ports(&mut self, first: Option<String>) {
        let model = self.model();
        let session = model.selected.clone();
        let title = session
            .as_ref()
            .and_then(|id| model.sessions.iter().find(|s| &s.session_id == id))
            .map(|s| s.label.as_str())
            .filter(|label| !label.is_empty())
            .map_or_else(
                || "Listening ports".into(),
                |label| format!("Listening ports · {label}"),
            );
        let target = Target {
            host: self.hosts[self.active].id,
            session,
            first,
        };
        self.open_panel(
            PanelKind::Ports(target),
            &title,
            &[("", "Filter ports…", "")],
        );
        // The pane menu's block only sets the initial grouping, not the popup's lifetime.
        self.panel.as_mut().unwrap().target = None;
    }

    pub fn ports_target(&self) -> Option<&Target> {
        match &self.panel.as_ref()?.kind {
            PanelKind::Ports(target) => Some(target),
            _ => None,
        }
    }

    pub fn ports_in_context(&self, target: &Target) -> bool {
        let host = &self.hosts[self.active];
        host.id == target.host && (!host.model.connected || host.model.selected == target.session)
    }

    pub(super) fn refresh_ports_watch(&mut self) {
        let next = self.ports_target().and_then(|target| {
            let model = self.model();
            if self.ports_in_context(target) && supported(model) {
                let session = target.session.as_ref()?;
                if !model.views.contains_key(session) || model.background_views.contains(session) {
                    return None;
                }
                Some(Watch {
                    host: target.host,
                    epoch: model.connection_epoch,
                    session: session.clone(),
                })
            } else {
                None
            }
        });
        if self.ports_watch == next {
            return;
        }
        if let Some(previous) = self.ports_watch.take()
            && let Some(i) = self.host_index(previous.host)
            && self.hosts[i].model.connection_epoch == previous.epoch
            && supported(&self.hosts[i].model)
        {
            self.send(
                i,
                HostCommand::WatchPorts {
                    epoch: previous.epoch,
                    session: None,
                },
            );
        }
        if let Some(watch) = &next {
            self.send(
                self.active,
                HostCommand::WatchPorts {
                    epoch: watch.epoch,
                    session: Some(watch.session.clone()),
                },
            );
        }
        self.ports_watch = next;
    }

    pub fn ports_entries(&self, target: &Target, lower: &str) -> Vec<Entry> {
        let model = self.model();
        if !self.ports_in_context(target)
            || !supported(model)
            || target
                .session
                .as_ref()
                .is_some_and(|session| model.background_views.contains(session))
        {
            return Vec::new();
        }
        let host = &self.settings.hosts[self.active];
        let locality = HostLocality::from_endpoint(&host.endpoint);
        let mut ordered: Vec<_> = blocks(model).collect();
        if let Some(i) = ordered
            .iter()
            .position(|(_, _, id)| Some(*id) == target.first.as_deref())
        {
            ordered[..=i].rotate_right(1);
        }
        let mut entries = Vec::new();
        for (window, block, id) in ordered {
            let Some(snapshot) = model
                .ports
                .get(id)
                .filter(|snapshot| !snapshot.ports.is_empty())
            else {
                continue;
            };
            let tab = tab_name(model, window, self.generated_titles());
            let pane = pane_name(model, block, self.generated_titles());
            let group = format!("{tab} · {pane}");
            let group_search = group.to_lowercase();
            for port in &snapshot.ports {
                let address = port.address.canonical();
                let label = format!(":{}", port.port);
                let process = snapshot
                    .process(port.pid)
                    .map(|p| p.label.as_str())
                    .filter(|p| !p.is_empty());
                if !lower.is_empty()
                    && !label.contains(lower)
                    && !address.to_lowercase().contains(lower)
                    && !group_search.contains(lower)
                    && !process.is_some_and(|p| p.to_lowercase().contains(lower))
                {
                    continue;
                }
                let detail = match port.address.scope() {
                    AddressScope::Loopback if locality == HostLocality::Local => {
                        "this computer only".into()
                    }
                    AddressScope::Loopback => format!("loopback on {}", host.label),
                    AddressScope::Wildcard => "all interfaces".into(),
                    AddressScope::Specific | AddressScope::LinkLocalZoned => address.clone(),
                    AddressScope::Unparsable => "port only".into(),
                };
                let key = PortKey {
                    block: id.into(),
                    address,
                    port: port.port,
                };
                let row_id = key.id();
                let mut entry =
                    Entry::new(label, detail, &group, Intent::Port(key)).group_named(&group);
                entry.id = Some(row_id.into());
                entry.group_key = Some(id.into());
                entry.port = Some(Presentation {
                    process: process.map(str::to_owned),
                    reach: reach(&host.endpoint, locality, port),
                });
                entries.push(entry);
            }
        }
        entries
    }

    pub fn ports_coverage(&self) -> Coverage {
        let model = self.model();
        let host = &self.settings.hosts[self.active];
        let locality = HostLocality::from_endpoint(&host.endpoint);
        let mut coverage = Coverage {
            remote: (locality != HostLocality::Local).then(|| {
                format!(
                    "These ports are open on {}, not on this computer.",
                    host.label
                )
            }),
            ..Coverage::default()
        };
        if !model.connected {
            coverage.empty = "Host is disconnected.";
            return coverage;
        }
        if !supported(model) {
            coverage.empty = "This Rex server can't list ports. Update Rex on the host.";
            return coverage;
        }
        coverage.locality = Some(locality);
        coverage.api15 = !model
            .protocol
            .api_revision_at_least(PORTS_EVENTS_MIN_API_REVISION);
        let mut count = 0;
        let mut unsupported = 0;
        let mut reading = false;
        let mut missing = false;
        let mut any_ports = false;
        let mut capped = false;
        let mut truncated = false;
        for (_, _, id) in blocks(model) {
            count += 1;
            reading |= model.ports_reading.contains(id);
            if let Some(snap) = model.ports.get(id) {
                unsupported += usize::from(snap.status == PortsStatus::Unsupported);
                coverage.partial |= snap.status == PortsStatus::Partial;
                truncated |= snap.truncated;
                capped |= snap.capped;
                any_ports |= !snap.ports.is_empty();
            } else {
                missing = true;
            }
        }
        coverage.limits = match (truncated, capped) {
            (true, true) => {
                Some("The server shortened its report, and the client reached its display limit.")
            }
            (true, false) => Some(TRUNCATED),
            (false, true) => Some(CAPPED),
            _ => None,
        };
        coverage.empty = if any_ports {
            "No matching ports."
        } else if reading {
            "Reading ports…"
        } else if count > 0 && unsupported == count {
            "Rex can't list ports for this terminal."
        } else if missing {
            "Some ports could not be read. Reopen to try again."
        } else if coverage.partial || coverage.limits.is_some() || unsupported > 0 {
            "No listening ports reported."
        } else {
            "No listening ports in this session."
        };
        coverage
    }

    pub(super) fn seed_ports_selection(&mut self) {
        if self.ports_target().is_some() && self.panel.as_ref().is_some_and(|p| p.row.is_none()) {
            self.select_row(0);
        }
    }

    pub fn submit_port(&mut self, alternate: bool, tx: &mut Tx) {
        if tx.key_repeat() {
            return;
        }
        let Some((_, id)) = self.panel.as_ref().and_then(|p| p.row.as_ref()) else {
            return;
        };
        if let Some(Intent::Port(key)) = self
            .entries()
            .into_iter()
            .find(|entry| entry.id.as_deref() == Some(id))
            .map(|entry| entry.action)
        {
            self.use_port(&key, alternate, tx);
        }
    }

    fn current_ports(&self, key: &PortKey) -> Option<&PortSnapshot> {
        let target = self.ports_target()?;
        let model = self.model();
        if !self.ports_in_context(target)
            || !supported(model)
            || !blocks(model).any(|(_, _, id)| id == key.block)
        {
            return None;
        }
        model.ports.get(&key.block)
    }

    pub(super) fn use_port(&mut self, key: &PortKey, alternate: bool, tx: &mut Tx) {
        let Some(port) = self.current_ports(key).and_then(|snapshot| {
            snapshot
                .ports
                .iter()
                .find(|p| p.port == key.port && p.address.canonical() == key.address)
        }) else {
            return;
        };
        let host = &self.settings.hosts[self.active];
        let locality = HostLocality::from_endpoint(&host.endpoint);
        if !alternate {
            let address = ports::copy_address(&port.address, port.port);
            let notice = if locality == HostLocality::Local {
                format!("Copied {address}")
            } else {
                format!("Copied {address} (on {})", host.label)
            };
            tx.copy(address);
            self.inform(notice);
        } else if let Some(url) = ports::open_url(locality, &port.address, port.port) {
            self.open_link(url);
        } else if let Some(command) =
            ports::ssh_forward_command(&host.endpoint, &port.address, port.port)
        {
            let notice = format!("Copied ssh -L command for {}", host.label);
            tx.copy(command);
            self.inform(notice);
        }
    }
}

pub fn port_row(entry: &Entry, port: &Presentation, index: usize, selected: bool) -> El {
    let Intent::Port(key) = &entry.action else {
        unreachable!()
    };
    let id = key.id();
    let copy = key.clone();
    let controls = key.clone();
    let mut result = row()
        .key(format!("entry:{index}"))
        .role("menuitem")
        .label(format!("{}, {}", entry.label, entry.detail))
        .w_full()
        .h(44.)
        .px(9.)
        .center_y()
        .cursor(Cursor::Pointer)
        .on_hover(move |s: &mut Workspace| {
            if let Some(i) = s
                .entries()
                .iter()
                .position(|e| e.id.as_deref() == Some(id.as_str()))
            {
                s.select_row(i);
            }
        })
        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| s.use_port(&copy, false, tx));
    if selected {
        result = result.child(highlight("sel:ports", 7., false));
    }
    let primary = row()
        .w_full()
        .gap(10.)
        .center_y()
        .child(text(entry.label.clone()).font(mono(13.)).color(rgb(ink())))
        .children(port.process.as_ref().map(|p| {
            text(p.clone())
                .font(mono(12.))
                .color(rgb(if selected { ink() } else { muted() }))
                .shrink()
                .min_w(0.)
                .truncate()
                .help_when_truncated(p.clone())
        }));
    let content = col().grow(1.).shrink().min_w(0.).gap(2.).child(primary);
    let secondary = if selected {
        row()
            .gap(5.)
            .child(port_button("Copy", controls.clone(), false))
            .children(
                port.reach
                    .map(|action| port_button(action.label(), controls, true)),
            )
    } else {
        row().w_full().child(
            text(entry.detail.clone())
                .font(ui(11.))
                .color(rgb(muted()))
                .truncate()
                .help_when_truncated(entry.detail.clone()),
        )
    };
    result.child(content.child(secondary))
}

fn port_button(label: &'static str, key: PortKey, alternate: bool) -> El {
    el().key(label)
        .role("button")
        .label(label)
        .h(18.)
        .px(6.)
        .center()
        .radius(4.)
        .fill(white(0.065))
        .hover(white(0.12))
        .press(white(0.16))
        .preserve_focus_on_press(true)
        .tab_index(-1)
        .cursor(Cursor::Pointer)
        .on_press_tx(move |s: &mut Workspace, tx: &mut Tx| s.use_port(&key, alternate, tx))
        .child(text(label).font(medium(10.5)).color(rgb(ink())))
}

pub fn ports_footer(coverage: &Coverage) -> El {
    let hint = |modifier: Option<&str>, label: &str| {
        row()
            .gap(3.)
            .center_y()
            .children(modifier.map(|key| keycap(key, false)))
            .child(keycap("Enter", false))
            .child(
                text(label.to_owned())
                    .ml(2.)
                    .font(ui(11.))
                    .color(rgb(muted())),
            )
    };
    let mut footer = col().w_full().gap(1.);
    if coverage.api15 {
        footer = footer.child(coverage_line(API_15));
    }
    if let Some(locality) = coverage.locality {
        footer = footer.child(
            row()
                .h(28.)
                .px(10.)
                .gap(16.)
                .center_y()
                .child(hint(None, "Copy"))
                .children(
                    match locality {
                        HostLocality::Local => Some("Open"),
                        HostLocality::Ssh => Some("Copy ssh -L"),
                        HostLocality::Remote => None,
                    }
                    .map(|label| hint(Some(super::platform::keys::primary_name()), label)),
                ),
        );
    }
    footer
}

pub fn coverage_line(label: &str) -> El {
    row().w_full().h(23.).px(10.).center_y().child(
        text(label.to_owned())
            .font(ui(11.))
            .color(rgb(muted()))
            .truncate()
            .help_when_truncated(label.to_owned()),
    )
}

pub fn partial_line() -> El {
    row()
        .w_full()
        .h(23.)
        .px(10.)
        .gap(6.)
        .center_y()
        .child(text("Partial").font(medium(11.)).color(rgb(warning())))
        .child(
            text(PARTIAL)
                .font(ui(11.))
                .color(rgb(muted()))
                .truncate()
                .help_when_truncated(PARTIAL),
        )
}
