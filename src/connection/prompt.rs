//! Connection-scoped `rex.ui` offers (API 17+). No consumer is opted in by default.
use super::{Client, Shared, perform};
use anyhow::Result;
use rexrs::api::{Empty, Method, client::Reply};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

pub(super) const MIN_API: u32 = 17;
const MAX_EVENTS: usize = 32;
const REPLY_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub call_id: String,
    pub action: Option<String>,
    pub title: String,
    pub placeholder: Option<String>,
    pub value: Option<String>,
    pub confirm: Option<String>,
    pub kind: PromptKind,
    pub timeout_ms: u64,
    pub deadline: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Input,
    Pick {
        multiple: bool,
        allow_other: bool,
        items: Vec<PromptItem>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptItem {
    /// Opaque server identity, never display it or change it.
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Option<PromptIcon>,
    pub detail: Option<String>,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptIcon {
    Document,
    Folder,
    Branch,
    Terminal,
    Code,
    Server,
    Database,
    Package,
    Globe,
    Link,
    Person,
    Tag,
    Clock,
    Star,
    Gear,
    Plus,
    #[serde(other)]
    Generic,
}

pub enum PromptAnswer {
    Text(String),
    /// For multiple selection, list the IDs in the original offer's order.
    Ids(Vec<String>),
    Cancelled,
}

impl fmt::Debug for PromptAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f
                .debug_struct("Text")
                .field("bytes", &text.len())
                .field("chars", &text.chars().count())
                .finish(),
            Self::Ids(ids) => f.debug_struct("Ids").field("count", &ids.len()).finish(),
            Self::Cancelled => f.write_str("Cancelled"),
        }
    }
}

impl PromptAnswer {
    fn result(self) -> Value {
        match self {
            Self::Text(text) => json!({ "text": text }),
            Self::Ids(ids) => json!({ "ids": ids }),
            Self::Cancelled => json!({ "cancelled": true }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    Answered,
    Timeout,
    Cancelled,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyOutcome {
    Accepted,
    NotFound,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptEvent {
    pub epoch: u64,
    pub kind: PromptEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptEventKind {
    Requested(Arc<Prompt>),
    /// Still offered. The consumer may send a different answer.
    ReplyRejected {
        call_id: String,
        message: String,
    },
    Ended {
        call_id: String,
        reason: EndReason,
    },
    ReplyOutcome {
        call_id: String,
        outcome: ReplyOutcome,
    },
    /// The server dropped an offer before sending it. There is no call ID to answer.
    Dropped,
}

#[derive(Deserialize)]
struct Fields {
    kind: String,
    title: String,
    placeholder: Option<String>,
    value: Option<String>,
    confirm: Option<String>,
}

#[derive(Deserialize)]
struct PickFields {
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    allow_other: bool,
    items: Option<Vec<ItemFields>>,
}

#[derive(Deserialize)]
struct ItemFields {
    id: String,
    title: String,
    subtitle: Option<String>,
    icon: Option<PromptIcon>,
    detail: Option<String>,
    #[serde(default)]
    selected: bool,
}

fn timeout_ms(payload: &Value) -> Option<u64> {
    let ms = &payload["timeout_ms"];
    Some(
        ms.as_u64()
            .or_else(|| ms.as_i64().map(|_| 0))?
            .clamp(1_000, 3_660_000),
    )
}

fn clean(text: Option<String>, max: usize) -> Option<String> {
    text.map(|s| perform::sanitize_text(&s, max))
        .filter(|s| !s.is_empty())
}

fn parse(payload: &Value, call_id: &str, received: Instant) -> Option<Prompt> {
    let timeout_ms = timeout_ms(payload)?;
    let body = &payload["prompt"];
    let fields = Fields::deserialize(body).ok()?;
    if fields
        .value
        .as_ref()
        .is_some_and(|s| s.chars().take(4097).count() > 4096)
    {
        return None;
    }
    let kind = match fields.kind.as_str() {
        "input" => PromptKind::Input,
        "pick" => {
            if body["items"]
                .as_array()
                .is_some_and(|items| items.len() > 2000)
            {
                return None;
            }
            let pick = PickFields::deserialize(body).ok()?;
            let items = pick.items.unwrap_or_default();
            let mut ids = HashSet::with_capacity(items.len());
            for item in &items {
                if item.id.is_empty() || item.id.len() > 1024 || !ids.insert(item.id.as_str()) {
                    return None;
                }
            }
            PromptKind::Pick {
                multiple: pick.multiple,
                allow_other: !pick.multiple && pick.allow_other,
                items: items
                    .into_iter()
                    .map(|item| PromptItem {
                        id: item.id,
                        title: perform::sanitize_text(&item.title, 200),
                        subtitle: clean(item.subtitle, 200),
                        icon: item.icon,
                        detail: clean(item.detail, 32),
                        selected: pick.multiple && item.selected,
                    })
                    .collect(),
            }
        }
        _ => return None,
    };
    Some(Prompt {
        call_id: call_id.to_owned(),
        action: clean(payload["action"].as_str().map(str::to_owned), 60),
        title: clean(Some(fields.title), 120).unwrap_or_else(|| "Question".into()),
        placeholder: clean(fields.placeholder, 80),
        value: fields.value.map(|s| perform::sanitize_text(&s, 4096)),
        confirm: clean(fields.confirm, 24).filter(|s| !s.eq_ignore_ascii_case("cancel")),
        kind,
        timeout_ms,
        deadline: received + Duration::from_millis(timeout_ms),
    })
}

fn emit(shared: &Shared, epoch: u64, kind: PromptEventKind) {
    shared.update(|m| {
        if m.prompt_events.len() == MAX_EVENTS {
            m.prompt_events.pop_front();
        }
        m.prompt_events.push_back(PromptEvent { epoch, kind });
    });
}

fn log_failure(call_id: &str, code: &str) {
    // Never log the answer, the wire response, or the server's human-readable message.
    let call_id = perform::sanitize_text(call_id, 120);
    let code = perform::sanitize_text(code, 60);
    log::debug!("rex.ui client.reply call_id={call_id:?} code={code:?}");
}

async fn refuse(control: &mut Client, call_id: &str, message: String) -> Result<()> {
    let reply = perform::Outcome::error("not_supported", message).reply(call_id);
    // A refusal must not make the next prompt or the action wait for a receipt.
    match control.transport().detach(Reply::NAME, reply).await {
        Ok(_) => Ok(()),
        Err(error @ (rexrs::Error::Transport(_) | rexrs::Error::Closed { .. })) => {
            Err(error.into())
        }
        Err(_) => {
            log_failure(call_id, "not_sent");
            Ok(())
        }
    }
}

struct Live {
    epoch: u64,
    prompt: Arc<Prompt>,
    reply: Option<InFlight>,
}

struct InFlight {
    response: oneshot::Receiver<String>,
    deadline: tokio::time::Instant,
}

#[derive(Default)]
pub(super) struct PromptState {
    live: Option<Live>,
}

impl PromptState {
    pub fn deadline(&self) -> Option<Instant> {
        self.live.as_ref().map(|live| live.prompt.deadline)
    }

    pub fn is_event(event: &rexrs::Event) -> bool {
        matches!(
            event.name.as_str(),
            "client_prompt_requested" | "client_prompt_ended"
        ) || (event.name == "event_dropped"
            && event.payload["event_type"] == "client_prompt_requested")
    }

    /// Return the deadline to give any suspended `client.action`, even if we refuse
    /// the offer: another app may still be answering on the server.
    pub async fn event(
        &mut self,
        control: &mut Client,
        shared: &Shared,
        enabled: bool,
        event: rexrs::Event,
    ) -> Result<Option<Instant>> {
        if control.welcome().api_revision < MIN_API {
            return Ok(None);
        }
        // The transport can read a reply and the next event in one poll. Settle the
        // reply first, so a subsequent offer does not see a slot that already closed.
        if let Some(reply) = self.live.as_mut().and_then(|live| live.reply.as_mut()) {
            match reply.response.try_recv() {
                Ok(text) => self.record(shared, crate::transport::detached_result(&text)),
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(error) => return Err(rexrs::Error::transport(error).into()),
            }
        }
        let payload = event.payload;
        let epoch = shared.model.lock().expect("host model").connection_epoch;
        if event.name == "event_dropped" {
            if enabled {
                emit(shared, epoch, PromptEventKind::Dropped);
            }
            return Ok(None);
        }
        let Some(call_id) = payload["call_id"].as_str().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        if event.name == "client_prompt_ended" {
            if self
                .live
                .as_ref()
                .is_some_and(|live| live.prompt.call_id == call_id)
            {
                let reason = match payload["reason"].as_str() {
                    Some("answered") => EndReason::Answered,
                    Some("timeout") => EndReason::Timeout,
                    Some("cancelled") => EndReason::Cancelled,
                    _ => EndReason::Unknown,
                };
                self.end(shared, reason);
            }
            return Ok(None);
        }
        let received = Instant::now();
        let deadline = timeout_ms(&payload).map(|ms| received + Duration::from_millis(ms));
        self.expire(shared);
        if !enabled {
            let action = clean(payload["action"].as_str().map(str::to_owned), 60);
            let message = action.map_or_else(
                || "Rex Rune is not showing rex.ui prompts.".into(),
                |action| format!("Rex Rune is not showing rex.ui prompts (action \"{action}\")."),
            );
            refuse(control, call_id, message).await?;
        } else if self.live.is_some() {
            refuse(control, call_id, "Another prompt is already open.".into()).await?;
        } else if let Some(prompt) = parse(&payload, call_id, received) {
            let prompt = Arc::new(prompt);
            self.live = Some(Live {
                epoch,
                prompt: prompt.clone(),
                reply: None,
            });
            emit(shared, epoch, PromptEventKind::Requested(prompt));
        } else {
            refuse(
                control,
                call_id,
                "Rex Rune cannot display this prompt.".into(),
            )
            .await?;
        }
        Ok(deadline)
    }

    pub async fn disable(&mut self, control: &mut Client, shared: &Shared) -> Result<()> {
        if let Some(prompt) = self.end(shared, EndReason::Cancelled) {
            refuse(
                control,
                &prompt.call_id,
                "Rex Rune is not showing rex.ui prompts.".into(),
            )
            .await?;
        }
        Ok(())
    }

    fn end(&mut self, shared: &Shared, reason: EndReason) -> Option<Arc<Prompt>> {
        let live = self.live.take()?;
        emit(
            shared,
            live.epoch,
            PromptEventKind::Ended {
                call_id: live.prompt.call_id.clone(),
                reason,
            },
        );
        Some(live.prompt)
    }

    pub fn expire(&mut self, shared: &Shared) {
        // The offer deadline stops new answers. A sent answer has its own reply budget.
        if self
            .live
            .as_ref()
            .is_some_and(|live| live.reply.is_none() && Instant::now() >= live.prompt.deadline)
        {
            self.end(shared, EndReason::Timeout);
        }
    }

    pub async fn answer(
        &mut self,
        control: &mut Client,
        shared: &Shared,
        epoch: u64,
        call_id: String,
        answer: PromptAnswer,
    ) -> Result<()> {
        self.expire(shared);
        let Some(live) = self.live.as_mut().filter(|live| {
            live.epoch == epoch && live.prompt.call_id == call_id && live.reply.is_none()
        }) else {
            return Ok(());
        };
        let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
        let reply = Reply::new(&call_id).result(answer.result());
        match control.transport().detach(Reply::NAME, reply).await {
            Ok(response) => live.reply = Some(InFlight { response, deadline }),
            Err(error @ (rexrs::Error::Transport(_) | rexrs::Error::Closed { .. })) => {
                return Err(error.into());
            }
            Err(error) => self.record(shared, Err(error)),
        }
        Ok(())
    }

    pub async fn finished(&mut self) -> rexrs::Result<Empty> {
        let Some(reply) = self.live.as_mut().and_then(|live| live.reply.as_mut()) else {
            return std::future::pending().await;
        };
        match tokio::time::timeout_at(reply.deadline, &mut reply.response).await {
            Ok(Ok(text)) => crate::transport::detached_result(&text),
            Ok(Err(error)) => Err(rexrs::Error::transport(error)),
            Err(_) => Err(rexrs::Error::TimedOut),
        }
    }

    pub fn record(&mut self, shared: &Shared, result: rexrs::Result<Empty>) {
        let live = self.live.as_mut().expect("live prompt with reply");
        let epoch = live.epoch;
        let call_id = live.prompt.call_id.clone();
        let rejection = match &result {
            Err(rexrs::Error::Api(rexrs::ApiError::BadRequest { message })) => {
                log_failure(&call_id, "bad_request");
                Some(
                    clean(Some(message.clone()), 200)
                        .unwrap_or_else(|| "Answer not accepted.".into()),
                )
            }
            Err(rexrs::Error::RequestTooLarge { .. }) => {
                log_failure(&call_id, "request_too_large");
                Some("The answer is too large for this server. Shorten it and try again.".into())
            }
            _ => None,
        };
        if let Some(message) = rejection {
            live.reply = None;
            emit(
                shared,
                epoch,
                PromptEventKind::ReplyRejected { call_id, message },
            );
            return;
        }
        let outcome = match result {
            Ok(_) => ReplyOutcome::Accepted,
            Err(rexrs::Error::Api(rexrs::ApiError::NotFound { .. })) => {
                log_failure(&call_id, "not_found");
                ReplyOutcome::NotFound
            }
            Err(error) => {
                let code = match &error {
                    rexrs::Error::Api(error) => error.code(),
                    rexrs::Error::TimedOut => "timeout",
                    _ => "invalid_reply",
                };
                log_failure(&call_id, code);
                ReplyOutcome::Failed
            }
        };
        self.live = None;
        emit(
            shared,
            epoch,
            PromptEventKind::ReplyOutcome { call_id, outcome },
        );
    }
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
