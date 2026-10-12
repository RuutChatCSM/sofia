//! Provider-specific replay envelope kept in opaque reasoning history.
use serde_json::Value;

pub const ANTHROPIC_THINKING_PREFIX: &str = "sofia:anthropic-thinking:v1:";

pub fn decode_anthropic_thinking(value: &str) -> Option<Vec<Value>> {
    let blocks: Vec<Value> =
        serde_json::from_str(value.strip_prefix(ANTHROPIC_THINKING_PREFIX)?).ok()?;
    blocks
        .iter()
        .all(|block| match block["type"].as_str() {
            Some("thinking") => {
                block["thinking"].is_string()
                    && block["signature"]
                        .as_str()
                        .is_some_and(|signature| !signature.is_empty())
            }
            Some("redacted_thinking") => block["data"].is_string(),
            _ => false,
        })
        .then_some(blocks)
}
