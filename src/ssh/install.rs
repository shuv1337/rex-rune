//! One interactive local installer, driven only by explicit per-question answers.
use super::{Facts, KeyProblem, MissingServer, Problem, SetupOffer, Target, hostkey, setup};
use crate::platform::{local, pty::Pty};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    io,
    process::Command,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const INERT: Duration = Duration::from_millis(400);
pub const TRANSCRIPT_LIMIT: usize = 64 * 1024;
const QUIET: Duration = Duration::from_millis(250);
const UNRECOGNIZED: Duration = Duration::from_secs(20);
const STOP_GRACE: Duration = Duration::from_secs(3);

pub fn install_args(target: &Target) -> Result<Vec<String>> {
    ensure!(
        target.socket.is_none(),
        "Installing does not apply to an explicitly named socket."
    );
    Ok(vec![
        "--color".into(),
        "never".into(),
        "server".into(),
        "install".into(),
        target.endpoint(),
    ])
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver};

    #[test]
    fn install_argv_is_pinned_without_consent_or_package_flags() {
        let target = Target::parse("ssh://tester@[fd7a::1]:2022")
            .unwrap()
            .unwrap();
        let args = install_args(&target).unwrap();
        assert_eq!(
            args,
            [
                "--color",
                "never",
                "server",
                "install",
                "ssh://tester@[fd7a::1]:2022"
            ]
        );
        for bad in ["--yes", "-y", "--package", "--json"] {
            assert!(!args.iter().any(|a| a == bad));
        }
        assert!(install_args(&Target::parse("ssh://box/socket").unwrap().unwrap()).is_err());
    }

    #[test]
    fn recognises_only_a_final_rex_prompt_and_cleans_streams() {
        for (tail, expected) in [
            ("Install? [y/N] ", true),
            ("command\nInstall? [y/N] ", true),
            ("Install? [y/N]", false),
            ("Install? [y/N] \n", false),
            ("Install? [y/N] more", false),
            ("Install? [Y/n] ", false),
            ("Install? (yes/no) ", false),
            ("Password:", false),
            ("tester@box's password: [y/N] ", false),
            ("Enter passphrase for key: [y/N] ", false),
            (
                "Are you sure you want to continue connecting (yes/no/[fingerprint])?",
                false,
            ),
        ] {
            assert_eq!(waiting_for_answer(tail), expected, "{tail:?}");
        }
        let mut cleaner = Cleaner::default();
        assert_eq!(cleaner.push(b"\x1b[3"), "");
        assert_eq!(cleaner.push(b"2mRex\x1b[0m\r\n\x1b]title"), "Rex\n");
        assert_eq!(cleaner.push(b"\x1b\\next\t\x07"), "next\t");
        assert_eq!(cleaner.push(b"\xe2\x80"), "");
        assert_eq!(cleaner.push(b"\xa6"), "…");
        let mut tail = "é".repeat(TRANSCRIPT_LIMIT);
        append_bounded(&mut tail, "end");
        assert!(tail.ends_with("end") && tail.len() <= TRANSCRIPT_LIMIT);
    }

    fn shell(script: &str) -> (Run, Receiver<Event>) {
        let (send, recv) = mpsc::channel();
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        let run = Run::spawn_command(&mut cmd, move |event| {
            let _ = send.send(event);
        })
        .unwrap();
        (run, recv)
    }

    fn event(recv: &Receiver<Event>, predicate: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let event = recv
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if predicate(&event) {
                return event;
            }
        }
    }

    #[test]
    fn each_question_can_be_answered_once_and_stale_serial_cannot_answer_next() {
        let (run, recv) = shell(
            "printf 'Install? [y/N] '; read one; printf 'Enable? [y/N] '; read two; [ \"$one:$two\" = y:n ]",
        );
        event(&recv, |e| matches!(e, Event::Asking { serial: 1 }));
        assert!(matches!(
            run.answer(1, Answer::Yes),
            Err(AnswerError::Inert)
        ));
        event(&recv, |e| matches!(e, Event::Ready { serial: 1 }));
        run.answer(1, Answer::Yes).unwrap();
        assert!(matches!(
            run.answer(1, Answer::Yes),
            Err(AnswerError::Stale)
        ));
        event(&recv, |e| matches!(e, Event::Ready { serial: 2 }));
        assert!(matches!(
            run.answer(1, Answer::Yes),
            Err(AnswerError::Stale)
        ));
        run.answer(2, Answer::No).unwrap();
        assert!(matches!(
            event(&recv, |e| matches!(e, Event::Exited { .. })),
            Event::Exited { code: Some(0) }
        ));
    }

    #[test]
    fn stop_observes_term_exit_without_waiting_for_the_kill_deadline() {
        let (run, recv) =
            shell("trap 'exit 17' TERM; printf 'ready\\n'; while :; do sleep 1; done");
        event(
            &recv,
            |e| matches!(e, Event::Output(text) if text.contains("ready")),
        );
        let started = Instant::now();
        run.stop();
        let exit = event(&recv, |e| matches!(e, Event::Exited { .. }));
        assert!(matches!(exit, Event::Exited { code: Some(17) }), "{exit:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "TERM exit was not reported promptly: {:?}",
            started.elapsed()
        );
        assert!(!run.shared.lock().unwrap().state.deadline_kill_sent);
    }

    #[test]
    fn stop_kills_a_term_ignoring_installer_after_the_grace_period() {
        let (run, recv) = shell("trap '' TERM; printf 'ready\\n'; exec sleep 30");
        event(
            &recv,
            |e| matches!(e, Event::Output(text) if text.contains("ready")),
        );
        let started = Instant::now();
        run.stop();
        let exit = event(&recv, |e| matches!(e, Event::Exited { .. }));
        assert!(matches!(exit, Event::Exited { code: None }), "{exit:?}");
        assert!(
            started.elapsed() >= STOP_GRACE,
            "KILL was sent before the grace period"
        );
        assert!(run.shared.lock().unwrap().state.deadline_kill_sent);
    }

    #[test]
    fn stop_sweeps_a_term_ignoring_group_member_without_terminal_fds() {
        struct Stray {
            pid: libc::pid_t,
            group: libc::pid_t,
        }
        impl Drop for Stray {
            fn drop(&mut self) {
                // Clean up the owned fixture process even when the assertion fails.
                unsafe {
                    if libc::getpgid(self.pid) == self.group {
                        libc::kill(self.pid, libc::SIGKILL);
                    }
                }
            }
        }

        let (run, recv) = shell(
            "trap '' HUP TERM; (exec sleep 600) </dev/null >/dev/null 2>&1 & \
             pid=$!; trap 'exit 17' TERM; printf 'stray=%s %s\\n' \"$pid\" \"$$\"; wait",
        );
        let mut output = String::new();
        let stray = loop {
            if let Event::Output(text) = event(&recv, |e| matches!(e, Event::Output(_))) {
                output.push_str(&text);
            }
            if let Some((line, _)) = output.split_once('\n') {
                let (pid, group) = line
                    .strip_prefix("stray=")
                    .unwrap()
                    .split_once(' ')
                    .unwrap();
                break Stray {
                    pid: pid.parse().unwrap(),
                    group: group.parse().unwrap(),
                };
            }
        };
        assert_eq!(unsafe { libc::getpgid(stray.pid) }, stray.group);
        let started = Instant::now();
        run.stop();
        let exit = event(&recv, |e| matches!(e, Event::Exited { .. }));
        assert!(matches!(exit, Event::Exited { code: Some(17) }), "{exit:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "Stop waited for the deadline"
        );
        assert!(!run.shared.lock().unwrap().state.deadline_kill_sent);

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if unsafe { libc::kill(stray.pid, 0) } == -1 {
                break;
            }
            // The orphan may already be dead but not yet reaped by its new parent.
            let status = Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", &stray.pid.to_string()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&status.stdout);
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "stray PID {} still alive after Stop: {state}",
                stray.pid
            );
            thread::sleep(Duration::from_millis(15));
        }
    }

    #[test]
    fn stop_kills_group_and_drop_kills_without_waiting_for_a_question() {
        for stop in [true, false] {
            let (run, recv) = shell(
                "trap 'wait; exit' TERM; /bin/sh -c 'trap \"exit 0\" TERM; printf \"child ready\\n\"; while :; do sleep 1; done' & pid=$!; printf 'pid=%s\\n' \"$pid\"; wait",
            );
            let mut text = String::new();
            while !text.contains("child ready") || !text.contains("pid=") {
                if let Event::Output(output) = recv.recv_timeout(Duration::from_secs(3)).unwrap() {
                    text.push_str(&output);
                }
            }
            let pid: libc::pid_t = text
                .lines()
                .find_map(|line| line.strip_prefix("pid="))
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
            if stop {
                run.stop();
                event(&recv, |e| matches!(e, Event::Exited { .. }));
                assert_eq!(
                    unsafe { libc::kill(pid, 0) },
                    -1,
                    "TERM reached the child and parent reaped it"
                );
            } else {
                drop(run);
                // An orphan can briefly be a zombie; that is not a live process.
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    let status = Command::new("/bin/ps")
                        .args(["-o", "stat=", "-p", &pid.to_string()])
                        .output()
                        .expect("read fixture child status");
                    let state = String::from_utf8_lossy(&status.stdout);
                    if state.trim().is_empty() || state.trim().starts_with('Z') {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "grandchild {pid} is still alive: {state}"
                    );
                    thread::sleep(Duration::from_millis(15));
                }
            }
        }
    }
}

pub fn command_preview(target: &Target) -> Result<String> {
    Ok(format!(
        "rex {}",
        install_args(target)?
            .iter()
            .map(|a| setup::quote_arg(a))
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

#[derive(Debug)]
pub enum Refusal {
    LocalRex(anyhow::Error),
    Config(anyhow::Error),
    Lookup(anyhow::Error),
    Key(KeyProblem),
    NamedSocket,
    ServerPresent,
    RexPresent,
    OtherOs,
    Root,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalRex(e) => write!(
                f,
                "Local Rex preview.14 (API 12) or newer is required. {e:#}"
            ),
            Self::Config(e) | Self::Lookup(e) => write!(f, "{e:#}"),
            Self::Key(e) => write!(f, "{e}"),
            Self::NamedSocket => {
                f.write_str("This endpoint names a socket. Set up a server for it by hand.")
            }
            Self::ServerPresent => {
                f.write_str("A Rex server already answered here. Nothing needs installing.")
            }
            Self::RexPresent => f.write_str("Rex is already installed on this host."),
            Self::OtherOs => f.write_str(
                "In-app installation is for Linux hosts only. See docs/hosts.md for manual setup.",
            ),
            Self::Root => f.write_str(
                "In-app installation is for a non-root login. See docs/hosts.md for manual setup.",
            ),
        }
    }
}

impl std::error::Error for Refusal {}

pub async fn preflight(target: &Target) -> Result<Facts, Refusal> {
    local::require_ssh().map_err(Refusal::Config)?;
    if target.socket.is_some() {
        return Err(Refusal::NamedSocket);
    }
    let rex = setup::local_rex();
    let version = setup::output(rex.as_os_str(), &["version".into(), "--json".into()])
        .await
        .map_err(Refusal::LocalRex)?;
    setup::require_api12(&version, "Local").map_err(Refusal::LocalRex)?;
    hostkey::require_safe_config(target)
        .await
        .map_err(Refusal::Config)?;
    let error = match super::resolve(target).await {
        Ok(_) => return Err(Refusal::ServerPresent),
        Err(e) => e,
    };
    if let Some(key) = error.downcast_ref::<KeyProblem>() {
        return Err(Refusal::Key(*key));
    }
    let facts = error
        .downcast::<MissingServer>()
        .map_err(Refusal::Lookup)?
        .facts;
    if target.setup_offer(&Problem::NoServer(facts.clone())) == Some(SetupOffer::Install) {
        Ok(facts)
    } else if facts.rex {
        Err(Refusal::RexPresent)
    } else if facts.os != "Linux" {
        Err(Refusal::OtherOs)
    } else {
        Err(Refusal::Root)
    }
}

#[derive(Debug)]
pub enum Event {
    Output(String),
    Asking { serial: u32 },
    Ready { serial: u32 },
    QuestionGone,
    Unrecognized,
    Exited { code: Option<i32> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
}

#[derive(Debug)]
pub enum AnswerError {
    Stale,
    Inert,
    Write(io::Error),
}

#[derive(Default)]
struct State {
    question: Option<(u32, Instant)>,
    answered: bool,
    stopping: Option<Instant>,
    #[cfg(test)]
    deadline_kill_sent: bool,
    dropped: bool,
    exited: bool,
}

struct Shared {
    pty: Pty,
    state: State,
}

pub struct Run {
    shared: Arc<Mutex<Shared>>,
    worker: Option<JoinHandle<()>>,
}

impl Run {
    pub fn spawn(
        target: &Target,
        rex: &OsStr,
        events: impl Fn(Event) + Send + 'static,
    ) -> Result<Self> {
        local::require_ssh()?;
        Self::spawn_command(
            Command::new(rex)
                .args(install_args(target)?)
                .env("TERM", "dumb")
                .env("NO_COLOR", "1"),
            events,
        )
    }

    pub fn start(target: &Target, events: impl Fn(Event) + Send + 'static) -> Result<Self> {
        Self::spawn(target, setup::local_rex().as_os_str(), events)
    }

    fn spawn_command(cmd: &mut Command, events: impl Fn(Event) + Send + 'static) -> Result<Self> {
        let pty = Pty::spawn(cmd).context("Couldn't start Rex in an install terminal")?;
        let shared = Arc::new(Mutex::new(Shared {
            pty,
            state: State::default(),
        }));
        let worker_state = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("rex-install-pty".into())
            .spawn(move || relay(&worker_state, events))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub fn answer(&self, serial: u32, answer: Answer) -> Result<(), AnswerError> {
        let mut shared = self.shared.lock().expect("installer state");
        let Some((current, ready)) = shared.state.question else {
            return Err(AnswerError::Stale);
        };
        if current != serial {
            return Err(AnswerError::Stale);
        }
        if Instant::now() < ready {
            return Err(AnswerError::Inert);
        }
        shared.state.question = None;
        shared.state.answered = true;
        let line: &[u8] = match answer {
            Answer::Yes => b"y\n",
            Answer::No => b"n\n",
        };
        if let Err(e) = shared.pty.answer(line) {
            shared.stop();
            return Err(AnswerError::Write(e));
        }
        Ok(())
    }

    pub fn stop(&self) {
        self.shared.lock().expect("installer state").stop();
    }
}

impl Shared {
    fn stop(&mut self) {
        if self.state.stopping.is_none() && !self.state.exited {
            self.state.question = None;
            self.state.stopping = Some(Instant::now());
            self.pty.terminate();
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        {
            let mut shared = self.shared.lock().expect("installer state");
            shared.state.dropped = true;
            shared.pty.kill();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn relay(shared: &Mutex<Shared>, events: impl Fn(Event)) {
    let mut last_output = Instant::now();
    let mut serial = 0_u32;
    let mut unrecognized = false;
    let mut announced_ready = false;
    let mut tail = String::new();
    let mut cleaner = Cleaner::default();
    let mut bytes = [0; 8192];
    let mut terminal_closed = false;
    loop {
        let mut pending = Vec::new();
        let finished = {
            let mut shared = shared.lock().expect("installer state");
            if shared.state.dropped {
                return;
            }
            if std::mem::take(&mut shared.state.answered) {
                tail.clear();
                last_output = Instant::now();
            }
            match shared.pty.read(&mut bytes) {
                Ok(0) => terminal_closed = true,
                Ok(n) if n > 0 => {
                    last_output = Instant::now();
                    unrecognized = false;
                    if shared.state.question.take().is_some() {
                        pending.push(Event::QuestionGone);
                    }
                    let output = cleaner.push(&bytes[..n]);
                    append_bounded(&mut tail, &output);
                    if !output.is_empty() {
                        pending.push(Event::Output(output));
                    }
                }
                Err(e)
                    if !matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    pending.push(Event::Output(format!(
                        "\nCouldn't read the install terminal: {e}\n"
                    )));
                    shared.stop();
                }
                _ => {}
            }
            let now = Instant::now();
            let stopping = shared.state.stopping;
            let mut status = shared.pty.try_wait(stopping.is_some(), terminal_closed);
            if matches!(status, Ok(None))
                && stopping.is_some_and(|stop| now.duration_since(stop) >= STOP_GRACE)
            {
                shared.pty.kill();
                #[cfg(test)]
                {
                    shared.state.deadline_kill_sent = true;
                }
                status = shared.pty.try_wait(true, true);
            }
            let code = match status {
                Ok(Some(exit)) => Some(exit.code()),
                Ok(None) => None,
                Err(error) => {
                    pending.push(Event::Output(format!("\nCouldn't wait for Rex: {error}\n")));
                    shared.pty.kill();
                    Some(None)
                }
            };
            if let Some(code) = code {
                shared.state.question = None;
                shared.state.exited = true;
                // A fast child can exit with more than one read still buffered.
                while let Ok(n @ 1..) = shared.pty.read(&mut bytes) {
                    let output = cleaner.push(&bytes[..n]);
                    if !output.is_empty() {
                        pending.push(Event::Output(output));
                    }
                }
                pending.push(Event::Exited { code });
            } else if shared.state.stopping.is_none() && shared.state.question.is_none() {
                let quiet = now.duration_since(last_output);
                if quiet >= QUIET && waiting_for_answer(&tail) {
                    serial += 1;
                    announced_ready = false;
                    shared.state.question = Some((serial, now + INERT));
                    pending.push(Event::Asking { serial });
                } else if quiet >= UNRECOGNIZED && !unrecognized {
                    unrecognized = true;
                    pending.push(Event::Unrecognized);
                }
            } else if let Some((serial, ready)) = shared.state.question
                && !announced_ready
                && now >= ready
            {
                announced_ready = true;
                pending.push(Event::Ready { serial });
            }
            code.is_some()
        };
        for event in pending {
            events(event);
        }
        if finished {
            return;
        }
        thread::sleep(Duration::from_millis(15));
    }
}

pub fn append_bounded(buffer: &mut String, text: &str) {
    buffer.push_str(text);
    if buffer.len() > TRANSCRIPT_LIMIT {
        let cut = buffer.ceil_char_boundary(buffer.len() - TRANSCRIPT_LIMIT);
        buffer.drain(..cut);
    }
}

pub fn waiting_for_answer(text: &str) -> bool {
    let line = text.rsplit('\n').next().unwrap_or_default();
    line.strip_suffix(" [y/N] ").is_some_and(|prompt| {
        let lower = prompt.to_ascii_lowercase();
        !prompt.trim().is_empty()
            && !lower.contains("password:")
            && !lower.contains("passphrase")
            && !lower.contains("are you sure you want to continue connecting")
            && !lower.contains("host key verification failed")
    })
}

#[derive(Default)]
enum Escape {
    #[default]
    Plain,
    Start,
    Csi,
    String,
    StringEnd,
}

#[derive(Default)]
struct Cleaner {
    escape: Escape,
    utf8: Vec<u8>,
}

impl Cleaner {
    fn push(&mut self, bytes: &[u8]) -> String {
        for &byte in bytes {
            self.escape = match self.escape {
                Escape::Plain if byte == 27 => Escape::Start,
                Escape::Plain => {
                    if byte == b'\n' || byte == b'\t' || byte >= 32 && byte != 127 {
                        self.utf8.push(byte);
                    }
                    Escape::Plain
                }
                Escape::Start if byte == b'[' => Escape::Csi,
                Escape::Start if b"]PX^_".contains(&byte) => Escape::String,
                Escape::Start if (0x20..=0x2f).contains(&byte) => Escape::Start,
                Escape::Start => Escape::Plain,
                Escape::Csi if (0x40..=0x7e).contains(&byte) => Escape::Plain,
                Escape::Csi => Escape::Csi,
                Escape::String if byte == 7 => Escape::Plain,
                Escape::String if byte == 27 => Escape::StringEnd,
                Escape::StringEnd if byte == b'\\' => Escape::Plain,
                Escape::String | Escape::StringEnd => Escape::String,
            };
        }
        let end = match std::str::from_utf8(&self.utf8) {
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            _ => self.utf8.len(),
        };
        let output = String::from_utf8_lossy(&self.utf8[..end])
            .chars()
            .filter(|&ch| !ch.is_control() || ch == '\n' || ch == '\t')
            .collect();
        self.utf8.drain(..end);
        output
    }
}
