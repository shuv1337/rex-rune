//! Pure shared-hosts core: parsing Rex `hosts.list` records while preserving unknown fields,
//! expanding `-S`-style endpoints, canonical endpoint comparison, and deterministic five-rule
//! reconciliation against Rex Rune's saved host list.
use crate::settings::{Host, SharedLink, validate_host};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// One host record returned by Rex's `hosts.list`, keeping its raw JSON object intact so
/// unknown fields survive untouched for future write paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedHost {
    pub id: String,
    pub label: String,
    pub endpoints: Vec<String>,
    pub host_id: Option<String>,
    pub source: Option<String>,
    pub raw: Map<String, Value>,
}

impl SharedHost {
    /// Parse one object from `hosts.list`'s `hosts` array. Returns `None` if it is not a
    /// usable host object (missing `id` or `label`).
    pub fn from_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let id = trimmed_str(obj.get("id"))?;
        let label = trimmed_str(obj.get("label"))?;
        let endpoints = obj
            .get("endpoints")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| trimmed_str(Some(v)))
            .collect();
        let host_id = trimmed_str(obj.get("host_id"));
        let source = trimmed_str(obj.get("source"));
        Some(Self {
            id,
            label,
            endpoints,
            host_id,
            source,
            raw: obj.clone(),
        })
    }

    /// Expanded candidate endpoints in preference order, deduplicated.
    pub fn expanded_endpoints(&self) -> Vec<String> {
        expand_endpoints(&self.endpoints)
    }

    /// First expanded endpoint accepted by [`validate_host`] for this record's label.
    pub fn first_valid_endpoint(&self) -> Option<String> {
        self.expanded_endpoints()
            .into_iter()
            .find(|ep| validate_host(&self.label, ep).is_ok())
    }
}

/// One diagnostic from `hosts.list`'s `diagnostics` array, preserved ready to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedHostDiagnostic {
    pub severity: String,
    pub message: String,
    pub source: Option<String>,
}

impl SharedHostDiagnostic {
    pub fn from_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let message = trimmed_str(obj.get("message"))?;
        let severity = trimmed_str(obj.get("severity")).unwrap_or_else(|| "warning".into());
        let source = trimmed_str(obj.get("source"));
        Some(Self {
            severity,
            message,
            source,
        })
    }
}

/// A successfully read `hosts.list` response payload.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SharedHostList {
    pub hash: String,
    pub hosts: Vec<SharedHost>,
    pub diagnostics: Vec<SharedHostDiagnostic>,
}

impl SharedHostList {
    /// Parse the `payload` object of a successful `hosts.list` response.
    pub fn from_payload(payload: &Value) -> Option<Self> {
        let obj = payload.as_object()?;
        let hash = obj.get("hash").and_then(Value::as_str)?.to_owned();
        let hosts_val = obj.get("hosts")?;
        let hosts = match hosts_val {
            Value::Null => Vec::new(),
            Value::Array(items) => items.iter().filter_map(SharedHost::from_value).collect(),
            _ => return None,
        };
        let diagnostics = match obj.get("diagnostics") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(SharedHostDiagnostic::from_value)
                .collect(),
            Some(_) => Vec::new(),
        };
        Some(Self {
            hash,
            hosts,
            diagnostics,
        })
    }
}

fn trimmed_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Expand one Rex `-S` / `hosts.list` endpoint string into concrete candidate endpoints:
/// - Explicit schemes (`https://`, `http://`, `ssh://`, `unix://`, or any `scheme://`) are
///   kept as written.
/// - Bare `host:port` (including bracketed IPv6 `[::1]:7297`) expands to `https://host:port`
///   then `http://host:port`.
/// - Bare `host` expands to `https://host`, `https://host:7297`, `http://host:7297`.
pub fn expand_endpoint(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.contains("://") {
        return vec![trimmed.to_owned()];
    }
    if has_bare_port(trimmed) {
        vec![format!("https://{trimmed}"), format!("http://{trimmed}")]
    } else {
        vec![
            format!("https://{trimmed}"),
            format!("https://{trimmed}:7297"),
            format!("http://{trimmed}:7297"),
        ]
    }
}

fn has_bare_port(value: &str) -> bool {
    if let Some(rest) = value.strip_prefix('[') {
        let Some((_, after_bracket)) = rest.split_once(']') else {
            return false;
        };
        let Some(port) = after_bracket.strip_prefix(':') else {
            return false;
        };
        return !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
    }
    if let Some((host, port)) = value.rsplit_once(':') {
        return !host.is_empty()
            && !host.contains(':')
            && !host.contains('/')
            && !port.is_empty()
            && port.bytes().all(|b| b.is_ascii_digit());
    }
    false
}

/// Expand all endpoints in order, preserving first-seen preference and removing duplicates.
pub fn expand_endpoints(endpoints: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in endpoints {
        for expanded in expand_endpoint(raw) {
            if seen.insert(expanded.clone()) {
                out.push(expanded);
            }
        }
    }
    out
}

/// Canonical form of an endpoint for matching:
/// - lowercase scheme and host,
/// - strip default port (`:443` for `https`/`wss`, `:80` for `http`/`ws`),
/// - strip trailing `/` (except for bare `unix:///`).
pub fn canonical_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim();
    if let Some((scheme, rest)) = trimmed.split_once("://") {
        let scheme_lower = scheme.to_ascii_lowercase();
        if scheme_lower == "unix" {
            let path = if rest == "/" {
                "/"
            } else {
                rest.trim_end_matches('/')
            };
            return format!("unix://{path}");
        }
        if scheme_lower == "ssh" {
            // Lowercase scheme and host; keep user and socket path case-sensitive.
            let (authority, path) = match rest.find('/') {
                Some(idx) => (&rest[..idx], rest[idx..].trim_end_matches('/')),
                None => (rest, ""),
            };
            let (userinfo, hostport) = match authority.rsplit_once('@') {
                Some((u, h)) => (Some(u), h),
                None => (None, authority),
            };
            let hostport_lower = hostport.to_ascii_lowercase();
            return match userinfo {
                Some(u) => format!("ssh://{u}@{hostport_lower}{path}"),
                None => format!("ssh://{hostport_lower}{path}"),
            };
        }
        if matches!(scheme_lower.as_str(), "https" | "http" | "wss" | "ws")
            && let Ok(mut parsed) = url::Url::parse(trimmed)
        {
            // url::Url lowercases scheme and ASCII domain host and strips default ports
            // (443 for https/wss, 80 for http/ws) on serialization.
            let _ = parsed.set_scheme(&scheme_lower);
            return parsed.as_str().trim_end_matches('/').to_owned();
        }
        return format!("{scheme_lower}://{}", rest.trim_end_matches('/'));
    }
    trimmed.trim_end_matches('/').to_ascii_lowercase()
}

/// Whether two endpoint strings refer to the same canonical endpoint.
pub fn same_endpoint(a: &str, b: &str) -> bool {
    canonical_endpoint(a) == canonical_endpoint(b)
}

/// Update to an existing linked host row (rule 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostUpdate {
    pub index: usize,
    pub label: String,
    pub endpoint: String,
    pub endpoint_changed: bool,
}

/// Linking an existing unlinked local host row to a shared record (rule 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostLink {
    pub index: usize,
    pub label: String,
    pub shared: SharedLink,
}

/// Appending a new host row for an unmatched connectable shared record (rule 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostAddition {
    pub label: String,
    pub endpoint: String,
    pub shared: SharedLink,
}

/// Unlinking a previously linked host whose record left a successfully read list (rule 5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostUnlink {
    pub index: usize,
    pub label: String,
    pub id: String,
}

/// A shared host record with no endpoint accepted by [`validate_host`] (rule 4 fallback).
/// Shown in Settings → Remote Hosts only; never spawns a host worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnconnectableHost {
    pub id: String,
    pub label: String,
    pub endpoints: Vec<String>,
    pub source: Option<String>,
    pub reason: String,
}

/// Pure reconciliation plan produced by [`reconcile`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcilePlan {
    pub updates: Vec<HostUpdate>,
    pub links: Vec<HostLink>,
    pub additions: Vec<HostAddition>,
    pub unlinks: Vec<HostUnlink>,
    pub unconnectable: Vec<UnconnectableHost>,
}

impl ReconcilePlan {
    /// Whether applying this plan changes any saved host row.
    pub fn has_host_changes(&self) -> bool {
        !self.updates.is_empty()
            || !self.links.is_empty()
            || !self.additions.is_empty()
            || !self.unlinks.is_empty()
    }
}

/// Reconcile Rex Rune's saved `local_hosts` against a successfully read `list` from
/// `source_endpoint`, applying the five rules in exact order:
/// 1. Suppress any shared record whose `host_id` equals `local_host_id`.
/// 2. Match existing rows linked by `shared.id` (from `source_endpoint`): update label to
///    Rex's label; if the saved endpoint is no longer in the record's expanded endpoints,
///    switch to the first valid endpoint and mark `endpoint_changed = true`.
/// 3. Match unlinked local rows (other than `source_endpoint` itself) whose canonical endpoint
///    appears in an unmatched record's expanded endpoints: link and adopt Rex's label while
///    preserving all local preferences.
/// 4. For remaining unmatched records: append a linked host if a valid endpoint exists, else
///    report in `unconnectable`.
/// 5. Any row linked to `source_endpoint` whose `shared.id` is absent from the active list is
///    unlinked and kept as a local host (never silently deleted).
pub fn reconcile(
    local_hosts: &[Host],
    source_endpoint: &str,
    list: &SharedHostList,
    local_host_id: Option<&str>,
) -> ReconcilePlan {
    let source_canon = canonical_endpoint(source_endpoint);
    let local_id = local_host_id.map(str::trim).filter(|s| !s.is_empty());

    // Rule 1: suppress any record referring to this machine itself.
    let active: Vec<&SharedHost> = list
        .hosts
        .iter()
        .filter(|record| {
            !local_id.is_some_and(|local| {
                record
                    .host_id
                    .as_deref()
                    .is_some_and(|rid| rid.eq_ignore_ascii_case(local))
            })
        })
        .collect();

    let expanded_canon: Vec<Vec<String>> = active
        .iter()
        .map(|r| {
            r.expanded_endpoints()
                .iter()
                .map(|ep| canonical_endpoint(ep))
                .collect()
        })
        .collect();

    let mut plan = ReconcilePlan::default();
    let mut matched_records = vec![false; active.len()];
    let mut matched_rows = vec![false; local_hosts.len()];

    // Rule 2: existing rows already linked to `source_endpoint` by `shared.id`.
    for (row_idx, host) in local_hosts.iter().enumerate() {
        let Some(link) = &host.shared else {
            continue;
        };
        if canonical_endpoint(&link.from) != source_canon {
            continue;
        }
        if let Some(rec_idx) = active.iter().position(|r| r.id == link.id) {
            matched_records[rec_idx] = true;
            matched_rows[row_idx] = true;
            let record = active[rec_idx];
            let host_canon = canonical_endpoint(&host.endpoint);
            let still_present = expanded_canon[rec_idx].iter().any(|ep| ep == &host_canon);
            let (next_endpoint, endpoint_changed) = if still_present {
                (host.endpoint.clone(), false)
            } else if let Some(first_valid) = record.first_valid_endpoint() {
                (first_valid, true)
            } else {
                // Record has no usable endpoint left; keep existing saved endpoint untouched
                // rather than spawning a broken worker, and surface the unusable record.
                plan.unconnectable.push(UnconnectableHost {
                    id: record.id.clone(),
                    label: record.label.clone(),
                    endpoints: record.endpoints.clone(),
                    source: record.source.clone(),
                    reason: unconnectable_reason(record),
                });
                (host.endpoint.clone(), false)
            };
            if record.label != host.label || endpoint_changed {
                plan.updates.push(HostUpdate {
                    index: row_idx,
                    label: record.label.clone(),
                    endpoint: next_endpoint,
                    endpoint_changed,
                });
            }
        } else {
            // Rule 5: linked row for this source is no longer in the successfully read list.
            // Never silently delete: unlink and keep as a local host.
            matched_rows[row_idx] = true;
            plan.unlinks.push(HostUnlink {
                index: row_idx,
                label: host.label.clone(),
                id: link.id.clone(),
            });
        }
    }

    // Rule 3: unlinked rows whose canonical endpoint matches an unmatched record's expanded endpoints.
    for (rec_idx, record) in active.iter().enumerate() {
        if matched_records[rec_idx] {
            continue;
        }
        let matching_row = local_hosts.iter().enumerate().position(|(row_idx, host)| {
            if matched_rows[row_idx] || host.shared.is_some() {
                return false;
            }
            let canon = canonical_endpoint(&host.endpoint);
            // Never link the source server's own local row as a shared remote host.
            if canon == source_canon {
                return false;
            }
            expanded_canon[rec_idx].iter().any(|ep| ep == &canon)
        });
        if let Some(row_idx) = matching_row {
            matched_records[rec_idx] = true;
            matched_rows[row_idx] = true;
            plan.links.push(HostLink {
                index: row_idx,
                label: record.label.clone(),
                shared: SharedLink {
                    id: record.id.clone(),
                    from: source_endpoint.to_owned(),
                },
            });
        }
    }

    // Rule 4: unmatched shared records -> append if a valid endpoint exists, else unconnectable.
    for (rec_idx, record) in active.iter().enumerate() {
        if matched_records[rec_idx] {
            continue;
        }
        match record.first_valid_endpoint() {
            Some(endpoint) => {
                plan.additions.push(HostAddition {
                    label: record.label.clone(),
                    endpoint,
                    shared: SharedLink {
                        id: record.id.clone(),
                        from: source_endpoint.to_owned(),
                    },
                });
            }
            None => {
                plan.unconnectable.push(UnconnectableHost {
                    id: record.id.clone(),
                    label: record.label.clone(),
                    endpoints: record.endpoints.clone(),
                    source: record.source.clone(),
                    reason: unconnectable_reason(record),
                });
            }
        }
    }

    plan
}

fn unconnectable_reason(record: &SharedHost) -> String {
    if record.endpoints.is_empty() {
        return "no endpoints configured".into();
    }
    let first_err = record.expanded_endpoints().into_iter().find_map(|ep| {
        validate_host(&record.label, &ep)
            .err()
            .map(|e| e.to_string())
    });
    first_err.unwrap_or_else(|| "no supported endpoint".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SOURCE: &str = "unix:///run/user/1000/rex/server.sock";

    fn local_host(label: &str, endpoint: &str) -> Host {
        Host {
            label: label.into(),
            endpoint: endpoint.into(),
            remote_control: false,
            collapsed: false,
            shell: None,
            shared: None,
        }
    }

    fn linked_host(label: &str, endpoint: &str, id: &str) -> Host {
        Host {
            shared: Some(SharedLink {
                id: id.into(),
                from: SOURCE.into(),
            }),
            ..local_host(label, endpoint)
        }
    }

    fn shared_record(
        id: &str,
        label: &str,
        endpoints: &[&str],
        host_id: Option<&str>,
    ) -> SharedHost {
        let mut obj = json!({
            "id": id,
            "label": label,
            "endpoints": endpoints,
        });
        if let Some(hid) = host_id {
            obj["host_id"] = json!(hid);
        }
        SharedHost::from_value(&obj).unwrap()
    }

    fn list_of(hosts: Vec<SharedHost>) -> SharedHostList {
        SharedHostList {
            hash: "h1".into(),
            hosts,
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn expands_bare_names_ports_and_preserves_explicit_schemes() {
        assert_eq!(
            expand_endpoint("studio.tail.ts.net"),
            vec![
                "https://studio.tail.ts.net",
                "https://studio.tail.ts.net:7297",
                "http://studio.tail.ts.net:7297",
            ]
        );
        assert_eq!(
            expand_endpoint("box:8443"),
            vec!["https://box:8443", "http://box:8443"]
        );
        assert_eq!(
            expand_endpoint("[::1]:7297"),
            vec!["https://[::1]:7297", "http://[::1]:7297"]
        );
        assert_eq!(
            expand_endpoint("[::1]"),
            vec!["https://[::1]", "https://[::1]:7297", "http://[::1]:7297"]
        );
        for explicit in [
            "https://studio.tail.ts.net",
            "http://10.0.0.5:7297",
            "ssh://me@studio",
            "unix:///tmp/rex.sock",
            "ftp://unsupported",
        ] {
            assert_eq!(expand_endpoint(explicit), vec![explicit]);
        }
        assert!(expand_endpoint("   ").is_empty());
    }

    #[test]
    fn canonicalization_normalizes_case_default_ports_and_trailing_slashes() {
        assert_eq!(
            canonical_endpoint("HTTPS://Studio.TS.net:443/"),
            "https://studio.ts.net"
        );
        assert_eq!(
            canonical_endpoint("https://studio.ts.net"),
            "https://studio.ts.net"
        );
        assert_eq!(canonical_endpoint("HTTP://Box:80/"), "http://box");
        assert_eq!(canonical_endpoint("http://Box:7297/"), "http://box:7297");
        assert_eq!(
            canonical_endpoint("SSH://Alice@Studio.TS.net/Run/Rex.sock/"),
            "ssh://Alice@studio.ts.net/Run/Rex.sock"
        );
        assert_eq!(
            canonical_endpoint("UNIX:///Run/User/1000/rex.sock/"),
            "unix:///Run/User/1000/rex.sock"
        );
        assert!(same_endpoint(
            "https://STUDIO.ts.net:443/",
            "https://studio.ts.net"
        ));
    }

    #[test]
    fn parses_hosts_list_and_preserves_unknown_fields_in_raw() {
        let payload = json!({
            "hash": "abc123",
            "hosts": [
                {
                    "id": "host:0199abcd-1234-7abc-8def-0123456789ab",
                    "label": "Studio",
                    "endpoints": ["studio.ts.net", "ssh://me@studio"],
                    "host_id": "11111111-2222-3333-4444-555555555555",
                    "source": "init.lua:2",
                    "icon": "desktop",
                    "added_from": "laptop",
                    "future_field": {"nested": [1, 2, 3]}
                }
            ],
            "diagnostics": [
                {
                    "severity": "warning",
                    "source": "init.lua:7",
                    "message": "rex.host: member \"endpoints\" is empty"
                }
            ]
        });
        let list = SharedHostList::from_payload(&payload).unwrap();
        assert_eq!(list.hash, "abc123");
        assert_eq!(list.hosts.len(), 1);
        let h = &list.hosts[0];
        assert_eq!(h.id, "host:0199abcd-1234-7abc-8def-0123456789ab");
        assert_eq!(h.label, "Studio");
        assert_eq!(h.endpoints, vec!["studio.ts.net", "ssh://me@studio"]);
        assert_eq!(
            h.host_id.as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert_eq!(h.source.as_deref(), Some("init.lua:2"));
        assert_eq!(h.raw.get("icon"), Some(&json!("desktop")));
        assert_eq!(h.raw.get("added_from"), Some(&json!("laptop")));
        assert_eq!(
            h.raw.get("future_field"),
            Some(&json!({"nested": [1, 2, 3]}))
        );
        assert_eq!(list.diagnostics.len(), 1);
        assert_eq!(list.diagnostics[0].severity, "warning");
        assert_eq!(list.diagnostics[0].source.as_deref(), Some("init.lua:7"));
        assert_eq!(
            list.diagnostics[0].message,
            "rex.host: member \"endpoints\" is empty"
        );
    }

    #[test]
    fn rule_1_suppresses_self_by_host_id() {
        let local = vec![local_host("This Mac", SOURCE)];
        let list = list_of(vec![
            shared_record("host:self", "Me", &["me.ts.net"], Some("LOCAL-UUID-1")),
            shared_record(
                "host:other",
                "Studio",
                &["studio.ts.net"],
                Some("OTHER-UUID-2"),
            ),
        ]);
        let plan = reconcile(&local, SOURCE, &list, Some("local-uuid-1"));
        assert_eq!(plan.additions.len(), 1);
        assert_eq!(plan.additions[0].label, "Studio");
        assert_eq!(plan.additions[0].endpoint, "https://studio.ts.net");
        assert!(plan.unconnectable.is_empty());
    }

    #[test]
    fn rule_2_tracks_label_and_flags_endpoint_change_only_when_dropped() {
        let local = vec![
            local_host("Local", SOURCE),
            linked_host("OldName", "https://studio.ts.net:7297", "host:1"),
            linked_host("Moved", "https://old.ts.net", "host:2"),
        ];
        let list = list_of(vec![
            // studio.ts.net expands to include https://studio.ts.net:7297, so endpoint stays!
            shared_record("host:1", "Studio", &["studio.ts.net"], None),
            // old.ts.net is gone -> switches to first valid endpoint with endpoint_changed = true!
            shared_record(
                "host:2",
                "MovedRenamed",
                &["ftp://bad", "new.ts.net:8443"],
                None,
            ),
        ]);
        let plan = reconcile(&local, SOURCE, &list, None);
        assert_eq!(plan.updates.len(), 2);
        assert_eq!(
            plan.updates[0],
            HostUpdate {
                index: 1,
                label: "Studio".into(),
                endpoint: "https://studio.ts.net:7297".into(),
                endpoint_changed: false,
            }
        );
        assert_eq!(
            plan.updates[1],
            HostUpdate {
                index: 2,
                label: "MovedRenamed".into(),
                endpoint: "https://new.ts.net:8443".into(),
                endpoint_changed: true,
            }
        );
        assert!(plan.links.is_empty());
        assert!(plan.additions.is_empty());
        assert!(plan.unlinks.is_empty());
    }

    #[test]
    fn rule_3_links_existing_unlinked_host_by_canonical_expanded_endpoint() {
        let mut existing = local_host("My Custom Name", "HTTPS://Studio.TS.net:443/");
        existing.shell = Some("/bin/zsh".into());
        existing.collapsed = true;
        existing.remote_control = true;
        let local = vec![local_host("Local", SOURCE), existing];
        let list = list_of(vec![shared_record(
            "host:studio",
            "Studio",
            &["studio.ts.net"],
            None,
        )]);
        let plan = reconcile(&local, SOURCE, &list, None);
        assert_eq!(plan.links.len(), 1);
        assert_eq!(plan.links[0].index, 1);
        assert_eq!(plan.links[0].label, "Studio");
        assert_eq!(
            plan.links[0].shared,
            SharedLink {
                id: "host:studio".into(),
                from: SOURCE.into(),
            }
        );
        assert!(plan.additions.is_empty());
        assert!(plan.updates.is_empty());
    }

    #[test]
    fn rule_4_appends_connectable_and_reports_unconnectable_without_spawning() {
        let local = vec![local_host("Local", SOURCE)];
        let list = list_of(vec![
            shared_record("host:ok", "Lab", &["ftp://nope", "ssh://me@lab"], None),
            shared_record("host:bad1", "Empty", &[], None),
            shared_record("host:bad2", "BadScheme", &["ftp://only-ftp"], None),
        ]);
        let plan = reconcile(&local, SOURCE, &list, None);
        assert_eq!(plan.additions.len(), 1);
        assert_eq!(plan.additions[0].label, "Lab");
        assert_eq!(plan.additions[0].endpoint, "ssh://me@lab");
        assert_eq!(plan.unconnectable.len(), 2);
        assert_eq!(plan.unconnectable[0].label, "Empty");
        assert_eq!(plan.unconnectable[1].label, "BadScheme");
    }

    #[test]
    fn rule_5_unlinks_departed_host_without_deleting_and_is_idempotent() {
        let mut hosts = vec![
            local_host("Local", SOURCE),
            linked_host("Gone", "https://gone.ts.net", "host:gone"),
            linked_host("Kept", "https://kept.ts.net", "host:kept"),
        ];
        let list = list_of(vec![shared_record(
            "host:kept",
            "Kept",
            &["kept.ts.net"],
            None,
        )]);
        let plan = reconcile(&hosts, SOURCE, &list, None);
        assert_eq!(plan.unlinks.len(), 1);
        assert_eq!(plan.unlinks[0].index, 1);
        assert_eq!(plan.unlinks[0].label, "Gone");
        assert!(plan.updates.is_empty());
        assert!(plan.additions.is_empty());

        // Apply plan and verify second pass is a complete no-op (idempotent).
        hosts[plan.unlinks[0].index].shared = None;
        let again = reconcile(&hosts, SOURCE, &list, None);
        assert!(!again.has_host_changes());
        assert!(again.unconnectable.is_empty());
    }

    #[test]
    fn never_links_the_source_server_row_itself() {
        let local = vec![local_host("Local", SOURCE)];
        let list = list_of(vec![shared_record("host:loop", "Loop", &[SOURCE], None)]);
        let plan = reconcile(&local, SOURCE, &list, None);
        assert!(plan.links.is_empty(), "must not link the source host row");
        // Without host_id suppression, rule 4 would still treat it as unmatched; with host_id
        // rule 1 suppresses it completely.
        let suppressed = reconcile(
            &local,
            SOURCE,
            &list_of(vec![shared_record(
                "host:loop",
                "Loop",
                &[SOURCE],
                Some("self-id"),
            )]),
            Some("self-id"),
        );
        assert!(!suppressed.has_host_changes());
    }
}
