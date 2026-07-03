//! Semantic diff between two runs.
//!
//! Runs are compared step-by-step by content hash, not by text. The output
//! answers the question that matters when an agent regresses: *where did the
//! two runs first diverge, and what changed* — a different tool call, a changed
//! tool input, a different result, or added/removed steps.

use crate::model::{Block, Run, Step};

#[derive(Debug, PartialEq, Eq)]
pub enum StepDiff {
    Same,
    Changed { detail: String },
    OnlyInA,
    OnlyInB,
}

pub struct RunDiff {
    pub rows: Vec<(usize, StepDiff)>,
    pub first_divergence: Option<usize>,
    pub identical: bool,
}

pub fn diff(a: &Run, b: &Run) -> RunDiff {
    let mut rows = Vec::new();
    let mut first_divergence = None;
    let max = a.steps.len().max(b.steps.len());

    for i in 0..max {
        let sa = a.steps.get(i);
        let sb = b.steps.get(i);
        let d = match (sa, sb) {
            (Some(sa), Some(sb)) if sa.hash == sb.hash => StepDiff::Same,
            (Some(sa), Some(sb)) => StepDiff::Changed {
                detail: describe_change(sa, sb),
            },
            (Some(_), None) => StepDiff::OnlyInA,
            (None, Some(_)) => StepDiff::OnlyInB,
            (None, None) => unreachable!(),
        };
        if d != StepDiff::Same && first_divergence.is_none() {
            first_divergence = Some(i);
        }
        rows.push((i, d));
    }

    RunDiff {
        identical: first_divergence.is_none(),
        first_divergence,
        rows,
    }
}

/// Describe, in one line, how two same-position steps differ — prioritizing the
/// signal a debugger wants first (role change, then tool calls, then text).
fn describe_change(a: &Step, b: &Step) -> String {
    if a.role != b.role {
        return format!("role changed: {:?} → {:?}", a.role, b.role);
    }
    let ta = a.tool_names();
    let tb = b.tool_names();
    if ta != tb {
        return format!(
            "tool calls changed: [{}] → [{}]",
            ta.join(", "),
            tb.join(", ")
        );
    }
    // Same tool names, in order — look for changed inputs or results.
    if let Some(d) = first_block_change(&a.blocks, &b.blocks) {
        return d;
    }
    "content changed".to_string()
}

fn first_block_change(a: &[Block], b: &[Block]) -> Option<String> {
    for (x, y) in a.iter().zip(b.iter()) {
        match (x, y) {
            (
                Block::ToolUse {
                    name, input: ia, ..
                },
                Block::ToolUse { input: ib, .. },
            ) if ia != ib => {
                return Some(format!("tool `{}` input changed", name));
            }
            (
                Block::ToolResult {
                    is_error: ea,
                    output: oa,
                    ..
                },
                Block::ToolResult {
                    is_error: eb,
                    output: ob,
                    ..
                },
            ) if ea != eb || oa != ob => {
                let flip = match (ea, eb) {
                    (false, true) => " (now errored)",
                    (true, false) => " (now succeeded)",
                    _ => "",
                };
                return Some(format!("tool result changed{}", flip));
            }
            (Block::Text { text: xa }, Block::Text { text: xb }) if xa != xb => {
                return Some("assistant text changed".to_string());
            }
            _ => {}
        }
    }
    if a.len() != b.len() {
        return Some(format!("block count changed: {} → {}", a.len(), b.len()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Role, RunMeta, Step};

    fn step(seq: usize, role: Role, blocks: Vec<Block>) -> Step {
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
    }
    fn txt(s: &str) -> Block {
        Block::Text { text: s.into() }
    }
    fn run(steps: Vec<Step>) -> Run {
        Run::new(RunMeta::default(), steps)
    }

    #[test]
    fn identical_runs() {
        let a = run(vec![step(0, Role::User, vec![txt("hi")])]);
        let b = run(vec![step(0, Role::User, vec![txt("hi")])]);
        let d = diff(&a, &b);
        assert!(d.identical);
        assert_eq!(d.first_divergence, None);
    }

    #[test]
    fn detects_changed_tool_and_divergence_point() {
        let a = run(vec![
            step(0, Role::User, vec![txt("go")]),
            step(
                1,
                Role::Assistant,
                vec![Block::ToolUse {
                    id: "1".into(),
                    name: "Read".into(),
                    input: "{}".into(),
                }],
            ),
        ]);
        let b = run(vec![
            step(0, Role::User, vec![txt("go")]),
            step(
                1,
                Role::Assistant,
                vec![Block::ToolUse {
                    id: "1".into(),
                    name: "Edit".into(),
                    input: "{}".into(),
                }],
            ),
        ]);
        let d = diff(&a, &b);
        assert!(!d.identical);
        assert_eq!(d.first_divergence, Some(1));
        match &d.rows[1].1 {
            StepDiff::Changed { detail } => assert!(detail.contains("tool calls changed")),
            _ => panic!("expected a change at step 1"),
        }
    }

    #[test]
    fn detects_added_steps() {
        let a = run(vec![step(0, Role::User, vec![txt("hi")])]);
        let b = run(vec![
            step(0, Role::User, vec![txt("hi")]),
            step(1, Role::Assistant, vec![txt("more")]),
        ]);
        let d = diff(&a, &b);
        assert_eq!(d.rows[1].1, StepDiff::OnlyInB);
    }
}
