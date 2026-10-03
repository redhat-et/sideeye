use std::sync::OnceLock;

use regex::Regex;

use crate::ReviewPacket;

/// Redact credentials from text before it crosses a process or provider
/// boundary. The patterns are intentionally conservative about what they
/// replace and are tested against the formats Side-Eye itself uses.
pub fn redact_text(input: &str) -> String {
    let assignment = assignment_regex().replace_all(input, "$1[REDACTED]");
    let header = header_regex().replace_all(&assignment, "$1[REDACTED]");
    let query = query_regex().replace_all(&header, "$1[REDACTED]");
    token_regex().replace_all(&query, "[REDACTED]").into_owned()
}

pub fn redact_packet(packet: &ReviewPacket) -> ReviewPacket {
    let mut redacted = packet.clone();
    redacted.task = redact_text(&redacted.task);
    for turn in &mut redacted.turns {
        turn.text = redact_text(&turn.text);
    }
    for artifact in &mut redacted.artifacts {
        artifact.content = redact_text(&artifact.content);
    }
    redacted
}

fn assignment_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)((?:[A-Z][A-Z0-9]*_)*(?:api[_-]?key|auth(?:entication)?[_-]?token|access[_-]?token|client[_-]?secret|password|secret)\b\s*[:=]\s*[\"']?)[^\"'\s,;}]+"#,
        )
        .expect("credential assignment regex is valid")
    })
}

fn header_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\b(authorization\s*:\s*bearer\s+|x-api-key\s*:\s*)[^\s,}]+")
            .expect("credential header regex is valid")
    })
}

fn query_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)([?&](?:api[_-]?key|token|secret|password)=)[^&#\s]+")
            .expect("credential query regex is valid")
    })
}

fn token_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"\b(?:sk-ant-[A-Za-z0-9_-]+|sk-[A-Za-z0-9_-]+|ghp_[A-Za-z0-9]+|github_pat_[A-Za-z0-9_]+)\b")
            .expect("known token regex is valid")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Artifact, REVIEW_PACKET_SCHEMA, Role, Turn};

    #[test]
    fn redacts_common_credentials_but_preserves_prose() {
        let input = "API_KEY=sk-ant-secret123 Authorization: Bearer bearer-secret x-api-key: key-secret https://example.test/?token=url-secret hello";
        let output = redact_text(input);
        assert!(!output.contains("secret123"));
        assert!(!output.contains("bearer-secret"));
        assert!(!output.contains("key-secret"));
        assert!(!output.contains("url-secret"));
        assert!(output.contains("hello"));
        assert_eq!(output.matches("[REDACTED]").count(), 4);
    }

    #[test]
    fn packet_redaction_does_not_change_identity_or_structure() {
        let packet = ReviewPacket {
            schema_version: REVIEW_PACKET_SCHEMA.to_owned(),
            packet_id: "packet-1".to_owned(),
            session_id: "session-1".to_owned(),
            source: "opencode".to_owned(),
            client: "opencode".to_owned(),
            generator_model: None,
            task: "Use API_KEY=secret".to_owned(),
            turns: vec![Turn {
                role: Role::User,
                text: "hello".to_owned(),
            }],
            artifacts: vec![Artifact {
                kind: "test_output".to_owned(),
                path: None,
                content: "API_KEY=secret".to_owned(),
                complete: true,
            }],
            generation_usage: None,
        };
        let redacted = packet.redacted();
        assert_eq!(redacted.packet_id, packet.packet_id);
        assert_eq!(redacted.task, "Use API_KEY=[REDACTED]");
        assert_eq!(redacted.artifacts[0].content, "API_KEY=[REDACTED]");
        redacted.validate().unwrap();
    }
}
