use super::{
    panel::PanelKind,
    theme::*,
    widgets::{Pill, pill},
    workspace::{HostId, Workspace},
};
use rex_rune::ssh::{
    Facts, Problem, SetupOffer, Target,
    install::{self, Answer, AnswerError, Event, Run},
};
use rune::{El, Tx, col, row, text};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Review,
    Checking,
    Refused(String),
    Running {
        unrecognized: bool,
    },
    Asking {
        serial: u32,
        ready: bool,
    },
    Stopping,
    Finished {
        code: Option<i32>,
        stopped: bool,
        connection: &'static str,
    },
}

impl Phase {
    fn live(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Running { .. } | Self::Asking { .. } | Self::Stopping
        )
    }

    fn running() -> Self {
        Self::Running {
            unrecognized: false,
        }
    }
}

pub(super) struct Install {
    pub(super) run: u64,
    pub(super) phase: Phase,
    host: HostId,
    endpoint: String,
    user: String,
    transcript: String,
    yes: u32,
    process: Option<Run>,
    #[cfg(test)]
    pub(super) answers: Vec<(u32, Answer)>,
}

impl Workspace {
    pub(super) fn can_install_ssh(&self, host: usize) -> bool {
        self.ssh_offer(host) == Some(SetupOffer::Install)
    }

    pub(super) fn install_is_live(&self) -> bool {
        self.ssh_install
            .as_ref()
            .is_some_and(|run| run.phase.live())
    }

    pub(super) fn preview_ssh_install(&mut self, index: usize) {
        if !self.can_install_ssh(index)
            || self.install_is_live()
            || self.ssh_setup.running.is_some()
        {
            return;
        }
        let saved = &self.settings.hosts[index];
        let endpoint = saved.endpoint.clone();
        let title = format!("Install Rex on {}?", saved.label);
        let host = self.hosts[index].id;
        let Some(Problem::NoServer(facts)) = &self.hosts[index].model.ssh_problem else {
            return;
        };
        let user = facts.user.clone();
        self.open_panel(
            PanelKind::InstallSsh {
                endpoint: endpoint.clone(),
            },
            &title,
            &[],
        );
        self.install_serial += 1;
        self.ssh_install = Some(Install {
            run: self.install_serial,
            host,
            endpoint,
            user,
            phase: Phase::Review,
            transcript: String::new(),
            yes: 0,
            process: None,
            #[cfg(test)]
            answers: Vec::new(),
        });
    }

    fn install_host_exists(&self, run: &Install) -> bool {
        self.host_index(run.host)
            .and_then(|index| self.settings.hosts.get(index))
            .is_some_and(|saved| saved.endpoint == run.endpoint)
    }

    pub(super) fn begin_install(&mut self, serial: u64) {
        let Some(run) = &self.ssh_install else {
            return;
        };
        if run.run != serial || run.phase != Phase::Review || self.ssh_setup.running.is_some() {
            return;
        }
        if !self.install_host_exists(run) {
            self.ssh_install.as_mut().unwrap().phase =
                Phase::Refused("This host is no longer saved.".into());
            return;
        }
        self.ssh_install.as_mut().unwrap().phase = Phase::Checking;
        #[cfg(not(test))]
        {
            let endpoint = self.ssh_install.as_ref().unwrap().endpoint.clone();
            let handle = self.handle.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("rex-install-check".into())
                .spawn(move || {
                    let result = (|| -> anyhow::Result<Facts> {
                        let target = Target::parse(&endpoint)?
                            .ok_or_else(|| anyhow::anyhow!("SSH endpoint required"))?;
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()?;
                        Ok(rt.block_on(install::preflight(&target))?)
                    })();
                    handle.update(move |s, _| s.install_checked(serial, result));
                })
            {
                self.install_checked(serial, Err(error.into()));
            }
        }
    }

    pub(super) fn install_checked(&mut self, serial: u64, result: anyhow::Result<Facts>) {
        let Some(run) = &self.ssh_install else {
            return;
        };
        if run.run != serial || run.phase != Phase::Checking {
            return;
        }
        let present = self.install_host_exists(run);
        let run = self.ssh_install.as_mut().unwrap();
        if !present {
            run.phase = Phase::Refused("This host is no longer saved.".into());
            return;
        }
        match result {
            Err(error) => run.phase = Phase::Refused(format!("{error:#}")),
            Ok(facts) => {
                run.user = facts.user;
                #[cfg(not(test))]
                {
                    let target = Target::parse(&run.endpoint)
                        .expect("checked SSH endpoint")
                        .unwrap();
                    let handle = self.handle.clone();
                    match Run::start(&target, move |event| {
                        handle.update(move |s, _| s.install_event(serial, event));
                    }) {
                        Ok(process) => run.process = Some(process),
                        Err(error) => {
                            run.phase = Phase::Refused(format!("{error:#}"));
                            return;
                        }
                    }
                }
                run.phase = Phase::running();
            }
        }
    }

    pub(super) fn answer_install(&mut self, run_serial: u64, serial: u32, answer: Answer) {
        let Some(run) = &self.ssh_install else {
            return;
        };
        if run.run != run_serial
            || run.phase
                != (Phase::Asking {
                    serial,
                    ready: true,
                })
        {
            return;
        }
        if !self.install_host_exists(run) {
            self.stop_install(run_serial);
            return;
        }
        let run = self.ssh_install.as_mut().unwrap();
        #[cfg(test)]
        run.answers.push((serial, answer));
        let result = run
            .process
            .as_ref()
            .map_or(Ok(()), |process| process.answer(serial, answer));
        match result {
            Ok(()) => {
                run.yes += u32::from(answer == Answer::Yes);
                run.phase = Phase::running();
            }
            Err(AnswerError::Stale | AnswerError::Inert) => {}
            Err(AnswerError::Write(error)) => {
                install::append_bounded(
                    &mut run.transcript,
                    &format!("\nCouldn't answer Rex: {error}\n"),
                );
                run.phase = Phase::Stopping;
            }
        }
    }

    pub(super) fn install_enter(&mut self, tx: &mut Tx) {
        if tx.key_repeat() {
            return;
        }
        let Some(run) = &self.ssh_install else { return };
        match run.phase {
            Phase::Asking {
                serial,
                ready: true,
            } => self.answer_install(run.run, serial, Answer::No),
            Phase::Review | Phase::Refused(_) | Phase::Finished { .. } => self.cancel_panel(tx),
            _ => {}
        }
    }

    fn dismiss_install(&mut self, serial: u64, tx: &mut Tx) {
        if self.ssh_install.as_ref().is_some_and(|run| {
            run.run == serial
                && matches!(
                    run.phase,
                    Phase::Review | Phase::Checking | Phase::Refused(_) | Phase::Finished { .. }
                )
        }) {
            self.cancel_panel(tx);
        }
    }

    pub(super) fn stop_install(&mut self, serial: u64) {
        let Some(run) = &mut self.ssh_install else {
            return;
        };
        if run.run != serial {
            return;
        }
        match run.phase {
            Phase::Checking => {
                run.phase = Phase::Refused("Install stopped before Rex was started.".into())
            }
            Phase::Running { .. } | Phase::Asking { .. } => {
                run.phase = Phase::Stopping;
                if let Some(process) = &run.process {
                    process.stop();
                }
            }
            _ => {}
        }
    }

    pub(super) fn leave_install_dialog(&mut self) {
        if let Some(run) = &self.ssh_install {
            self.stop_install(run.run);
        }
    }

    pub(super) fn stop_install_for_host(&mut self, index: usize) {
        if let Some(run) = &self.ssh_install
            && self
                .hosts
                .get(index)
                .is_some_and(|host| host.id == run.host)
        {
            self.stop_install(run.run);
        }
    }

    pub(super) fn install_event(&mut self, serial: u64, event: Event) {
        let Some(run) = &mut self.ssh_install else {
            return;
        };
        if run.run != serial
            || !matches!(
                run.phase,
                Phase::Running { .. } | Phase::Asking { .. } | Phase::Stopping
            )
        {
            return;
        }
        match event {
            Event::Output(text) => {
                install::append_bounded(&mut run.transcript, &text);
                if matches!(run.phase, Phase::Running { .. }) {
                    run.phase = Phase::running();
                }
            }
            Event::Asking { serial } if matches!(run.phase, Phase::Running { .. }) => {
                run.phase = Phase::Asking {
                    serial,
                    ready: false,
                };
            }
            Event::Ready { serial }
                if run.phase
                    == (Phase::Asking {
                        serial,
                        ready: false,
                    }) =>
            {
                run.phase = Phase::Asking {
                    serial,
                    ready: true,
                };
            }
            Event::QuestionGone if matches!(run.phase, Phase::Asking { .. }) => {
                run.phase = Phase::running()
            }
            Event::Unrecognized if matches!(run.phase, Phase::Running { .. }) => {
                run.phase = Phase::Running { unrecognized: true };
            }
            Event::Exited { code } => {
                run.process.take();
                let stopped = run.phase == Phase::Stopping;
                let endpoint = run.endpoint.clone();
                let connection = if run.yes == 0 {
                    "No steps were approved; no connection was requested."
                } else if self.install_host_exists(self.ssh_install.as_ref().unwrap()) {
                    self.reconnect_ssh_if_needed(&endpoint)
                } else {
                    "The host is no longer saved; no connection was requested."
                };
                self.ssh_install.as_mut().unwrap().phase = Phase::Finished {
                    code,
                    stopped,
                    connection,
                };
            }
            _ => {}
        }
    }
}

pub(super) fn dialog(s: &Workspace, endpoint: &str) -> El {
    let Some(run) = s
        .ssh_install
        .as_ref()
        .filter(|run| run.endpoint == endpoint)
    else {
        return col();
    };
    let heading = &s.panel.as_ref().expect("install panel").title;
    let command = Target::parse(endpoint)
        .ok()
        .flatten()
        .and_then(|t| install::command_preview(&t).ok())
        .unwrap_or_default();
    let serial = run.run;
    let paragraph = |body: String| text(body).font(ui(12.)).color(rgb(muted())).wrap();
    let mut body = col()
        .gap(12.)
        .min_h(0.)
        .shrink()
        .child(text(heading).font(strong(17.)).color(rgb(ink())).wrap())
        .child(paragraph(endpoint.into()));
    let mut actions = row().gap(8.).justify_end();
    match &run.phase {
        Phase::Review => {
            let user = &run.user;
            body = body.child(col().min_h(0.).shrink().scroll(rune::ScrollSpec { stick_bottom: false })
                .scroll_edge_fade(10.).child(paragraph(format!(
                    "No Rex was found for {user} on this Linux machine. This runs Rex's own installer from this computer and shows each command before it changes anything there. You answer each question; this app never answers for you.\n\n\
                    If you say yes to every step, the installer:\n\
                    • Puts rex in ~/.local/bin for {user}, without sudo.\n\
                    • Enables rex.socket, a systemd user unit, so the server starts when something connects, including this app.\n\
                    • Turns on lingering for {user}, so the server and its sessions survive logging out.\n\
                    • Adds this machine to the rex command's hosts on this computer.\n\n\
                    Read-only checks run without asking. It stops if ~/.config/systemd/user already has a rex.service or rex.socket that Rex did not write."
                )))).child(text(format!("On this machine: {command}")).font(ui(12.).mono()).color(rgb(ink())).wrap());
            actions = actions
                .child(copy(command, 3))
                .child(rune::el().grow(1.))
                .child(close("Not now", serial))
                .child(
                    pill(
                        "begin-install",
                        "Begin install",
                        Pill::Primary,
                        32.,
                        move |s: &mut Workspace, _| s.begin_install(serial),
                    )
                    .on_key(install_key)
                    .tab_index(2),
                );
        }
        Phase::Checking => {
            body = body.child(paragraph(
                "Checking local Rex, the SSH key and login, and what is installed on the host…"
                    .into(),
            ));
            actions = actions.child(close("Not now", serial));
        }
        Phase::Refused(reason) => {
            body = body.child(
                text(reason.clone())
                    .font(ui(12.))
                    .color(rgb(danger()))
                    .wrap(),
            );
            actions = actions.child(close("Close", serial));
        }
        phase => {
            body = body.child(
                col()
                    .key("install-transcript")
                    .h(210.)
                    .min_h(0.)
                    .shrink()
                    .scroll(rune::ScrollSpec { stick_bottom: true })
                    .scroll_edge_fade(10.)
                    .child(
                        text(&run.transcript)
                            .font(ui(12.).mono())
                            .color(rgb(ink()))
                            .wrap(),
                    ),
            );
            let message = match phase {
                Phase::Finished { code, stopped, connection } => {
                    if *stopped {
                        format!("Install stopped after {} approved step(s). A step that had already started on the host may have finished or stopped part-way. {connection}", run.yes)
                    } else if *code == Some(0) {
                        format!("The installer finished. {connection}")
                    } else {
                        let status = code.map_or_else(|| "no exit status".into(), |n| format!("status {n}"));
                        format!("The installer stopped ({status}) after {} approved step(s). Its last lines are above. {connection}", run.yes)
                    }
                }
                Phase::Stopping => "Stopping the install… A step that had already started may have finished or stopped part-way.".into(),
                Phase::Running { unrecognized: true } => "Rex seems to be waiting for something this app does not recognise. Closing this dialog stops the install.".into(),
                _ => "Closing this dialog stops the install.".into(),
            };
            body = body.child(paragraph(message));
            if matches!(phase, Phase::Finished { .. }) {
                actions = actions.child(close("Close", serial));
            } else if !matches!(phase, Phase::Stopping) {
                actions = actions.child(
                    pill(
                        "stop-install",
                        "Stop install",
                        Pill::Neutral,
                        32.,
                        move |s: &mut Workspace, _| s.stop_install(serial),
                    )
                    .on_key(install_key)
                    .tab_index(3),
                );
                if let Phase::Asking {
                    serial: step,
                    ready,
                } = *phase
                {
                    actions = actions.child(rune::el().grow(1.));
                    for (answer, key, label, style, tab) in [
                        (Answer::No, "no-install-step", "No", Pill::Neutral, 1),
                        (
                            Answer::Yes,
                            "yes-install-step",
                            "Yes, run this step",
                            Pill::Primary,
                            2,
                        ),
                    ] {
                        let mut button =
                            pill(key, label, style, 32., move |s: &mut Workspace, _| {
                                s.answer_install(serial, step, answer)
                            })
                            .on_key(install_key)
                            .tab_index(tab);
                        if !ready {
                            button = button.inert().opacity(0.45);
                        }
                        if answer == Answer::No {
                            button = button.default_button();
                        }
                        actions = actions.child(button);
                    }
                } else if matches!(phase, Phase::Running { unrecognized: true }) {
                    actions = actions.child(rune::el().grow(1.)).child(copy(command, 2));
                }
            }
        }
    }
    body.child(actions).on_key(install_key)
}

fn install_key(s: &mut Workspace, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == rune::Key::Enter && ev.mods == rune::Mods::default() {
        s.install_enter(tx);
        true
    } else {
        false
    }
}

fn close(label: &'static str, serial: u64) -> El {
    pill("close-install", label, Pill::Neutral, 32., move |s, tx| {
        s.dismiss_install(serial, tx)
    })
    .on_key(install_key)
    .default_button()
    .tab_index(1)
}

fn copy(command: String, tab: i32) -> El {
    pill(
        "copy-install-command",
        "Copy command",
        Pill::Neutral,
        32.,
        move |_: &mut Workspace, tx: &mut Tx| tx.copy(command.clone()),
    )
    .on_key(install_key)
    .tab_index(tab)
}
