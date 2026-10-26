//! Capture adapters that normalize harness sessions into `ReviewPacket`.
//!
//! The first adapter is OpenCode V2 session export. It is intentionally
//! provider-neutral: it keeps user intent, visible assistant claims, tool
//! inputs/results/errors, per-turn generator models, and token usage. Hidden
//! reasoning, system messages, compaction summaries, and synthetic messages are
//! excluded so Side-Eye cannot review its own injected verdict.

pub mod git_diff;

pub use git_diff::{MAX_DIFF_CHARS, collect_diff_artifacts};

use serde_json::Value;
use sideeye_core::{
    Artifact, ContractError, GenerationUsage, REVIEW_PACKET_SCHEMA, ReviewPacket, Role, Turn,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("invalid OpenCode export: {0}")]
    Invalid(String),
    #[error("review packet is invalid: {0}")]
    Contract(#[from] ContractError),
    #[error("session has no user request")]
    NoTask,
}

/// Which part of the session the judge should see.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReviewScope {
    /// All visible context in the session.
    #[default]
    Session,
    /// The last real user request and the assistant work that followed it.
    LastExchange,
}

impl ReviewScope {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "session" | "all" => Some(Self::Session),
            "last" | "last-exchange" | "last_exchange" => Some(Self::LastExchange),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::LastExchange => "last-exchange",
        }
    }
}

const SELF_REVIEW_MARKERS: &[&str] = &[
    "/sideeye",
    "sideeye ·",
    "sideeye /",
    "sideeye doctor",
    "session : opencode export",
    "escalating to ",
    "side-eye verdict",
];

/// Convert an OpenCode V2 session export into a `ReviewPacket`.
///
/// `export` may be the API response (`{"data": {...}}`) or the inner data
/// object. `artifacts` are supplied by the caller after repository/diff
/// capture; pass an empty vector for narrative-only review.
pub fn from_opencode_export(
    export: &Value,
    artifacts: Vec<Artifact>,
) -> Result<ReviewPacket, CaptureError> {
    from_opencode_export_scoped(export, artifacts, ReviewScope::Session)
}

/// Convert an OpenCode export into a packet scoped to all context or the last
/// real user exchange.
pub fn from_opencode_export_scoped(
    export: &Value,
    artifacts: Vec<Artifact>,
    scope: ReviewScope,
) -> Result<ReviewPacket, CaptureError> {
    let data = export.get("data").unwrap_or(export);
    if !data.is_object() {
        return Err(CaptureError::Invalid("data must be an object".to_owned()));
    }
    let info = data.get("info").unwrap_or(&Value::Null);
    let messages = data
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| CaptureError::Invalid("messages must be an array".to_owned()))?;

    let session_id = info
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| CaptureError::Invalid("session id is required".to_owned()))?;

    let selected: &[Value] = match scope {
        ReviewScope::Session => messages,
        ReviewScope::LastExchange => {
            let start = messages
                .iter()
                .rposition(is_real_user_request)
                .ok_or(CaptureError::NoTask)?;
            &messages[start..]
        }
    };

    let mut turns = Vec::new();
    let mut models: Vec<String> = Vec::new();
    let mut task: Option<String> = None;

    for message in selected {
        let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "user" => {
                let text = message.get("text").and_then(Value::as_str).unwrap_or("");
                if is_self_review(text) {
                    continue;
                }
                if task.is_none() && !text.trim().is_empty() {
                    task = Some(text.trim().to_owned());
                }
                turns.push(Turn {
                    role: Role::User,
                    text: text.to_owned(),
                });
            }
            "assistant" => {
                let model = message.get("model").and_then(model_name);
                if let Some(model) = &model
                    && !models.contains(model)
                {
                    models.push(model.clone());
                }
                for part in message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match part.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
                            if !is_self_review(text) {
                                turns.push(Turn {
                                    role: Role::Assistant,
                                    text: text.to_owned(),
                                });
                            }
                        }
                        Some("tool") => {
                            turns.push(Turn {
                                role: Role::Tool,
                                text: render_tool(part),
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    let task = task.ok_or(CaptureError::NoTask)?;
    let (generator_model, generation_usage, packet_id) = match scope {
        ReviewScope::Session => (
            info.get("model").and_then(model_name),
            usage(info.get("tokens")),
            format!("opencode-{session_id}"),
        ),
        // Whole-session token totals would be misleading for a single exchange.
        ReviewScope::LastExchange => (
            models.first().cloned(),
            None,
            format!("opencode-{session_id}-last-exchange"),
        ),
    };
    let packet = ReviewPacket {
        schema_version: REVIEW_PACKET_SCHEMA.to_owned(),
        packet_id,
        session_id: session_id.to_owned(),
        source: "opencode".to_owned(),
        client: "opencode".to_owned(),
        generator_model,
        task,
        turns,
        artifacts,
        generation_usage,
        generator_models: models,
    };
    packet.validate()?;
    Ok(packet)
}

fn is_real_user_request(message: &Value) -> bool {
    if message.get("type").and_then(Value::as_str) != Some("user") {
        return false;
    }
    let text = message.get("text").and_then(Value::as_str).unwrap_or("");
    !text.trim().is_empty() && !is_self_review(text)
}

fn model_name(value: &Value) -> Option<String> {
    let provider = value.get("providerID").and_then(Value::as_str);
    let id = value.get("id").and_then(Value::as_str);
    match (provider, id) {
        (Some(provider), Some(id)) => Some(format!("{provider}/{id}")),
        (None, Some(id)) => Some(id.to_owned()),
        _ => None,
    }
}

fn render_tool(part: &Value) -> String {
    let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
    let state = part.get("state").unwrap_or(&Value::Null);
    let status = state
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let input = state
        .get("input")
        .map(|value| value.to_string())
        .unwrap_or_else(|| "(none)".to_owned());
    let mut output = String::new();
    for item in state
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            output.push_str(text);
        }
    }
    if output.is_empty() {
        output.push_str("(no output)");
    }
    let error = state
        .get("error")
        .filter(|value| !value.is_null())
        .map(|value| format!("\nerror: {value}"))
        .unwrap_or_default();
    format!("{name} input:\n{input}\n{name} output (status={status}):\n{output}{error}")
}

fn usage(value: Option<&Value>) -> Option<GenerationUsage> {
    let value = value?;
    if !value.is_object() {
        return None;
    }
    let input = value.get("input").and_then(Value::as_u64);
    let output = value.get("output").and_then(Value::as_u64);
    let reasoning = value.get("reasoning").and_then(Value::as_u64);
    let cache = value.get("cache").unwrap_or(&Value::Null);
    let cache_read = cache.get("read").and_then(Value::as_u64);
    let cache_write = cache.get("write").and_then(Value::as_u64);
    if input.is_none() && output.is_none() && reasoning.is_none() {
        return None;
    }
    Some(GenerationUsage {
        input_tokens: input,
        output_tokens: Some(output.unwrap_or(0) + reasoning.unwrap_or(0)),
        total_tokens: Some(input.unwrap_or(0) + output.unwrap_or(0) + reasoning.unwrap_or(0)),
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
    })
}

fn is_self_review(text: &str) -> bool {
    let head = text.trim_start().to_lowercase();
    let head = head.chars().take(120).collect::<String>();
    SELF_REVIEW_MARKERS
        .iter()
        .any(|marker| head.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/sample_opencode_export.json"
        ))
        .expect("fixture is valid JSON")
    }

    #[test]
    fn maps_visible_turns_and_tool_evidence() {
        let packet = from_opencode_export(&fixture(), Vec::new()).expect("packet");
        assert_eq!(packet.session_id, "ses_fixture_opencode");
        assert_eq!(packet.source, "opencode");
        assert_eq!(packet.task, "Fix the parser");
        assert_eq!(packet.generator_model.as_deref(), Some("openai/gpt-test"));
        assert_eq!(packet.generator_models, vec!["openai/gpt-test"]);
        assert_eq!(
            packet
                .turns
                .iter()
                .map(|turn| &turn.role)
                .collect::<Vec<_>>(),
            vec![&Role::User, &Role::Assistant, &Role::Tool, &Role::Tool]
        );
        let evidence = packet
            .turns
            .iter()
            .map(|turn| turn.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(evidence.contains("parser.rs contents"));
        assert!(evidence.contains("pytest -q"));
        assert!(evidence.contains("test command failed"));
        assert!(!evidence.contains("hidden reasoning"));
        assert!(!evidence.contains("Side-Eye verdict"));
        assert!(!evidence.contains("/sideeye review"));
    }

    #[test]
    fn preserves_usage_including_cache() {
        let packet = from_opencode_export(&fixture(), Vec::new()).expect("packet");
        let usage = packet.generation_usage.expect("usage");
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(30));
        assert_eq!(usage.total_tokens, Some(130));
        assert_eq!(usage.cache_read_tokens, Some(900));
        assert_eq!(usage.cache_write_tokens, Some(10));
    }

    #[test]
    fn missing_user_request_fails_closed() {
        let mut export = fixture();
        export["data"]["messages"] = serde_json::json!([]);
        assert!(matches!(
            from_opencode_export(&export, Vec::new()),
            Err(CaptureError::NoTask)
        ));
    }

    #[test]
    fn last_exchange_drops_earlier_turns_and_sideeye_output() {
        let export = serde_json::json!({
            "data": {
                "info": {"id": "ses-scope", "model": {"providerID": "openai", "id": "gpt-test"},
                    "tokens": {"input": 900, "output": 100}},
                "messages": [
                    {"type": "user", "text": "first request"},
                    {"type": "assistant", "model": {"providerID": "openai", "id": "gpt-test"},
                        "content": [{"type": "text", "text": "first answer"}]},
                    {"type": "user", "text": "second request"},
                    {"type": "assistant", "model": {"providerID": "openai", "id": "gpt-test"},
                        "content": [{"type": "text", "text": "second answer"}]},
                    {"type": "user", "text": "/sideeye"},
                    {"type": "assistant", "model": {"providerID": "openai", "id": "gpt-test"},
                        "content": [{"type": "text", "text": "Side-Eye verdict: done"}]},
                    {"type": "synthetic", "text": "Side-Eye verdict"}
                ]
            }
        });
        let packet = from_opencode_export_scoped(&export, Vec::new(), ReviewScope::LastExchange)
            .expect("packet");
        assert_eq!(packet.task, "second request");
        assert_eq!(
            packet
                .turns
                .iter()
                .map(|turn| turn.text.as_str())
                .collect::<Vec<_>>(),
            vec!["second request", "second answer"]
        );
        assert_eq!(packet.generator_models, vec!["openai/gpt-test"]);
        assert!(packet.generation_usage.is_none());
        assert_eq!(packet.packet_id, "opencode-ses-scope-last-exchange");
    }

    #[test]
    fn last_exchange_uses_the_last_real_request_when_sideeye_is_last() {
        let packet = from_opencode_export_scoped(&fixture(), Vec::new(), ReviewScope::LastExchange)
            .expect("packet");
        assert_eq!(packet.task, "Fix the parser");
        assert_eq!(
            packet
                .turns
                .iter()
                .map(|turn| &turn.role)
                .collect::<Vec<_>>(),
            vec![&Role::User, &Role::Assistant, &Role::Tool, &Role::Tool]
        );
        assert!(!packet.turns.iter().any(|turn| is_self_review(&turn.text)));
    }

    #[test]
    fn scope_parser_accepts_user_facing_aliases() {
        assert_eq!(ReviewScope::parse("all"), Some(ReviewScope::Session));
        assert_eq!(ReviewScope::parse("session"), Some(ReviewScope::Session));
        assert_eq!(ReviewScope::parse("last"), Some(ReviewScope::LastExchange));
        assert_eq!(
            ReviewScope::parse("LAST-EXCHANGE"),
            Some(ReviewScope::LastExchange)
        );
        assert_eq!(ReviewScope::parse("everything"), None);
        assert_eq!(ReviewScope::Session.as_str(), "session");
        assert_eq!(ReviewScope::LastExchange.as_str(), "last-exchange");
    }
}
