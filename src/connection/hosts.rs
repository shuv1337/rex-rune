//! Read-only synchronization of Rex's shared host list (`hosts.list` + `hosts_changed`).
//!
//! Modelled on [`super::server_info`]: once enabled via `HostCommand::WatchSharedHosts`, each
//! control connection sends one detached `hosts.list` read when ready and re-reads only when a
//! `hosts_changed` event reports a different (or missing) hash. Raw JSON is parsed directly so
//! unknown fields on host records are preserved. Never sends `hosts.put` or `hosts.remove`.
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
    /// Server does not implement or enable `hosts.list` (`method_not_found` / `not_supported`).
    Unsupported,
    /// Server reported a `conflict` reading its hosts file (e.g. broken JSON). Last-known UI
    /// rows are preserved and reconcile is skipped.
    Conflict(String),
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
}

/// Whether a `hosts_changed` event requires re-reading `hosts.list`.
/// Same hash as the cached `Loaded` list is a no-op; `Unsupported` is quiet.
pub(super) fn changed(state: &SharedHostState, payload: &Value) -> bool {
    match state {
        SharedHostState::Unsupported => false,
        SharedHostState::Loaded(list) => payload["hash"]
            .as_str()
            .is_none_or(|hash| hash != list.hash),
        SharedHostState::Unloaded | SharedHostState::Conflict(_) => true,
    }
}

/// Outcome of one detached `hosts.list` read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Loaded(SharedHostList),
    Conflict(String),
    Unsupported,
    /// Timeout, transport drop, or unexpected error: keep previous state standing.
    Transient,
}

fn parse_reply(text: Option<String>) -> Reply {
    let Some(text) = text else {
        return Reply::Transient;
    };
    let Ok(mut response) = serde_json::from_str::<Value>(&text) else {
        return Reply::Transient;
    };
    if !response["error"].is_null() {
        let code = response["error"]["code"].as_str().unwrap_or("");
        return match code {
            "method_not_found" | "not_supported" => Reply::Unsupported,
            "conflict" => {
                let msg = response["error"]["message"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Shared hosts file could not be read.")
                    .to_owned();
                Reply::Conflict(msg)
            }
            _ => Reply::Transient,
        };
    }
    let payload = response["payload"].take();
    match SharedHostList::from_payload(&payload) {
        Some(list) => Reply::Loaded(list),
        None => Reply::Transient,
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
        Reply::Transient => return,
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
    fn parses_loaded_conflict_unsupported_and_transient_replies() {
        let loaded = parse_reply(response(json!({
            "hash": "h1",
            "hosts": [{"id":"host:1","label":"Lab","endpoints":["lab.ts.net"],"extra":"keep"}]
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

        for code in ["method_not_found", "not_supported"] {
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

        for transient in [
            None,
            Some("not json".into()),
            response(json!(null)),
            Some(
                json!({"type":"response","id":"detached:1","error":{"code":"internal","message":"oops"}})
                    .to_string(),
            ),
        ] {
            assert_eq!(parse_reply(transient), Reply::Transient);
        }
    }

    #[test]
    fn changed_skips_same_hash_and_rereads_on_different_or_missing_hash() {
        let loaded = SharedHostState::Loaded(SharedHostList {
            hash: "h1".into(),
            hosts: Vec::new(),
            diagnostics: Vec::new(),
        });
        assert!(!changed(&loaded, &json!({"hash": "h1"})));
        assert!(changed(&loaded, &json!({"hash": "h2"})));
        assert!(changed(&loaded, &json!({})));
        assert!(!changed(
            &SharedHostState::Unsupported,
            &json!({"hash": "h2"})
        ));
        assert!(changed(&SharedHostState::Unloaded, &json!({"hash": "h1"})));
        assert!(changed(
            &SharedHostState::Conflict("broken".into()),
            &json!({"hash": "h1"})
        ));
    }
}
