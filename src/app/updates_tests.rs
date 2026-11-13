use super::*;
use crate::app::updates::Installation;
use rex_rune::update::{InstallChannel, UpdateState};

#[test]
fn updater_check_and_install_are_mutually_exclusive() {
    let mut ws = workspace();
    ws.updates.channel = InstallChannel::Tarball;
    ws.updates.state.latest_version = Some("99.0.0".into());
    ws.updates.checking = true;
    ws.install_update_now();
    assert!(matches!(ws.updates.installation, Installation::Idle));
    ws.updates.checking = false;
    ws.updates.installation = Installation::Working("Downloading…".into());
    ws.check_updates_now();
    assert!(!ws.updates.checking);
    ws.updates.installation = Installation::Ready("/nonexistent/rex-rune".into());
    ws.check_updates_now();
    assert!(!ws.updates.checking);
    let _ = std::fs::remove_file(&ws.settings_path);
}

#[test]
fn updater_stale_workspace_notice_actions_never_lower_verified_version() {
    use rex_rune::update::CheckOutcome;
    let mut ws = workspace();
    let temp = tempfile::tempdir().unwrap();
    ws.updates.config.state_path = temp.path().join("state.json");
    let old = UpdateState {
        highest_verified: Some("0.8.0".into()),
        latest_version: Some("0.8.0".into()),
        ..UpdateState::default()
    };
    let newer = UpdateState {
        highest_verified: Some("99.0.0".into()),
        latest_version: Some("99.0.0".into()),
        ..UpdateState::default()
    };
    newer.save(&ws.updates.config.state_path).unwrap();
    ws.updates.state = old.clone();
    ws.dismiss_update_version("0.8.0");
    assert_eq!(
        UpdateState::load(&ws.updates.config.state_path).highest_verified,
        newer.highest_verified
    );
    ws.finish_update_check(
        true,
        Ok((
            CheckOutcome::UpdateAvailable {
                version: "0.8.0".into(),
                notes_url: String::new(),
                published: String::new(),
            },
            old,
        )),
    );
    let saved = UpdateState::load(&ws.updates.config.state_path);
    assert_eq!(saved.highest_verified, newer.highest_verified);
    assert_eq!(saved.latest_version, newer.latest_version);
    assert_eq!(saved.dismissed_version, None);
    assert_eq!(ws.updates.state, saved);
    let _ = std::fs::remove_file(&ws.settings_path);
}

#[test]
fn updater_restart_requests_normal_shutdown_instead_of_exec_in_callback() {
    let mut ws = workspace();
    ws.updates.channel = InstallChannel::Tarball;
    let binary = std::path::PathBuf::from("/nonexistent/rex-rune");
    ws.updates.installation = Installation::Ready(binary.clone());
    let restart = ws.restart.clone();
    ws.open_updates_settings();
    let mut rt = runtime(ws);
    for _ in 0..60 {
        rt.frame(1. / 60.);
    }
    click(&mut rt, "updates-restart");
    assert_eq!(*restart.lock().unwrap(), Some(binary));
    assert!(
        !rt.state
            .notices
            .iter()
            .any(|notice| notice.text.contains("Restart failed"))
    );
    let _ = std::fs::remove_file(&rt.state.settings_path);
}

#[test]
fn updater_controls_are_channel_gated_and_show_progress_error_and_restart() {
    let mut ws = workspace();
    ws.updates.channel = InstallChannel::Tarball;
    ws.updates.state = UpdateState {
        latest_version: Some("99.0.0".into()),
        notes_url: Some("https://github.com/shuv1337/rex-rune/releases/tag/v99.0.0".into()),
        ..UpdateState::default()
    };
    ws.open_updates_settings();
    let mut rt = runtime(ws);
    for _ in 0..60 {
        rt.frame(1. / 60.);
    }
    assert!(has(&rt, "updates-install-now"));
    assert!(has(&rt, "updates-copy-command"));
    assert!(!rt.state.settings.updates.install_automatically);
    click(&mut rt, "updates-install-auto-switch");
    assert!(
        Settings::load(&rt.state.settings_path)
            .unwrap()
            .updates
            .install_automatically
    );
    rt.update(|s, _| {
        s.updates.installation = Installation::Working("Downloading update… 42%".into())
    });
    rt.frame(1. / 60.);
    assert!(!has(&rt, "updates-install-now"));
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.text.as_deref().is_some_and(|t| t.contains("42%")))
    );
    rt.update(|s, _| {
        s.updates.installation =
            Installation::Failed("tarball SHA-256 differs from signed manifest".into())
    });
    rt.frame(1. / 60.);
    assert!(has(&rt, "updates-install-now"));
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.text.as_deref().is_some_and(|t| t.contains("SHA-256")))
    );
    rt.update(|s, _| s.updates.installation = Installation::Ready("/nonexistent/rex-rune".into()));
    rt.frame(1. / 60.);
    assert!(has(&rt, "updates-restart"));
    assert!(!has(&rt, "updates-install-now"));
    // Do not re-exec in a headless test. A second click cannot start a new install.
    rt.state.install_update_now();
    assert!(matches!(
        rt.state.updates.installation,
        Installation::Ready(_)
    ));
    rt.update(|s, _| {
        s.updates.installation = Installation::Idle;
        s.updates.state.unsupported_floor = Some("98.0.0".into());
    });
    rt.frame(1. / 60.);
    assert!(!has(&rt, "updates-install-now"));
    for channel in [
        InstallChannel::Source,
        InstallChannel::Nix,
        InstallChannel::Unmanaged,
        InstallChannel::Deb { custom_hint: None },
        InstallChannel::Pacman {
            omarchy: false,
            custom_hint: None,
        },
    ] {
        rt.update(|s, _| {
            s.updates.channel = channel;
            s.updates.state.unsupported_floor = None;
        });
        rt.frame(1. / 60.);
        assert!(!has(&rt, "updates-install-now"));
        assert!(!has(&rt, "updates-install-auto-switch"));
        rt.state.install_update_now();
        assert!(matches!(rt.state.updates.installation, Installation::Idle));
    }
    let _ = std::fs::remove_file(&rt.state.settings_path);
}

/// Run alone with REX_TEST_SNAP_DIR for real-font captures; optionally REX_UPDATE_MINIMUM=1.
#[test]
fn updater_settings_render_install_states() {
    let fonts = || {
        let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
        rune::text::TextSystem::new(fonts.files, fonts.families)
    };
    let mut camera = super::pane_verb_tests::Camera::new();
    if camera.is_some() {
        drop(fonts());
    }
    let mut ws = workspace();
    ws.updates.channel = InstallChannel::Tarball;
    ws.updates.state = UpdateState {
        latest_version: Some("99.0.0".into()),
        notes_url: Some("https://github.com/shuv1337/rex-rune/releases/tag/v99.0.0".into()),
        ..UpdateState::default()
    };
    ws.open_updates_settings();
    let size = if std::env::var_os("REX_UPDATE_MINIMUM").is_some() {
        (640., 480.)
    } else {
        (1024., 700.)
    };
    let mut rt = if camera.is_some() {
        Runtime::with_text(ws, view, size, Env::new(true, 2.), fonts())
    } else {
        Runtime::new(ws, view, size, Env::new(true, 2.))
    };
    for (name, state) in [
        ("updater-available", Installation::Idle),
        (
            "updater-progress",
            Installation::Working("Downloading update… 42%".into()),
        ),
        (
            "updater-failed",
            Installation::Failed("tarball SHA-256 differs from signed manifest".into()),
        ),
        (
            "updater-ready",
            Installation::Ready("/nonexistent/rex-rune".into()),
        ),
    ] {
        rt.update(|s, _| s.updates.installation = state);
        for _ in 0..60 {
            rt.frame(1. / 60.);
        }
        if let Some(camera) = &mut camera {
            camera.snap(&mut rt, name);
        }
    }
}

/// A pacman install with its package's helper. The paths do not exist, so the worker a
/// click starts cannot run anything: nothing in this suite starts `pkexec`.
fn pacman_workspace(omarchy: bool) -> Workspace {
    use rex_rune::update::pacman::Helper;
    let mut ws = workspace();
    ws.updates.channel = InstallChannel::Pacman {
        omarchy,
        custom_hint: None,
    };
    ws.updates.pacman = Some(Helper {
        pkexec: "/nonexistent/pkexec".into(),
        helper: "/nonexistent/rex-rune-update".into(),
    });
    ws.updates.state = UpdateState {
        latest_version: Some("99.0.0".into()),
        notes_url: Some("https://github.com/shuv1337/rex-rune/releases/tag/v99.0.0".into()),
        ..UpdateState::default()
    };
    ws
}

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..60 {
        rt.frame(1. / 60.);
    }
}

fn page_says(rt: &Runtime<Workspace>, words: &str) -> bool {
    rt.elements()
        .iter()
        .any(|e| e.text.as_deref().is_some_and(|t| t.contains(words)))
}

#[test]
fn a_pacman_install_offers_update_now_only_with_its_own_helper() {
    let _families = rune::text::with_families("Inter", "Inter");
    for omarchy in [false, true] {
        let mut ws = pacman_workspace(omarchy);
        ws.open_updates_settings();
        let mut rt = runtime(ws);
        settle(&mut rt);
        assert!(has(&rt, "updates-install-now"));
        // Says what the button will do, and the command is still there to copy.
        assert!(page_says(&rt, "asks for an administrator's password"));
        assert!(page_says(&rt, "this one package"));
        assert!(has(&rt, "updates-copy-command"));
        assert_eq!(
            page_says(&rt, "`omarchy update` is the recommended way"),
            omarchy
        );
        assert!(page_says(
            &rt,
            if omarchy {
                "omarchy update"
            } else {
                "sudo pacman -Syu rex-rune-bin"
            }
        ));
        // Installing on its own is for install.sh installs: it would open a password
        // dialog nobody asked for.
        assert!(!has(&rt, "updates-install-auto-switch"));

        // No helper (a package from before it, or no polkit): instructions only, as before.
        rt.update(|s, _| s.updates.pacman = None);
        settle(&mut rt);
        assert!(!has(&rt, "updates-install-now"));
        assert!(!page_says(&rt, "administrator's password"));
        assert!(has(&rt, "updates-copy-command"));
        rt.state.install_update_now();
        assert!(matches!(rt.state.updates.installation, Installation::Idle));
        let _ = std::fs::remove_file(&rt.state.settings_path);
    }

    // Someone else's package says how to update it; the helper is not theirs to use, and
    // an install.sh install never goes near it.
    for channel in [
        InstallChannel::Pacman {
            omarchy: false,
            custom_hint: Some("yay -Syu rex-rune".into()),
        },
        InstallChannel::Deb { custom_hint: None },
        InstallChannel::Nix,
        InstallChannel::Source,
        InstallChannel::Unmanaged,
    ] {
        let mut ws = pacman_workspace(false);
        ws.updates.channel = channel.clone();
        assert!(ws.updates.pacman_helper().is_none(), "{channel:?}");
        ws.open_updates_settings();
        let mut rt = runtime(ws);
        settle(&mut rt);
        assert!(!has(&rt, "updates-install-now"), "{channel:?}");
        rt.state.install_update_now();
        assert!(matches!(rt.state.updates.installation, Installation::Idle));
        let _ = std::fs::remove_file(&rt.state.settings_path);
    }
    let mut ws = pacman_workspace(false);
    ws.updates.channel = InstallChannel::Tarball;
    assert!(ws.updates.pacman_helper().is_none());
    let _ = std::fs::remove_file(&ws.settings_path);
}

#[test]
fn a_pacman_update_reports_each_ending_and_restarts_only_after_an_upgrade() {
    use rex_rune::update::pacman::{Outcome, Refusal};
    let _families = rune::text::with_families("Inter", "Inter");
    let mut ws = pacman_workspace(true);
    ws.open_updates_settings();
    let mut rt = runtime(ws);
    settle(&mut rt);

    // The click says what is being waited for; a second one starts nothing more.
    click(&mut rt, "updates-install-now");
    settle(&mut rt);
    assert!(matches!(
        rt.state.updates.installation,
        Installation::Working(_)
    ));
    assert!(page_says(&rt, "Waiting for the administrator's password"));
    assert!(!has(&rt, "updates-install-now"));
    assert!(!has(&rt, "updates-restart"));
    rt.state.check_updates_now();
    assert!(!rt.state.updates.checking, "no check during an update");

    // The dialog was closed: not a failure, nothing to restart, the button is back.
    rt.update(|s, _| s.finish_pacman_update(Outcome::Dismissed, "/nonexistent/rex-rune".into()));
    settle(&mut rt);
    assert!(matches!(rt.state.updates.installation, Installation::Idle));
    assert!(has(&rt, "updates-install-now"));
    assert!(!has(&rt, "updates-restart"));
    assert!(!page_says(&rt, "Update failed"));
    assert!(
        rt.state
            .notices
            .iter()
            .any(|n| n.text.contains("Update cancelled"))
    );

    // Every refusal says why, never offers Restart, and leaves the command to copy.
    for (refusal, words) in [
        (Refusal::NotAuthorized, "administrator's password"),
        (Refusal::NotNewer, "already this version or newer"),
        (Refusal::HeldBack, "IgnorePkg"),
        (Refusal::Busy, "another pacman is running"),
        (Refusal::Download, "pkg.shuv.dev"),
        (Refusal::Signature, "not signed by the Rex package key"),
        (Refusal::Untrusted, "pacman keyring does not trust"),
        (Refusal::WrongPackage, "something other than the package"),
        (Refusal::OtherPackages, "other packages"),
        (Refusal::Pacman, "journalctl -t rex-rune-update"),
        (Refusal::Unsupported, "systemd"),
        (Refusal::NoPkexec, "pkexec could not be started"),
        (Refusal::Unexpected(Some(1)), "exit code 1"),
        (Refusal::Unexpected(None), "did not finish"),
    ] {
        rt.update(|s, _| {
            s.finish_pacman_update(Outcome::Refused(refusal), "/nonexistent/rex-rune".into())
        });
        settle(&mut rt);
        assert!(
            matches!(&rt.state.updates.installation, Installation::Failed(e) if e.contains(words)),
            "{refusal:?}"
        );
        assert!(page_says(&rt, "Update failed") && page_says(&rt, words));
        assert!(page_says(&rt, "You can retry or use Copy command"));
        assert!(!has(&rt, "updates-restart"), "{refusal:?}");
        assert!(has(&rt, "updates-install-now") && has(&rt, "updates-copy-command"));
    }

    // An upgrade: Restart to finish, with the path this client was started from.
    let restart = rt.state.restart.clone();
    rt.update(|s, _| s.finish_pacman_update(Outcome::Installed, "/nonexistent/rex-rune".into()));
    settle(&mut rt);
    assert!(has(&rt, "updates-restart"));
    assert!(!has(&rt, "updates-install-now"));
    rt.state.install_update_now();
    assert!(matches!(
        rt.state.updates.installation,
        Installation::Ready(_)
    ));
    click(&mut rt, "updates-restart");
    assert_eq!(
        *restart.lock().unwrap(),
        Some(std::path::PathBuf::from("/nonexistent/rex-rune"))
    );
    let _ = std::fs::remove_file(&rt.state.settings_path);
}

#[test]
fn a_pacman_install_is_never_updated_without_being_asked() {
    use rex_rune::update::CheckOutcome;
    let mut ws = pacman_workspace(false);
    let temp = tempfile::tempdir().unwrap();
    ws.updates.config.state_path = temp.path().join("state.json");
    // The preference can be left on from an install.sh install of the same account.
    ws.settings.updates.check = true;
    ws.settings.updates.install_automatically = true;
    let state = ws.updates.state.clone();
    state.save(&ws.updates.config.state_path).unwrap();
    ws.finish_update_check(
        false,
        Ok((
            CheckOutcome::UpdateAvailable {
                version: "99.0.0".into(),
                notes_url: String::new(),
                published: String::new(),
            },
            state,
        )),
    );
    assert!(ws.updates.available().is_some());
    assert!(
        matches!(ws.updates.installation, Installation::Idle),
        "a check must not open a password dialog"
    );
    let _ = std::fs::remove_file(&ws.settings_path);
}

#[test]
fn the_update_notice_points_a_pacman_install_at_settings_when_it_can_update() {
    let ws = pacman_workspace(false);
    assert_eq!(
        ws.updates.guidance("99.0.0").summary,
        "Rex Rune 99.0.0 is available. Open Settings → Updates to install it."
    );
    let omarchy = pacman_workspace(true);
    let guidance = omarchy.updates.guidance("99.0.0");
    assert_eq!(
        guidance.summary,
        "Rex Rune 99.0.0 is available. Run `omarchy update`, or open Settings → Updates to install it."
    );
    assert_eq!(guidance.command.as_deref(), Some("omarchy update"));
    // Without the helper the words are the channel's own.
    let mut plain = pacman_workspace(true);
    plain.updates.pacman = None;
    assert_eq!(
        plain.updates.guidance("99.0.0"),
        plain.updates.channel.guidance("99.0.0")
    );
    for ws in [ws, omarchy, plain] {
        let _ = std::fs::remove_file(&ws.settings_path);
    }
}

/// Run alone with REX_TEST_SNAP_DIR for real-font captures; optionally REX_UPDATE_MINIMUM=1.
#[test]
fn pacman_update_draw() {
    use rex_rune::update::pacman::{Outcome, Refusal};
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let fonts = crate::app::fonts::load("IBM Plex Mono", &[]);
    let mut ws = pacman_workspace(true);
    ws.open_updates_settings();
    let size = if std::env::var_os("REX_UPDATE_MINIMUM").is_some() {
        (640., 480.)
    } else {
        (1024., 700.)
    };
    let mut rt = Runtime::with_text(
        ws,
        view,
        size,
        Env::new(true, 1.),
        rune::text::TextSystem::new(fonts.files, fonts.families),
    );
    settle(&mut rt);
    camera.snap(&mut rt, "pacman-update-available");
    click(&mut rt, "updates-install-now");
    settle(&mut rt);
    camera.snap(&mut rt, "pacman-update-waiting");
    rt.update(|s, _| {
        s.finish_pacman_update(
            Outcome::Refused(Refusal::NotAuthorized),
            "/nonexistent/rex-rune".into(),
        )
    });
    settle(&mut rt);
    camera.snap(&mut rt, "pacman-update-refused");
    rt.update(|s, _| s.finish_pacman_update(Outcome::Installed, "/nonexistent/rex-rune".into()));
    settle(&mut rt);
    camera.snap(&mut rt, "pacman-update-ready");
}
