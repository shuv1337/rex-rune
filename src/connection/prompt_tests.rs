use super::*;

fn payload(prompt: Value) -> Value {
    json!({"call_id":"a", "timeout_ms":5000, "prompt":prompt})
}

fn parsed(payload: &Value) -> Option<Prompt> {
    parse(payload, "a", Instant::now())
}

#[test]
fn sanitizes_control_bidi_and_formatting_characters_and_collapses_whitespace() {
    let controls = "\x00\x1b\x7f\u{0085}\u{061c}\u{200b}\u{200e}\u{200f}\
                    \u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2060}\u{2066}\u{2067}\
                    \u{2068}\u{2069}\u{feff}\u{fff9}\u{fffa}\u{fffb}\
                    \u{e0000}\u{e0020}\u{e007f}";
    let s = format!(" \tA{controls} \n\r\u{2028}\u{2029} \x1b[32mB\t C  ");
    assert_eq!(perform::sanitize_text(&s, 100), "A [32mB C");
    assert_eq!(perform::sanitize_text("abc   xy", 4), "abc");
    assert_eq!(perform::sanitize_text("abc   xy", 5), "abc x");
    assert_eq!(perform::sanitize_text("abc", 0), "");
}

#[test]
fn title_and_confirm_fall_back_and_display_labels_cap_by_character() {
    let mut input = payload(json!({
        "kind":"input", "title":"\x1b\u{202e} \n", "placeholder":"界".repeat(81),
        "value":"界".repeat(4096), "confirm":" \tCaNCeL\u{200b}\u{feff}\n"
    }));
    input["action"] = json!("界".repeat(61));
    let prompt = parsed(&input).unwrap();
    assert_eq!(prompt.action.unwrap(), "界".repeat(60));
    assert_eq!(prompt.title, "Question");
    assert_eq!(prompt.placeholder.unwrap(), "界".repeat(80));
    assert_eq!(prompt.value.unwrap(), "界".repeat(4096));
    assert_eq!(prompt.confirm, None);
    input["prompt"]["title"] = json!("界".repeat(121));
    input["prompt"]["confirm"] = json!("界".repeat(25));
    let prompt = parsed(&input).unwrap();
    assert_eq!(prompt.title, "界".repeat(120));
    assert_eq!(prompt.confirm.unwrap(), "界".repeat(24));
    input["prompt"]["confirm"] = json!("\u{2060} ");
    assert_eq!(parsed(&input).unwrap().confirm, None);
    input["prompt"]["value"] = json!(" ".repeat(4097));
    assert!(
        parsed(&input).is_none(),
        "the raw value must fit before cleaning"
    );
}

#[test]
fn item_bounds_opaque_ids_and_multiple_excludes_allow_other() {
    let opaque = format!("\u{202e}\x1b{}", "é".repeat(510));
    assert_eq!(opaque.len(), 1024);
    let item = json!({
        "id": opaque, "title": "界".repeat(201), "subtitle": "界".repeat(201),
        "detail": "界".repeat(33), "selected": true, "icon": "future_icon"
    });
    let mut req = payload(json!({
        "kind": "pick", "title": "Choice", "multiple":true, "allow_other":true,
        "items":[item, {"id":"plain", "title":"Second", "icon":"branch"}]
    }));
    let PromptKind::Pick {
        multiple,
        allow_other,
        items,
    } = parsed(&req).unwrap().kind
    else {
        panic!("expected pick");
    };
    assert!(multiple && !allow_other);
    assert_eq!(items[0].id, opaque);
    assert_eq!(items[0].title, "界".repeat(200));
    assert_eq!(
        items[0].subtitle.as_deref(),
        Some("界".repeat(200).as_str())
    );
    assert_eq!(items[0].detail.as_deref(), Some("界".repeat(32).as_str()));
    assert_eq!(items[0].icon, Some(PromptIcon::Generic));
    assert_eq!(items[1].icon, Some(PromptIcon::Branch));
    assert!(items[0].selected && !items[1].selected);
    assert_eq!(
        PromptAnswer::Ids(vec![items[0].id.clone(), items[1].id.clone()]).result(),
        json!({"ids":[opaque, "plain"]})
    );

    req["prompt"]["multiple"] = false.into();
    let PromptKind::Pick {
        multiple,
        allow_other,
        items,
    } = parsed(&req).unwrap().kind
    else {
        panic!("expected pick");
    };
    assert!(!multiple && allow_other && !items[0].selected);
    for invalid in [String::new(), "plain".to_owned(), format!("{opaque}a")] {
        req["prompt"]["items"][0]["id"] = json!(invalid);
        assert!(parsed(&req).is_none());
    }
}

#[test]
fn item_cap_is_a_refusal_not_truncation_and_empty_items_are_allowed() {
    let mut req = payload(json!({"kind":"pick", "title":"Choice", "items":null}));
    let empty = PromptKind::Pick {
        multiple: false,
        allow_other: false,
        items: vec![],
    };
    assert_eq!(parsed(&req).unwrap().kind, empty);
    req["prompt"].as_object_mut().unwrap().remove("items");
    assert_eq!(parsed(&req).unwrap().kind, empty);
    let mut items: Vec<_> = (0..2000)
        .map(|i| json!({"id":i.to_string(),"title":"Option"}))
        .collect();
    req["prompt"]["items"] = json!(items);
    let PromptKind::Pick {
        items: parsed_items,
        ..
    } = parsed(&req).unwrap().kind
    else {
        panic!("expected pick");
    };
    assert_eq!(parsed_items.len(), 2000);
    items.push(json!({"id":"extra", "title":"Extra"}));
    req["prompt"]["items"] = json!(items);
    assert!(parsed(&req).is_none());
}

#[test]
fn validates_kind_title_and_integer_timeout() {
    let mut req = payload(json!({"kind":"input", "title":"Input"}));
    let start = Instant::now();
    for (wire, effective) in [
        (json!(-500), 1_000),
        (json!(999), 1_000),
        (json!(11_000), 11_000),
        (json!(u64::MAX), 3_660_000),
    ] {
        req["timeout_ms"] = wire;
        let prompt = parse(&req, "a", start).unwrap();
        assert_eq!(prompt.timeout_ms, effective);
        assert_eq!(prompt.deadline, start + Duration::from_millis(effective));
    }
    for invalid in [json!(null), json!("5000"), json!(5000.0)] {
        req["timeout_ms"] = invalid;
        assert!(parsed(&req).is_none());
    }
    req["timeout_ms"] = 5000.into();
    req["prompt"]["kind"] = "future_prompt".into();
    assert!(parsed(&req).is_none());
    req["prompt"] = json!({"kind":"input"});
    assert!(parsed(&req).is_none());
}

#[test]
fn answer_payload_and_debug_redaction() {
    let answer = PromptAnswer::Text("secret 界".into());
    assert_eq!(format!("{answer:?}"), "Text { bytes: 10, chars: 8 }");
    assert_eq!(answer.result(), json!({"text":"secret 界"}));
    assert_eq!(
        PromptAnswer::Text(String::new()).result(),
        json!({"text":""})
    );
    assert_eq!(PromptAnswer::Ids(vec![]).result(), json!({"ids":[]}));
    assert_eq!(PromptAnswer::Cancelled.result(), json!({"cancelled":true}));
}

#[test]
fn recognizes_every_named_icon_and_defaults_unknown_to_generic() {
    for (name, icon) in [
        ("document", PromptIcon::Document),
        ("folder", PromptIcon::Folder),
        ("branch", PromptIcon::Branch),
        ("terminal", PromptIcon::Terminal),
        ("code", PromptIcon::Code),
        ("server", PromptIcon::Server),
        ("database", PromptIcon::Database),
        ("package", PromptIcon::Package),
        ("globe", PromptIcon::Globe),
        ("link", PromptIcon::Link),
        ("person", PromptIcon::Person),
        ("tag", PromptIcon::Tag),
        ("clock", PromptIcon::Clock),
        ("star", PromptIcon::Star),
        ("gear", PromptIcon::Gear),
        ("plus", PromptIcon::Plus),
        ("future", PromptIcon::Generic),
    ] {
        assert_eq!(
            serde_json::from_value::<PromptIcon>(json!(name)).unwrap(),
            icon
        );
    }
}

#[test]
fn prompt_event_queue_is_bounded_and_keeps_order_and_epoch() {
    let (wake, _) = flume::bounded(1);
    let shared = Shared {
        model: Arc::default(),
        wake: crate::connection::title_wake::HostWake::new(wake),
    };
    for epoch in 0..=MAX_EVENTS as u64 {
        emit(&shared, epoch, PromptEventKind::Dropped);
    }
    let model = shared.model.lock().unwrap();
    assert_eq!(model.prompt_events.len(), MAX_EVENTS);
    assert_eq!(
        model
            .prompt_events
            .iter()
            .map(|event| event.epoch)
            .collect::<Vec<_>>(),
        (1..=MAX_EVENTS as u64).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn in_flight_reply_survives_the_offer_deadline() {
    let (wake, _) = flume::bounded(1);
    let shared = Shared {
        model: Arc::default(),
        wake: crate::connection::title_wake::HostWake::new(wake),
    };
    let req = payload(json!({"kind":"input", "title":"Answer"}));
    let prompt = Arc::new(parse(&req, "a", Instant::now() - Duration::from_secs(6)).unwrap());
    for (wire, expected) in [
        (
            json!({"payload":{}}),
            PromptEventKind::ReplyOutcome {
                call_id: "a".into(),
                outcome: ReplyOutcome::Accepted,
            },
        ),
        (
            json!({"error":{"code":"bad_request","message":"Try again"}}),
            PromptEventKind::ReplyRejected {
                call_id: "a".into(),
                message: "Try again".into(),
            },
        ),
    ] {
        let (send, response) = oneshot::channel();
        let mut state = PromptState {
            live: Some(Live {
                epoch: 7,
                prompt: prompt.clone(),
                reply: Some(InFlight {
                    response,
                    deadline: tokio::time::Instant::now() + REPLY_TIMEOUT,
                }),
            }),
        };
        state.expire(&shared);
        assert!(shared.model.lock().unwrap().prompt_events.is_empty());
        assert!(state.live.as_ref().unwrap().reply.is_some());
        send.send(wire.to_string()).unwrap();
        let result = state.finished().await;
        state.record(&shared, result);
        assert_eq!(
            shared.model.lock().unwrap().prompt_events.pop_front(),
            Some(PromptEvent {
                epoch: 7,
                kind: expected,
            })
        );
        if let Some(live) = &state.live {
            assert!(live.reply.is_none());
            state.expire(&shared);
            assert_eq!(
                shared.model.lock().unwrap().prompt_events.pop_front(),
                Some(PromptEvent {
                    epoch: 7,
                    kind: PromptEventKind::Ended {
                        call_id: "a".into(),
                        reason: EndReason::Timeout
                    },
                })
            );
        }
        assert!(state.live.is_none());
    }
}
