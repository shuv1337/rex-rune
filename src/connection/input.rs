//! Pipelined terminal input requests.
//!
//! Rex guarantees that `client.key` and `block.event` execute in FIFO arrival order on one control
//! connection, so key presses, releases, pastes, pointer events and wheel turns can be
//! transmitted immediately without blocking the host loop for a full round trip per event.
//!
//! Responses are still tracked on the same connection: API errors surface as notices without
//! dropping the host, while transport errors or an 8-second stall on the oldest unanswered request
//! disconnect so failed input pauses and is never replayed.
use super::{Client, Command, HostCommand, Shared};
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::{collections::VecDeque, future::Future, pin::Pin, task::Poll, time::Duration};
use tokio::{
    sync::oneshot,
    time::{Instant, Sleep},
};

/// Maximum unanswered input requests before applying backpressure.
pub(super) const MAX_IN_FLIGHT: usize = 64;

/// Maximum time the oldest unanswered input request may wait without progress before failing.
pub(super) const TIMEOUT: Duration = Duration::from_secs(8);

pub(super) struct Queue {
    in_flight: VecDeque<oneshot::Receiver<String>>,
    timer: Pin<Box<Sleep>>,
}

/// None for commands outside the ordered-input pipeline.
pub(super) fn passive(command: &Command) -> Option<bool> {
    Some(match command {
        Command::Key(k) | Command::KeyWithPrefix { key: k, .. } | Command::SendKey(k) => {
            k["action"] == "release"
        }
        Command::Mouse(m) => m["action"] != "press",
        Command::Wheel(_) => true,
        Command::Paste(_) => false,
        _ => return None,
    })
}

pub(super) fn wire_request(
    session: &str,
    block: &str,
    input: Command,
) -> Option<(&'static str, Value)> {
    use serde_json::json;
    let event = |kind, payload| {
        (
            "block.event",
            json!({"session_id":session,"block_id":block,"event":{"type":kind,"payload":payload}}),
        )
    };
    Some(match input {
        Command::Key(key) => (
            "client.key",
            json!({"session_id":session,"block_id":block,"key":key}),
        ),
        Command::KeyWithPrefix { key, prefix } => (
            "client.key",
            json!({"session_id":session,"block_id":block,"key":key,"prefix":prefix}),
        ),
        Command::SendKey(key) => event("key", key),
        Command::Paste(text) => event("paste", json!({"text":text})),
        Command::Mouse(mouse) => event("mouse", mouse),
        Command::Wheel(wheel) => event("wheel", wheel),
        Command::TakeControl => return None,
        _ => unreachable!("not an input command"),
    })
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            in_flight: VecDeque::new(),
            timer: Box::pin(tokio::time::sleep(Duration::ZERO)),
        }
    }
}

impl Queue {
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    pub fn is_full(&self) -> bool {
        self.in_flight.len() >= MAX_IN_FLIGHT
    }

    /// Only ordered wire input may pass an unanswered input. An ownership claim is a
    /// separate control call, so the first active input to an unowned pane also fences.
    pub fn admits(&self, command: &HostCommand, shared: &Shared) -> bool {
        if self.in_flight.is_empty()
            || matches!(
                command,
                HostCommand::PromptReply { .. }
                    | HostCommand::Visibility(_)
                    | HostCommand::Prompts(_)
                    | HostCommand::Shutdown
                    | HostCommand::Reconnect
            )
        {
            return true;
        }
        let HostCommand::Terminal { block, command, .. } = command else {
            return false;
        };
        let Some(passive) = passive(command) else {
            return false;
        };
        !self.is_full()
            && (passive
                || shared
                    .model
                    .lock()
                    .expect("host model")
                    .terminals
                    .get(block)
                    .is_some_and(|pane| pane.model.lock().expect("terminal model").owner))
    }

    fn push(&mut self, reply: oneshot::Receiver<String>, started: Instant) {
        if self.in_flight.is_empty() {
            self.timer.as_mut().reset(started + TIMEOUT);
        }
        self.in_flight.push_back(reply);
    }

    /// Transmit one ordered input request without waiting for its reply.
    /// Local serialization/size errors become a host notice; transport errors disconnect.
    pub async fn send(
        &mut self,
        control: &mut Client,
        shared: &Shared,
        method: &str,
        payload: Value,
    ) -> Result<()> {
        debug_assert!(!self.is_full());
        let started = Instant::now();
        let mut deadline = started + TIMEOUT;
        if !self.in_flight.is_empty() {
            deadline = deadline.min(self.timer.deadline());
        }
        match tokio::time::timeout_at(deadline, control.transport().detach(method, payload))
            .await
            .map_err(|_| anyhow!("Rex input write timed out; action was not retried"))?
        {
            Ok(reply) => {
                self.push(reply, started);
                Ok(())
            }
            Err(error @ (rexrs::Error::Json(_) | rexrs::Error::RequestTooLarge { .. })) => {
                shared.update(|m| m.notice = Some(format!("{error:#}")));
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Wait for any in-flight input reply or for the oldest unanswered request to time out.
    /// Cancel-safe: an entry is removed only in the poll that returns it.
    pub async fn finished(&mut self, shared: &Shared) -> Result<()> {
        if self.in_flight.is_empty() {
            return std::future::pending().await;
        }
        let outcome = std::future::poll_fn(|cx| {
            if let Poll::Ready(reply) = Pin::new(&mut self.in_flight[0]).poll(cx) {
                return Poll::Ready(Ok((0, reply)));
            }
            if Instant::now() >= self.timer.deadline() || self.timer.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(()));
            }
            for (index, entry) in self.in_flight.iter_mut().enumerate().skip(1) {
                if let Poll::Ready(reply) = Pin::new(entry).poll(cx) {
                    return Poll::Ready(Ok((index, reply)));
                }
            }
            Poll::Pending
        })
        .await;
        let Ok((index, reply)) = outcome else {
            return Err(anyhow!("Rex request timed out; action was not retried"));
        };
        self.in_flight.remove(index);
        if index == 0 && !self.in_flight.is_empty() {
            self.timer.as_mut().reset(Instant::now() + TIMEOUT);
        }
        let text = reply
            .map_err(|_| anyhow!("Rex control connection closed while awaiting input reply"))?;
        match crate::transport::detached_result::<Value>(&text) {
            Ok(_) => Ok(()),
            Err(error @ rexrs::Error::Api(_)) => {
                shared.update(|m| m.notice = Some(format!("{error:#}")));
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::HostModel;
    use std::sync::{Arc, Mutex};

    fn shared() -> Shared {
        let (wake, _rx) = flume::bounded(1);
        Shared {
            model: Arc::new(Mutex::new(HostModel::default())),
            wake: crate::connection::title_wake::HostWake::new(wake),
        }
    }

    fn push(queue: &mut Queue, sent_at: Instant) -> oneshot::Sender<String> {
        let (tx, rx) = oneshot::channel();
        queue.push(rx, sent_at);
        tx
    }

    #[tokio::test]
    async fn api_errors_notify_without_disconnecting_while_success_is_quiet() {
        let shared = shared();
        let mut queue = Queue::default();
        let now = Instant::now();
        let ok_tx = push(&mut queue, now);
        let err_tx = push(&mut queue, now);
        assert_eq!(queue.in_flight(), 2);

        ok_tx
            .send(
                r#"{"type":"response","id":"detached:1","seq":1,"payload":{"handled":false}}"#
                    .into(),
            )
            .unwrap();
        queue.finished(&shared).await.unwrap();
        assert_eq!(queue.in_flight(), 1);
        assert_eq!(shared.model.lock().unwrap().notice, None);

        err_tx
            .send(r#"{"type":"response","id":"detached:2","seq":2,"error":{"code":"not_found","status":404,"message":"block gone"}}"#.into())
            .unwrap();
        queue.finished(&shared).await.unwrap();
        assert_eq!(queue.in_flight(), 0);
        assert!(
            shared
                .model
                .lock()
                .unwrap()
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("block gone"))
        );
    }

    #[tokio::test]
    async fn head_progress_resets_next_deadline_but_tail_reply_does_not() {
        let shared = shared();
        let mut queue = Queue::default();
        let now = Instant::now();
        let first = push(&mut queue, now);
        let second = push(&mut queue, now);

        let deadline = queue.timer.deadline();
        second
            .send(r#"{"type":"response","id":"detached:2","seq":2,"payload":{}}"#.into())
            .unwrap();
        queue.finished(&shared).await.unwrap();
        assert_eq!(queue.in_flight(), 1);

        assert_eq!(queue.timer.deadline(), deadline);
        let tail = push(&mut queue, now);
        tail.send(r#"{"payload":{}}"#.into()).unwrap();
        queue.timer.as_mut().reset(now - TIMEOUT);
        let err = queue.finished(&shared).await.unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err:#}");
        assert_eq!(queue.in_flight(), 2);
        first
            .send(r#"{"type":"response","id":"detached:3","seq":3,"payload":{}}"#.into())
            .unwrap();
        queue.finished(&shared).await.unwrap();

        assert!(queue.timer.deadline() >= deadline);
        queue
            .finished(&shared)
            .await
            .expect("second received fresh timeout after head completed");
    }

    #[tokio::test]
    async fn dropped_sender_returns_transport_error() {
        let shared = shared();
        let mut queue = Queue::default();
        let tx = push(&mut queue, Instant::now());
        drop(tx);
        let err = queue.finished(&shared).await.unwrap_err();
        assert!(err.to_string().contains("closed"), "{err:#}");
    }
}
