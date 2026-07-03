//! Deterministic replay.
//!
//! "Replay" reconstructs a run's observable behavior from the recording alone —
//! no model call, no network, no tool execution. Every assistant turn and its
//! tool results are re-emitted from the stored steps, and each step's content
//! hash is recomputed and checked against the recording. If all hashes match,
//! the replay is *verified deterministic*: the recording is a faithful, tamper-
//! evident fixture of the run.
//!
//! This is the honest boundary the project takes a stance on: it is
//! record-substitution replay (deterministic given the recorded model and tool
//! I/O), not full-OS replay of filesystem or clock state.

use crate::model::{Block, Run};
use serde::Serialize;

#[derive(Serialize)]
pub struct ReplayReport {
    pub digest: String,
    pub steps: usize,
    pub tool_calls: usize,
    /// None if every recomputed step hash matched; Some(seq) at the first that did not.
    pub integrity_break: Option<usize>,
    /// True when every recomputed step hash matched the recording.
    pub verified: bool,
}

pub fn replay(run: &Run) -> ReplayReport {
    let integrity_break = run.verify_integrity().err();
    ReplayReport {
        digest: run.digest(),
        steps: run.steps.len(),
        tool_calls: run.tool_calls(),
        verified: integrity_break.is_none(),
        integrity_break,
    }
}

/// A compact, human-readable transcript of the reconstructed run. `substitute`
/// controls whether tool results are shown as replayed (they always are here —
/// the flag is surfaced in the CLI to make the substitution explicit).
pub fn render_playback(run: &Run, max_chars: usize) -> String {
    let mut out = String::new();
    for step in &run.steps {
        let tag = match step.role {
            crate::model::Role::User => "user",
            crate::model::Role::Assistant => "assistant",
            crate::model::Role::System => "system",
        };
        out.push_str(&format!("[{:>3}] {}\n", step.seq, tag));
        for block in &step.blocks {
            match block {
                Block::Text { text } => {
                    out.push_str(&format!("      {}\n", truncate(text, max_chars)));
                }
                Block::Thinking { len } => {
                    out.push_str(&format!("      (thinking, {len} chars)\n"));
                }
                Block::ToolUse { name, input, .. } => {
                    out.push_str(&format!(
                        "      → {} {}\n",
                        name,
                        truncate(input, max_chars)
                    ));
                }
                Block::ToolResult {
                    output, is_error, ..
                } => {
                    let marker = if *is_error { "✘ error" } else { "✔" };
                    out.push_str(&format!(
                        "      {} {}\n",
                        marker,
                        truncate(output, max_chars)
                    ));
                }
            }
        }
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        let cut: String = one_line.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Role, RunMeta, Step};

    fn run() -> Run {
        let mk = |seq: usize, role: Role, t: &str| {
            let blocks = vec![Block::Text { text: t.into() }];
            let hash = Step::compute_hash(&role, &blocks);
            Step {
                seq,
                uuid: format!("u{seq}"),
                parent: None,
                role,
                ts: None,
                blocks,
                hash,
            }
        };
        Run::new(
            RunMeta::default(),
            vec![mk(0, Role::User, "hi"), mk(1, Role::Assistant, "ok")],
        )
    }

    #[test]
    fn clean_run_verifies() {
        let r = replay(&run());
        assert!(r.verified);
        assert_eq!(r.integrity_break, None);
        assert_eq!(r.steps, 2);
    }

    #[test]
    fn tampered_run_fails_verification_at_the_right_step() {
        let mut run = run();
        run.steps[1].blocks = vec![Block::Text {
            text: "tampered".into(),
        }]; // hash now stale
        let r = replay(&run);
        assert!(!r.verified);
        assert_eq!(r.integrity_break, Some(1));
    }
}
