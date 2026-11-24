//! API 19 adds `focus_history.reverse` with minimal and full responses.
use rexrs::api::{
    Method,
    focus_history::{Reverse, ReverseResponse},
};
use serde_json::json;

#[test]
fn focus_history_reverse_decodes_minimal_and_full_responses() {
    assert_eq!(Reverse::NAME, "focus_history.reverse");
    assert_eq!(serde_json::to_value(Reverse::new()).unwrap(), json!({}));

    let request = Reverse::new()
        .session_id("session:1")
        .not_window_id("window:1")
        .cursor(4);
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "session_id": "session:1",
            "not_window_id": "window:1",
            "cursor": 4
        })
    );

    let none: ReverseResponse = serde_json::from_value(json!({"reversed": false})).unwrap();
    assert!(!none.reversed);
    assert_eq!(none.cursor, None);
    assert_eq!(none.session_id, None);
    assert_eq!(none.window_id, None);
    assert_eq!(none.block_id, None);
    assert_eq!(
        serde_json::to_value(&none).unwrap(),
        json!({"reversed": false})
    );

    let full_json = json!({
        "reversed": true,
        "cursor": 7,
        "session_id": "session:1",
        "window_id": "window:2",
        "block_id": "block:3"
    });
    let full: ReverseResponse = serde_json::from_value(full_json.clone()).unwrap();
    assert!(full.reversed);
    assert_eq!(full.cursor, Some(7));
    assert_eq!(full.session_id.as_deref(), Some("session:1"));
    assert_eq!(full.window_id.as_deref(), Some("window:2"));
    assert_eq!(full.block_id.as_deref(), Some("block:3"));
    assert_eq!(serde_json::to_value(&full).unwrap(), full_json);
}
