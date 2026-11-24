//! Synchronization of Rex's shared host list (`hosts.list`, `hosts_changed`, `hosts.put`,
//! and `hosts.remove`).
//!
//! Modelled on [`super::server_info`] for reads and `HostCommand::StopServer` / `ResizeLayer`
//! for epoch-scoped writes: once enabled via `HostCommand::WatchSharedHosts`, each control
//! connection sends one detached `hosts.list` read when ready and re-reads when a
//! `hosts_changed` event reports a different (or missing) hash, or arrives while an earlier
//! read is still in flight for a different hash. Explicit writes (`PutSharedHost` and
//! `RemoveSharedHost`) are single-attempt, epoch-scoped, never auto-retried, and never queued
//! across reconnects.
use crate::{
    hosts::{self as core, SharedHost, SharedHostList},
    transport::Client,
};
use anyhow::Context;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

const TIMEOUT: Duration = Duration::from_secs(8);
const MAX_PENDING_RESULTS: usize = 32;

/// Which shared-host write method produced a [`WriteResult`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteKind {
    Put,
    Remove,
}

/// Outcome of one epoch-scoped `hosts.put` or `hosts.remove` attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    Put {
        host: SharedHost,
        hash: String,
    },
    Removed {
        host_id: String,
        removed: bool,
        hash: String,
    },
    Failed {
        code: Option<String>,
        message: String,
    },
}

/// Structured result delivered on [`super::HostModel::shared_writes`] for a single write request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteResult {
    pub id: u64,
    pub epoch: u64,
    pub kind: WriteKind,
    pub outcome: WriteOutcome,
}

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
/// - While a read is already in flight (`in_flight` is `Some(expected_hash)`), skip only if
///   `payload.hash` matches the exact `expected_hash` that read was started for; otherwise
///   ALWAYS re-read (guards against `h1` → in-flight `h2` → `hosts_changed(h1)` settling on
///   stale `h2`, while avoiding a redundant read when `hosts.put`/`hosts.remove` already
///   triggered a refresh for the same `hash`).
/// - Otherwise same hash as the cached `Loaded` list is a no-op.
pub(super) fn changed(
    state: &SharedHostState,
    in_flight: Option<Option<&str>>,
    payload: &Value,
) -> bool {
    let ev_hash = payload["hash"].as_str();
    match state {
        SharedHostState::Unsupported => false,
        _ if let Some(expected) = in_flight => !(ev_hash.is_some() && expected == ev_hash),
        SharedHostState::Loaded(list) => ev_hash.is_none_or(|hash| hash != list.hash),
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
    expected_hash: Option<String>,
}

impl InFlight {
    fn new(reply: oneshot::Receiver<String>, expected_hash: Option<String>) -> Self {
        Self {
            reply,
            deadline: Instant::now() + TIMEOUT,
            expected_hash,
        }
    }
}

/// One host's `hosts.list` reader, across its connections.
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
        self.send(control, None).await
    }

    /// Trigger an initial read, or retry when the cached state is retryable (`Failed`/`Conflict`
    /// /`Unloaded`) and no read is currently in flight.
    pub async fn watch(&mut self, control: &mut Client, retryable: bool) -> rexrs::Result<()> {
        self.wanted = true;
        if !self.asked {
            self.asked = true;
            return self.send(control, None).await;
        }
        if retryable && self.in_flight.is_none() {
            return self.send(control, None).await;
        }
        Ok(())
    }

    /// Re-read `hosts.list` after a `hosts_changed` event or successful write. Replaces any older
    /// read in flight.
    pub async fn reread_for(
        &mut self,
        control: &mut Client,
        expected_hash: Option<&str>,
    ) -> rexrs::Result<()> {
        if !self.wanted || !self.asked {
            return Ok(());
        }
        self.send(control, expected_hash.map(str::to_owned)).await
    }

    async fn send(
        &mut self,
        control: &mut Client,
        expected_hash: Option<String>,
    ) -> rexrs::Result<()> {
        let reply = control.transport().detach("hosts.list", json!({})).await?;
        self.in_flight = Some(InFlight::new(reply, expected_hash));
        Ok(())
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    pub fn in_flight_hash(&self) -> Option<Option<&str>> {
        self.in_flight.as_ref().map(|r| r.expected_hash.as_deref())
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

/// Push one bounded write outcome onto `HostModel::shared_writes` and wake the UI.
pub(super) fn record_write(shared: &super::Shared, result: WriteResult) {
    shared.update(|m| {
        if m.shared_writes.len() >= MAX_PENDING_RESULTS {
            m.shared_writes.remove(0);
        }
        m.shared_writes.push(result);
    });
}

/// Record an immediate local refusal (stale epoch or disconnected) without touching the wire or
/// cached `SharedHostState`.
pub(super) fn reject_write(
    shared: &super::Shared,
    id: u64,
    epoch: u64,
    kind: WriteKind,
    code: &str,
    message: impl Into<String>,
) {
    record_write(
        shared,
        WriteResult {
            id,
            epoch,
            kind,
            outcome: WriteOutcome::Failed {
                code: Some(code.into()),
                message: message.into(),
            },
        },
    );
}

fn api_failure(api: &rexrs::ApiError) -> WriteOutcome {
    let code = api.code().to_owned();
    let msg = crate::download::display_text(api.message());
    let message = if msg.trim().is_empty() {
        format!("Rex returned {code}.")
    } else {
        msg
    };
    WriteOutcome::Failed {
        code: Some(code),
        message,
    }
}

/// Execute one epoch-scoped `hosts.put` request. Never retries; never mutates `m.shared` on error.
/// On success, triggers a `hosts.list` refresh when the returned `hash` differs from the cached
/// list so `SharedHostState::Loaded` stays authoritative even if `hosts_changed` is delayed.
pub(super) async fn execute_put(
    control: &mut Client,
    shared: &super::Shared,
    reader: &mut Reader,
    id: u64,
    epoch: u64,
    host: Value,
) -> anyhow::Result<()> {
    let live_epoch = shared.model.lock().expect("host model").connection_epoch;
    if epoch != live_epoch {
        reject_write(
            shared,
            id,
            epoch,
            WriteKind::Put,
            "stale_epoch",
            "The connection changed before the shared host change was sent. Try again.",
        );
        return Ok(());
    }
    let result = tokio::time::timeout(
        TIMEOUT,
        control.call_raw("hosts.put", json!({ "host": host })),
    )
    .await;
    match result {
        Ok(Ok(payload)) => {
            let Some(stored) = SharedHost::from_value(&payload["host"]) else {
                record_write(
                    shared,
                    WriteResult {
                        id,
                        epoch,
                        kind: WriteKind::Put,
                        outcome: WriteOutcome::Failed {
                            code: None,
                            message: "hosts.put returned an unexpected response.".into(),
                        },
                    },
                );
                return Ok(());
            };
            let Some(hash) = payload["hash"].as_str().map(str::to_owned) else {
                record_write(
                    shared,
                    WriteResult {
                        id,
                        epoch,
                        kind: WriteKind::Put,
                        outcome: WriteOutcome::Failed {
                            code: None,
                            message: "hosts.put response was missing hash.".into(),
                        },
                    },
                );
                return Ok(());
            };
            let needs_refresh = reader.in_flight()
                || shared.model.lock().expect("host model").shared.hash() != Some(hash.as_str());
            record_write(
                shared,
                WriteResult {
                    id,
                    epoch,
                    kind: WriteKind::Put,
                    outcome: WriteOutcome::Put {
                        host: stored,
                        hash: hash.clone(),
                    },
                },
            );
            if needs_refresh {
                reader.reread_for(control, Some(&hash)).await?;
            }
            Ok(())
        }
        Ok(Err(rexrs::Error::Api(api))) => {
            record_write(
                shared,
                WriteResult {
                    id,
                    epoch,
                    kind: WriteKind::Put,
                    outcome: api_failure(&api),
                },
            );
            Ok(())
        }
        Ok(Err(error)) => {
            record_write(
                shared,
                WriteResult {
                    id,
                    epoch,
                    kind: WriteKind::Put,
                    outcome: WriteOutcome::Failed {
                        code: None,
                        message: "Shared host change could not be confirmed: the connection dropped. Nothing was retried.".into(),
                    },
                },
            );
            Err(error.into())
        }
        Err(elapsed) => {
            record_write(
                shared,
                WriteResult {
                    id,
                    epoch,
                    kind: WriteKind::Put,
                    outcome: WriteOutcome::Failed {
                        code: Some("timeout".into()),
                        message: "Shared host change timed out. Check the host list before trying again. Nothing was retried.".into(),
                    },
                },
            );
            Err(elapsed).context("hosts.put timed out; action was not retried")
        }
    }
}

/// Execute one epoch-scoped `hosts.remove` request. Never retries; never mutates `m.shared` on
/// error. On success, triggers a `hosts.list` refresh when the returned `hash` differs from cache.
pub(super) async fn execute_remove(
    control: &mut Client,
    shared: &super::Shared,
    reader: &mut Reader,
    id: u64,
    epoch: u64,
    host_id: String,
) -> anyhow::Result<()> {
    let live_epoch = shared.model.lock().expect("host model").connection_epoch;
    if epoch != live_epoch {
        reject_write(
            shared,
            id,
            epoch,
            WriteKind::Remove,
            "stale_epoch",
            "The connection changed before the shared host removal was sent. Try again.",
        );
        return Ok(());
    }
    let result = tokio::time::timeout(
        TIMEOUT,
        control.call_raw("hosts.remove", core::build_remove_payload(&host_id)),
    )
    .await;
    match result {
        Ok(Ok(payload)) => {
            let Some(removed) = payload["removed"].as_bool() else {
                record_write(
                    shared,
                    WriteResult {
                        id,
                        epoch,
                        kind: WriteKind::Remove,
                        outcome: WriteOutcome::Failed {
                            code: None,
                            message: "hosts.remove returned an unexpected response.".into(),
                        },
                    },
                );
                return Ok(());
            };
            let Some(hash) = payload["hash"].as_str().map(str::to_owned) else {
                record_write(
                    shared,
                    WriteResult {
                        id,
                        epoch,
                        kind: WriteKind::Remove,
                        outcome: WriteOutcome::Failed {
                            code: None,
                            message: "hosts.remove response was missing hash.".into(),
                        },
                    },
                );
                return Ok(());
            };
            let needs_refresh = reader.in_flight()
                || shared.model.lock().expect("host model").shared.hash() != Some(hash.as_str());
            record_write(
                shared,
                WriteResult {
                    id,
                    epoch,
                    kind: WriteKind::Remove,
                    outcome: WriteOutcome::Removed {
                        host_id,
                        removed,
                        hash: hash.clone(),
                    },
                },
            );
            if needs_refresh {
                reader.reread_for(control, Some(&hash)).await?;
            }
            Ok(())
        }
        Ok(Err(rexrs::Error::Api(api))) => {
            record_write(
                shared,
                WriteResult {
                    id,
                    epoch,
                    kind: WriteKind::Remove,
                    outcome: api_failure(&api),
                },
            );
            Ok(())
        }
        Ok(Err(error)) => {
            record_write(
                shared,
                WriteResult {
                    id,
                    epoch,
                    kind: WriteKind::Remove,
                    outcome: WriteOutcome::Failed {
                        code: None,
                        message: "Shared host removal could not be confirmed: the connection dropped. Nothing was retried.".into(),
                    },
                },
            );
            Err(error.into())
        }
        Err(elapsed) => {
            record_write(
                shared,
                WriteResult {
                    id,
                    epoch,
                    kind: WriteKind::Remove,
                    outcome: WriteOutcome::Failed {
                        code: Some("timeout".into()),
                        message: "Shared host removal timed out. Check the host list before trying again. Nothing was retried.".into(),
                    },
                },
            );
            Err(elapsed).context("hosts.remove timed out; action was not retried")
        }
    }
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
        assert!(!changed(&loaded, None, &json!({"hash": "h1"})));
        assert!(
            changed(&loaded, Some(Some("h2")), &json!({"hash": "h1"})),
            "reverting hash while a read for h2 is in flight must trigger a re-read"
        );
        assert!(
            changed(&loaded, Some(None), &json!({"hash": "h1"})),
            "hash arriving while an initial read is in flight must trigger a re-read"
        );
        assert!(
            !changed(&loaded, Some(Some("h2")), &json!({"hash": "h2"})),
            "duplicate notification for the exact hash already in flight is skipped"
        );
        assert!(changed(&loaded, None, &json!({"hash": "h2"})));
        assert!(changed(&loaded, None, &json!({})));
        assert!(!changed(
            &SharedHostState::Unsupported,
            None,
            &json!({"hash": "h2"})
        ));
        assert!(!changed(
            &SharedHostState::Unsupported,
            Some(None),
            &json!({"hash": "h2"})
        ));
        assert!(changed(
            &SharedHostState::Unloaded,
            None,
            &json!({"hash": "h1"})
        ));
        assert!(changed(
            &SharedHostState::Conflict("broken".into()),
            None,
            &json!({"hash": "h1"})
        ));
        assert!(changed(
            &SharedHostState::Failed("timed out".into()),
            None,
            &json!({"hash": "h1"})
        ));
    }
}
