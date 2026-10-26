//! Claude Code JSONL transcript adapter.
//!
//! Claude Code overloads `role:user` for real input, tool results, and injected
//! context. The adapter splits those into user/tool/system turns so the judge
//! sees human intent separately from machine evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::Value;
use sideeye_core::{GenerationUsage, Role, Turn};

use crate::{CaptureError, CapturedSession, TouchedFile};

const NOISE_MARKERS: &[&str] = &[
    "<local-command-",
    "<command-name>",
    "<command-message>",
    "<command-stdout>",
    "Caveat: The messages below",
];

const SYSTEM_INJECT_MARKERS: &[&str] = &[
    "<task-notification>",
    "<system-reminder>",
    "[SYSTEM NOTIFICATION",
    "<hindsight_memories>",
    "UserPromptSubmit hook",
];

const ESCALATION_MARKERS: &[&str] = &[
    "Escalating to claude-",
    "judge cost:",
    "score=",
    "severity=",
    "claims_supported=",
    "sideeye review",
    "sideeye advise",
    "/escalate",
];

const FILE_EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

pub fn parse_claude_session(path: &Path) -> Result<CapturedSession, CaptureError> {
    let text = fs::read_to_string(path)
        .map_err(|error| CaptureError::Io(format!("{}: {error}", path.display())))?;
    let session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("claude-session")
        .to_owned();
    parse_claude_jsonl(&text, &session_id)
}

pub fn parse_claude_jsonl(text: &str, session_id: &str) -> Result<CapturedSession, CaptureError> {
    let mut turns = Vec::new();
    let mut touched: BTreeMap<String, u32> = BTreeMap::new();
    let mut models = BTreeSet::new();
    let mut model = None;
    let mut output_tokens = 0_u64;
    let mut last_input_tokens = 0_u64;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(object) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "user" && kind != "assistant" {
            continue;
        }
        let message = object.get("message").unwrap_or(&Value::Null);
        let role = message.get("role").and_then(Value::as_str);
        if role != Some("user") && role != Some("assistant") {
            continue;
        }
        let content = message.get("content").unwrap_or(&Value::Null);
        let rendered = render_content(content, Some(&mut touched));
        if rendered.trim().is_empty() {
            continue;
        }

        if role == Some("assistant") {
            turns.push(Turn {
                role: Role::Assistant,
                text: rendered,
            });
            if let Some(value) = message.get("model").and_then(Value::as_str) {
                model = Some(value.to_owned());
                models.insert(value.to_owned());
            }
            let usage = message.get("usage").unwrap_or(&Value::Null);
            output_tokens += usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if let Some(input) = usage.get("input_tokens").and_then(Value::as_u64) {
                last_input_tokens = input;
            }
            continue;
        }

        if has_tool_result(content) {
            turns.push(Turn {
                role: Role::Tool,
                text: rendered,
            });
        } else if looks_like_injected_system(&rendered) {
            turns.push(Turn {
                role: Role::System,
                text: rendered,
            });
        } else if looks_like_noise(&rendered) {
            continue;
        } else {
            turns.push(Turn {
                role: Role::User,
                text: rendered,
            });
        }
    }

    while turns
        .last()
        .is_some_and(|turn| is_escalation_output(&turn.text))
    {
        turns.pop();
    }
    if turns.is_empty() {
        return Err(CaptureError::NoTask);
    }

    let generation_usage = (output_tokens > 0 || last_input_tokens > 0).then(|| GenerationUsage {
        input_tokens: Some(last_input_tokens),
        output_tokens: Some(output_tokens),
        total_tokens: Some(last_input_tokens + output_tokens),
        cache_read_tokens: None,
        cache_write_tokens: None,
    });
    Ok(CapturedSession {
        session_id: session_id.to_owned(),
        source: "claude_code".to_owned(),
        client: "claude".to_owned(),
        generator_model: model,
        turns,
        generator_models: models.into_iter().collect(),
        generation_usage,
        touched_files: touched
            .into_iter()
            .map(|(path, count)| TouchedFile { path, count })
            .collect(),
    })
}

fn looks_like_noise(text: &str) -> bool {
    let head = text.trim_start().chars().take(80).collect::<String>();
    let window = text.chars().take(200).collect::<String>();
    NOISE_MARKERS
        .iter()
        .any(|marker| head.contains(marker) || window.contains(marker))
}

fn looks_like_injected_system(text: &str) -> bool {
    let head = text.trim_start().chars().take(200).collect::<String>();
    SYSTEM_INJECT_MARKERS
        .iter()
        .any(|marker| head.contains(marker))
}

fn has_tool_result(content: &Value) -> bool {
    content.as_array().is_some_and(|blocks| {
        blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
    })
}

fn is_escalation_output(text: &str) -> bool {
    ESCALATION_MARKERS
        .iter()
        .any(|marker| text.contains(marker))
}

fn render_content(content: &Value, mut touched: Option<&mut BTreeMap<String, u32>>) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    let Some(blocks) = content.as_array() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    parts.push(text.to_owned());
                }
            }
            Some("tool_use") => {
                let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                let input = block.get("input").unwrap_or(&Value::Null);
                if FILE_EDIT_TOOLS.contains(&name) {
                    let path = input
                        .get("file_path")
                        .or_else(|| input.get("notebook_path"))
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    if let Some(touched) = touched.as_deref_mut() {
                        *touched.entry(path.to_owned()).or_insert(0) += 1;
                    }
                    parts.push(format!("[{name}: {path}]"));
                } else if name == "Bash" {
                    let command = input
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect::<String>();
                    parts.push(format!("[ran: {command}]"));
                } else {
                    parts.push(format!("[tool_use: {name}]"));
                }
            }
            Some("tool_result") => {
                let value = block.get("content").unwrap_or(&Value::Null);
                let text = if let Some(text) = value.as_str() {
                    text.to_owned()
                } else if let Some(blocks) = value.as_array() {
                    blocks
                        .iter()
                        .filter_map(|block| block.get("text").and_then(Value::as_str))
                        .collect::<String>()
                } else {
                    String::new()
                };
                parts.push(format!("[tool_result: {text}]"));
            }
            // Thinking is intentionally omitted: the rubric grades served
            // content, not hidden reasoning.
            _ => {}
        }
    }
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/sample_claude_session.jsonl");

    #[test]
    fn separates_human_tool_and_noise_turns() {
        let session = parse_claude_jsonl(FIXTURE, "session-1").expect("session");
        assert_eq!(session.source, "claude_code");
        assert_eq!(session.client, "claude");
        assert_eq!(session.generator_model.as_deref(), Some("glm-5-2-fp8"));
        let user = session
            .turns
            .iter()
            .find(|turn| turn.role == Role::User)
            .expect("user turn");
        assert_eq!(user.text, "write a Go function to add two ints");
        let joined = session
            .turns
            .iter()
            .map(|turn| turn.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("func Add"));
        assert!(joined.contains("[tool_result: ok]"));
        assert!(!joined.contains("local-command"));
        assert!(!joined.contains("wants a simple add"));
    }

    #[test]
    fn captures_usage_and_file_edits() {
        let text = concat!(
            r#"{"type":"user","message":{"role":"user","content":"edit it"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"m","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"src/lib.rs"}}],"usage":{"input_tokens":10,"output_tokens":2}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"m","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":12,"output_tokens":3}}}"#
        );
        let session = parse_claude_jsonl(text, "s").expect("session");
        assert_eq!(
            session.touched_files,
            vec![TouchedFile {
                path: "src/lib.rs".to_owned(),
                count: 1
            }]
        );
        let usage = session.generation_usage.expect("usage");
        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(5));
    }

    #[test]
    fn empty_transcript_fails_closed() {
        assert!(matches!(
            parse_claude_jsonl("{\"type\":\"mode\"}\n", "s"),
            Err(CaptureError::NoTask)
        ));
    }
}
