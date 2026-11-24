//! API 20 adds optional `undoable` flags on close/destroy calls and `client.undo_close`.
use rexrs::api::{
    Method, block,
    client::{UndoClose, UndoCloseResponse},
    session,
};
use serde_json::json;

#[test]
fn undoable_is_omitted_by_default_and_serialized_when_set() {
    let close_block = block::Close::new("session:1");
    let value = serde_json::to_value(&close_block).unwrap();
    assert!(value.get("undoable").is_none());
    let value = serde_json::to_value(close_block.undoable(true)).unwrap();
    assert_eq!(value["undoable"], json!(true));

    let close_window = session::CloseWindow::new("session:1").window_id("window:1");
    let value = serde_json::to_value(&close_window).unwrap();
    assert!(value.get("undoable").is_none());
    let value = serde_json::to_value(close_window.undoable(true)).unwrap();
    assert_eq!(value["undoable"], json!(true));

    let destroy = session::Destroy::new("session:1");
    let value = serde_json::to_value(&destroy).unwrap();
    assert!(value.get("undoable").is_none());
    let value = serde_json::to_value(destroy.undoable(true)).unwrap();
    assert_eq!(value["undoable"], json!(true));
}

#[test]
fn undo_close_decodes_empty_and_full_responses() {
    assert_eq!(UndoClose::NAME, "client.undo_close");
    assert_eq!(serde_json::to_value(UndoClose::new()).unwrap(), json!({}));

    let empty: UndoCloseResponse = serde_json::from_value(json!({})).unwrap();
    assert_eq!(empty.session_id, None);
    assert_eq!(empty.window_id, None);
    assert_eq!(empty.block_id, None);
    assert_eq!(serde_json::to_value(&empty).unwrap(), json!({}));

    let full_json = json!({
        "session_id": "session:1",
        "window_id": "window:2",
        "block_id": "block:3"
    });
    let full: UndoCloseResponse = serde_json::from_value(full_json.clone()).unwrap();
    assert_eq!(full.session_id.as_deref(), Some("session:1"));
    assert_eq!(full.window_id.as_deref(), Some("window:2"));
    assert_eq!(full.block_id.as_deref(), Some("block:3"));
    assert_eq!(serde_json::to_value(&full).unwrap(), full_json);
}
