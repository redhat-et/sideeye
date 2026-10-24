//! Context budgeting for review packets.
//!
//! A real OpenCode session can be far larger than a judge's context window:
//! long transcripts plus a full branch diff. This module keeps human intent and
//! the final conclusion, then keeps the newest tool evidence and narration
//! within a character budget. Dropped content is marked in-band so the judge
//! knows its view is partial.

use crate::{Artifact, ReviewPacket, Role, Turn};

const USER_TURN_CAP: usize = 8_000;
const ASSISTANT_TURN_CAP: usize = 8_000;
const TOOL_TURN_CAP: usize = 4_000;
const SYSTEM_TURN_CAP: usize = 2_000;
const MIN_USER_TURN_CAP: usize = 1_000;

impl ReviewPacket {
    /// Return a packet whose transcript and artifacts fit `max_chars`.
    ///
    /// `max_chars == 0` disables budgeting. The original packet is returned
    /// unchanged when it already fits.
    pub fn budgeted(&self, max_chars: usize) -> ReviewPacket {
        if max_chars == 0 {
            return self.clone();
        }
        let total = self.turns.iter().map(|turn| turn.text.len()).sum::<usize>()
            + self
                .artifacts
                .iter()
                .map(|artifact| artifact.content.len())
                .sum::<usize>();
        if total <= max_chars {
            return self.clone();
        }

        let artifact_budget = (max_chars / 4).min(120_000);
        let transcript_budget = max_chars.saturating_sub(artifact_budget);
        let mut budgeted = self.clone();
        budgeted.artifacts = budget_artifacts(&self.artifacts, artifact_budget);
        let (turns, trimmed) = budget_turns(&self.turns, transcript_budget);
        budgeted.turns = turns;
        if trimmed {
            budgeted.turns.push(Turn {
                role: Role::System,
                text: "[Side-Eye budget: lower-salience turns were trimmed to fit the judge context window. Human turns and the final assistant turn were preserved; tool evidence and narration were kept newest-first.]".to_owned(),
            });
        }
        budgeted
    }
}

fn budget_artifacts(artifacts: &[Artifact], budget: usize) -> Vec<Artifact> {
    if artifacts.is_empty() || budget == 0 {
        return Vec::new();
    }
    let mut remaining = budget;
    let mut out = Vec::new();
    for (index, artifact) in artifacts.iter().enumerate() {
        let left = artifacts.len() - index;
        let share = remaining / left;
        let (content, complete) = cap(&artifact.content, share);
        remaining = remaining.saturating_sub(content.len());
        out.push(Artifact {
            content,
            complete: artifact.complete && !complete,
            ..artifact.clone()
        });
    }
    out
}

fn budget_turns(turns: &[Turn], budget: usize) -> (Vec<Turn>, bool) {
    if turns.is_empty() {
        return (Vec::new(), false);
    }
    let mut limits: Vec<usize> = turns
        .iter()
        .map(|turn| match turn.role {
            Role::User => USER_TURN_CAP,
            Role::Assistant => ASSISTANT_TURN_CAP,
            Role::Tool => TOOL_TURN_CAP,
            Role::System => SYSTEM_TURN_CAP,
        })
        .collect();

    let last_assistant = turns
        .iter()
        .rposition(|turn| turn.role == Role::Assistant && !turn.text.trim().is_empty());
    let user_count = turns.iter().filter(|turn| turn.role == Role::User).count();

    // All human turns are load-bearing. If they alone overflow, tighten each
    // one rather than dropping the user's intent.
    let user_cost = |limits: &[usize]| {
        turns
            .iter()
            .zip(limits)
            .filter(|(turn, _)| turn.role == Role::User)
            .map(|(turn, limit)| turn.text.len().min(*limit))
            .sum::<usize>()
    };
    if user_cost(&limits) > budget && user_count > 0 {
        let per_user = (budget / user_count).max(MIN_USER_TURN_CAP);
        for (turn, limit) in turns.iter().zip(limits.iter_mut()) {
            if turn.role == Role::User {
                *limit = (*limit).min(per_user);
            }
        }
    }

    let capped: Vec<(String, bool)> = turns
        .iter()
        .zip(&limits)
        .map(|(turn, limit)| cap(&turn.text, *limit))
        .collect();

    let mut selected = vec![false; turns.len()];
    let mut used = 0usize;
    for (index, turn) in turns.iter().enumerate() {
        if turn.role == Role::User {
            selected[index] = true;
            used += capped[index].0.len();
        }
    }
    if let Some(index) = last_assistant
        && !selected[index]
    {
        selected[index] = true;
        used += capped[index].0.len();
    }

    // Add remaining turns newest-first by salience: tool evidence, then
    // assistant narration, then system notes.
    let mut order: Vec<usize> = (0..turns.len()).filter(|index| !selected[*index]).collect();
    order.sort_by_key(|index| match turns[*index].role {
        Role::Tool => 0,
        Role::Assistant => 1,
        Role::System => 2,
        Role::User => 3,
    });
    order.reverse();
    for index in order {
        let cost = capped[index].0.len();
        if used + cost > budget {
            continue;
        }
        selected[index] = true;
        used += cost;
    }

    let trimmed =
        selected.iter().any(|keep| !keep) || capped.iter().any(|(_, was_capped)| *was_capped);
    let mut out = Vec::new();
    let mut skipped = false;
    for (index, turn) in turns.iter().enumerate() {
        if !selected[index] {
            skipped = true;
            continue;
        }
        if skipped {
            out.push(Turn {
                role: Role::System,
                text: "[Side-Eye budget: turns elided here.]".to_owned(),
            });
            skipped = false;
        }
        out.push(Turn {
            role: turn.role.clone(),
            text: capped[index].0.clone(),
        });
    }
    (out, trimmed)
}

fn cap(text: &str, cap: usize) -> (String, bool) {
    if text.len() <= cap {
        return (text.to_owned(), false);
    }
    let head = cap * 3 / 4;
    let tail = cap - head;
    let mut out = String::with_capacity(cap + 64);
    out.push_str(&text[..floor_char_boundary(text, head)]);
    out.push_str(&format!(
        "\n... ({} chars elided to fit the budget) ...\n",
        text.len() - cap
    ));
    out.push_str(&text[floor_char_boundary(text, text.len() - tail)..]);
    (out, true)
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{REVIEW_PACKET_SCHEMA, ReviewPacket};

    fn packet(turns: Vec<Turn>) -> ReviewPacket {
        ReviewPacket {
            schema_version: REVIEW_PACKET_SCHEMA.to_owned(),
            packet_id: "p".to_owned(),
            session_id: "s".to_owned(),
            source: "opencode".to_owned(),
            client: "opencode".to_owned(),
            generator_model: None,
            task: "task".to_owned(),
            turns,
            artifacts: vec![Artifact {
                kind: "git_diff".to_owned(),
                path: None,
                content: "d".repeat(100_000),
                complete: true,
            }],
            generation_usage: None,
            generator_models: vec![],
        }
    }

    #[test]
    fn fits_a_large_packet_and_keeps_human_and_final_turn() {
        let mut turns = vec![
            Turn {
                role: Role::User,
                text: "first ask".to_owned(),
            },
            Turn {
                role: Role::Tool,
                text: "tool ".repeat(50_000),
            },
        ];
        for index in 0..200 {
            turns.push(Turn {
                role: Role::Assistant,
                text: format!("narration {index} {}", "x".repeat(2_000)),
            });
        }
        turns.push(Turn {
            role: Role::Assistant,
            text: "final conclusion".to_owned(),
        });
        let packet = packet(turns);
        let budgeted = packet.budgeted(50_000);
        let size = budgeted
            .turns
            .iter()
            .map(|turn| turn.text.len())
            .sum::<usize>()
            + budgeted
                .artifacts
                .iter()
                .map(|artifact| artifact.content.len())
                .sum::<usize>();
        assert!(size <= 50_000, "size {size}");
        assert!(budgeted.turns.iter().any(|turn| turn.text == "first ask"));
        assert!(
            budgeted
                .turns
                .iter()
                .any(|turn| turn.text == "final conclusion")
        );
        assert!(
            budgeted
                .turns
                .iter()
                .any(|turn| turn.text.contains("Side-Eye budget"))
        );
        assert!(!budgeted.artifacts[0].complete);
    }

    #[test]
    fn leaves_a_small_packet_unchanged() {
        let packet = packet(vec![Turn {
            role: Role::User,
            text: "hello".to_owned(),
        }]);
        assert_eq!(packet.budgeted(1_000_000), packet);
    }
}
