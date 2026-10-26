//! OpenAI-compatible Chat Completions provider.
//!
//! This covers OpenAI, gateways that implement the same API, and local servers
//! such as Ollama, LM Studio, and vLLM. The verdict contract is identical to the
//! Anthropic provider; only the transport and tool-call shape differ.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use sideeye_core::{
    Correctness, IssueSeverity, ReviewPacket, Severity, VERDICT_SCHEMA, Verdict, VerdictIssue,
};

use crate::{
    Estimate, JudgeProvider, ProviderError,
    anthropic::{render_packet, verdict_schema},
};

const DEFAULT_MAX_TOKENS: u32 = 4096;
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;

pub struct OpenAiProvider {
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

impl OpenAiProvider {
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
        let base = base.strip_suffix("/v1").unwrap_or(base);
        Ok(format!("{base}{path}"))
    }

    fn request_body(&self, packet: &ReviewPacket) -> Value {
        let (asked, produced) = render_packet(packet);
        let focus = self
            .instructions
            .as_deref()
            .map(|value| format!("## Reviewer instructions\n\n{}\n\n", value.trim()))
            .unwrap_or_default();
        let user = format!(
            "## What was asked\n\n{}\n\n{focus}## The response under review\n\n{}\n\nGrade the response using the rubric, then call record_verdict exactly once with every required field.",
            asked.trim(),
            produced.trim()
        );
        json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": [
                {"role": "system", "content": self.rubric},
                {"role": "user", "content": user},
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "record_verdict",
                    "description": "Record the structured grading verdict for the response under review.",
                    "parameters": verdict_schema(),
                },
            }],
            "tool_choice": {"type": "function", "function": {"name": "record_verdict"}},
        })
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, ProviderError> {
        let url = self.endpoint(path)?;
        let response = ureq::post(&url)
            .set("authorization", &format!("Bearer {}", self.api_key))
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

    /// Free reachability/auth check for setup and `doctor --verify`.
    /// OpenAI-compatible servers are not required to implement `/v1/models`;
    /// a 404 is reported as a warning rather than a credential failure.
    pub fn probe(&self) -> Result<String, ProviderError> {
        let url = self.endpoint("/v1/models")?;
        let response = ureq::get(&url)
            .set("authorization", &format!("Bearer {}", self.api_key))
            .set("user-agent", &self.user_agent)
            .timeout(self.timeout)
            .call();
        match response {
            Ok(_) => Ok("route reachable".to_owned()),
            Err(ureq::Error::Status(404, _)) => Ok("route reachable (no /v1/models)".to_owned()),
            Err(ureq::Error::Status(status, response)) => {
                let detail = response
                    .into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect::<String>();
                Err(ProviderError::Network(format!(
                    "{status} from {url}: {detail}"
                )))
            }
            Err(error) => Err(ProviderError::Network(format!("{error} from {url}"))),
        }
    }

    fn finish(&self, packet: &ReviewPacket, core: CoreVerdict, usage: Usage) -> Verdict {
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
            judge_provider: Some("openai-compatible".to_owned()),
            judge_model: Some(self.model.clone()),
            input_tokens: Some(usage.input_tokens),
            output_tokens: Some(usage.output_tokens),
            // Pricing is provider-specific. Do not invent a zero cost for a
            // paid gateway; its own metering is authoritative.
            cost_usd: None,
        }
    }
}

impl JudgeProvider for OpenAiProvider {
    fn provider_name(&self) -> &str {
        "openai-compatible"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn estimate(&self, packet: &ReviewPacket) -> Result<Estimate, ProviderError> {
        packet.validate()?;
        let body = self.request_body(packet);
        // OpenAI has no mandatory free count_tokens endpoint. Keep the estimate
        // cheap and label it approximate; the judge call still enforces usage.
        let chars = body.to_string().len();
        let input_tokens = (chars as u64).div_ceil(4);
        if input_tokens + self.max_tokens as u64 > DEFAULT_CONTEXT_WINDOW {
            return Err(ProviderError::Configuration(format!(
                "estimated packet is {input_tokens} input tokens; the default context window is {DEFAULT_CONTEXT_WINDOW} (need {} with output). Reduce the packet or use a larger-context route.",
                input_tokens + self.max_tokens as u64
            )));
        }
        let diffs = packet
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == "git_diff");
        let artifacts = diffs.clone().count();
        Ok(Estimate {
            input_tokens,
            estimated_cost_usd: 0.0,
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            turns: packet.turns.len(),
            artifacts,
            has_diff: artifacts > 0,
            diff_complete: diffs.clone().all(|artifact| artifact.complete),
            exact: false,
            cost_known: false,
        })
    }

    fn judge(&self, packet: &ReviewPacket) -> Result<Verdict, ProviderError> {
        packet.validate()?;
        let body = self.request_body(packet);
        let response = self.post("/v1/chat/completions", &body)?;
        ensure_not_truncated(&response, self.max_tokens)?;
        let core = parse_verdict(&response)?;
        let usage = Usage::from(&response);
        let verdict = self.finish(packet, core, usage);
        verdict
            .validate()
            .map_err(|error| ProviderError::InvalidVerdict(error.to_string()))?;
        Ok(verdict)
    }
}

fn ensure_not_truncated(response: &Value, max_tokens: u32) -> Result<(), ProviderError> {
    let finish = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str);
    if finish == Some("length") {
        return Err(ProviderError::InvalidVerdict(format!(
            "judge output was truncated at max_tokens={max_tokens}; raise --max-tokens"
        )));
    }
    Ok(())
}

fn parse_verdict(response: &Value) -> Result<CoreVerdict, ProviderError> {
    let message = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| ProviderError::InvalidVerdict("response has no message".to_owned()))?;

    if let Some(arguments) = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .and_then(|calls| calls.first())
        .and_then(|call| call.get("function"))
        .and_then(|function| function.get("arguments"))
        .and_then(Value::as_str)
    {
        return parse_arguments(arguments);
    }
    if let Some(arguments) = message
        .get("function_call")
        .and_then(|call| call.get("arguments"))
        .and_then(Value::as_str)
    {
        return parse_arguments(arguments);
    }
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        return parse_arguments(content);
    }
    Err(ProviderError::InvalidVerdict(
        "no record_verdict tool call or JSON content; this route may not support tool calling"
            .to_owned(),
    ))
}

fn parse_arguments(arguments: &str) -> Result<CoreVerdict, ProviderError> {
    let trimmed = arguments.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .strip_suffix("```")
        .unwrap_or(trimmed)
        .trim();
    serde_json::from_str(trimmed).map_err(|error| {
        ProviderError::InvalidVerdict(format!("invalid record_verdict JSON: {error}"))
    })
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
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideeye_core::{REVIEW_PACKET_SCHEMA, ReviewPacket, Role, Turn};

    fn packet() -> ReviewPacket {
        ReviewPacket {
            schema_version: REVIEW_PACKET_SCHEMA.to_owned(),
            packet_id: "p".to_owned(),
            session_id: "s".to_owned(),
            source: "test".to_owned(),
            client: "test".to_owned(),
            generator_model: None,
            task: "task".to_owned(),
            turns: vec![Turn {
                role: Role::User,
                text: "task".to_owned(),
            }],
            artifacts: vec![],
            generation_usage: None,
            generator_models: vec![],
        }
    }

    #[test]
    fn request_uses_openai_function_tool_shape() {
        let provider =
            OpenAiProvider::new("http://127.0.0.1:11434", "local", "qwen", "rubric", "r1");
        let body = provider.request_body(&packet());
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "record_verdict");
        assert_eq!(
            body["tools"][0]["function"]["parameters"]["required"][0],
            "answered_what_was_asked"
        );
        assert_eq!(body["tool_choice"]["function"]["name"], "record_verdict");
    }

    #[test]
    fn parses_tool_call_arguments() {
        let response = json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "tool_calls": [{
                        "function": {
                            "name": "record_verdict",
                            "arguments": "{\"answered_what_was_asked\":true,\"correctness\":\"correct\",\"claims_supported\":true,\"score\":5,\"issues\":[],\"overall_severity\":\"none\",\"summary\":\"ok\"}"
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let verdict = parse_verdict(&response).expect("verdict");
        assert_eq!(verdict.score, 5);
        assert_eq!(Usage::from(&response).input_tokens, 10);
    }

    #[test]
    fn parses_plain_json_content_for_local_models() {
        let response = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "content": "```json\n{\"answered_what_was_asked\":true,\"correctness\":\"correct\",\"claims_supported\":true,\"score\":4,\"issues\":[],\"overall_severity\":\"minor\",\"summary\":\"ok\"}\n```"
                }
            }]
        });
        assert_eq!(parse_verdict(&response).expect("verdict").score, 4);
    }

    #[test]
    fn reports_truncation() {
        let response = json!({"choices": [{"finish_reason": "length"}]});
        let error = ensure_not_truncated(&response, 512).expect_err("must fail");
        assert!(error.to_string().contains("truncated at max_tokens=512"));
    }

    #[test]
    fn heuristic_estimate_is_labeled_not_exact() {
        let provider =
            OpenAiProvider::new("http://127.0.0.1:11434", "local", "qwen", "rubric", "r1");
        let estimate = provider.estimate(&packet()).expect("estimate");
        assert!(!estimate.exact);
        assert!(estimate.input_tokens > 0);
        assert_eq!(estimate.estimated_cost_usd, 0.0);
    }
}
