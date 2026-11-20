use super::{
    panel::PanelKind,
    theme::*,
    widgets::{Pill, pill},
    workspace::Workspace,
};
use rex_rune::ssh::{
    KeyProblem, Problem, SetupOffer, Target,
    hostkey::{self, Offer},
};
use rune::{El, Tx, col, row, text};

#[derive(Default)]
pub(super) struct Check {
    serial: u64,
    phase: Phase,
}

#[derive(Default)]
enum Phase {
    #[default]
    Scanning,
    Offer(Offer),
    Saving,
    Changed(Option<String>),
    Failed(String),
    Saved(&'static str),
}

impl Workspace {
    pub(super) fn can_check_key(&self, host: usize) -> Option<KeyProblem> {
        if self.ssh_offer(host) != Some(SetupOffer::Key) {
            return None;
        }
        match self.hosts.get(host)?.model.ssh_problem.as_ref()? {
            Problem::UnknownKey => Some(KeyProblem::Unknown),
            Problem::ChangedKey => Some(KeyProblem::Changed),
            Problem::NoServer(_) => None,
        }
    }

    pub(super) fn preview_host_key(&mut self, host: usize) {
        let Some(problem) = self.can_check_key(host) else {
            return;
        };
        let saved = &self.settings.hosts[host];
        let endpoint = saved.endpoint.clone();
        let Ok(Some(target)) = Target::parse(&endpoint) else {
            return;
        };
        let title = match problem {
            KeyProblem::Unknown => format!("Trust the key for {}?", saved.label),
            KeyProblem::Changed => format!("The key for {} changed", saved.label),
        };
        let serial = self.host_key.serial + 1;
        self.host_key = Check {
            serial,
            phase: Phase::Scanning,
        };
        self.open_panel(
            PanelKind::HostKey {
                endpoint: endpoint.clone(),
            },
            &title,
            &[],
        );
        if problem == KeyProblem::Changed {
            self.host_key.phase = Phase::Changed(None);
            self.key_work(serial, endpoint, async move {
                Ok(Phase::Changed(hostkey::known_host_name(&target).await.ok()))
            });
            return;
        }
        self.key_work(serial, endpoint, async move {
            hostkey::scan(&target).await.map(Phase::Offer)
        });
    }

    fn trust_host_key(&mut self, serial: u64, endpoint: &str) {
        if !self.key_panel(serial, endpoint)
            || !self.settings.hosts.iter().enumerate().any(|(host, saved)| {
                saved.endpoint == endpoint && self.can_check_key(host) == Some(KeyProblem::Unknown)
            })
        {
            return;
        }
        let Phase::Offer(offer) = &self.host_key.phase else {
            return;
        };
        let offer = offer.clone();
        self.host_key.phase = Phase::Saving;
        self.key_work(serial, endpoint.into(), async move {
            hostkey::trust(&offer).await?;
            Ok(Phase::Saved(""))
        });
    }

    fn key_panel(&self, serial: u64, endpoint: &str) -> bool {
        serial == self.host_key.serial
            && self.panel.as_ref().is_some_and(
                |p| matches!(&p.kind, PanelKind::HostKey { endpoint: e } if e == endpoint),
            )
    }

    fn key_work(
        &mut self,
        serial: u64,
        endpoint: String,
        work: impl Future<Output = anyhow::Result<Phase>> + Send + 'static,
    ) {
        #[cfg(test)]
        {
            drop(work);
            // No subprocesses in a headless dialog test.
            assert!(self.key_panel(serial, &endpoint));
        }
        #[cfg(not(test))]
        {
            let handle = self.handle.clone();
            let callback_endpoint = endpoint.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("rex-ssh-key".into())
                .spawn(move || {
                    let result = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(Into::into)
                        .and_then(|r| r.block_on(work));
                    handle.update(move |s, _| s.key_done(serial, &callback_endpoint, result));
                })
            {
                self.key_done(serial, &endpoint, Err(error.into()));
            }
        }
    }

    fn key_done(&mut self, serial: u64, endpoint: &str, result: anyhow::Result<Phase>) {
        if serial != self.host_key.serial {
            return;
        }
        let phase = match result {
            Ok(Phase::Saved(_)) => Phase::Saved(self.reconnect_ssh_if_needed(endpoint)),
            Ok(phase) => phase,
            Err(e) if e.downcast_ref::<KeyProblem>() == Some(&KeyProblem::Changed) => {
                if self.key_panel(serial, endpoint)
                    && let Some(panel) = &mut self.panel
                {
                    panel.title = self
                        .settings
                        .hosts
                        .iter()
                        .find(|h| h.endpoint == endpoint)
                        .map_or_else(
                            || "The host key changed".into(),
                            |h| format!("The key for {} changed", h.label),
                        );
                }
                if e.chain().count() > 1 {
                    Phase::Failed(format!("{e:#}"))
                } else {
                    Phase::Changed(None)
                }
            }
            Err(e) => Phase::Failed(format!("{e:#}")),
        };
        self.host_key.phase = phase;
    }
}

#[cfg(test)]
impl Workspace {
    pub(super) fn finish_key_check_for_test(&mut self, endpoint: &str, result: anyhow::Result<()>) {
        self.key_done(
            self.host_key.serial,
            endpoint,
            result.map(|()| Phase::Saved("")),
        );
    }
}

pub(super) fn dialog(s: &Workspace, endpoint: &str) -> El {
    let panel = s.panel.as_ref().expect("host-key panel");
    let (body, cancel) = match &s.host_key.phase {
        Phase::Scanning => ("Reading the host key without signing in…".into(), "Not now"),
        Phase::Saving => (
            "Saving the key and checking it with OpenSSH…".into(),
            "Close",
        ),
        Phase::Saved(check) => (format!("Host key trusted. {check}"), "Close"),
        Phase::Failed(reason) => (reason.clone(), "Close"),
        Phase::Changed(name) => {
            let mut reason = "Rex Rune will not connect. If the machine was reinstalled, check the new key independently and remove the old one yourself.".to_owned();
            if let Some(name) = name {
                reason.push_str(&format!(
                    " For example: ssh-keygen -R {}",
                    rex_rune::ssh::setup::quote_arg(name)
                ));
            }
            (reason, "Close")
        }
        Phase::Offer(offer) => (
            format!(
                "This computer does not trust this machine's key yet. It presented:\n\n{} {}\n\nCompare this with the key on the machine itself (ssh-keygen -lf /etc/ssh/ssh_host_{}_key.pub). If they differ, someone may be in between: choose Not now.\n\nTrusting adds this key to {}. A different key later is refused.",
                offer.key_type,
                offer.fingerprint,
                offer.key_type.to_ascii_lowercase(),
                offer.file.display()
            ),
            "Not now",
        ),
    };
    let mut actions = row().gap(8.).justify_end().child(
        pill("skip-host-key", cancel, Pill::Neutral, 32., |s, tx| {
            s.cancel_panel(tx)
        })
        .default_button()
        .tab_index(1),
    );
    if matches!(s.host_key.phase, Phase::Offer(_)) {
        let (serial, endpoint) = (s.host_key.serial, endpoint.to_owned());
        actions = actions.child(
            pill(
                "trust-host-key",
                "Trust this key",
                Pill::Primary,
                32.,
                move |s: &mut Workspace, _: &mut Tx| s.trust_host_key(serial, &endpoint),
            )
            .tab_index(2),
        );
    }
    col()
        .gap(12.)
        .min_h(0.)
        .shrink()
        .child(
            text(&panel.title)
                .font(strong(17.))
                .color(rgb(ink()))
                .wrap(),
        )
        .child(text(endpoint).font(ui(12.)).color(rgb(muted())).wrap())
        .child(
            col()
                .min_h(0.)
                .shrink()
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(text(body).font(ui(12.)).color(rgb(ink())).wrap()),
        )
        .child(actions)
}
