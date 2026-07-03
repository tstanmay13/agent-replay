//! Discover local Claude Code sessions under `~/.claude/projects`, so `record`
//! and `ls` can work without the user hunting for transcript paths.

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::time::SystemTime;

pub struct SessionFile {
    pub path: PathBuf,
    pub session_id: String,
    pub project: String,
    pub modified: SystemTime,
    pub size: u64,
}

fn projects_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join(".claude").join("projects");
    p.is_dir().then_some(p)
}

/// List all transcript files, newest first.
pub fn list() -> Result<Vec<SessionFile>> {
    let dir = projects_dir().ok_or_else(|| anyhow!("no ~/.claude/projects directory found"))?;
    let mut out = Vec::new();
    for project in read_dir_sorted(&dir)? {
        if !project.is_dir() {
            continue;
        }
        let project_name = project
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        for entry in read_dir_sorted(&project)? {
            if entry.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let meta = std::fs::metadata(&entry)?;
            out.push(SessionFile {
                session_id: entry
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string(),
                project: project_name.clone(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                size: meta.len(),
                path: entry,
            });
        }
    }
    // Newest first.
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    Ok(out)
}

/// The most recently modified session transcript (the "current" one).
pub fn latest() -> Result<SessionFile> {
    list()?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no Claude Code sessions found under ~/.claude/projects"))
}

fn read_dir_sorted(dir: &PathBuf) -> Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    v.sort();
    Ok(v)
}
