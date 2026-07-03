//! The normalized run model.
//!
//! A Claude Code session on disk is a tree of records linked by `parentUuid`.
//! We normalize the active root→leaf path into an ordered list of [`Step`]s,
//! each carrying a content-addressed hash. That hashing is what makes a run a
//! *reproducible artifact*: two runs are identical iff their step hashes match,
//! replay is verifiable by recomputing the digest, and a diff is just a walk
//! over mismatched hashes.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

/// A normalized content block. We keep tool I/O verbatim (it is the load-bearing
/// part of a run) but reduce free-form thinking to its length — thinking is
/// non-reproducible model internals and would make every run hash unique.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Thinking {
        len: usize,
    },
    ToolUse {
        id: String,
        name: String,
        /// Canonicalized JSON of the tool input, as a string, so hashing is stable.
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        output: String,
        is_error: bool,
    },
}

impl Block {
    /// Feed the block's semantic content into a hasher. Deliberately excludes
    /// volatile identifiers (tool_use ids, which are random per run) so that two
    /// runs that did the same work hash the same.
    fn hash_into(&self, h: &mut Sha256) {
        match self {
            Block::Text { text } => {
                h.update(b"text\0");
                h.update(text.as_bytes());
            }
            Block::Thinking { len } => {
                h.update(b"thinking\0");
                h.update(len.to_le_bytes());
            }
            Block::ToolUse { name, input, .. } => {
                h.update(b"tool_use\0");
                h.update(name.as_bytes());
                h.update(b"\0");
                h.update(input.as_bytes());
            }
            Block::ToolResult {
                output, is_error, ..
            } => {
                h.update(b"tool_result\0");
                h.update([*is_error as u8]);
                h.update(output.as_bytes());
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub seq: usize,
    pub uuid: String,
    pub parent: Option<String>,
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
    pub blocks: Vec<Block>,
    /// Content-addressed hash of (role, blocks). Stable across runs doing the
    /// same work; changes the instant the agent does something different.
    pub hash: String,
}

impl Step {
    /// Compute the content hash of a step from its role and blocks.
    pub fn compute_hash(role: &Role, blocks: &[Block]) -> String {
        let mut h = Sha256::new();
        match role {
            Role::User => h.update(b"user\n"),
            Role::Assistant => h.update(b"assistant\n"),
            Role::System => h.update(b"system\n"),
        }
        for b in blocks {
            b.hash_into(&mut h);
            h.update(b"\x1e"); // record separator between blocks
        }
        short_hex(&h.finalize())
    }

    /// True if the step made at least one tool call.
    pub fn tool_names(&self) -> Vec<&str> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn has_error_result(&self) -> bool {
        self.blocks
            .iter()
            .any(|b| matches!(b, Block::ToolResult { is_error: true, .. }))
    }

    /// The concatenated visible text of the step (for prompts / previews).
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_version: Option<String>,
    /// Where this run came from, e.g. "transcript:<path>" or "fork:<parent>@<seq>".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Format version of the .replay file.
    pub format: u32,
    pub meta: RunMeta,
    pub steps: Vec<Step>,
}

pub const FORMAT_VERSION: u32 = 1;

impl Run {
    pub fn new(meta: RunMeta, steps: Vec<Step>) -> Self {
        Run {
            format: FORMAT_VERSION,
            meta,
            steps,
        }
    }

    /// The run-level digest: a hash over every step hash in order. Two runs are
    /// byte-for-byte reproductions of each other iff their digests match.
    pub fn digest(&self) -> String {
        let mut h = Sha256::new();
        for s in &self.steps {
            h.update(s.hash.as_bytes());
            h.update(b"\n");
        }
        short_hex(&h.finalize())
    }

    /// Recompute every step hash from its content and confirm it matches the
    /// stored hash. Returns the seq of the first tampered/corrupt step, if any.
    pub fn verify_integrity(&self) -> Result<(), usize> {
        for s in &self.steps {
            if Step::compute_hash(&s.role, &s.blocks) != s.hash {
                return Err(s.seq);
            }
        }
        Ok(())
    }

    pub fn assistant_steps(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.role == Role::Assistant)
            .count()
    }

    pub fn tool_calls(&self) -> usize {
        self.steps.iter().map(|s| s.tool_names().len()).sum()
    }
}

/// A short, human-friendly hex prefix (12 bytes / 24 hex chars) of a digest.
/// Full 256-bit collision resistance is unnecessary for run identity; a 96-bit
/// prefix keeps the display compact while remaining collision-safe in practice.
fn short_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(24);
    for b in &bytes[..12] {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_step(seq: usize, role: Role, text: &str) -> Step {
        let blocks = vec![Block::Text { text: text.into() }];
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

    #[test]
    fn hash_is_stable_and_order_sensitive() {
        let a = Step::compute_hash(&Role::User, &[Block::Text { text: "hi".into() }]);
        let b = Step::compute_hash(&Role::User, &[Block::Text { text: "hi".into() }]);
        assert_eq!(a, b, "same content hashes the same");
        let c = Step::compute_hash(&Role::Assistant, &[Block::Text { text: "hi".into() }]);
        assert_ne!(a, c, "role participates in the hash");
    }

    #[test]
    fn tooluse_id_does_not_affect_hash_but_input_does() {
        let h1 = Step::compute_hash(
            &Role::Assistant,
            &[Block::ToolUse {
                id: "a".into(),
                name: "Bash".into(),
                input: "{\"x\":1}".into(),
            }],
        );
        let h2 = Step::compute_hash(
            &Role::Assistant,
            &[Block::ToolUse {
                id: "DIFFERENT".into(),
                name: "Bash".into(),
                input: "{\"x\":1}".into(),
            }],
        );
        let h3 = Step::compute_hash(
            &Role::Assistant,
            &[Block::ToolUse {
                id: "a".into(),
                name: "Bash".into(),
                input: "{\"x\":2}".into(),
            }],
        );
        assert_eq!(h1, h2, "volatile tool_use id is excluded from the hash");
        assert_ne!(h1, h3, "tool input is included in the hash");
    }

    #[test]
    fn digest_and_integrity() {
        let run = Run::new(
            RunMeta::default(),
            vec![
                text_step(0, Role::User, "do X"),
                text_step(1, Role::Assistant, "ok"),
            ],
        );
        assert!(run.verify_integrity().is_ok());
        assert_eq!(
            run.digest(),
            run.clone().digest(),
            "digest is deterministic"
        );

        // Tamper with a step's content without updating its hash.
        let mut bad = run.clone();
        bad.steps[1].blocks = vec![Block::Text {
            text: "tampered".into(),
        }];
        assert_eq!(
            bad.verify_integrity(),
            Err(1),
            "integrity catches tampering at the right step"
        );
    }

    #[test]
    fn helpers() {
        let s = Step {
            seq: 0,
            uuid: "u".into(),
            parent: None,
            role: Role::Assistant,
            ts: None,
            blocks: vec![
                Block::Text {
                    text: "hello".into(),
                },
                Block::ToolUse {
                    id: "1".into(),
                    name: "Read".into(),
                    input: "{}".into(),
                },
                Block::ToolResult {
                    tool_use_id: "1".into(),
                    output: "boom".into(),
                    is_error: true,
                },
            ],
            hash: String::new(),
        };
        assert_eq!(s.tool_names(), vec!["Read"]);
        assert!(s.has_error_result());
        assert_eq!(s.text(), "hello");
    }
}
