//! Parse a Claude Code session transcript (JSONL) into a normalized [`Run`].
//!
//! On-disk records are linked by `parentUuid` into a tree — sidechains, edited
//! prompts, and retries create branches. We reconstruct the tree and follow the
//! path from the root to the *active leaf* (the deepest node reachable on the
//! main line), which is the conversation as it actually ended. Meta record
//! types (mode changes, snapshots, titles) are ignored.

use crate::model::{Block, Role, Run, RunMeta, Step};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// A raw record we care about, extracted from one JSONL line.
struct Raw {
    uuid: String,
    parent: Option<String>,
    role: Role,
    ts: Option<String>,
    blocks: Vec<Block>,
    is_meta: bool,
}

pub fn parse_file(path: &Path) -> Result<Run> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading transcript {}", path.display()))?;
    let mut run = parse_str(&text)?;
    if run.meta.source.is_none() {
        run.meta.source = Some(format!("transcript:{}", path.display()));
    }
    Ok(run)
}

pub fn parse_str(text: &str) -> Result<Run> {
    let mut raws: Vec<Raw> = Vec::new();
    let mut meta = RunMeta::default();
    // Parent links for EVERY record (any type), so we can bridge the chain
    // through non-message records (attachments, snapshots) that sit between two
    // messages — otherwise the conversation fragments and we follow a stub.
    let mut all_parents: HashMap<String, Option<String>> = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // tolerate the occasional non-JSON line
        };
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        let uuid_opt = v.get("uuid").and_then(Value::as_str).map(String::from);
        let parent = v
            .get("parentUuid")
            .and_then(Value::as_str)
            .map(String::from);
        if let Some(u) = &uuid_opt {
            all_parents.insert(u.clone(), parent.clone());
        }
        // Capture session metadata from the first record that carries it.
        if meta.session_id.is_none() {
            meta.session_id = v.get("sessionId").and_then(Value::as_str).map(String::from);
            meta.cwd = v.get("cwd").and_then(Value::as_str).map(String::from);
            meta.git_branch = v.get("gitBranch").and_then(Value::as_str).map(String::from);
            meta.claude_version = v.get("version").and_then(Value::as_str).map(String::from);
        }
        let role = match ty {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            _ => continue, // mode/attachment/snapshot/title/etc. are not steps
        };
        let Some(uuid) = uuid_opt else { continue };
        let ts = v.get("timestamp").and_then(Value::as_str).map(String::from);
        let is_meta = v.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
        let blocks = extract_blocks(v.get("message"));
        // Skip records that carry no normalized content (e.g. meta pings).
        if blocks.is_empty() {
            continue;
        }
        raws.push(Raw {
            uuid,
            parent, // rewritten to the effective message-parent below
            role,
            ts,
            blocks,
            is_meta,
        });
    }

    if raws.is_empty() {
        return Err(anyhow!("no user/assistant records found in transcript"));
    }

    // Rewrite each message's parent to its nearest *message* ancestor, walking
    // up through any non-message records that broke the direct link.
    let message_ids: std::collections::HashSet<&str> =
        raws.iter().map(|r| r.uuid.as_str()).collect();
    let effective: HashMap<String, Option<String>> = raws
        .iter()
        .map(|r| {
            (
                r.uuid.clone(),
                effective_parent(&r.uuid, &all_parents, &message_ids),
            )
        })
        .collect();
    for r in &mut raws {
        r.parent = effective.get(&r.uuid).cloned().flatten();
    }

    let ordered = follow_active_path(&raws);
    let steps = ordered
        .into_iter()
        .enumerate()
        .map(|(seq, r)| {
            let hash = Step::compute_hash(&r.role, &r.blocks);
            Step {
                seq,
                uuid: r.uuid.clone(),
                parent: r.parent.clone(),
                role: r.role.clone(),
                ts: r.ts.clone(),
                blocks: r.blocks.clone(),
                hash,
            }
        })
        .collect();

    Ok(Run::new(meta, steps))
}

/// Walk up the full parent chain from `uuid` until reaching a record that is a
/// message, returning its uuid. Bridges non-message records (attachments,
/// snapshots) that would otherwise fragment the conversation tree.
fn effective_parent(
    uuid: &str,
    all_parents: &HashMap<String, Option<String>>,
    message_ids: &std::collections::HashSet<&str>,
) -> Option<String> {
    let mut cur = all_parents.get(uuid).cloned().flatten();
    let mut guard = 0;
    while let Some(p) = cur {
        if message_ids.contains(p.as_str()) {
            return Some(p);
        }
        cur = all_parents.get(&p).cloned().flatten();
        guard += 1;
        if guard > 100_000 {
            break;
        }
    }
    None
}

/// Extract normalized blocks from a `message` value, whose `content` is either
/// a plain string (a user prompt) or an array of typed blocks.
fn extract_blocks(message: Option<&Value>) -> Vec<Block> {
    let Some(msg) = message else { return vec![] };
    let content = msg.get("content");
    match content {
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                vec![]
            } else {
                vec![Block::Text { text: s.clone() }]
            }
        }
        Some(Value::Array(items)) => items.iter().filter_map(block_from_value).collect(),
        _ => vec![],
    }
}

fn block_from_value(b: &Value) -> Option<Block> {
    match b.get("type").and_then(Value::as_str)? {
        "text" => Some(Block::Text {
            text: b.get("text").and_then(Value::as_str)?.to_string(),
        }),
        "thinking" => Some(Block::Thinking {
            len: b
                .get("thinking")
                .and_then(Value::as_str)
                .map(str::len)
                .unwrap_or(0),
        }),
        "tool_use" => Some(Block::ToolUse {
            id: b
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name: b.get("name").and_then(Value::as_str)?.to_string(),
            // Canonicalize input via serde_json so key order is stable for hashing.
            input: canonical_json(b.get("input").unwrap_or(&Value::Null)),
        }),
        "tool_result" => Some(Block::ToolResult {
            tool_use_id: b
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            output: stringify_tool_output(b.get("content")),
            is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        }),
        _ => None,
    }
}

/// A tool_result's `content` may be a string or an array of text blocks.
fn stringify_tool_output(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Serialize a JSON value with sorted object keys, so semantically equal inputs
/// always produce the same string (and therefore the same hash).
fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap(),
                        canonical_json(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

/// Reconstruct the tree from `parentUuid` and return the root→active-leaf path.
/// The active leaf is the deepest node; ties break toward the last-seen record,
/// which corresponds to the most recent branch the user was on.
fn follow_active_path(raws: &[Raw]) -> Vec<&Raw> {
    let by_uuid: HashMap<&str, &Raw> = raws.iter().map(|r| (r.uuid.as_str(), r)).collect();
    let children: HashMap<Option<&str>, Vec<&Raw>> = {
        let mut m: HashMap<Option<&str>, Vec<&Raw>> = HashMap::new();
        for r in raws {
            m.entry(r.parent.as_deref()).or_default().push(r);
        }
        m
    };

    // Depth of each node from its root, memoized, to pick the deepest leaf.
    fn depth(uuid: &str, by_uuid: &HashMap<&str, &Raw>) -> usize {
        let mut d = 0;
        let mut cur = Some(uuid);
        let mut guard = 0;
        while let Some(u) = cur {
            let Some(r) = by_uuid.get(u) else { break };
            d += 1;
            cur = r.parent.as_deref();
            guard += 1;
            if guard > 100_000 {
                break; // cycle guard against malformed input
            }
        }
        d
    }

    // Pick the leaf (a node with no children) that is deepest / latest.
    let mut best_leaf: Option<&Raw> = None;
    let mut best_depth = 0usize;
    for r in raws {
        let is_leaf = !children.contains_key(&Some(r.uuid.as_str()));
        if !is_leaf {
            continue;
        }
        let d = depth(&r.uuid, &by_uuid);
        if d >= best_depth {
            best_depth = d;
            best_leaf = Some(r);
        }
    }

    // Walk from the chosen leaf up to the root, then reverse.
    let mut path: Vec<&Raw> = Vec::new();
    let mut cur = best_leaf.map(|r| r.uuid.as_str());
    let mut guard = 0;
    while let Some(u) = cur {
        let Some(r) = by_uuid.get(u) else { break };
        path.push(r);
        cur = r.parent.as_deref();
        guard += 1;
        if guard > 100_000 {
            break;
        }
    }
    path.reverse();
    // Drop meta records that slipped through (they are not real turns).
    path.into_iter().filter(|r| !r.is_meta).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Role;

    // A synthetic transcript where an `attachment` record sits between two
    // messages — the exact case that fragmented real Claude Code transcripts.
    const SAMPLE: &str = r#"
{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","cwd":"/x","message":{"role":"user","content":"start"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"role":"assistant","content":[{"type":"text","text":"working"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}
{"type":"attachment","uuid":"att1","parentUuid":"a1"}
{"type":"user","uuid":"u2","parentUuid":"att1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"file.txt","is_error":false}]}}
{"type":"assistant","uuid":"a2","parentUuid":"u2","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}
"#;

    #[test]
    fn parses_and_bridges_attachment_records() {
        let run = parse_str(SAMPLE).unwrap();
        // All 4 messages are on the active path despite the attachment in between.
        assert_eq!(run.steps.len(), 4);
        assert_eq!(run.steps[0].role, Role::User);
        assert_eq!(run.steps[3].text(), "done");
        // The tool call and its result both survived normalization.
        assert_eq!(run.tool_calls(), 1);
        assert_eq!(run.steps[1].tool_names(), vec!["Bash"]);
        assert_eq!(run.meta.session_id.as_deref(), Some("s"));
        // A parsed run is internally consistent.
        assert!(run.verify_integrity().is_ok());
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let a = canonical_json(&serde_json::json!({"b":1,"a":2}));
        let b = canonical_json(&serde_json::json!({"a":2,"b":1}));
        assert_eq!(a, b, "key order does not change the canonical form");
    }

    #[test]
    fn empty_transcript_errors() {
        assert!(parse_str("\n\n").is_err());
    }
}
