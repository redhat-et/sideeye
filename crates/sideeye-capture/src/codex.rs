//! Codex CLI rollout adapter.
//!
//! Codex writes one JSONL rollout per session. This adapter keeps user and
//! assistant messages, drops developer/system scaffolding and instruction
//! dumps, and reads cumulative token usage from `token_count` events.

use std::{collections::BTreeSet, fs, path::Path};

use serde_json::Value;
use sideeye_core::{GenerationUsage, Role, Turn};

use crate::{CaptureError, CapturedSession};

const INSTRUCTION_MARKERS: &[&str] = &["# AGENTS.md", "<INSTRUCTIONS>", "<permissions"];

pub fn parse_codex_rollout(path: &Path) -> Result<CapturedSession, CaptureError> {
    let text = fs::read_to_string(path)
        .map_err(|error| CaptureError::Io(format!("{}: {error}", path.display())))?;
    let fallback_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("codex-session");
    parse_codex_jsonl(&text, fallback_id)
}

pub fn parse_codex_jsonl(text: &str, fallback_id: &str) -> Result<CapturedSession, CaptureError> {
    let mut session_id = fallback_id.to_owned();
    let mut model = None;
    let mut models = BTreeSet::new();
    let mut turns = Vec::new();
    let mut usage = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(object) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
        let payload = object.get("payload").unwrap_or(&Value::Null);
        match kind {
            "session_meta" => {
                if let Some(value) = payload.get("session_id").and_then(Value::as_str) {
                    session_id = value.to_owned();
                }
            }
            "turn_context" => {
                if let Some(value) = payload.get("model").and_then(Value::as_str) {
                    model = Some(value.to_owned());
                    models.insert(value.to_owned());
                }
            }
            "response_item" if payload.get("type").and_then(Value::as_str) == Some("message") => {
                let Some(role) = payload.get("role").and_then(Value::as_str) else {
                    continue;
                };
                if role != "user" && role != "assistant" {
                    continue;
                }
                let content = message_text(payload.get("content").unwrap_or(&Value::Null));
                if content.trim().is_empty() {
                    continue;
                }
                if role == "user" && looks_like_instruction_dump(&content) {
                    continue;
                }
                turns.push(Turn {
                    role: if role == "user" {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    text: content,
                });
            }
            "event_msg" if payload.get("type").and_then(Value::as_str) == Some("token_count") => {
                let info = payload.get("info").unwrap_or(&Value::Null);
                if let Some(total) = info.get("total_token_usage") {
                    usage = Some(GenerationUsage {
                        input_tokens: total.get("input_tokens").and_then(Value::as_u64),
                        output_tokens: total.get("output_tokens").and_then(Value::as_u64),
                        total_tokens: total.get("total_tokens").and_then(Value::as_u64),
                        cache_read_tokens: total.get("cached_input_tokens").and_then(Value::as_u64),
                        cache_write_tokens: None,
                    });
                }
            }
            _ => {}
        }
    }

    if turns.is_empty() {
        return Err(CaptureError::NoTask);
    }
    Ok(CapturedSession {
        session_id,
        source: "codex_rollout".to_owned(),
        client: "codex".to_owned(),
        generator_model: model,
        turns,
        generator_models: models.into_iter().collect(),
        generation_usage: usage,
        touched_files: Vec::new(),
    })
}

fn message_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn looks_like_instruction_dump(text: &str) -> bool {
    let head = text.trim_start().chars().take(64).collect::<String>();
    let window = text.chars().take(200).collect::<String>();
    INSTRUCTION_MARKERS
        .iter()
        .any(|marker| head.starts_with(marker) || window.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/sample_rollout.jsonl");

    #[test]
    fn keeps_real_messages_and_drops_scaffolding() {
        let session = parse_codex_jsonl(FIXTURE, "fallback").expect("session");
        assert_eq!(session.session_id, "test-session-123");
        assert_eq!(session.source, "codex_rollout");
        assert_eq!(session.client, "codex");
        assert_eq!(session.generator_model.as_deref(), Some("glm-5-2-fp8"));
        assert_eq!(
            session
                .turns
                .iter()
                .map(|turn| &turn.role)
                .collect::<Vec<_>>(),
            vec![&Role::User, &Role::Assistant]
        );
        assert_eq!(session.turns[0].text, "write a Go function to add two ints");
        let usage = session.generation_usage.expect("usage");
        assert_eq!(usage.input_tokens, Some(1000));
        assert_eq!(usage.output_tokens, Some(200));
        assert_eq!(usage.total_tokens, Some(1250));
    }

    #[test]
    fn empty_rollout_fails_closed() {
        assert!(matches!(
            parse_codex_jsonl(
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
                "s"
            ),
            Err(CaptureError::NoTask)
        ));
    }
}
