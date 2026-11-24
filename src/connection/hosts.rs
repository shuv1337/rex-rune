//! Read-only synchronization of Rex's shared host list (`hosts.list` + `hosts_changed`).
//!
//! Modelled on [`super::server_info`]: once enabled via `HostCommand::WatchSharedHosts`, each
//! control connection sends one detached `hosts.list` read when ready and re-reads when a
//! `hosts_changed` event reports a different (or missing) hash, or arrives while an earlier
//! read is still in flight. Raw JSON is parsed directly so unknown fields on host records are
//! preserved. Never sends `hosts.put` or `hosts.remove`.
use crate::{hosts::SharedHostList, transport::Client};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

const TIMEOUT: Duration = Duration::from_secs(8);

/// What a host connection knows about its server's shared host list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SharedHostState {
    /// Not requested yet, or first read still in flight.
    #[default]
    Unloaded,
    /// Server does not implement or enable `hosts.list`
    /// (`method_not_found` / `not_supported` / `revision_too_low`).
    Unsupported,
    /// Server reported a `conflict` reading its hosts file (e.g. broken JSON). Last-known UI
    /// rows are preserved and reconcile is skipped.
    Conflict(String),
    /// A `hosts.list` read timed out, returned malformed JSON, or failed with another error.
    /// Last-known UI rows are preserved, the Settings card reflects the error instead of
    /// hanging on Loading, and toggling sync off→on (`WatchSharedHosts`) can retry.
    Failed(String),
    /// Successfully read list, including raw objects and any config diagnostics.
    Loaded(SharedHostList),
}

impl SharedHostState {
    pub fn list(&self) -> Option<&SharedHostList> {
        match self {
            Self::Loaded(list) => Some(list),
            _ => None,
        }
    }

    pub fn hash(&self) -> Option<&str> {
        self.list().map(|l| l.hash.as_str())
    }

    /// Whether `HostCommand::WatchSharedHosts` may retry a read while already connected.
    pub(super) fn retryable(&self) -> bool {
        matches!(self, Self::Unloaded | Self::Failed(_) | Self::Conflict(_))
    }
}

/// Whether a `hosts_changed` event requires re-reading `hosts.list`.
/// - `Unsupported` stays quiet.
/// - While a read is already `in_flight`, ALWAYS re-read even if `payload.hash` matches the
///   old cached hash (guards against `h1` → in-flight `h2` → `hosts_changed(h1)` settling on
///   stale `h2`).
/// - Otherwise same hash as the cached `Loaded` list is a no-op.
pub(super) fn changed(state: &SharedHostState, in_flight: bool, payload: &Value) -> bool {
    match state {
        SharedHostState::Unsupported => false,
        _ if in_flight => true,
        SharedHostState::Loaded(list) => payload["hash"]
            .as_str()
            .is_none_or(|hash| hash != list.hash),
        SharedHostState::Unloaded | SharedHostState::Conflict(_) | SharedHostState::Failed(_) => {
            true
        }
    }
}

/// Outcome of one detached `hosts.list` read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Loaded(SharedHostList),
    Conflict(String),
    Unsupported,
    /// Timeout, transport drop, malformed payload, or non-conflict server error.
    Failed(String),
}

fn parse_reply(text: Option<String>) -> Reply {
    let Some(text) = text else {
        return Reply::Failed("hosts.list request timed out or connection closed.".into());
    };
    let Ok(mut response) = serde_json::from_str::<Value>(&text) else {
        return Reply::Failed("hosts.list returned unreadable JSON.".into());
    };
    if !response["error"].is_null() {
        let code = response["error"]["code"].as_str().unwrap_or("");
        return match code {
            "method_not_found" | "not_supported" | "revision_too_low" => Reply::Unsupported,
            "conflict" => {
                let msg = response["error"]["message"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Shared hosts file could not be read.")
                    .to_owned();
                Reply::Conflict(msg)
            }
            _ => {
                let msg = response["error"]["message"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("hosts.list failed.")
                    .to_owned();
                Reply::Failed(msg)
            }
        };
    }
    let payload = response["payload"].take();
    match SharedHostList::from_payload(&payload) {
        Some(list) => Reply::Loaded(list),
        None => Reply::Failed("hosts.list payload was missing required fields.".into()),
    }
}

struct InFlight {
    reply: oneshot::Receiver<String>,
    deadline: Instant,
}

impl InFlight {
    fn new(reply: oneshot::Receiver<String>) -> Self {
        Self {
            reply,
            deadline: Instant::now() + TIMEOUT,
        }
    }
}

/// One host's read-only `hosts.list` reader, across its connections.
#[derive(Default)]
pub struct Reader {
    /// Enabled by the front end (kept across reconnects).
    wanted: bool,
    /// Initial read already sent on the current connection epoch.
    asked: bool,
    in_flight: Option<InFlight>,
}

impl Reader {
    pub fn want(&mut self) {
        self.wanted = true;
    }

    pub fn reset(&mut self) {
        *self = Self {
            wanted: self.wanted,
            ..Self::default()
        };
    }

    /// Send this connection's initial `hosts.list` read once, without blocking control traffic.
    pub async fn start(&mut self, control: &mut Client) -> rexrs::Result<()> {
        if !self.wanted || self.asked {
            return Ok(());
        }
        self.asked = true;
        self.send(control).await
    }

    /// Trigger an initial read, or retry when the cached state is retryable (`Failed`/`Conflict`
    /// /`Unloaded`) and no read is currently in flight.
    pub async fn watch(&mut self, control: &mut Client, retryable: bool) -> rexrs::Result<()> {
        self.wanted = true;
        if !self.asked {
            self.asked = true;
            return self.send(control).await;
        }
        if retryable && self.in_flight.is_none() {
            return self.send(control).await;
        }
        Ok(())
    }

    /// Re-read `hosts.list` after a `hosts_changed` event. Replaces any older read in flight.
    pub async fn reread(&mut self, control: &mut Client) -> rexrs::Result<()> {
        if !self.wanted || !self.asked {
            return Ok(());
        }
        self.send(control).await
    }

    async fn send(&mut self, control: &mut Client) -> rexrs::Result<()> {
        let reply = control.transport().detach("hosts.list", json!({})).await?;
        self.in_flight = Some(InFlight::new(reply));
        Ok(())
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Cancel-safe wait for the in-flight `hosts.list` read to finish or time out.
    pub async fn finished(&mut self) -> Reply {
        let Some(read) = &mut self.in_flight else {
            return std::future::pending().await;
        };
        let deadline = tokio::time::Instant::from_std(read.deadline);
        let text = tokio::time::timeout_at(deadline, &mut read.reply)
            .await
            .ok()
            .and_then(Result::ok);
        self.in_flight = None;
        parse_reply(text)
    }
}

/// Apply a finished `hosts.list` read to the host's model and bump `shared_revision`.
pub(super) fn record(shared: &super::Shared, reply: Reply) {
    let next = match reply {
        Reply::Loaded(list) => SharedHostState::Loaded(list),
        Reply::Conflict(msg) => SharedHostState::Conflict(msg),
        Reply::Unsupported => SharedHostState::Unsupported,
        Reply::Failed(msg) => SharedHostState::Failed(msg),
    };
    shared.update(|m| {
        m.shared = next;
        m.shared_revision += 1;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(payload: Value) -> Option<String> {
        Some(json!({"type":"response","id":"detached:1","payload":payload}).to_string())
    }

    #[test]
    fn parses_loaded_conflict_unsupported_and_failed_replies() {
        let loaded = parse_reply(response(json!({
            "hash": "h1",
            "hosts": [{"id":"host:1","label":"Lab","endpoints":["lab.invalid"],"extra":"keep"}]
        })));
        let Reply::Loaded(list) = loaded else {
            panic!("expected Loaded, got {loaded:?}");
        };
        assert_eq!(list.hash, "h1");
        assert_eq!(list.hosts[0].raw.get("extra"), Some(&json!("keep")));

        let conflict = parse_reply(Some(
            json!({
                "type": "response",
                "id": "detached:1",
                "error": {
                    "code": "conflict",
                    "message": "hosts/everywhere.json is not a JSON object",
                    "status": 409
                }
            })
            .to_string(),
        ));
        assert_eq!(
            conflict,
            Reply::Conflict("hosts/everywhere.json is not a JSON object".into())
        );

        for code in ["method_not_found", "not_supported", "revision_too_low"] {
            let reply = parse_reply(Some(
                json!({
                    "type": "response",
                    "id": "detached:1",
                    "error": {"code": code, "message": "unsupported", "status": 404}
                })
                .to_string(),
            ));
            assert_eq!(reply, Reply::Unsupported, "{code}");
        }

        for failed in [
            None,
            Some("not json".into()),
            response(json!(null)),
            Some(
                json!({"type":"response","id":"detached:1","error":{"code":"internal","message":"oops"}})
                    .to_string(),
            ),
        ] {
            assert!(
                matches!(parse_reply(failed), Reply::Failed(ref m) if !m.is_empty()),
                "expected Failed"
            );
        }
    }

    #[test]
    fn changed_skips_same_hash_unless_in_flight_and_rereads_on_different_or_missing_hash() {
        let loaded = SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: Vec::new(),
            diagnostics: Vec::new(),
        });
        assert!(!changed(&loaded, false, &json!({"hash": "h1"})));
        assert!(
            changed(&loaded, true, &json!({"hash": "h1"})),
            "reverting hash while a read is in flight must trigger a re-read"
        );
        assert!(changed(&loaded, false, &json!({"hash": "h2"})));
        assert!(changed(&loaded, false, &json!({})));
        assert!(!changed(
            &SharedHostState::Unsupported,
            false,
            &json!({"hash": "h2"})
        ));
        assert!(!changed(
            &SharedHostState::Unsupported,
            true,
            &json!({"hash": "h2"})
        ));
        assert!(changed(
            &SharedHostState::Unloaded,
            false,
            &json!({"hash": "h1"})
        ));
        assert!(changed(
            &SharedHostState::Conflict("broken".into()),
            false,
            &json!({"hash": "h1"})
        ));
        assert!(changed(
            &SharedHostState::Failed("timed out".into()),
            false,
            &json!({"hash": "h1"})
        ));
    }
}
