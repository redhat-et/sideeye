//! Anthropic Messages provider.
//!
//! This mirrors the Python judge contract: the rubric is the system prompt, the
//! packet is the user turn, and `record_verdict` is forced so the verdict is
//! structured JSON instead of free text. A malformed tool call gets one
//! corrective retry that answers the prior `tool_use` with a `tool_result`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sideeye_core::{
    Correctness, IssueSeverity, ReviewPacket, Role, Severity, VERDICT_SCHEMA, Verdict, VerdictIssue,
};

use crate::{JudgeProvider, ProviderError};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;
const DEFAULT_TIMEOUT_SECS: u64 = 90;
const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
const JUDGE_FIELDS: &str = "answered_what_was_asked, correctness, claims_supported, score, issues, overall_severity, summary";

/// Routes that are known to serve the cheap drafting model. A judge on these
/// routes would grade its own output.
const FORBIDDEN_ROUTES: &[(&str, &str)] = &[
    ("127.0.0.1:818", "the local GLM gateway"),
    ("localhost:818", "the local GLM gateway"),
    ("ai-gateway-qwen", "the dogfood Qwen route"),
];

/// Pre-spend estimate from the free count_tokens endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct Estimate {
    pub input_tokens: u64,
    pub estimated_cost_usd: f64,
    pub model: String,
    pub max_tokens: u32,
    pub turns: usize,
    pub artifacts: usize,
    pub has_diff: bool,
    pub diff_complete: bool,
}

pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    model: String,
    rubric: String,
    rubric_version: String,
    max_tokens: u32,
    timeout: Duration,
    user_agent: String,
    instructions: Option<String>,
}

impl AnthropicProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        rubric: impl Into<String>,
        rubric_version: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            rubric: rubric.into(),
            rubric_version: rubric_version.into(),
            max_tokens: DEFAULT_MAX_TOKENS,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            user_agent: concat!("sideeye/", env!("CARGO_PKG_VERSION")).to_owned(),
            instructions: None,
        }
    }

    /// Extra focus from the user, e.g. "review for security vulnerabilities".
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        let instructions = instructions.into();
        self.instructions = (!instructions.trim().is_empty()).then_some(instructions);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_timeout_secs(mut self, seconds: u64) -> Self {
        self.timeout = Duration::from_secs(seconds);
        self
    }

    fn endpoint(&self, path: &str) -> Result<String, ProviderError> {
        let base = self.base_url.trim().trim_end_matches('/');
        if base.is_empty() {
            return Err(ProviderError::Configuration(
                "no judge base URL configured".to_owned(),
            ));
        }
        if let Some((_, what)) = FORBIDDEN_ROUTES
            .iter()
            .find(|(needle, _)| base.contains(needle))
        {
            return Err(ProviderError::Configuration(format!(
                "judge route points at {what} ({base}); the judge must use a real Claude route"
            )));
        }
        let base = base.strip_suffix("/v1").unwrap_or(base);
        Ok(format!("{base}{path}"))
    }

    fn request_body(&self, packet: &ReviewPacket) -> Value {
        let (asked, produced) = render_packet(packet);
        let instruction = if self.supports_forced_tool() {
            "Grade the response using the rubric, then call record_verdict."
        } else {
            "Grade the response using the rubric. You must call record_verdict exactly once with every required field."
        };
        let focus = self
            .instructions
            .as_deref()
            .map(|value| format!("## Reviewer instructions\n\n{}\n\n", value.trim()))
            .unwrap_or_default();
        let user = format!(
            "## What was asked\n\n{}\n\n{focus}## The response under review\n\n{}\n\n{instruction}",
            asked.trim(),
            produced.trim()
        );
        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": self.rubric,
            "messages": [{"role": "user", "content": user}],
            "tools": [record_verdict_tool()],
        });
        if self.supports_forced_tool() {
            body["tool_choice"] = json!({"type": "tool", "name": "record_verdict"});
        }
        body
    }

    /// Some newer models reject forced `tool_choice`. They still call the tool
    /// when the prompt requires it, so omit the forcing parameter for them.
    fn supports_forced_tool(&self) -> bool {
        !self.model.starts_with("claude-opus-5")
    }

    fn probe_body(&self) -> Value {
        json!({
            "model": self.model,
            "messages": [{"role": "user", "content": "sideeye setup check"}],
        })
    }

    /// Free route check used by setup and `doctor --verify`. It proves the base
    /// URL, credential, and model without calling the judging endpoint.
    pub fn probe(&self) -> Result<u64, ProviderError> {
        let response = self.post_path("/v1/messages/count_tokens", &self.probe_body())?;
        response
            .get("input_tokens")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                ProviderError::Network("count_tokens returned no input_tokens".to_owned())
            })
    }

    /// Exact input token count for a packet. This is the free count_tokens
    /// endpoint, so it can run before the user confirms spend.
    pub fn count_tokens(&self, packet: &ReviewPacket) -> Result<u64, ProviderError> {
        packet.validate()?;
        let mut body = self.request_body(packet);
        if let Some(object) = body.as_object_mut() {
            object.remove("max_tokens");
            if !self.supports_forced_tool() {
                object.remove("tool_choice");
            }
        }
        let response = self.post_path("/v1/messages/count_tokens", &body)?;
        response
            .get("input_tokens")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                ProviderError::Network("count_tokens returned no input_tokens".to_owned())
            })
    }

    /// Estimate cost before judging. Output is bounded by `max_tokens` and is
    /// labeled an estimate, matching the Python oracle.
    pub fn estimate(&self, packet: &ReviewPacket) -> Result<Estimate, ProviderError> {
        let input_tokens = self.count_tokens(packet)?;
        ensure_fits(input_tokens, self.max_tokens as u64, &self.model)?;
        let (input_price, output_price) = pricing(&self.model);
        let cost = input_tokens as f64 * input_price + self.max_tokens as f64 * output_price;
        let diffs = packet
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == "git_diff");
        let artifacts = diffs.clone().count();
        Ok(Estimate {
            input_tokens,
            estimated_cost_usd: (cost * 1_000_000.0).round() / 1_000_000.0,
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            turns: packet.turns.len(),
            artifacts,
            has_diff: artifacts > 0,
            diff_complete: diffs.clone().all(|artifact| artifact.complete),
        })
    }

    fn post(&self, body: &Value) -> Result<Value, ProviderError> {
        self.post_path("/v1/messages", body)
    }

    fn post_path(&self, path: &str, body: &Value) -> Result<Value, ProviderError> {
        let url = self.endpoint(path)?;
        let response = ureq::post(&url)
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("content-type", "application/json")
            .set("user-agent", &self.user_agent)
            .timeout(self.timeout)
            .send_json(body);
        match response {
            Ok(response) => response.into_json::<Value>().map_err(|error| {
                ProviderError::Network(format!("invalid JSON from {url}: {error}"))
            }),
            Err(ureq::Error::Status(status, response)) => {
                let detail = response
                    .into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(500)
                    .collect::<String>();
                Err(ProviderError::Network(format!(
                    "{status} from {url}: {detail}"
                )))
            }
            Err(error) => Err(ProviderError::Network(format!("{error} from {url}"))),
        }
    }

    fn retry_body(&self, body: &Value, response: &Value, error: &str) -> Option<Value> {
        let content = response.get("content")?.as_array()?;
        let tool_use_id = content.iter().find_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("tool_use"))
                .then(|| block.get("id").and_then(Value::as_str))
                .flatten()
        })?;
        let correction = format!(
            "Your record_verdict call was invalid: {error}. Call record_verdict again, this time including ALL required fields: {JUDGE_FIELDS}."
        );
        Some(json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": body["system"],
            "messages": [
                body["messages"][0].clone(),
                {"role": "assistant", "content": content},
                {"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": correction,
                    "is_error": true,
                }]},
            ],
            "tools": body["tools"],
            "tool_choice": body["tool_choice"],
        }))
        .map(|mut body| {
            if !self.supports_forced_tool()
                && let Some(object) = body.as_object_mut()
            {
                object.remove("tool_choice");
            }
            body
        })
    }
}

impl JudgeProvider for AnthropicProvider {
    fn provider_name(&self) -> &str {
        "anthropic"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn judge(&self, packet: &ReviewPacket) -> Result<Verdict, ProviderError> {
        packet.validate()?;
        // Best-effort guard: some gateways do not implement count_tokens.
        // Their judge call still fails closed on an oversized request.
        if let Ok(input_tokens) = self.count_tokens(packet) {
            ensure_fits(input_tokens, self.max_tokens as u64, &self.model)?;
        }
        let body = self.request_body(packet);
        let first = self.post(&body)?;
        ensure_not_truncated(&first, self.max_tokens)?;
        let usage = Usage::from(&first);
        match parse_verdict(&first) {
            Ok(core) => Ok(self.finish(packet, core, usage)),
            Err(error) => {
                let retry = self.retry_body(&body, &first, &error).ok_or_else(|| {
                    ProviderError::InvalidVerdict(format!(
                        "no record_verdict tool_use block found: {error}"
                    ))
                })?;
                let second = self.post(&retry)?;
                ensure_not_truncated(&second, self.max_tokens)?;
                let usage = usage + Usage::from(&second);
                let core = parse_verdict(&second).map_err(|error| {
                    ProviderError::InvalidVerdict(format!(
                        "judge returned an invalid verdict after retry: {error}"
                    ))
                })?;
                Ok(self.finish(packet, core, usage))
            }
        }
    }
}

impl AnthropicProvider {
    fn finish(&self, packet: &ReviewPacket, core: CoreVerdict, usage: Usage) -> Verdict {
        let (input_price, output_price) = pricing(&self.model);
        let cost =
            usage.input_tokens as f64 * input_price + usage.output_tokens as f64 * output_price;
        Verdict {
            schema_version: VERDICT_SCHEMA.to_owned(),
            packet_id: Some(packet.packet_id.clone()),
            rubric_version: Some(self.rubric_version.clone()),
            answered_what_was_asked: core.answered_what_was_asked,
            correctness: core.correctness,
            claims_supported: core.claims_supported,
            score: core.score,
            issues: core
                .issues
                .into_iter()
                .map(|issue| VerdictIssue {
                    description: issue.description,
                    severity: issue.severity,
                    evidence: issue.evidence,
                })
                .collect(),
            overall_severity: core.overall_severity,
            summary: core.summary,
            judge_provider: Some("anthropic".to_owned()),
            judge_model: Some(self.model.clone()),
            input_tokens: Some(usage.input_tokens),
            output_tokens: Some(usage.output_tokens),
            cost_usd: Some((cost * 1_000_000.0).round() / 1_000_000.0),
        }
    }
}

fn render_packet(packet: &ReviewPacket) -> (String, String) {
    let asked = packet.task.clone();
    let mut produced = String::new();
    for turn in &packet.turns {
        produced.push_str(match turn.role {
            Role::User => "USER:\n",
            Role::Assistant => "ASSISTANT:\n",
            Role::Tool => "TOOL:\n",
            Role::System => "SYSTEM:\n",
        });
        produced.push_str(turn.text.trim());
        produced.push_str("\n\n");
    }
    for artifact in &packet.artifacts {
        produced.push_str(&format!("## {} evidence", artifact.kind));
        if let Some(path) = &artifact.path {
            produced.push_str(&format!(" ({path})"));
        }
        produced.push('\n');
        produced.push_str(&artifact.content);
        produced.push_str("\n\n");
    }
    (asked, produced)
}

#[derive(Debug, Deserialize)]
struct CoreVerdict {
    answered_what_was_asked: bool,
    correctness: Correctness,
    claims_supported: bool,
    score: u8,
    #[serde(default)]
    issues: Vec<CoreIssue>,
    overall_severity: Severity,
    summary: String,
}

#[derive(Debug, Deserialize)]
struct CoreIssue {
    description: String,
    severity: IssueSeverity,
    #[serde(default)]
    evidence: Vec<String>,
}

fn context_window(_model: &str) -> u64 {
    DEFAULT_CONTEXT_WINDOW
}

fn ensure_fits(input_tokens: u64, max_tokens: u64, model: &str) -> Result<(), ProviderError> {
    let window = context_window(model);
    if input_tokens + max_tokens > window {
        return Err(ProviderError::Configuration(format!(
            "packet is {input_tokens} input tokens; {model}'s context window is {window} (need {} with output). Reduce the packet or use a larger-context judge.",
            input_tokens + max_tokens
        )));
    }
    Ok(())
}

/// A verdict cut off at `max_tokens` cannot be repaired by retrying: the same
/// budget will truncate again. Fail with an actionable message instead.
fn ensure_not_truncated(response: &Value, max_tokens: u32) -> Result<(), ProviderError> {
    if response.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
        return Err(ProviderError::InvalidVerdict(format!(
            "judge output was truncated at max_tokens={max_tokens}; raise --max-tokens"
        )));
    }
    Ok(())
}

fn parse_verdict(response: &Value) -> Result<CoreVerdict, String> {
    let content = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or("judge response has no content array")?;
    let input = content
        .iter()
        .find_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("tool_use")
                && block.get("name").and_then(Value::as_str) == Some("record_verdict"))
            .then(|| block.get("input"))
            .flatten()
        })
        .ok_or("no record_verdict tool_use block found")?;
    serde_json::from_value(input.clone()).map_err(|error| format!("invalid verdict: {error}"))
}

#[derive(Debug, Default, Clone, Copy)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

impl From<&Value> for Usage {
    fn from(response: &Value) -> Self {
        let usage = response.get("usage").unwrap_or(&Value::Null);
        Self {
            input_tokens: usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }
    }
}

impl std::ops::Add for Usage {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens + other.input_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
        }
    }
}

fn pricing(model: &str) -> (f64, f64) {
    match model {
        "claude-sonnet-5" => (2.0 / 1e6, 10.0 / 1e6),
        "claude-fable-5" => (10.0 / 1e6, 50.0 / 1e6),
        "claude-opus-4-8" | "claude-opus-5-5" => (5.0 / 1e6, 25.0 / 1e6),
        _ => (5.0 / 1e6, 25.0 / 1e6),
    }
}

fn record_verdict_tool() -> Value {
    json!({
        "name": "record_verdict",
        "description": "Record the structured grading verdict for the response under review.",
        "input_schema": {
            "type": "object",
            "properties": {
                "answered_what_was_asked": {"type": "boolean"},
                "correctness": {"type": "string", "enum": ["correct", "partially_correct", "incorrect"]},
                "claims_supported": {"type": "boolean"},
                "score": {"type": "integer", "minimum": 1, "maximum": 5},
                "issues": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "description": {"type": "string"},
                            "severity": {"type": "string", "enum": ["minor", "major", "critical"]},
                        },
                        "required": ["description", "severity"],
                    },
                },
                "overall_severity": {"type": "string", "enum": ["none", "minor", "major", "critical"]},
                "summary": {"type": "string"},
            },
            "required": [
                "answered_what_was_asked",
                "correctness",
                "claims_supported",
                "score",
                "issues",
                "overall_severity",
                "summary",
            ],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_body_is_a_minimal_free_count_request() {
        let provider =
            AnthropicProvider::new("https://example.test", "k", "claude-opus-4-8", "r", "r1");
        let body = provider.probe_body();
        assert_eq!(body["model"], "claude-opus-4-8");
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body.get("tools").is_none());
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn newer_models_omit_forced_tool_choice() {
        let provider =
            AnthropicProvider::new("https://example.test", "k", "claude-opus-5-5", "r", "r1");
        let packet = ReviewPacket {
            schema_version: sideeye_core::REVIEW_PACKET_SCHEMA.to_owned(),
            packet_id: "p".to_owned(),
            session_id: "s".to_owned(),
            source: "test".to_owned(),
            client: "test".to_owned(),
            generator_model: None,
            task: "task".to_owned(),
            turns: vec![sideeye_core::Turn {
                role: sideeye_core::Role::User,
                text: "task".to_owned(),
            }],
            artifacts: vec![],
            generation_usage: None,
            generator_models: vec![],
        };
        let body = provider.request_body(&packet);
        assert!(body.get("tool_choice").is_none());
        let legacy =
            AnthropicProvider::new("https://example.test", "k", "claude-opus-4-8", "r", "r1");
        assert!(legacy.request_body(&packet).get("tool_choice").is_some());
    }

    #[test]
    fn context_guard_rejects_overflow_before_spend() {
        let error = ensure_fits(199_000, 2_048, "claude-opus-4-8").expect_err("must fail");
        assert!(error.to_string().contains("context window is 200000"));
        ensure_fits(150_000, 2_048, "claude-opus-4-8").expect("fits");
    }

    #[test]
    fn truncation_is_reported_before_retry() {
        let response = json!({"stop_reason": "max_tokens", "content": []});
        let error = ensure_not_truncated(&response, 512).expect_err("must fail");
        assert!(error.to_string().contains("truncated at max_tokens=512"));
    }

    #[test]
    fn complete_response_passes_truncation_guard() {
        let response = json!({"stop_reason": "tool_use", "content": []});
        ensure_not_truncated(&response, 4096).expect("not truncated");
    }

    #[test]
    fn parses_record_verdict_tool_input() {
        let response = json!({
            "content": [{
                "type": "tool_use",
                "name": "record_verdict",
                "input": {
                    "answered_what_was_asked": true,
                    "correctness": "correct",
                    "claims_supported": true,
                    "score": 5,
                    "issues": [],
                    "overall_severity": "none",
                    "summary": "ok"
                }
            }]
        });
        let verdict = parse_verdict(&response).expect("verdict");
        assert_eq!(verdict.score, 5);
    }
}
