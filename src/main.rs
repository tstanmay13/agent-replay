//! agentreplay — record, deterministically replay, diff, and fork Claude Code
//! agent sessions. See README.md for the why.

mod check;
mod diff;
mod fork;
mod model;
mod replay;
mod sessions;
mod store;
mod transcript;

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "agentreplay",
    version,
    about = "Record, replay, diff, and fork Claude Code agent sessions.",
    long_about = "Turn a flaky agent run into a reproducible fixture. Record a Claude Code \
session into a portable .replay file, replay it deterministically (no model, no network), \
diff two runs to find where they diverged, fork a run to test an alternative, and assert \
on runs in CI."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List local Claude Code sessions (newest first).
    Ls(LsArgs),
    /// Record a Claude Code session (or the latest) into a .replay file.
    Record(RecordArgs),
    /// Deterministically replay a .replay file and verify its integrity.
    Replay(ReplayArgs),
    /// Semantically diff two runs and report where they first diverged.
    Diff(DiffArgs),
    /// Fork a run at a step, optionally seeding a new prompt.
    Fork(ForkArgs),
    /// Assert rules over a run for CI (exits non-zero on violation).
    Check(CheckArgs),
}

#[derive(Args)]
struct LsArgs {
    /// Show at most this many sessions.
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Args)]
struct RecordArgs {
    /// Transcript file to record. If omitted, records the latest Claude Code session.
    #[arg(long)]
    from: Option<PathBuf>,
    /// Output .replay path (default: <session-id>.replay in the current directory).
    #[arg(short, long)]
    out: Option<PathBuf>,
}

#[derive(Args)]
struct ReplayArgs {
    /// The .replay file to replay.
    file: PathBuf,
    /// Print the reconstructed transcript, not just the verification summary.
    #[arg(long)]
    show: bool,
    /// Truncate each rendered line to this many characters.
    #[arg(long, default_value_t = 100)]
    width: usize,
}

#[derive(Args)]
struct DiffArgs {
    /// Baseline run (.replay or transcript .jsonl).
    a: PathBuf,
    /// Candidate run (.replay or transcript .jsonl).
    b: PathBuf,
}

#[derive(Args)]
struct ForkArgs {
    /// The run to fork (.replay or transcript .jsonl).
    file: PathBuf,
    /// Step index to fork at (keep steps 0..=at).
    #[arg(long)]
    at: usize,
    /// New user prompt to seed after the fork point.
    #[arg(long)]
    prompt: Option<String>,
    /// Output .replay path.
    #[arg(short, long)]
    out: PathBuf,
}

#[derive(Args)]
struct CheckArgs {
    /// The run to check (.replay or transcript .jsonl).
    file: PathBuf,
    #[arg(long)]
    max_steps: Option<usize>,
    #[arg(long)]
    max_tool_calls: Option<usize>,
    /// Fail if this tool was called (repeatable).
    #[arg(long = "forbid-tool")]
    forbid_tool: Vec<String>,
    /// Fail if any tool input references this substring, e.g. a secret path (repeatable).
    #[arg(long = "forbid-path")]
    forbid_path: Vec<String>,
    /// Fail if any tool result errored.
    #[arg(long)]
    must_succeed: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ls(a) => cmd_ls(a),
        Command::Record(a) => cmd_record(a),
        Command::Replay(a) => cmd_replay(a),
        Command::Diff(a) => cmd_diff(a),
        Command::Fork(a) => cmd_fork(a),
        Command::Check(a) => cmd_check(a),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Load a run from either a .replay file or a raw Claude Code transcript,
/// dispatching on extension so most commands accept both.
fn load_run(path: &std::path::Path) -> Result<model::Run> {
    match path.extension().and_then(|s| s.to_str()) {
        Some("jsonl") => transcript::parse_file(path),
        _ => store::read(path),
    }
}

fn cmd_ls(args: LsArgs) -> Result<ExitCode> {
    let sessions = sessions::list()?;
    if sessions.is_empty() {
        println!("no Claude Code sessions found under ~/.claude/projects");
        return Ok(ExitCode::SUCCESS);
    }
    println!("{:<38}  {:>7}  PROJECT", "SESSION", "SIZE");
    for s in sessions.into_iter().take(args.limit) {
        println!("{:<38}  {:>6}K  {}", s.session_id, s.size / 1024, s.project);
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_record(args: RecordArgs) -> Result<ExitCode> {
    let path = match args.from {
        Some(p) => p,
        None => {
            let latest = sessions::latest()?;
            eprintln!("recording latest session: {}", latest.session_id);
            latest.path
        }
    };
    let run = transcript::parse_file(&path)?;
    let default_name = format!(
        "{}.replay",
        run.meta.session_id.as_deref().unwrap_or("session")
    );
    let out = args.out.unwrap_or_else(|| PathBuf::from(default_name));
    store::write(&run, &out)?;
    if let Some(first) = run.steps.iter().find(|s| s.role == model::Role::User) {
        let preview = first.text();
        let preview = preview.lines().next().unwrap_or("").trim();
        if !preview.is_empty() {
            println!("“{}”", truncate(preview, 72));
        }
    }
    println!(
        "recorded {} steps ({} assistant turns, {} tool calls) → {}",
        run.steps.len(),
        run.assistant_steps(),
        run.tool_calls(),
        out.display()
    );
    println!("digest {}", run.digest());
    Ok(ExitCode::SUCCESS)
}

fn cmd_replay(args: ReplayArgs) -> Result<ExitCode> {
    let run = load_run(&args.file)?;
    let report = replay::replay(&run);
    if args.show {
        print!("{}", replay::render_playback(&run, args.width));
        println!();
    }
    println!(
        "replayed {} steps · {} assistant turns · {} tool calls · digest {}",
        report.steps,
        run.assistant_steps(),
        report.tool_calls,
        report.digest
    );
    match report.integrity_break {
        None => {
            println!("integrity: verified deterministic (all step hashes match)");
            Ok(ExitCode::SUCCESS)
        }
        Some(seq) => {
            eprintln!(
                "integrity: FAILED — step {seq} content does not match its recorded hash (tampered or corrupt)"
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

fn cmd_diff(args: DiffArgs) -> Result<ExitCode> {
    let a = load_run(&args.a)?;
    let b = load_run(&args.b)?;
    let d = diff::diff(&a, &b);

    println!(
        "A {}  ({} steps, digest {})",
        args.a.display(),
        a.steps.len(),
        a.digest()
    );
    println!(
        "B {}  ({} steps, digest {})",
        args.b.display(),
        b.steps.len(),
        b.digest()
    );
    println!();

    if d.identical {
        println!("runs are identical (matching digests)");
        return Ok(ExitCode::SUCCESS);
    }

    for (i, sd) in &d.rows {
        match sd {
            diff::StepDiff::Same => {}
            diff::StepDiff::Changed { detail } => println!("~ step {i}: {detail}"),
            diff::StepDiff::OnlyInA => println!("- step {i}: only in A"),
            diff::StepDiff::OnlyInB => println!("+ step {i}: only in B"),
        }
    }
    if let Some(seq) = d.first_divergence {
        println!("\nfirst divergence at step {seq}");
    }
    // Non-zero exit so `diff` is usable as a CI/regression gate.
    Ok(ExitCode::FAILURE)
}

fn cmd_fork(args: ForkArgs) -> Result<ExitCode> {
    let run = load_run(&args.file)?;
    let forked = fork::fork(&run, args.at, args.prompt.as_deref())?;
    store::write(&forked, &args.out)?;
    println!(
        "forked at step {} → {} ({} steps){}",
        args.at,
        args.out.display(),
        forked.steps.len(),
        if args.prompt.is_some() {
            ", seeded new prompt"
        } else {
            ""
        }
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_check(args: CheckArgs) -> Result<ExitCode> {
    let run = load_run(&args.file)?;
    let rules = check::Rules {
        max_steps: args.max_steps,
        max_tool_calls: args.max_tool_calls,
        forbid_tools: args.forbid_tool,
        forbid_paths: args.forbid_path,
        must_succeed: args.must_succeed,
    };
    if rules.is_empty() {
        return Err(anyhow!(
            "no rules given; pass at least one of --max-steps, --max-tool-calls, --forbid-tool, --forbid-path, --must-succeed"
        ));
    }
    let violations = check::check(&run, &rules);
    if violations.is_empty() {
        println!(
            "check passed ({} steps, {} tool calls)",
            run.steps.len(),
            run.tool_calls()
        );
        Ok(ExitCode::SUCCESS)
    } else {
        for v in &violations {
            eprintln!("✘ [{}] {}", v.rule, v.detail);
        }
        eprintln!("\n{} violation(s)", violations.len());
        Ok(ExitCode::FAILURE)
    }
}
