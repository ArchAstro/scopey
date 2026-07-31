# scopey

[![CI](https://github.com/ArchAstro/scopey/actions/workflows/ci.yml/badge.svg)](https://github.com/ArchAstro/scopey/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

<p align="center">
  <img src="assets/scopey.jpg" alt="Scopey — the Scope Guy" width="320" />
</p>

<p align="center"><em>It looks like you're writing code.<br/>Would you like help staying on scope?</em></p>

**Keep Claude Code, Codex, Grok, Pi, and OpenCode sessions aligned with your
current intent.**

Scopey is a lightweight Rust CLI that turns your prompts into a current scope,
checks coding-agent activity against it, and surfaces sessions that need your
attention. It can inject a correction into the active session and notify you
when work drifts off scope.

Scopey is pre-1.0 software. Config and session formats are designed to remain
compatible, but may still evolve before the first stable release.

## Install

Prerequisites: stable Rust and at least one supported coding-agent CLI for live
scope extraction. Tests do not require an agent CLI.

Install directly from GitHub:

```bash
cargo install --git https://github.com/ArchAstro/scopey.git
scopey setup
scopey doctor
```

Or clone the repository and use the Makefile:

```bash
make                 # debug build
make build-release
make install         # cargo install --path . --force
make setup           # release build + hooks + config
make doctor
make verify-models   # probe claude/codex fast defaults
make test            # unit + CLI integration tests
make lint
make release-check
```

Tests cover recursion guards, session storage, JSONL logs, judgement parsing,
hook detection, config loading, model resolution, and hook CLI paths under
isolated `SCOPEY_HOME`.

Or without make:

```bash
cargo install --path .
scopey setup
scopey doctor
scopey models --verify
```

### Harnesses

| Flag | Installs |
|------|----------|
| `--claude` / `--no-claude` | `~/.claude/settings.json` |
| `--codex` / `--no-codex` | `~/.codex/hooks.json` (trust via `/hooks`) |
| `--grok` / `--no-grok` | `~/.grok/hooks/scopey.json` |
| `--pi` / `--no-pi` | `~/.pi/agent/extensions/scopey.ts` (restart Pi) |
| `--opencode` / `--no-opencode` | `~/.config/opencode/plugins/scopey.js` |

```bash
scopey setup --force
scopey setup --no-claude --no-codex --grok --no-pi --no-opencode   # grok only
```

## Model selection

Summarize/judge use a **cheap/fast** model on the **same harness as the agent session** when possible.

| Config | Default | Meaning |
|--------|---------|---------|
| `model_runner` | `auto` | Use the session harness when available, or pin `claude`, `codex`, `grok`, `pi`, or `opencode` |
| `model` | `auto` | `auto` → product shipped fast tier |
| `claude_fast_model` | `haiku` | Claude Code alias for current fast Haiku |
| `codex_fast_model` | `gpt-5.6-terra` | Codex mini-like / lower-cost GPT-5.6 tier |

Claude invoke: `claude -p --model <fast> --bare`  
Codex invoke: `codex exec --ephemeral -m <fast> --output-last-message …`

```bash
scopey models              # print resolution table
scopey models --verify     # live one-word probes
```

## Session insights

`scopey insights` turns stored judgement history into a cross-session drift
report. Off-track sessions sort first, and each result includes its scope,
evaluated tool windows, attention rate, and the judge's explanation.

```bash
scopey insights                              # recent overview
scopey insights --off-scope --since 2026-07-01
scopey insights --session 019fb598 --details # exact id or unique prefix
scopey insights --date 2026-07-30 --harness codex
scopey insights --cwd . --verdict warning
scopey insights --since 2026-07-01T09:00:00-07:00 --json
scopey insights --include-empty                 # audit raw zero-tool stores
```

Date-only filters use the machine's local timezone. `--date` selects one
calendar day; `--since` and `--until` accept either `YYYY-MM-DD` or RFC3339.
Verdict filters select sessions containing that signal while retaining all
their evaluated windows, so the reported attention rate still has useful
context.

Zero-tool stores are excluded from default reports because they contain no
agent trajectory to assess; the report states how many it excluded. Exact
`--session` lookups include an empty match automatically, and `--include-empty`
restores the raw cross-session view. Reports also show session evaluation
coverage, judge failure rate, and contaminated scope records. Historical or
new warning/off-track results that explicitly say the transcript or tool
evidence is missing are counted as `insufficient-evidence`, not agent drift.

`--json` exposes the same totals, data-quality counts, per-session scope
quality, summaries, details, tool windows, and timestamps for scripts.

## Session logs

Hooks and background jobs append structured JSONL for debugging:

```text
~/.scopey/logs/<session_id>.jsonl
```

```bash
scopey logs                              # list recent session log files
scopey logs --session <id>               # pretty print
scopey logs --session <id> --tail 50
scopey logs --session <id> --level warn
scopey logs --session <id> --event guard
scopey logs --session <id> --follow
scopey logs --session <id> --raw
scopey logs --session <id> --path
```

## Runtime safety

Scopey keeps hooks responsive by doing model calls in bounded background jobs.
It prevents its own model subprocesses from triggering Scopey again, limits
machine-wide concurrency, and serializes work per session. If a session is
busy, its hook returns without delaying the coding agent and queued analysis is
picked up later.

Use `scopey doctor` to check an installation. `scopey purge` stops stale
background jobs, while `scopey setup --force` refreshes installed hooks.
`scopey uninstall` removes hooks but keeps local data; add `--purge-data` to
remove Scopey's config, sessions, logs, and locks as well.

## Herdr awareness

[Herdr](https://herdr.dev) is an agent multiplexer with its own notification + state API.

When Claude/Codex run **inside a Herdr pane**, scopey can:

1. **Notify via Herdr** — `herdr notification show … --sound request`  
   (Herdr routes to in-app toast, outer terminal, or OS depending on `[ui.toast] delivery`)
2. **Report pane state** — `herdr pane report-agent … --state blocked` so the sidebar shows needs-attention

| scopey config | Default | Meaning |
|---|---|---|
| `notify_backend` | `auto` | `auto` → Herdr if available, else OS; or pin `herdr` / `os` / `command` |
| `herdr_report_state` | `true` | Also mark the pane blocked on off-track/warning |
| `herdr_notify_sound` | (auto) | `none` \| `done` \| `request` |
| `notify_fallback_os_if_herdr_disabled` | `true` | If Herdr returns `shown=false`, use OS notify |

```bash
scopey herdr           # detection status
scopey herdr --probe   # test toast path
```

Enable toasts in Herdr if probes say `shown=false`:

```toml
# ~/.config/herdr/config.toml
[ui.toast]
delivery = "system"   # or "herdr" / "terminal"
```

## How it works

```
UserPromptSubmit  →  scopey hook user-prompt
                     · append user_prompt to session JSON
                     · background: scopey summarize → scope_requirements

PostToolUse / PostToolBatch  →  scopey hook post-tool
                     · tool_call_count += batch size
                     · if ready off_track/warning judgement → inject correction
                     · else every M tools → inject scope reminder
                     · every N tools → trajectory_mark + background scopey judge
                       (judgement becomes injectable at the *next* N boundary ≈ 2N lag)

Stop  →  scopey hook stop
                     · inject any pending correction
```

Scope extraction is intentionally **current-state**, not an ever-growing union
of the conversation. Each new prompt is an authoritative mutation that may add,
subtract, modify, or replace requirements. Additions and modifications preserve
unaffected active requirements; explicit removals, contradictions, and
replacements win. Questions add a read-only obligation without becoming new
implementation authorization. Machine-generated task notifications are treated
as context, not new user goals. Each transition is written to the structured
session log as `job.summarize.transition`, including the operation and
before/after scope hashes. If the summarizer model is unavailable, the fallback
preserves only the latest request instead of replaying the full prompt history.

### Development checks

Contributors should run `make install-pre-commit` once after installing
[`pre-commit`](https://pre-commit.com/). The checked-in hook runs
`cargo fmt --all -- --check` before each commit. CI enforces the same formatting
check, Clippy, rustdoc warnings, clean-runner tests, and package assembly. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the complete workflow. CI also checks the
locked dependency graph against the RustSec advisory database.

### Session store path

Sessions are keyed by **`session_id` only** (not cwd). One agent session stays
one file even when the agent `cd`s into subdirectories.

```text
~/.scopey/work/by-id/<session_id>.json
```

`SessionData.cwd` still tracks the latest working directory. On first open,
legacy files at `work/<escaped-cwd>/<session_id>.json` are migrated into
`by-id/`.

### Config (`~/.scopey/config.toml`)

| Key | Default | Meaning |
|-----|---------|---------|
| `n_tool_calls` | 10 | Journal + start background judge every N tools |
| `m_reminder` | 20 | Inject scope reminder every M tools |
| `model_runner` | `auto` | Session harness, or pin any supported runner |
| `model` | `auto` | Shipped fast tier for that runner |
| `claude_fast_model` | `haiku` | Claude fast alias |
| `codex_fast_model` | `gpt-5.6-terra` | Codex fast/mini-like tier |
| `notify_on_off_track` | true | Desktop alert on off-track judgement |

Project overlay: `<cwd>/.scopey/config.toml` wins when present.

## Commands (models: read each `--help`)

```bash
scopey --help
scopey setup --help
scopey doctor
scopey config
scopey config --init
scopey status --session-id <id>
scopey sessions
scopey insights --off-scope --since 2026-07-01
scopey path escape --cwd .
scopey path session-file --cwd . --session-id <id>
scopey hook user-prompt     # stdin: harness JSON
scopey hook session-start
scopey hook post-tool
scopey hook stop
scopey summarize --session-id <id> --cwd .
scopey judge --session-id <id> --cwd . --from-count 0 --to-count 10
scopey notify --title scopey --body "test"
```

### Hook contract for harness authors

- **stdin**: normalized harness event JSON (`session_id`, `cwd`, `prompt` / tools, `transcript_path`, …).
- **stdout**: harness-compatible injection JSON when steering, else empty.
- **stderr**: diagnostics.
- Hooks must stay fast; model work is detached (`~/.scopey/logs/`).

Injection shape:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PostToolUse",
    "additionalContext": "…"
  }
}
```

## Manual smoke test

```bash
export PATH="$PWD/target/debug:$PATH"
scopey setup --force --no-codex

# simulate user prompt
echo '{"session_id":"demo1","cwd":"'"$PWD"'","prompt":"Only refactor pathutil tests; do not touch main.rs"}' \
  | scopey hook user-prompt

scopey sessions
scopey status --session-id demo1 --cwd .

# simulate tools (will schedule judge every N)
for i in $(seq 1 10); do
  echo '{"session_id":"demo1","cwd":"'"$PWD"'","hook_event_name":"PostToolUse","tool_name":"Bash"}' \
    | scopey hook post-tool
done

scopey status --session-id demo1 --cwd . --raw | head
```

## Privacy

Session files and logs under `~/.scopey/` contain **user prompts and trajectory excerpts**. Treat that directory like other agent transcripts. Do not commit it.

Scopey invokes locally installed third-party agent CLIs, which may send prompts
to their configured providers under those providers' terms. Review your harness
configuration before using Scopey with sensitive material. See
[SECURITY.md](SECURITY.md) for the complete security model and private reporting
process.

## License

[MIT](LICENSE). See [CONTRIBUTING.md](CONTRIBUTING.md) and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) before participating.
Maintainers should follow [RELEASING.md](RELEASING.md) before changing
repository visibility or publishing a version.

Scopey is an independent project and is not affiliated with or endorsed by the
vendors of Claude Code, Codex, Grok, Pi, or OpenCode. Product names are used only
to describe compatibility.
