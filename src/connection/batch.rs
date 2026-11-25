//! Selected-session reads and theme declarations, at most eight blocks per round.
use super::*;
use std::{future::Future, pin::Pin, task::Poll};
use tokio::{sync::oneshot, time::Sleep};

const BLOCKS: usize = 8;

struct Pending<K> {
    key: K,
    reply: oneshot::Receiver<String>,
    deadline: Pin<Box<Sleep>>,
}

impl<K> Pending<K> {
    async fn send(
        control: &mut Client,
        key: K,
        method: &str,
        payload: impl serde::Serialize,
        reply_budget: Duration,
    ) -> Result<Self> {
        let reply =
            tokio::time::timeout(input::TIMEOUT, control.transport().detach(method, payload))
                .await
                .context("Rex batch write timed out")??;
        Ok(Self {
            key,
            reply,
            deadline: Box::pin(tokio::time::sleep(reply_budget)),
        })
    }
}

/// Only the winning select branch removes an entry. A reply timeout abandons its receiver;
/// losing the socket, a malformed reply, or failure to answer a prompt ends the connection.
async fn next<K>(
    pending: &mut Vec<Pending<K>>,
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    events: &mut VecDeque<rexrs::Event>,
) -> Result<(K, Option<String>)> {
    loop {
        tokio::select! {
            biased;
            (index, reply) = std::future::poll_fn(|cx| {
                for (i, p) in pending.iter_mut().enumerate() {
                    if let Poll::Ready(reply) = Pin::new(&mut p.reply).poll(cx) {
                        return Poll::Ready((i, Some(reply)));
                    }
                    if p.deadline.as_mut().poll(cx).is_ready() {
                        return Poll::Ready((i, None));
                    }
                }
                Poll::Pending
            }) => return Ok((pending.swap_remove(index).key, reply.transpose()?)),
            event = control.next_event() => {
                let event = event?;
                if prompt::PromptState::is_event(&event) {
                    state.prompt_event(control, shared, event).await?;
                } else {
                    events.push_back(event);
                }
            }
        }
    }
}

// Lifecycle events are applied by the host loop. Do not publish stale metadata in between.
fn invalidated(
    state: &ControlState,
    events: &VecDeque<rexrs::Event>,
    session: &str,
    block: &str,
) -> bool {
    events.iter().any(|e| {
        state.includes(e)
            && e.session_id
                .as_deref()
                .or_else(|| e.payload["session_id"].as_str())
                == Some(session)
            && match e.name.as_str() {
                "session_destroyed" => true,
                "block_closed" => e.payload["block_id"] == block,
                "event_dropped" => match e.payload["event_type"].as_str() {
                    Some("session_created" | "session_moved" | "session_label_changed") => false,
                    Some("block_event") => e.payload["block_event_name"] != "ports_changed",
                    _ => true,
                },
                _ => false,
            }
    })
}

pub(super) async fn query_activity(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    events: &mut VecDeque<rexrs::Event>,
) -> Result<()> {
    let (session, blocks) = {
        let m = shared.model.lock().expect("host model");
        let (Some(session), Some(view)) = (m.selected.clone(), m.view()) else {
            return Ok(());
        };
        (
            session,
            view_blocks(view)
                .filter(|b| !state.queried.contains(b))
                .collect::<Vec<_>>(),
        )
    };
    for chunk in blocks.chunks(BLOCKS) {
        let mut pending = Vec::new();
        for block in chunk {
            state.queried.insert(block.clone());
            let target = json!({"session_id":session,"block_id":block,"args":{}});
            for name in ["program_status", "title", "process"] {
                if name == "program_status" && !state.status_queried.insert(block.clone()) {
                    continue;
                }
                let method = format!("{}.{name}", activity::TERMINAL);
                let budget = if name == "process" {
                    Duration::from_secs(1)
                } else {
                    input::TIMEOUT
                };
                pending
                    .push(Pending::send(control, (block, name), &method, &target, budget).await?);
            }
        }
        while !pending.is_empty() {
            let ((block, name), text) = next(&mut pending, control, shared, state, events).await?;
            let reply = match text
                .as_deref()
                .map(transport::detached_result::<Value>)
                .transpose()
            {
                Err(rexrs::Error::Api(_)) => None,
                result => result?,
            };
            if invalidated(state, events, &session, block) {
                continue;
            }
            match (name, reply) {
                ("program_status", reply) => {
                    status_sweep::record(shared, state, &session, block, reply, &HashSet::new())
                }
                ("title", Some(v)) => {
                    shared.block_event(&session, block.clone(), activity::parse_title(&v))
                }
                ("process", Some(v)) => shared.update(|m| {
                    m.apply_block_event(&session, block.clone(), activity::parse_process(&v))
                }),
                _ => {}
            }
        }
    }
    Ok(())
}

pub(super) async fn declare_themes(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    workers: &HashMap<String, PaneWorker>,
    theme: Option<&SetTheme>,
    events: &mut VecDeque<rexrs::Event>,
) -> Result<()> {
    if state.theme_set_supported == Some(false) {
        return Ok(());
    }
    let due: Vec<_> = {
        let m = shared.model.lock().expect("host model");
        workers
            .iter()
            .filter_map(|(block, worker)| {
                let last = state.themed.get(block);
                let wanted = match theme {
                    Some(theme) => theme.clone(),
                    None if last.is_some_and(|t| *t != SetTheme::new()) => SetTheme::new(),
                    None => return None,
                };
                (last != Some(&wanted) && m.themable(&worker.session, block)).then_some((
                    &worker.session,
                    block,
                    wanted,
                ))
            })
            .collect()
    };
    let mut due = due.into_iter();
    while due.len() > 0 {
        let mut pending = Vec::new();
        // Until a probe succeeds, send only one at a time, including after an inconclusive reply.
        let limit = if state.theme_set_supported == Some(true) {
            BLOCKS
        } else {
            1
        };
        for (session, block, wanted) in due.by_ref().take(limit) {
            let request = wanted.clone().on(session.clone(), block.clone());
            pending.push(
                Pending::send(
                    control,
                    (session, block, wanted),
                    SetTheme::NAME,
                    request,
                    input::TIMEOUT,
                )
                .await?,
            );
        }
        while !pending.is_empty() {
            let ((session, block, wanted), text) =
                next(&mut pending, control, shared, state, events).await?;
            match text
                .as_deref()
                .map(transport::detached_result::<serde_json::Map<String, Value>>)
                .transpose()
            {
                Ok(Some(_)) => {
                    state.theme_set_supported = Some(true);
                    shared.update(|m| m.theme_declared = true);
                }
                Err(rexrs::Error::Api(
                    rexrs::ApiError::MethodNotFound { .. } | rexrs::ApiError::NotSupported { .. },
                )) => {
                    state.theme_set_supported = Some(false);
                    shared.update(|m| {
                        m.themes_unsupported = true;
                        m.notice = Some("This Rex server doesn't take terminal themes; its panes keep the server's colours.".into());
                    });
                    return Ok(());
                }
                Ok(None) | Err(rexrs::Error::Api(_)) => {}
                Err(error) => return Err(error.into()),
            }
            // Remember the attempt so a slow or vanished block isn't retried on every event.
            if !invalidated(state, events, session, block) {
                state.themed.insert(block.clone(), wanted);
            }
        }
    }
    Ok(())
}
