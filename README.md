# scopey

<p align="center">
  <img src="assets/scopey.jpg" alt="Scopey — the Scope Guy" width="320" />
</p>

<p align="center"><em>It looks like you're writing code.<br/>Would you like help staying on scope?</em></p>

**Keep Claude Code and Codex sessions on the original scope.**

scopey is a lightweight Rust CLI that installs harness hooks, caches user prompts, summarizes them into scope requirements with a cheap model, periodically judges trajectory (writes/bash especially), injects course-corrections that lag by ~2N tool calls, reminds the model of scope every M tools, and desktop-notifies you when things go off-track.

<p align="center">
  <img src="assets/scopey-alt.jpg" alt="Scopey watching your trajectory" width="240" />
</p>

## Install

Makefile targets match the `archastro/aster` style:

```bash
make                 # debug build
make build-release
make install         # cargo install --path . --force
make setup           # release build + hooks + config
make doctor
make verify-models   # probe claude/codex fast defaults
make test            # unit + CLI integration (58 tests)
make lint
make release-check
```

Tests cover storm guards, session store, JSONL logs, judgement parsing, hook
detection, config loading, model resolution, and hook CLI paths under isolated
`SCOPEY_HOME`.

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
| `model_runner` | `auto` | `auto` → session harness (`claude` or `codex`); or pin `claude` / `codex` |
| `model` | `auto` | `auto` → product shipped fast tier |
| `claude_fast_model` | `haiku` | Claude Code alias for current fast Haiku |
| `codex_fast_model` | `gpt-5.6-terra` | Codex mini-like / lower-cost GPT-5.6 tier |

Claude invoke: `claude -p --model <fast> --bare`  
Codex invoke: `codex exec --ephemeral -m <fast> --output-last-message …`

```bash
scopey models              # print resolution table
scopey models --verify     # live one-word probes
```

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

## Process-storm + hang guards (critical)

Headless `claude -p` / `codex exec` used for summarize/judge must **never** re-enter scopey hooks. That recursion was causing machine-killing process storms.

Hooks must also never block the agent turn. Session store exclusive flocks are held only for short read/write bursts — **not** across model network calls. Hook opens wait at most **~1.5s** for the lock; if busy they skip and exit 0 with empty stdout (never hang Codex’s 15s timeout).

Deterministic guards in the CLI (not left to the model):

| Guard | Behavior |
|---|---|
| `SCOPEY_INTERNAL=1` | Set on every child scopey/model process; **all hooks no-op** |
| Per-session job lock | `~/.scopey/locks/<session>.job.lock` — **one** live summarize/judge per session |
| Session store flock | Held only while reading/writing JSON; released before `model.complete` |
| Hook lock wait | Max ~1.5s; then skip (exit 0, empty stdout) so the harness never hangs |
| Tool journal | Judge reads structured tool events, not raw JSONL tails |
| Deferred jobs | Throttled summarize/judge are queued and drained when free |
| `min_job_interval_secs` | Default 60s between jobs for a session |
| `max_global_jobs` | Default 2 concurrent bg workers machine-wide |
| Claude hooks | **PostToolBatch only** (not PostToolUse) to avoid double count/spawn |
| Internal prompt filter | UserPromptSubmit ignores scopey analyst/judge text |
| Hook exit | Hooks always exit 0; errors go to stderr / logs only |

```bash
scopey purge              # SIGTERM leaked bg jobs / recursive claude storms
scopey setup --force      # reinstall hooks (strips legacy PostToolUse)
scopey uninstall          # remove hooks; keep ~/.scopey
scopey uninstall --purge-data   # also delete config/sessions/logs/locks
make uninstall            # same
make uninstall PURGE=1    # with --purge-data
```

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

### Session store path

Sessions are keyed by **`session_id` only** (not cwd). One Claude/Codex
session stays one file even when the agent `cd`s into subdirectories.

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
| `model_runner` | `auto` | Session harness, or pin `claude`/`codex` |
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

- **stdin**: full event JSON from Claude/Codex (`session_id`, `cwd`, `prompt` / tools, `transcript_path`, …).
- **stdout**: only Claude/Codex injection JSON when steering, else empty.
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

## License

MIT
