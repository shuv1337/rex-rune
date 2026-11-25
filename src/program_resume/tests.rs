use super::*;
use serde_json::{Value, json};

fn record() -> Value {
    json!({
        "owner": {"pid": 42, "name": "worker"},
        "program": "demo",
        "reported_at": "2026-11-25T22:31:01Z",
        "chosen": true,
        "cwd": "/work"
    })
}

fn parse(records: Vec<Value>) -> ResumeSnapshot {
    serde_json::from_str::<WireSnapshot>(&json!({"records": records}).to_string())
        .unwrap()
        .into_snapshot()
}

#[test]
fn trust_fails_closed() {
    for (fields, expected) in [
        (json!({}), Trust::Untrusted(Reason::Unstated)),
        (json!({"rule": "demo"}), Trust::Untrusted(Reason::Unstated)),
        (json!({"trusted": true}), Trust::Untrusted(Reason::Unstated)),
        (
            json!({"trusted": true, "rule": ""}),
            Trust::Untrusted(Reason::Unstated),
        ),
        (
            json!({"trusted": false, "rule": "demo"}),
            Trust::Untrusted(Reason::Unstated),
        ),
        (
            json!({"trusted": "true", "rule": "demo"}),
            Trust::Untrusted(Reason::Unstated),
        ),
        (
            json!({"trusted": true, "rule": []}),
            Trust::Untrusted(Reason::Unstated),
        ),
        (
            json!({"trusted": true, "rule": "demo", "untrusted_reason": "wrapped"}),
            Trust::Untrusted(Reason::Wrapped),
        ),
        (
            json!({"trusted": true, "rule": "demo", "untrusted_reason": "new_reason"}),
            Trust::Untrusted(Reason::Unrecognised),
        ),
        (
            json!({"trusted": true, "rule": "demo", "untrusted_reason": null}),
            Trust::Untrusted(Reason::Unrecognised),
        ),
        (
            json!({"trusted": true, "rule": "demo", "untrusted_reason": {"text": "bad"}}),
            Trust::Untrusted(Reason::Unrecognised),
        ),
        (
            json!({"trusted": true, "rule": "de\\mo"}),
            Trust::Trusted {
                rule: r"de\\mo".into(),
            },
        ),
    ] {
        let mut record = record();
        record
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        assert_eq!(parse(vec![record]).records[0].trust, expected);
    }
}

#[test]
fn skips_invalid_records_keeps_order_and_chooses_at_most_one() {
    let mut no_pid = record();
    no_pid["owner"] = json!({"name": "missing pid"});
    let mut no_program = record();
    no_program.as_object_mut().unwrap().remove("program");
    let mut no_time = record();
    no_time.as_object_mut().unwrap().remove("reported_at");
    let mut second = record();
    second["owner"] = json!({"pid": 43});
    second["reported_at"] = json!("bad time");
    second["unknown"] = json!(["new field"]);
    let snap = parse(vec![
        no_pid,
        no_program,
        no_time,
        Value::Null,
        record(),
        second,
    ]);
    assert_eq!(snap.records.len(), 2);
    let first = &snap.records[0];
    assert_eq!(first.key.pid, 42);
    assert_eq!(first.owner_name, "worker");
    assert_eq!(first.cwd.as_deref(), Some("/work"));
    assert!(first.reported_at.is_some() && first.chosen);
    let second = &snap.records[1];
    assert_eq!(second.key.pid, 43);
    assert_eq!(second.owner_name, "");
    assert!(second.reported_at.is_none() && !second.chosen);
    assert_ne!(first.key, second.key);
    assert!(!snap.truncated);
}

#[test]
fn caps_records_and_escaped_names_without_losing_raw_identity() {
    let mut long = record();
    for field in ["program", "rule", "cwd"] {
        long[field] = json!(format!("start{}end", "\u{202e}".repeat(130)));
    }
    long["trusted"] = json!(true);
    long["owner"]["name"] = json!("x".repeat(240));
    let mut extra = long.clone();
    extra["owner"]["name"] = json!(format!("{}y{}", "x".repeat(120), "x".repeat(119)));
    let mut snap = parse([vec![long.clone(), extra], vec![long; 20]].concat());
    assert!(snap.truncated);
    assert_eq!(snap.records.len(), MAX_RECORDS_PER_BLOCK);
    assert_ne!(snap.records[0].key, snap.records[1].key);
    assert_eq!(snap.records[0].owner_name, snap.records[1].owner_name);
    let row = &snap.records[0];
    let Trust::Trusted { rule } = &row.trust else {
        panic!("expected trusted");
    };
    for (text, cap) in [
        (row.program.as_str(), MAX_NAME),
        (&row.owner_name, MAX_NAME),
        (rule, MAX_NAME),
        (row.cwd.as_deref().unwrap(), MAX_CWD),
    ] {
        assert!(text.chars().count() <= cap && text.contains('…'));
    }
    snap.apply_host_budget(3);
    assert!(snap.truncated && snap.records.len() == 3);
}

#[test]
fn oversized_raw_key_fields_skip_the_record_and_mark_truncation() {
    let mut limit = record();
    limit["owner"]["name"] = json!("é".repeat(128));
    limit["reported_at"] = json!("t".repeat(64));
    let at_limit = parse(vec![limit.clone()]);
    assert_eq!(at_limit.records.len(), 1);
    assert!(!at_limit.truncated);

    let mut oversized_owner = limit.clone();
    oversized_owner["owner"]["name"] = json!("é".repeat(129));
    let mut oversized_time = limit.clone();
    oversized_time["reported_at"] = json!("t".repeat(65));
    for oversized in [oversized_owner, oversized_time] {
        let snap = parse(vec![oversized, limit.clone()]);
        assert_eq!(snap.records, at_limit.records);
        assert!(snap.truncated);
    }
}

#[test]
fn escape_exposes_invisibles_and_never_splits_an_escape() {
    assert_eq!(
        escape(
            "\u{0301}a\u{202E}\u{200b}\u{3164}\u{0000}\u{0085}\u{00a0}\\é",
            128
        ),
        r"\u{0301}a\u{202E}\u{200B}\u{3164}\u{0000}\u{0085}\u{00A0}\\é"
    );
    assert_eq!(
        escape(
            "\u{2028}\u{2029}\u{FE0F}\u{E0020}\u{E000}\u{0378}\u{2066}",
            128
        ),
        r"\u{2028}\u{2029}\u{FE0F}\u{E0020}\u{E000}\u{0378}\u{2066}"
    );
    assert_eq!(escape("e\u{0301}界", 3), "e\u{0301}界");
    assert_eq!(escape("left\u{202e}middle\u{200b}right", 11), "left…right");
    assert_eq!(escape("abc\\def", 6), "abc…ef");
    assert_eq!(escape("\u{202e}", 1), "…");
    assert_eq!(escape("abc", 0), "");
}

#[test]
fn escape_keeps_marks_off_visible_escapes_and_cut_markers() {
    assert_eq!(escape("\u{200B}\u{0301}", 16), r"\u{200B}\u{0301}");
    assert_eq!(escape("\\\u{0301}\u{0302}", 18), r"\\\u{0301}\u{0302}");
    let tail = format!("1234567890paddingA{}z", "\u{0301}".repeat(8));
    assert_eq!(escape(&tail, 20), r"1234567890…\u{0301}z");
    assert_eq!(
        escape("1234567890paddingA\u{0301}12345678", 20),
        "1234567890…12345678"
    );
}

#[test]
fn reason_wording_is_pinned() {
    let cases = [
        (
            "wrapped",
            "Runs inside another program, such as a sandbox, that the command alone would leave out.",
        ),
        ("no_rule", "No trust rule names this command."),
        (
            "program_differs",
            "The running program is not the one this command starts.",
        ),
        (
            "flag_not_listed",
            "Started with a flag its trust rule does not list.",
        ),
        ("flag_value", "A flag's value does not fit its trust rule."),
        (
            "flags_differ",
            "The command's flags differ from the ones the program was started with.",
        ),
        (
            "dir_not_allowed",
            "Would start outside the folders its trust rule allows.",
        ),
        (
            "launch_unreadable",
            "Rex could not read how the program was started.",
        ),
        (
            "config_incomplete",
            "The host's init.lua did not finish loading, so nothing is trusted.",
        ),
        (
            "unknown",
            "Not trusted. This version of Rex Rune does not know the reason.",
        ),
    ];
    for (wire, text) in cases {
        let mut record = record();
        record["untrusted_reason"] = json!(wire);
        let Trust::Untrusted(reason) = parse(vec![record]).records[0].trust else {
            panic!("expected untrusted");
        };
        assert_eq!(reason.explain(), text);
    }
    assert_eq!(Reason::Unstated.explain(), "Not trusted.");
}
