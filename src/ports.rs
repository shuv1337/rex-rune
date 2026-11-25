//! Pure model, wire parser, caps, and locality/reach policy for terminal listening ports
//! (Rex Server API 15 `com.superlogical.terminal.ports` and API 16 `ports_changed`).
use crate::connection::{activity::time, perform::sanitize_text};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    net::IpAddr,
};

/// Minimum Rex Server API revision for `com.superlogical.terminal.ports`.
pub const PORTS_MIN_API_REVISION: u32 = 15;
/// Minimum Rex Server API revision for `ports_changed` block events.
pub const PORTS_EVENTS_MIN_API_REVISION: u32 = 16;

/// Maximum listening ports kept for a single terminal block after filtering, dedup, and sorting.
pub const MAX_PORTS_PER_BLOCK: usize = 256;
/// Maximum listening ports kept across all blocks on a single host connection.
pub const MAX_PORTS_PER_HOST: usize = 4096;

const MAX_LABEL_CHARS: usize = 48;
const MAX_NAME_CHARS: usize = 64;
const MAX_USER_CHARS: usize = 64;
const MAX_ARGV0_CHARS: usize = 128;
const MAX_PATH_CHARS: usize = 1024;
const MAX_ZONE_CHARS: usize = 16;
const MAX_TIMESTAMP_CHARS: usize = 40;
const MAX_RAW_ADDRESS_CHARS: usize = 64;

/// Completeness reported by the server for a terminal's listening ports snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PortsStatus {
    /// Every process in the terminal was inspected.
    Ok,
    /// Some processes could not be inspected, or the server sent an unknown status string.
    #[default]
    Partial,
    /// The server cannot list ports for this terminal (for example, exited child or old server).
    Unsupported,
}

impl PortsStatus {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("ok") => Self::Ok,
            Some("unsupported") => Self::Unsupported,
            _ => Self::Partial,
        }
    }
}

/// Validated bind address for a listening TCP socket.
///
/// URLs and `ssh -L` targets are built strictly from parsed [`PortAddress::Ip`] values, never
/// from raw wire text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PortAddress {
    /// Parsed IP literal (never bracketed; IPv4-mapped IPv6 normalized to IPv4 when unzoned)
    /// with an optional validated IPv6 zone identifier (`[A-Za-z0-9._-]`, 1..=16 ASCII chars).
    Ip { ip: IpAddr, zone: Option<String> },
    /// Unparsable IP literal or invalid zone, sanitized for safe display only.
    Unparsable(String),
}

impl PortAddress {
    /// Parse a wire address literal without brackets, validating any `%zone` suffix.
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        let (ip_part, zone) = match trimmed.split_once('%') {
            Some((addr, z)) => {
                if !valid_zone(z) {
                    return Self::Unparsable(sanitize_text(raw, MAX_RAW_ADDRESS_CHARS));
                }
                (addr, Some(z.to_owned()))
            }
            None => (trimmed, None),
        };
        let Ok(ip) = ip_part.parse::<IpAddr>() else {
            return Self::Unparsable(sanitize_text(raw, MAX_RAW_ADDRESS_CHARS));
        };
        match (ip, zone) {
            // IPv4 addresses do not have RFC 4007 zone suffixes.
            (IpAddr::V4(_), Some(_)) => Self::Unparsable(sanitize_text(raw, MAX_RAW_ADDRESS_CHARS)),
            (IpAddr::V6(v6), None) => {
                if let Some(v4) = v6.to_ipv4_mapped() {
                    Self::Ip {
                        ip: IpAddr::V4(v4),
                        zone: None,
                    }
                } else {
                    Self::Ip {
                        ip: IpAddr::V6(v6),
                        zone: None,
                    }
                }
            }
            (ip, zone) => Self::Ip { ip, zone },
        }
    }

    /// Canonical string form used for deterministic sorting and display (no brackets).
    pub fn canonical(&self) -> String {
        match self {
            Self::Ip { ip, zone: None } => ip.to_string(),
            Self::Ip {
                ip,
                zone: Some(zone),
            } => format!("{ip}%{zone}"),
            Self::Unparsable(raw) => raw.clone(),
        }
    }

    /// Classify this address's bind scope for display and reach policy.
    pub fn scope(&self) -> AddressScope {
        match self {
            Self::Unparsable(_) => AddressScope::Unparsable,
            Self::Ip { zone: Some(_), .. } => AddressScope::LinkLocalZoned,
            Self::Ip { ip, zone: None } => match ip {
                IpAddr::V4(v4) if v4.is_loopback() => AddressScope::Loopback,
                IpAddr::V6(v6) if v6.is_loopback() => AddressScope::Loopback,
                IpAddr::V4(v4) if v4.is_unspecified() => AddressScope::Wildcard,
                IpAddr::V6(v6) if v6.is_unspecified() => AddressScope::Wildcard,
                IpAddr::V4(_) | IpAddr::V6(_) => AddressScope::Specific,
            },
        }
    }
}

impl fmt::Display for PortAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

fn valid_zone(zone: &str) -> bool {
    !zone.is_empty()
        && zone.len() <= MAX_ZONE_CHARS
        && zone
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Bind scope of a listening address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressScope {
    /// `127.0.0.0/8` or `::1`.
    Loopback,
    /// `0.0.0.0` or `::`.
    Wildcard,
    /// Specific non-loopback, non-wildcard IPv4 or IPv6 address without a zone.
    Specific,
    /// Zoned IPv6 address (`%zone`); copy address only everywhere.
    LinkLocalZoned,
    /// Unparsable address or invalid zone; copy port only everywhere.
    Unparsable,
}

/// Locality of a saved Rex host endpoint.
///
/// A host is [`HostLocality::Local`] only when its saved endpoint starts with `unix://`.
/// TCP/HTTP/WebSocket endpoints (even `localhost` or `127.0.0.1`) are treated as remote so
/// forwarded or reverse-proxied servers are never mistaken for the local machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostLocality {
    Local,
    Ssh,
    Remote,
}

impl HostLocality {
    pub fn from_endpoint(endpoint: &str) -> Self {
        if endpoint.starts_with("unix://") {
            Self::Local
        } else if endpoint.starts_with("ssh://") {
            Self::Ssh
        } else {
            Self::Remote
        }
    }
}

/// One validated listening TCP port entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortEntry {
    pub port: u16,
    pub address: PortAddress,
    /// Validated RFC 3339 timestamp when first observed by Rex (tooltip-only metadata).
    pub first_seen_at: Option<String>,
    /// Positive PID holding the socket, when Rex could identify it.
    pub pid: Option<u32>,
}

/// Sanitized process metadata referenced by one or more [`PortEntry`] rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortProcess {
    pub pid: u32,
    pub user_id: Option<u32>,
    pub user: Option<String>,
    pub name: String,
    pub path: Option<String>,
    pub invoked_path: Option<String>,
    pub argv0: Option<String>,
    pub script: Option<String>,
    pub cwd: Option<String>,
    /// Derived display label: last path component of the first non-empty of `script`, `argv0`,
    /// `name`, sanitized and capped at 48 characters.
    pub label: String,
}

/// Validated, deduplicated, sorted, and bounded listening-ports snapshot for one terminal block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortSnapshot {
    pub status: PortsStatus,
    pub ports: Vec<PortEntry>,
    pub processes: BTreeMap<u32, PortProcess>,
    /// Whether the server truncated entries from the end of `ports` to fit one control message.
    pub truncated: bool,
    /// Whether this client trimmed entries from the end to stay within per-block or per-host caps.
    pub capped: bool,
}

impl PortSnapshot {
    /// Snapshot representing an unsupported terminal or server.
    pub fn unsupported() -> Self {
        Self {
            status: PortsStatus::Unsupported,
            ports: Vec::new(),
            processes: BTreeMap::new(),
            truncated: false,
            capped: false,
        }
    }

    /// True only when the server scanned every process and neither server nor client dropped rows.
    pub fn is_complete(&self) -> bool {
        self.status == PortsStatus::Ok && !self.truncated && !self.capped
    }

    /// Look up the process metadata for a port's PID, if present.
    pub fn process(&self, pid: Option<u32>) -> Option<&PortProcess> {
        self.processes.get(&pid?)
    }

    /// Parse a raw `com.superlogical.terminal.ports` response or `ports_changed` payload.
    ///
    /// Never fails on unknown statuses, unknown protocols, malformed ports, or hostile text.
    pub fn parse(payload: &Value) -> Self {
        let status = PortsStatus::parse(payload["status"].as_str());
        let truncated = payload["truncated"].as_bool().unwrap_or(false);
        if status == PortsStatus::Unsupported {
            return Self {
                status,
                ports: Vec::new(),
                processes: BTreeMap::new(),
                truncated: false,
                capped: false,
            };
        }

        let mut dedup: BTreeMap<(u16, String), PortEntry> = BTreeMap::new();
        if let Some(items) = payload["ports"].as_array() {
            for item in items {
                let Some(entry) = parse_port_entry(item) else {
                    continue;
                };
                let key = (entry.port, entry.address.canonical());
                match dedup.get_mut(&key) {
                    Some(existing) => {
                        if existing.pid.is_none() && entry.pid.is_some() {
                            existing.pid = entry.pid;
                            if entry.first_seen_at.is_some() {
                                existing.first_seen_at = entry.first_seen_at;
                            }
                        } else if existing.first_seen_at.is_none() && entry.first_seen_at.is_some()
                        {
                            existing.first_seen_at = entry.first_seen_at;
                        }
                    }
                    None => {
                        dedup.insert(key, entry);
                    }
                }
            }
        }

        let mut ports: Vec<PortEntry> = dedup.into_values().collect();
        let mut capped = false;
        if ports.len() > MAX_PORTS_PER_BLOCK {
            ports.truncate(MAX_PORTS_PER_BLOCK);
            capped = true;
        }

        let referenced_pids: HashSet<u32> = ports.iter().filter_map(|p| p.pid).collect();
        let mut processes = BTreeMap::new();
        if !referenced_pids.is_empty()
            && let Some(items) = payload["processes"].as_array()
        {
            for item in items {
                let Some(proc_info) = parse_process(item) else {
                    continue;
                };
                if referenced_pids.contains(&proc_info.pid) {
                    processes.entry(proc_info.pid).or_insert(proc_info);
                }
            }
        }

        Self {
            status,
            ports,
            processes,
            truncated,
            capped,
        }
    }

    /// Trim entries from the end if needed to fit within a remaining host-wide port budget.
    pub fn apply_host_budget(&mut self, max_ports: usize) {
        if self.ports.len() <= max_ports {
            return;
        }
        self.ports.truncate(max_ports);
        self.capped = true;
        let referenced: HashSet<u32> = self.ports.iter().filter_map(|p| p.pid).collect();
        self.processes.retain(|pid, _| referenced.contains(pid));
    }
}

fn parse_port_entry(item: &Value) -> Option<PortEntry> {
    if item["protocol"].as_str() != Some("tcp") {
        return None;
    }
    let raw_port = item["port"].as_u64()?;
    if raw_port == 0 || raw_port > u64::from(u16::MAX) {
        return None;
    }
    let port = raw_port as u16;
    let raw_address = item["address"].as_str()?;
    let address = PortAddress::parse(raw_address);
    let first_seen_at = item["first_seen_at"].as_str().and_then(parse_timestamp);
    let pid = parse_positive_pid(&item["pid"]);
    Some(PortEntry {
        port,
        address,
        first_seen_at,
        pid,
    })
}

fn parse_positive_pid(value: &Value) -> Option<u32> {
    let pid = value.as_i64()?;
    if pid <= 0 {
        return None;
    }
    u32::try_from(pid).ok()
}

fn parse_non_negative_u32(value: &Value) -> Option<u32> {
    let n = value.as_i64()?;
    if n < 0 {
        return None;
    }
    u32::try_from(n).ok()
}

fn parse_timestamp(raw: &str) -> Option<String> {
    if raw.len() > MAX_TIMESTAMP_CHARS {
        return None;
    }
    time::timestamp(raw)?;
    Some(sanitize_text(raw, MAX_TIMESTAMP_CHARS))
}

fn sanitize_opt(value: &Value, max_chars: usize) -> Option<String> {
    let text = sanitize_text(value.as_str()?, max_chars);
    (!text.is_empty()).then_some(text)
}

fn parse_process(item: &Value) -> Option<PortProcess> {
    let pid = parse_positive_pid(&item["pid"])?;
    let user_id = parse_non_negative_u32(&item["user_id"]);
    let user = sanitize_opt(&item["user"], MAX_USER_CHARS);
    let name = item["name"]
        .as_str()
        .map(|s| sanitize_text(s, MAX_NAME_CHARS))
        .unwrap_or_default();
    let path = sanitize_opt(&item["path"], MAX_PATH_CHARS);
    let invoked_path = sanitize_opt(&item["invoked_path"], MAX_PATH_CHARS);
    let argv0 = sanitize_opt(&item["argv0"], MAX_ARGV0_CHARS);
    let script = sanitize_opt(&item["script"], MAX_PATH_CHARS);
    let cwd = sanitize_opt(&item["cwd"], MAX_PATH_CHARS);
    let label = derive_process_label(script.as_deref(), argv0.as_deref(), Some(name.as_str()));
    Some(PortProcess {
        pid,
        user_id,
        user,
        name,
        path,
        invoked_path,
        argv0,
        script,
        cwd,
        label,
    })
}

/// Derive a process display label as the last path component (splitting on `/` and `\`, trimming
/// trailing separators) of the first non-empty of `script`, `argv0`, `name`, sanitized and capped
/// at 48 characters.
pub fn derive_process_label(
    script: Option<&str>,
    argv0: Option<&str>,
    name: Option<&str>,
) -> String {
    for candidate in [script, argv0, name].into_iter().flatten() {
        let trimmed = candidate.trim().trim_end_matches(['/', '\\']);
        let component = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
        let cleaned = sanitize_text(component, MAX_LABEL_CHARS);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }
    String::new()
}

/// Browser URL for opening a listening port, permitted only on [`HostLocality::Local`] hosts.
///
/// Returns `None` for any `ssh://` or other remote host, for zoned IPv6 addresses, for unparsable
/// addresses, or for port 0. Wildcard binds map to loopback (`127.0.0.1` or `[::1]`).
pub fn open_url(locality: HostLocality, address: &PortAddress, port: u16) -> Option<String> {
    if locality != HostLocality::Local || port == 0 {
        return None;
    }
    match address {
        PortAddress::Ip { ip, zone: None } => match ip {
            IpAddr::V4(v4) if v4.is_unspecified() => Some(format!("http://127.0.0.1:{port}")),
            IpAddr::V4(v4) => Some(format!("http://{v4}:{port}")),
            IpAddr::V6(v6) if v6.is_unspecified() => Some(format!("http://[::1]:{port}")),
            IpAddr::V6(v6) => Some(format!("http://[{v6}]:{port}")),
        },
        PortAddress::Ip { zone: Some(_), .. } | PortAddress::Unparsable(_) => None,
    }
}

/// Safe clipboard text for copying a listening port's address.
///
/// Parsed IPv4 uses `<addr>:<port>`, parsed IPv6 uses `[<addr>]:<port>` (including validated
/// `%zone`), and unparsable addresses copy only the numeric port (`<port>`), never raw wire text.
pub fn copy_address(address: &PortAddress, port: u16) -> String {
    match address {
        PortAddress::Ip {
            ip: IpAddr::V4(v4),
            zone: None,
        } => format!("{v4}:{port}"),
        PortAddress::Ip {
            ip: IpAddr::V6(v6),
            zone: None,
        } => format!("[{v6}]:{port}"),
        PortAddress::Ip {
            ip: IpAddr::V6(v6),
            zone: Some(zone),
        } => format!("[{v6}%{zone}]:{port}"),
        PortAddress::Ip {
            ip: IpAddr::V4(_),
            zone: Some(_),
        }
        | PortAddress::Unparsable(_) => port.to_string(),
    }
}

/// Safe `ssh -N -L ...` port-forwarding command for an `ssh://` host endpoint.
///
/// Returns `None` for non-`ssh://` endpoints, malformed SSH endpoints, zoned addresses,
/// unparsable addresses, or port 0. Privileged remote ports (`< 1024`) map to local port
/// `10000 + port`. The SSH destination is always safely POSIX shell-quoted.
pub fn ssh_forward_command(endpoint: &str, address: &PortAddress, port: u16) -> Option<String> {
    if HostLocality::from_endpoint(endpoint) != HostLocality::Ssh || port == 0 {
        return None;
    }
    let target = crate::ssh::Target::parse(endpoint).ok()??;
    let bind_target = match address {
        PortAddress::Ip { ip, zone: None } => match ip {
            IpAddr::V4(v4) if v4.is_unspecified() => "127.0.0.1".to_owned(),
            IpAddr::V4(v4) => v4.to_string(),
            IpAddr::V6(v6) if v6.is_unspecified() => "[::1]".to_owned(),
            IpAddr::V6(v6) => format!("[{v6}]"),
        },
        PortAddress::Ip { zone: Some(_), .. } | PortAddress::Unparsable(_) => return None,
    };
    let destination = target.destination();
    if destination.is_empty() || destination.chars().any(has_control_or_bidi) {
        return None;
    }
    let local_port: u32 = if port < 1024 {
        10_000 + u32::from(port)
    } else {
        u32::from(port)
    };
    let forward_spec = format!("{local_port}:{bind_target}:{port}");
    // Quote IPv6 bracketed specs so shells such as zsh do not attempt globbing.
    let forward_arg = if forward_spec.contains(['[', ']']) {
        format!("'{forward_spec}'")
    } else {
        forward_spec
    };
    let quoted_dest = posix_quote_destination(&destination);
    match target.port {
        Some(ssh_port) => Some(format!(
            "ssh -N -L {forward_arg} -p {ssh_port} -- {quoted_dest}"
        )),
        None => Some(format!("ssh -N -L {forward_arg} -- {quoted_dest}")),
    }
}

fn has_control_or_bidi(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}'
                | '\u{200b}'
                | '\u{200e}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{e0000}'..='\u{e007f}'
        )
}

fn posix_quote_destination(dest: &str) -> String {
    if !dest.is_empty()
        && !dest.starts_with('-')
        && dest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.' | b'_' | b'-' | b':'))
    {
        dest.to_owned()
    } else {
        format!("'{}'", dest.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests;
