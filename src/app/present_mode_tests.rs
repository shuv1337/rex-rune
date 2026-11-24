//! Present-mode selection for native top-level windows (#146).

use rune::{PresentChoice, choose_present};

#[test]
fn wayland_default_prefers_mailbox_and_never_picks_immediate() {
    let cases: &[(&[PresentChoice], PresentChoice)] = &[
        (
            &[PresentChoice::Fifo, PresentChoice::Mailbox],
            PresentChoice::Mailbox,
        ),
        (
            &[
                PresentChoice::Fifo,
                PresentChoice::Immediate,
                PresentChoice::Mailbox,
            ],
            PresentChoice::Mailbox,
        ),
        (&[PresentChoice::Fifo], PresentChoice::Vsync),
        (
            &[PresentChoice::Fifo, PresentChoice::FifoRelaxed],
            PresentChoice::Vsync,
        ),
        (
            &[PresentChoice::Fifo, PresentChoice::Immediate],
            PresentChoice::Vsync,
        ),
        (&[PresentChoice::Immediate], PresentChoice::Vsync),
        (&[], PresentChoice::Vsync),
    ];

    for &(offered, expected) in cases {
        for asked in [
            None,
            Some(""),
            Some("   "),
            Some("auto"),
            Some("AUTO"),
            Some(" Auto \t"),
        ] {
            assert_eq!(
                choose_present(true, offered, asked),
                (expected, None),
                "wayland offered={offered:?} asked={asked:?}"
            );
        }
    }
}

#[test]
fn non_wayland_default_stays_vsync_even_when_mailbox_is_offered() {
    let offered = &[
        PresentChoice::Mailbox,
        PresentChoice::Fifo,
        PresentChoice::FifoRelaxed,
        PresentChoice::Immediate,
    ];
    for asked in [None, Some(""), Some("  "), Some("auto"), Some("AuTo")] {
        assert_eq!(
            choose_present(false, offered, asked),
            (PresentChoice::Vsync, None),
            "non-wayland asked={asked:?}"
        );
    }
}

#[test]
fn valid_overrides_select_offered_mode_case_insensitively() {
    let all = &[
        PresentChoice::Fifo,
        PresentChoice::FifoRelaxed,
        PresentChoice::Mailbox,
        PresentChoice::Immediate,
    ];
    let cases = [
        ("fifo", PresentChoice::Fifo),
        ("FIFO", PresentChoice::Fifo),
        ("  fifo\n", PresentChoice::Fifo),
        ("fifo-relaxed", PresentChoice::FifoRelaxed),
        ("fifo_relaxed", PresentChoice::FifoRelaxed),
        ("FIFO-RELAXED", PresentChoice::FifoRelaxed),
        (" Fifo_Relaxed ", PresentChoice::FifoRelaxed),
        ("mailbox", PresentChoice::Mailbox),
        ("MailBox", PresentChoice::Mailbox),
        ("immediate", PresentChoice::Immediate),
        ("IMMEDIATE", PresentChoice::Immediate),
    ];

    for prefer_mailbox in [false, true] {
        for (asked, expected) in cases {
            assert_eq!(
                choose_present(prefer_mailbox, all, Some(asked)),
                (expected, None),
                "prefer_mailbox={prefer_mailbox} asked={asked:?}"
            );
        }
    }
}

#[test]
fn unsupported_overrides_fall_back_to_platform_default_with_warning() {
    let cases: &[(bool, &[PresentChoice], &str, PresentChoice, &str)] = &[
        (
            true,
            &[PresentChoice::Fifo, PresentChoice::Immediate],
            "mailbox",
            PresentChoice::Vsync,
            "mailbox",
        ),
        (
            true,
            &[PresentChoice::Fifo, PresentChoice::Mailbox],
            "immediate",
            PresentChoice::Mailbox,
            "immediate",
        ),
        (
            false,
            &[PresentChoice::Fifo, PresentChoice::Mailbox],
            "immediate",
            PresentChoice::Vsync,
            "immediate",
        ),
        (
            true,
            &[PresentChoice::Fifo, PresentChoice::Mailbox],
            "fifo_relaxed",
            PresentChoice::Mailbox,
            "fifo-relaxed",
        ),
        (false, &[], "fifo", PresentChoice::Vsync, "fifo"),
    ];

    for &(prefer_mailbox, offered, asked, expected, requested_label) in cases {
        let (choice, warning) = choose_present(prefer_mailbox, offered, Some(asked));
        assert_eq!(
            choice, expected,
            "prefer_mailbox={prefer_mailbox} offered={offered:?} asked={asked:?}"
        );
        let warning = warning.expect("unsupported mode should produce a warning");
        assert!(
            warning.contains(requested_label),
            "warning should name requested mode {requested_label:?}: {warning}"
        );
        if offered.is_empty() {
            assert!(
                warning.contains("none"),
                "empty offered list should say none: {warning}"
            );
        }
    }
}

#[test]
fn invalid_overrides_fall_back_with_length_capped_warning() {
    let offered = &[PresentChoice::Fifo, PresentChoice::Mailbox];
    for (prefer_mailbox, expected) in [
        (true, PresentChoice::Mailbox),
        (false, PresentChoice::Vsync),
    ] {
        let (choice, warning) = choose_present(prefer_mailbox, offered, Some("vsync-please"));
        assert_eq!(choice, expected);
        let warning = warning.expect("invalid mode should warn");
        assert!(warning.contains("vsync-please"));
        assert!(warning.contains("auto, fifo, fifo-relaxed, mailbox, immediate"));
    }

    let huge = "x".repeat(4096);
    let (choice, warning) = choose_present(true, offered, Some(&huge));
    assert_eq!(choice, PresentChoice::Mailbox);
    let warning = warning.expect("huge invalid value should warn");
    assert!(warning.contains("..."));
    assert!(
        warning.len() < 256,
        "warning must cap raw input length, got {} bytes: {warning}",
        warning.len()
    );

    let multibyte = "🦀".repeat(200);
    let (choice, warning) = choose_present(false, offered, Some(&multibyte));
    assert_eq!(choice, PresentChoice::Vsync);
    let warning = warning.expect("multibyte invalid value should warn");
    assert!(warning.contains("..."));
    assert!(warning.chars().count() < 256);
}

#[test]
fn selection_always_obeys_capability_and_no_auto_immediate_invariants() {
    let modes = [
        PresentChoice::Fifo,
        PresentChoice::FifoRelaxed,
        PresentChoice::Mailbox,
        PresentChoice::Immediate,
    ];
    let asked_values = [
        None,
        Some(""),
        Some("   "),
        Some("auto"),
        Some("AUTO"),
        Some("fifo"),
        Some("fifo-relaxed"),
        Some("fifo_relaxed"),
        Some("mailbox"),
        Some("immediate"),
        Some("bogus"),
    ];

    // Check every subset of concrete wgpu present modes (2^4 = 16 subsets).
    for mask in 0usize..(1 << modes.len()) {
        let offered: Vec<PresentChoice> = modes
            .iter()
            .enumerate()
            .filter_map(|(bit, &mode)| ((mask & (1 << bit)) != 0).then_some(mode))
            .collect();
        for prefer_mailbox in [false, true] {
            for asked in asked_values {
                let (choice, warning) = choose_present(prefer_mailbox, &offered, asked);
                assert!(
                    choice == PresentChoice::Vsync || offered.contains(&choice),
                    "choice {choice:?} not Vsync or in offered {offered:?} (prefer_mailbox={prefer_mailbox}, asked={asked:?})"
                );
                let is_auto = asked.is_none_or(|raw| {
                    let t = raw.trim();
                    t.is_empty() || t.eq_ignore_ascii_case("auto")
                });
                if is_auto {
                    assert_ne!(
                        choice,
                        PresentChoice::Immediate,
                        "automatic path must never select Immediate (prefer_mailbox={prefer_mailbox}, offered={offered:?}, asked={asked:?})"
                    );
                    assert!(
                        warning.is_none(),
                        "automatic path must not warn (asked={asked:?}): {warning:?}"
                    );
                }
            }
        }
    }
}
