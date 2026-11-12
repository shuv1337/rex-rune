use super::{worker::*, *};
use rex_linux::connection::activity::ProgramState::*;

fn note(window: &str, kind: Kind) -> Note {
    Note {
        summary: window.into(),
        body: String::new(),
        kind,
        target: Some(NoteTarget {
            host: HostId(1),
            session: "session".into(),
            window: Some(window.into()),
        }),
    }
}

#[test]
fn dbus_missing_service_errors_are_not_found() {
    for error in [
        zbus::fdo::Error::ServiceUnknown("not installed".into()),
        zbus::fdo::Error::NameHasNoOwner("not running".into()),
    ] {
        assert_eq!(failure(error.into()), Service::NotFound);
    }
    assert_eq!(
        failure(zbus::fdo::Error::Failed("private daemon details".into()).into()),
        Service::Unreachable("Service rejected the request".into())
    );
}

#[test]
fn repeats_replace_but_hosts_sessions_and_tabs_are_isolated() {
    let mut out = Outbox::default();
    let first = note("one", Kind::Activity(Blocked));
    out.execute(Command::Send(first.clone()));
    out.execute(Command::Send(first.clone()));
    assert_eq!(out.sent[1].1, out.sent[0].2);
    let mut other_host = first.clone();
    other_host.target.as_mut().unwrap().host = HostId(2);
    let mut other_session = first.clone();
    other_session.target.as_mut().unwrap().session = "other".into();
    for n in [
        note("two", Kind::Activity(Blocked)),
        other_host,
        other_session,
    ] {
        out.execute(Command::Send(n));
        assert_eq!(out.sent.last().unwrap().1, 0);
    }
    out.execute(Command::Seen(first.target.unwrap()));
    assert_eq!(out.closed, [out.sent[0].2]);
    for (_, _, id) in &out.sent[2..] {
        assert!(out.ledger.target(*id).is_some());
    }
}

#[test]
fn program_messages_are_unreplaced_but_close_with_their_tab() {
    let mut out = Outbox::default();
    let n = note("one", Kind::Activity(Blocked));
    out.execute(Command::Send(n.clone()));
    for _ in 0..2 {
        out.execute(Command::Send(note("one", Kind::Program)));
    }
    out.execute(Command::Send(n.clone()));
    assert_eq!(
        out.sent
            .iter()
            .map(|(_, replaces, _)| *replaces)
            .collect::<Vec<_>>(),
        [0, 0, 0, 1]
    );
    out.execute(Command::Seen(n.target.clone().unwrap()));
    out.closed.sort_unstable();
    assert_eq!(out.closed, [1, 2, 3]);
    out.execute(Command::Seen(n.target.unwrap()));
    assert_eq!(out.closed, [1, 2, 3], "no double close");
}

#[test]
fn failed_notify_drops_once_and_a_later_note_succeeds() {
    let mut out = Outbox {
        send_failure: Some(Service::Unreachable("Service rejected the request".into())),
        ..Default::default()
    };
    out.execute(Command::Send(note("failed", Kind::Activity(Blocked))));
    assert!(matches!(out.ledger.health.service, Service::Unreachable(_)));
    out.execute(Command::Send(note("later", Kind::Activity(Blocked))));
    assert_eq!(
        out.attempts
            .iter()
            .map(|n| n.summary.as_str())
            .collect::<Vec<_>>(),
        ["failed", "later"]
    );
    assert_eq!(out.sent.len(), 1);
    assert_eq!(out.sent[0].0.summary, "later");
    assert_eq!(out.sent[0].1, 0);
    assert_eq!(out.ledger.health.service, Service::Available);
}

#[test]
fn missing_service_and_lost_bus_recover_only_on_a_later_command() {
    for failure in [
        Service::NotFound,
        Service::Unreachable("Session bus disconnected".into()),
    ] {
        let mut out = Outbox {
            failure: Some(failure.clone()),
            ..Default::default()
        };
        assert_eq!(out.ledger.health, Health::default());
        out.execute(Command::Test);
        assert_eq!(out.ledger.health.service, failure);
        assert_eq!(
            out.ledger.health.test,
            TestResult::Failed(failure.reason().into())
        );
        assert!(out.attempts.is_empty());
        out.execute(Command::Check);
        assert_eq!(out.ledger.health.service, Service::Available);
        assert!(out.attempts.is_empty(), "checks never send or replay");
        out.execute(Command::Test);
        assert_eq!(out.ledger.health.test, TestResult::Sent);
        assert_eq!(out.attempts, [Note::test()]);
        assert_eq!(out.ledger.target(out.sent[0].2), None);
    }
}

#[test]
fn urgency_is_critical_only_for_blocked_and_error() {
    for state in [Idle, Working, Blocked, Error, Done] {
        assert_eq!(
            Kind::Activity(state).urgency(),
            if matches!(state, Blocked | Error) {
                2
            } else {
                1
            }
        );
    }
    assert_eq!(Kind::Program.urgency(), 1);
    assert_eq!(Kind::Test.urgency(), 1);
}

#[test]
fn dismissed_and_restarted_daemon_ids_are_not_reused_or_closed() {
    let mut out = Outbox::default();
    let n = note("one", Kind::Activity(Blocked));
    out.execute(Command::Send(n.clone()));
    out.ledger.closed(1);
    out.execute(Command::Send(n.clone()));
    assert_eq!(out.sent[1].1, 0);
    out.owner = Some("new-daemon".into());
    out.execute(Command::Seen(n.target.clone().unwrap()));
    assert!(out.closed.is_empty());
    out.execute(Command::Send(n));
    assert_eq!(out.sent[2].1, 0);
}

#[test]
fn failed_replacement_keeps_the_previous_live_id() {
    let mut out = Outbox::default();
    let n = note("one", Kind::Activity(Blocked));
    out.execute(Command::Send(n.clone()));
    out.send_failure = Some(Service::Unreachable("Service rejected the request".into()));
    out.execute(Command::Send(n.clone()));
    out.execute(Command::Seen(n.target.unwrap()));
    assert_eq!(out.closed, [1]);
}
