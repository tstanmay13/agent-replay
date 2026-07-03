//! Fork a run at a step to explore an alternative.
//!
//! A fork keeps the run's prefix (steps 0..=at) verbatim and, if a new prompt
//! is given, appends it as the next user step — the seed for a "what if I'd
//! said X instead" re-run. Because the on-disk session is already a tree, this
//! is the same operation Claude Code performs when you edit an earlier message;
//! agent-replay just makes it explicit and portable.

use crate::model::{Block, Role, Run, RunMeta, Step};
use anyhow::{anyhow, Result};

pub fn fork(run: &Run, at: usize, new_prompt: Option<&str>) -> Result<Run> {
    if at >= run.steps.len() {
        return Err(anyhow!(
            "cannot fork at step {}: run has {} steps (0..{})",
            at,
            run.steps.len(),
            run.steps.len().saturating_sub(1)
        ));
    }

    let mut steps: Vec<Step> = run.steps[..=at].to_vec();

    if let Some(prompt) = new_prompt {
        let blocks = vec![Block::Text {
            text: prompt.to_string(),
        }];
        let hash = Step::compute_hash(&Role::User, &blocks);
        let parent = steps.last().map(|s| s.uuid.clone());
        steps.push(Step {
            seq: steps.len(),
            uuid: format!("fork-{}", &hash[..8]),
            parent,
            role: Role::User,
            ts: None,
            blocks,
            hash,
        });
    }

    let meta = RunMeta {
        source: Some(format!(
            "fork:{}@{}",
            run.meta.session_id.as_deref().unwrap_or("run"),
            at
        )),
        ..run.meta.clone()
    };
    Ok(Run::new(meta, steps))
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
            vec![
                mk(0, Role::User, "a"),
                mk(1, Role::Assistant, "b"),
                mk(2, Role::User, "c"),
            ],
        )
    }

    #[test]
    fn keeps_prefix_and_seeds_prompt() {
        let f = fork(&run(), 1, Some("new direction")).unwrap();
        assert_eq!(f.steps.len(), 3); // steps 0,1 + new prompt
        assert_eq!(f.steps[2].role, Role::User);
        assert_eq!(f.steps[2].text(), "new direction");
        assert!(f.verify_integrity().is_ok());
    }

    #[test]
    fn fork_without_prompt_just_truncates() {
        let f = fork(&run(), 0, None).unwrap();
        assert_eq!(f.steps.len(), 1);
    }

    #[test]
    fn fork_past_end_errors() {
        assert!(fork(&run(), 99, None).is_err());
    }
}
