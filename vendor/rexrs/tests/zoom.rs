//! API 21 adds `tiled_zoomed_block_id` on `session::Window` without breaking older views.
use rexrs::api::session::Window;
use serde_json::json;

#[test]
fn tiled_zoomed_block_id_absent_null_and_set() {
    let absent: Window = serde_json::from_value(json!({
        "window_id": "window:1",
        "focused_block_id": "block:a",
        "zoomed_block_id": "block:a",
        "layers": []
    }))
    .unwrap();
    assert_eq!(absent.zoomed_block_id.as_deref(), Some("block:a"));
    assert_eq!(absent.tiled_zoomed_block_id, None);
    let encoded = serde_json::to_value(&absent).unwrap();
    assert!(encoded.get("tiled_zoomed_block_id").is_none());

    let null_value: Window = serde_json::from_value(json!({
        "window_id": "window:1",
        "zoomed_block_id": null,
        "tiled_zoomed_block_id": null,
        "layers": []
    }))
    .unwrap();
    assert_eq!(null_value.zoomed_block_id, None);
    assert_eq!(null_value.tiled_zoomed_block_id, None);

    let dual: Window = serde_json::from_value(json!({
        "window_id": "window:1",
        "focused_block_id": "block:float",
        "zoomed_block_id": null,
        "tiled_zoomed_block_id": "block:tiled",
        "layers": []
    }))
    .unwrap();
    assert_eq!(dual.focused_block_id.as_deref(), Some("block:float"));
    assert_eq!(dual.zoomed_block_id, None);
    assert_eq!(dual.tiled_zoomed_block_id.as_deref(), Some("block:tiled"));

    let both: Window = serde_json::from_value(json!({
        "window_id": "window:1",
        "focused_block_id": "block:tiled",
        "zoomed_block_id": "block:tiled",
        "tiled_zoomed_block_id": "block:tiled",
        "layers": []
    }))
    .unwrap();
    assert_eq!(both.zoomed_block_id.as_deref(), Some("block:tiled"));
    assert_eq!(both.tiled_zoomed_block_id.as_deref(), Some("block:tiled"));
    let roundtrip: Window = serde_json::from_value(serde_json::to_value(&both).unwrap()).unwrap();
    assert_eq!(both, roundtrip);
}
