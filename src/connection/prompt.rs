//! Immediate refusal for `rex.ui` prompt requests (API revision 17+).
//!
//! At API revision 17+, `rex.ui` broadcasts `client_prompt_requested` to every connected
//! `ClientKind::App` client and waits up to an hour for an answer. Rex Rune does not support
//! `rex.ui` prompts yet (#144), so it answers immediately and once with `not_supported` to remove
//! itself from the prompt without stalling scripts or interfering with other apps.
use super::{Client, perform};
use anyhow::Result;
use serde_json::Value;

const REFUSAL_BASE: &str = "Rex Rune does not support rex.ui prompts yet (#144).";

pub(super) fn refusal_message(payload: &Value) -> String {
    let action = payload["action"]
        .as_str()
        .map(|s| perform::sanitize_text(s.trim(), 60))
        .filter(|s| !s.is_empty());
    match action {
        Some(action) => {
            format!("Rex Rune does not support rex.ui prompts yet (action \"{action}\"; see #144).")
        }
        None => REFUSAL_BASE.into(),
    }
}

pub(super) async fn receive(control: &mut Client, payload: Value) -> Result<()> {
    let Some(call_id) = payload["call_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
    else {
        return Ok(());
    };
    let message = refusal_message(&payload);
    perform::reply(
        control,
        &call_id,
        &perform::Outcome::error("not_supported", message),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refusal_message_sanitizes_and_bounds_action_name() {
        assert_eq!(refusal_message(&json!({})), REFUSAL_BASE);
        assert_eq!(refusal_message(&json!({"action": "   "})), REFUSAL_BASE);
        assert_eq!(
            refusal_message(&json!({"action": "git.branch"})),
            "Rex Rune does not support rex.ui prompts yet (action \"git.branch\"; see #144)."
        );
        let hostile = format!("a\n\r\u{202e}{}", "x".repeat(80));
        let msg = refusal_message(&json!({"action": hostile}));
        assert!(!msg.contains('\n'));
        assert!(!msg.contains('\u{202e}'));
        assert!(msg.contains(&format!("\"a{}\"", "x".repeat(59))));
    }
}
