//! Production-worker half of scripts/prompt-contract.py. No UI or saved workspace.
use rex_rune::connection::{
    HostCommand, HostConnection, HostModel,
    prompt::{PromptAnswer, PromptEventKind, PromptKind, ReplyOutcome},
    wait_for_hosts,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn wait<T>(
    host: &HostConnection,
    state: impl Fn(&mut HostModel) -> Option<T>,
) -> anyhow::Result<T> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(value) = state(&mut host.model.lock().expect("host model")) {
            return Ok(value);
        }
        host.wake.recv_deadline(deadline)?;
    }
}

fn ask(host: &HostConnection, generation: u64) -> anyhow::Result<()> {
    host.commands.send(HostCommand::Action {
        generation,
        request: Box::new(rexrs::api::client::Action::new("ask_input").origin("key")),
    })?;
    Ok(())
}

fn completed(host: &HostConnection, generation: u64) -> anyhow::Result<()> {
    wait(host, |m| {
        (!m.client_actions.is_empty()).then(|| {
            let queued = m.client_actions.remove(0);
            assert!(m.connected);
            assert_eq!(queued.generation, generation);
            assert_eq!(queued.action.action, "probe.completed");
        })
    })
}

fn main() -> anyhow::Result<()> {
    let root =
        PathBuf::from(std::env::args_os().nth(1).expect("private fixture root")).canonicalize()?;
    anyhow::ensure!(
        root.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rex-prompt-contract-"))
            && root.join("probe-fixture").is_file(),
        "only the prompt contract's private scratch socket is accepted"
    );
    let host = HostConnection::spawn(
        format!("unix://{}", root.join("rex.sock").display()),
        None,
        None,
        None,
        false,
        None,
    );
    wait(&host, |m| {
        (m.connected && m.protocol.api_revision == Some(21)).then_some(())
    })?;
    // The shipped behavior must also release a same-client action promptly.
    ask(&host, 41)?;
    completed(&host, 41)?;
    assert!(host.model.lock().unwrap().prompt_events.is_empty());

    host.commands.send(HostCommand::Prompts(true))?;
    ask(&host, 42)?;
    let (epoch, call_id) = wait(&host, |m| {
        assert!(
            m.client_actions.is_empty(),
            "the action completed before the answer"
        );
        m.prompt_events.pop_front().map(|event| {
            let PromptEventKind::Requested(prompt) = event.kind else {
                panic!("expected a prompt offer");
            };
            assert_eq!(prompt.action.as_deref(), Some("ask_input"));
            assert_eq!(prompt.kind, PromptKind::Input);
            assert_eq!(prompt.timeout_ms, 5000);
            (event.epoch, prompt.call_id.clone())
        })
    })?;
    host.commands.send(HostCommand::PromptReply {
        epoch,
        call_id: call_id.clone(),
        answer: PromptAnswer::Text("contract answer".into()),
    })?;
    wait(&host, |m| {
        m.prompt_events.pop_front().map(|event| {
            assert_eq!(event.epoch, epoch);
            assert_eq!(
                event.kind,
                PromptEventKind::ReplyOutcome {
                    call_id: call_id.clone(),
                    outcome: ReplyOutcome::Accepted,
                }
            );
        })
    })?;
    completed(&host, 42)?;
    drop(host);
    anyhow::ensure!(
        wait_for_hosts(Duration::from_secs(3)),
        "worker did not shut down"
    );
    println!("Rex Rune refused by default and answered when enabled; both actions completed");
    Ok(())
}
