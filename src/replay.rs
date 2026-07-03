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

pub struct ReplayReport {
    pub digest: String,
    pub steps: usize,
    pub tool_calls: usize,
    /// None if every recomputed step hash matched; Some(seq) at the first that did not.
    pub integrity_break: Option<usize>,
}

pub fn replay(run: &Run) -> ReplayReport {
    ReplayReport {
        digest: run.digest(),
        steps: run.steps.len(),
        tool_calls: run.tool_calls(),
        integrity_break: run.verify_integrity().err(),
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
