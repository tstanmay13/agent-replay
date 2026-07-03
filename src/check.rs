//! Rule-based assertions over a run, for regression-testing agents in CI.
//!
//! The point of recording a run as a fixture is being able to assert on it:
//! "this task should never touch the secrets file", "should finish in ≤20
//! steps", "should not end on a tool error". `check` evaluates such rules and
//! exits non-zero on violation, so a replayed session becomes a CI gate.

use crate::model::{Block, Run};

#[derive(Debug, Default)]
pub struct Rules {
    pub max_steps: Option<usize>,
    pub max_tool_calls: Option<usize>,
    /// Fail if any of these tools was called.
    pub forbid_tools: Vec<String>,
    /// Fail if any tool input mentions any of these substrings (e.g. a secret path).
    pub forbid_paths: Vec<String>,
    /// Fail if the run contains any errored tool result.
    pub must_succeed: bool,
}

impl Rules {
    pub fn is_empty(&self) -> bool {
        self.max_steps.is_none()
            && self.max_tool_calls.is_none()
            && self.forbid_tools.is_empty()
            && self.forbid_paths.is_empty()
            && !self.must_succeed
    }
}

pub struct Violation {
    pub rule: String,
    pub detail: String,
}

pub fn check(run: &Run, rules: &Rules) -> Vec<Violation> {
    let mut v = Vec::new();

    if let Some(max) = rules.max_steps {
        if run.steps.len() > max {
            v.push(Violation {
                rule: "max-steps".into(),
                detail: format!("{} steps > limit {}", run.steps.len(), max),
            });
        }
    }
    if let Some(max) = rules.max_tool_calls {
        let n = run.tool_calls();
        if n > max {
            v.push(Violation {
                rule: "max-tool-calls".into(),
                detail: format!("{} tool calls > limit {}", n, max),
            });
        }
    }
    for step in &run.steps {
        for block in &step.blocks {
            if let Block::ToolUse { name, input, .. } = block {
                if rules.forbid_tools.iter().any(|f| f == name) {
                    v.push(Violation {
                        rule: "forbid-tool".into(),
                        detail: format!("step {} called forbidden tool `{}`", step.seq, name),
                    });
                }
                for p in &rules.forbid_paths {
                    if input.contains(p.as_str()) {
                        v.push(Violation {
                            rule: "forbid-path".into(),
                            detail: format!(
                                "step {} tool `{}` input references forbidden `{}`",
                                step.seq, name, p
                            ),
                        });
                    }
                }
            }
        }
    }
    if rules.must_succeed {
        for step in &run.steps {
            if step.has_error_result() {
                v.push(Violation {
                    rule: "must-succeed".into(),
                    detail: format!("step {} has an errored tool result", step.seq),
                });
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Role, RunMeta, Step};

    fn run_with(blocks: Vec<Block>) -> Run {
        let hash = Step::compute_hash(&Role::Assistant, &blocks);
        Run::new(
            RunMeta::default(),
            vec![Step {
                seq: 0,
                uuid: "u".into(),
                parent: None,
                role: Role::Assistant,
                ts: None,
                blocks,
                hash,
            }],
        )
    }

    #[test]
    fn forbid_tool_and_path() {
        let r = run_with(vec![Block::ToolUse {
            id: "1".into(),
            name: "Bash".into(),
            input: "{\"command\":\"cat /etc/secrets\"}".into(),
        }]);
        let rules = Rules {
            forbid_tools: vec!["Bash".into()],
            forbid_paths: vec!["/etc/secrets".into()],
            ..Default::default()
        };
        let v = check(&r, &rules);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn must_succeed_flags_errored_result() {
        let r = run_with(vec![Block::ToolResult {
            tool_use_id: "1".into(),
            output: "boom".into(),
            is_error: true,
        }]);
        let v = check(
            &r,
            &Rules {
                must_succeed: true,
                ..Default::default()
            },
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule, "must-succeed");
    }

    #[test]
    fn max_steps_and_clean_pass() {
        let r = run_with(vec![Block::Text { text: "hi".into() }]);
        assert!(check(
            &r,
            &Rules {
                max_steps: Some(10),
                ..Default::default()
            }
        )
        .is_empty());
        assert_eq!(
            check(
                &r,
                &Rules {
                    max_steps: Some(0),
                    ..Default::default()
                }
            )
            .len(),
            1
        );
    }
}
