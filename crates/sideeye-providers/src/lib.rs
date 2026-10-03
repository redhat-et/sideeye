//! Provider boundary for Side-Eye judges.
//!
//! The core contract does not know whether a verdict came from Anthropic,
//! OpenAI-compatible infrastructure, a gateway, or a local model. Providers
//! implement [`JudgeProvider`] and return the same validated [`Verdict`].

use sideeye_core::{ContractError, ReviewPacket, Verdict};
use thiserror::Error;

pub trait JudgeProvider {
    fn provider_name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn judge(&self, packet: &ReviewPacket) -> Result<Verdict, ProviderError>;
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("review packet is invalid: {0}")]
    InvalidPacket(#[from] ContractError),
    #[error("judge returned an invalid verdict: {0}")]
    InvalidVerdict(ContractError),
}

/// Deterministic provider used by contract tests and hermetic harness CI.
///
/// It does not inspect or score the packet. Tests supply the verdict they want
/// returned, which makes provider and capture tests independent of network
/// access, model drift, and credentials.
#[derive(Debug, Clone)]
pub struct FakeJudgeProvider {
    provider: String,
    model: String,
    verdict: Verdict,
}

impl FakeJudgeProvider {
    pub fn new(provider: impl Into<String>, model: impl Into<String>, verdict: Verdict) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            verdict,
        }
    }
}

impl JudgeProvider for FakeJudgeProvider {
    fn provider_name(&self) -> &str {
        &self.provider
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn judge(&self, packet: &ReviewPacket) -> Result<Verdict, ProviderError> {
        packet.validate()?;

        let mut verdict = self.verdict.clone();
        verdict.packet_id = Some(packet.packet_id.clone());
        verdict.judge_provider = Some(self.provider.clone());
        verdict.judge_model = Some(self.model.clone());
        verdict.validate().map_err(ProviderError::InvalidVerdict)?;
        Ok(verdict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sideeye_core::{
        Correctness, REVIEW_PACKET_SCHEMA, ReviewPacket, Severity, Turn, VERDICT_SCHEMA,
    };

    fn packet() -> ReviewPacket {
        ReviewPacket {
            schema_version: REVIEW_PACKET_SCHEMA.to_owned(),
            packet_id: "packet-1".to_owned(),
            session_id: "session-1".to_owned(),
            source: "fake".to_owned(),
            client: "test".to_owned(),
            generator_model: None,
            task: "test".to_owned(),
            turns: vec![Turn {
                role: sideeye_core::Role::User,
                text: "test".to_owned(),
            }],
            artifacts: vec![],
            generation_usage: None,
        }
    }

    fn verdict() -> Verdict {
        Verdict {
            schema_version: VERDICT_SCHEMA.to_owned(),
            packet_id: None,
            rubric_version: Some("rubric_test_v1".to_owned()),
            answered_what_was_asked: true,
            correctness: Correctness::Correct,
            claims_supported: true,
            score: 5,
            issues: vec![],
            overall_severity: Severity::None,
            summary: "deterministic test verdict".to_owned(),
            judge_provider: None,
            judge_model: None,
            input_tokens: Some(1),
            output_tokens: Some(1),
            cost_usd: Some(0.0),
        }
    }

    #[test]
    fn fake_provider_adds_provenance_without_network_access() {
        let provider = FakeJudgeProvider::new("fake", "fixture-judge", verdict());
        let result = provider.judge(&packet()).unwrap();
        assert_eq!(result.packet_id.as_deref(), Some("packet-1"));
        assert_eq!(result.judge_provider.as_deref(), Some("fake"));
        assert_eq!(result.judge_model.as_deref(), Some("fixture-judge"));
        assert_eq!(result.rubric_version.as_deref(), Some("rubric_test_v1"));
    }

    #[test]
    fn fake_provider_rejects_invalid_packets_before_judging() {
        let mut invalid = packet();
        invalid.task.clear();
        let provider = FakeJudgeProvider::new("fake", "fixture-judge", verdict());
        assert!(matches!(
            provider.judge(&invalid),
            Err(ProviderError::InvalidPacket(ContractError::Empty(field))) if field == "task"
        ));
    }
}
