# agent-replay

[![CI](https://github.com/tstanmay13/agent-replay/actions/workflows/ci.yml/badge.svg)](https://github.com/tstanmay13/agent-replay/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

**Record, deterministically replay, diff, and fork Claude Code agent sessions.** Turn a flaky agent run into a reproducible, tamper-evident fixture you can replay without the model, diff against another run to see exactly where behavior changed, fork to test an alternative, and assert on in CI.

Agent runs are non-deterministic — the model samples, tools return different things, time passes — which makes them nearly impossible to debug or regression-test. The same task, run twice against a frozen model, does not reliably do the same thing. `agent-replay` takes the stance that **an agent run should be a reproducible artifact**: capture every step as content-addressed state, and replay/diff/fork become exact operations rather than "run it again and eyeball it."

```
$ agentreplay record --from ~/.claude/projects/.../session.jsonl
“Add a --version flag to the CLI and run the tests.”
recorded 225 steps (154 assistant turns, 65 tool calls) → session.replay
digest 1b22600c1f80d6e93b256dcf

$ agentreplay replay session.replay
replayed 225 steps · 154 assistant turns · 65 tool calls · digest 1b22600c1f80d6e93b256dcf
integrity: verified deterministic (all step hashes match)
```

## Why this and not a transcript viewer

The unit is the **content hash of each step** (role + text + tool calls + tool results), and the **run digest** over all of them. That one design choice is what makes the rest exact:

- **Replay is verifiable, not vibes.** Replaying recomputes every step hash and the run digest. If they match the recording, the run is *provably* an unmodified reproduction. Flip one character of a recorded tool result and replay catches it at the exact step:
  ```
  integrity: FAILED — step 6 content does not match its recorded hash (tampered or corrupt)
  ```
- **Diff is semantic.** Two runs are compared by hash, so `diff` answers the question that matters when an agent regresses — *where did the two runs first diverge, and what changed* — instead of drowning you in a text diff:
  ```
  $ agentreplay diff baseline.replay candidate.replay
  ~ step 7: tool calls changed: [Read] → [Edit]
  first divergence at step 7
  ```
- **Fork is a real branch.** Claude Code sessions are already a tree (records link by `parentUuid`; editing a message branches it). `fork --at N --prompt "..."` makes that explicit and portable: keep the prefix, seed a different next prompt, get a new `.replay` to drive an alternative run.
- **Check turns a run into a CI gate.** `check` asserts rules over a recorded run and exits non-zero on violation — so a known-good session becomes a regression test:
  ```
  $ agentreplay check session.replay --must-succeed --forbid-path .env --max-tool-calls 50
  ✘ [forbid-path] step 31 tool `Bash` input references forbidden `.env`
  ```

## Install

Prebuilt binaries for macOS (Apple Silicon + Intel) and Linux are attached to each [GitHub Release](https://github.com/tstanmay13/agent-replay/releases) — download, `tar -xzf`, and drop `agentreplay` on your `PATH`, no Rust toolchain needed. From source:

```bash
cargo install --path .        # or: cargo build --release
```

## Try it in 30 seconds (no Claude Code history needed)

A sample session is committed under `fixtures/`:

```bash
agentreplay record --from fixtures/sample-session.jsonl --out /tmp/s.replay
agentreplay replay  /tmp/s.replay --show          # reconstructed transcript
agentreplay fork    /tmp/s.replay --at 3 --prompt "Also update the changelog." --out /tmp/forked.replay
agentreplay diff    /tmp/s.replay /tmp/forked.replay
agentreplay check   /tmp/s.replay --must-succeed --max-tool-calls 10
```

On your own machine, `agentreplay ls` lists your Claude Code sessions and `agentreplay record` (no args) records the most recent one.

## Commands

| Command | What it does |
|---------|--------------|
| `ls` | List local Claude Code sessions (newest first). |
| `record [--from <transcript>] [--out <file>]` | Normalize a session into a portable `.replay`. Defaults to the latest session. |
| `replay <file> [--show]` | Deterministically replay and verify integrity; `--show` prints the reconstructed transcript. |
| `diff <a> <b>` | Semantic step-by-step diff; reports the first divergence. Exits non-zero if they differ. |
| `fork <file> --at <n> [--prompt <text>] --out <file>` | Branch a run at a step, optionally seeding a new prompt. |
| `check <file> [rules…]` | Assert `--max-steps`, `--max-tool-calls`, `--forbid-tool`, `--forbid-path`, `--must-succeed`. Non-zero on violation. |

`replay`, `diff`, `fork`, and `check` accept either a `.replay` file or a raw Claude Code transcript (`.jsonl`) directly. `replay`, `diff`, and `check` take `--json` for machine-readable output (and still exit non-zero on failure), so they drop straight into scripts and pipelines.

## Use it in CI

Commit a known-good `.replay` fixture and gate every change against it with the bundled GitHub Action — a regression test for your agent's behavior:

```yaml
- uses: tstanmay13/agent-replay@main
  with:
    file: fixtures/golden.replay
    must-succeed: "true"
    forbid-paths: ".env secrets/"
    max-tool-calls: "50"
```

The step fails the build if the run errored, touched a forbidden path, or exceeded the tool-call budget. Or call the CLI directly with `--json` and parse the result.

## How it works

`record` reads a Claude Code transcript — a tree of records linked by `parentUuid` — and follows the root→active-leaf path, **bridging non-message records** (attachments, snapshots) that otherwise fragment the chain, to reconstruct the conversation as it actually ended. Each turn is normalized into a `Step` of typed blocks (text, thinking, tool-use, tool-result); tool inputs are canonicalized (sorted keys) so semantically equal calls hash identically, and volatile `tool_use` ids are excluded from the hash so two runs that did the same work match. The `.replay` file is a single pretty-printed JSON document — diff-friendly in git and trivially inspectable.

**The honest boundary:** this is *record-substitution* replay — deterministic given the recorded model and tool I/O. It does not re-drive the live model or re-execute tools, and it does not snapshot filesystem or clock state. That is the point: it makes the *observable behavior* of a run reproducible and tamper-evident, which is what you need to debug, diff, and regression-test agents — not a full-OS time machine.

## Development

```bash
cargo test              # 18 tests, no network
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

## License

MIT
