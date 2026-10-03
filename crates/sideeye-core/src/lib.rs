//! Provider-neutral contracts shared by Side-Eye capture adapters and judges.
//!
//! This crate intentionally contains no provider or harness code. Claude Code,
//! Codex, OpenCode, Praxis, and future adapters produce [`ReviewPacket`] values;
//! judge providers consume them and return [`Verdict`] values.

use serde::{Deserialize, Serialize};
use thiserror::Error;

mod redaction;

pub use redaction::{redact_packet, redact_text};

pub const REVIEW_PACKET_SCHEMA: &str = "sideeye.review_packet.v1";
pub const VERDICT_SCHEMA: &str = "sideeye.verdict.v1";

fn default_verdict_schema() -> String {
    VERDICT_SCHEMA.to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    /// Adapter-defined kind, for example `git_diff`, `test_output`, or `file`.
    pub kind: String,
    pub path: Option<String>,
    pub content: String,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewPacket {
    pub schema_version: String,
    pub packet_id: String,
    pub session_id: String,
    /// Adapter provenance: `claude_code`, `codex`, `opencode`, or `praxis`.
    pub source: String,
    pub client: String,
    pub generator_model: Option<String>,
    pub task: String,
    pub turns: Vec<Turn>,
    pub artifacts: Vec<Artifact>,
    pub generation_usage: Option<GenerationUsage>,
}

impl ReviewPacket {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != REVIEW_PACKET_SCHEMA {
            return Err(ContractError::WrongSchema {
                expected: REVIEW_PACKET_SCHEMA,
                actual: self.schema_version.clone(),
            });
        }
        require_nonempty("packet_id", &self.packet_id)?;
        require_nonempty("session_id", &self.session_id)?;
        require_nonempty("source", &self.source)?;
        require_nonempty("client", &self.client)?;
        require_nonempty("task", &self.task)?;
        if self.turns.is_empty() {
            return Err(ContractError::Empty("turns".to_owned()));
        }
        for (index, artifact) in self.artifacts.iter().enumerate() {
            require_nonempty(&format!("artifacts[{index}].kind"), &artifact.kind)?;
            if artifact.content.is_empty() {
                return Err(ContractError::Empty(format!("artifacts[{index}].content")));
            }
        }
        Ok(())
    }

    /// Return a packet safe to pass to a remote judge by redacting credentials
    /// from human/tool text and artifact content. This is deliberately
    /// loss-preserving for ordinary prose and code; it is not a PII anonymizer.
    pub fn redacted(&self) -> Self {
        redact_packet(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Correctness {
    Correct,
    PartiallyCorrect,
    Incorrect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    None,
    Minor,
    Major,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerdictIssue {
    pub description: String,
    pub severity: IssueSeverity,
    /// Optional for compatibility with the Python judge's current verdict
    /// shape. New providers should populate evidence when available.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    Minor,
    Major,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Verdict {
    #[serde(default = "default_verdict_schema")]
    pub schema_version: String,
    /// Optional on the raw judge result; set when the engine persists the
    /// verdict alongside the packet that produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_id: Option<String>,
    /// Optional on the raw judge result; required for comparable persisted
    /// records and scoreboard aggregation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric_version: Option<String>,
    pub answered_what_was_asked: bool,
    pub correctness: Correctness,
    pub claims_supported: bool,
    pub score: u8,
    pub issues: Vec<VerdictIssue>,
    pub overall_severity: Severity,
    pub summary: String,
    /// The following envelope fields are absent from the Python judge's raw
    /// tool result and are added by its runner. Keep them optional so the Rust
    /// contract can consume both shapes during the migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl Verdict {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != VERDICT_SCHEMA {
            return Err(ContractError::WrongSchema {
                expected: VERDICT_SCHEMA,
                actual: self.schema_version.clone(),
            });
        }
        if !(1..=5).contains(&self.score) {
            return Err(ContractError::Score(self.score));
        }
        require_nonempty("summary", &self.summary)?;
        if let Some(provider) = &self.judge_provider {
            require_nonempty("judge_provider", provider)?;
        }
        if let Some(model) = &self.judge_model {
            require_nonempty("judge_model", model)?;
        }
        if let Some(packet_id) = &self.packet_id {
            require_nonempty("packet_id", packet_id)?;
        }
        if let Some(rubric_version) = &self.rubric_version {
            require_nonempty("rubric_version", rubric_version)?;
        }
        for (index, issue) in self.issues.iter().enumerate() {
            require_nonempty(&format!("issues[{index}].description"), &issue.description)?;
            for evidence in &issue.evidence {
                require_nonempty(&format!("issues[{index}].evidence"), evidence)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("{0} must not be empty")]
    Empty(String),
    #[error("schema version must be {expected}, got {actual}")]
    WrongSchema {
        expected: &'static str,
        actual: String,
    },
    #[error("score must be between 1 and 5, got {0}")]
    Score(u8),
}

fn require_nonempty(field: &str, value: &str) -> Result<(), ContractError> {
    if value.trim().is_empty() {
        return Err(ContractError::Empty(field.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet() -> ReviewPacket {
        ReviewPacket {
            schema_version: REVIEW_PACKET_SCHEMA.to_owned(),
            packet_id: "packet-1".to_owned(),
            session_id: "session-1".to_owned(),
            source: "opencode".to_owned(),
            client: "opencode".to_owned(),
            generator_model: Some("draft-model".to_owned()),
            task: "Fix the parser".to_owned(),
            turns: vec![Turn {
                role: Role::User,
                text: "Fix the parser".to_owned(),
            }],
            artifacts: vec![Artifact {
                kind: "git_diff".to_owned(),
                path: None,
                content: "diff --git a/parser.rs b/parser.rs".to_owned(),
                complete: true,
            }],
            generation_usage: None,
        }
    }

    #[test]
    fn valid_packet_round_trips_and_validates() {
        let packet = packet();
        packet.validate().unwrap();
        let encoded = serde_json::to_string(&packet).unwrap();
        let decoded: ReviewPacket = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, packet);
    }

    #[test]
    fn packet_requires_evidence_text() {
        let mut packet = packet();
        packet.artifacts[0].content.clear();
        assert_eq!(
            packet.validate(),
            Err(ContractError::Empty("artifacts[0].content".to_owned()))
        );
    }

    #[test]
    fn packet_allows_empty_turn_text_like_python_transcripts() {
        let mut packet = packet();
        packet.turns[0].text.clear();
        packet.validate().unwrap();
    }

    #[test]
    fn verdict_rejects_invalid_score() {
        let verdict = Verdict {
            schema_version: VERDICT_SCHEMA.to_owned(),
            packet_id: None,
            rubric_version: None,
            answered_what_was_asked: true,
            correctness: Correctness::Correct,
            claims_supported: true,
            score: 0,
            issues: vec![],
            overall_severity: Severity::None,
            summary: "Looks good".to_owned(),
            judge_provider: None,
            judge_model: None,
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
        };
        assert_eq!(verdict.validate(), Err(ContractError::Score(0)));
    }

    #[test]
    fn python_judge_core_verdict_is_compatible() {
        let json = r#"
        {
          "answered_what_was_asked": true,
          "correctness": "partially_correct",
          "claims_supported": false,
          "score": 3,
          "issues": [{"description": "Missing evidence", "severity": "major"}],
          "overall_severity": "major",
          "summary": "Needs evidence."
        }
        "#;
        let verdict: Verdict = serde_json::from_str(json).unwrap();
        assert_eq!(verdict.schema_version, VERDICT_SCHEMA);
        assert_eq!(verdict.issues[0].evidence, Vec::<String>::new());
        verdict.validate().unwrap();
    }

    #[test]
    fn enriched_verdict_keeps_packet_and_rubric_provenance() {
        let mut verdict = Verdict {
            schema_version: VERDICT_SCHEMA.to_owned(),
            packet_id: Some("packet-1".to_owned()),
            rubric_version: Some("rubric_session_v2".to_owned()),
            answered_what_was_asked: true,
            correctness: Correctness::Correct,
            claims_supported: true,
            score: 5,
            issues: vec![VerdictIssue {
                description: "No issue".to_owned(),
                severity: IssueSeverity::Minor,
                evidence: vec!["test output".to_owned()],
            }],
            overall_severity: Severity::None,
            summary: "Looks good".to_owned(),
            judge_provider: Some("fake".to_owned()),
            judge_model: Some("fake-judge".to_owned()),
            input_tokens: Some(10),
            output_tokens: Some(4),
            cost_usd: Some(0.01),
        };
        verdict.validate().unwrap();
        let encoded = serde_json::to_string(&verdict).unwrap();
        assert!(encoded.contains("packet_id"));
        assert!(encoded.contains("rubric_version"));
        verdict.packet_id = Some(" ".to_owned());
        assert!(verdict.validate().is_err());
    }
}
