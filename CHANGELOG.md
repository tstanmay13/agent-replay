# Changelog

## 0.1.0

Initial release.

- `record` a Claude Code session (or the latest) into a portable, content-addressed `.replay` file; bridges non-message transcript records so the full conversation is captured.
- `replay` deterministically and verify integrity (recomputes every step hash; catches a tampered tool result at the exact step).
- `diff` two runs semantically and report the first divergence.
- `fork` a run at a step, optionally seeding a new prompt.
- `check` rules for CI (`--max-steps`, `--max-tool-calls`, `--forbid-tool`, `--forbid-path`, `--must-succeed`); exits non-zero on violation.
- `--json` output on `replay`, `diff`, and `check`.
- Reusable GitHub Action (`action.yml`) to gate agent runs in CI.
- Prebuilt binaries for macOS (Apple Silicon + Intel) and Linux attached to each release.
