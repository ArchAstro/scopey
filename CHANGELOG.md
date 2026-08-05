# Changelog

All notable changes to Scopey will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project intends to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `scopey insights` now analyzes drift patterns and token usage, rendered
  readably: a drift-patterns block (archetype taxonomy bars, onset-position
  histogram, drift-by-session-length table, stated/implied-limit split, and
  per-correction next-check recovery), provider-reported main-session token
  totals with cache-read/fresh/output composition, and per-session drift
  shape, categories, correction outcomes, and tokens. Output uses validated
  truecolor palettes with unicode bars, degrades to plain text under
  `NO_COLOR` or when piped, and renders the onset histogram as an inline
  raster via the kitty graphics protocol on kitty/ghostty (layer-local
  detection only, so nested PTYs degrade safely). Metrics use plain-language
  labels with a full glossary in `--help`. New flags: `--no-patterns`,
  `--tokens shown|all|off`, `--graphics auto|kitty|off`; `--json`
  includes all new fields under stable names.

### Fixed

- Removed the `--bare` (API-key-only) fallback from the Claude analyzer
  runner. It could never succeed without an API key, and its "Not logged in"
  error overwrote the OAuth attempt's real diagnostics — masking a sustained
  claude-harness judge outage (0/214 checks succeeding) whose actual cause
  was a stale installed binary. Failures now report exit status, stdout,
  stderr, and presence-only auth environment facts (HOME, CLAUDECODE,
  CLAUDE_CODE_SIMPLE, ANTHROPIC_API_KEY, CLAUDE_CONFIG_DIR) in one record.

### Fixed

- New user prompts now start a fresh judgement epoch: pending verdicts from the
  previous prompt are invalidated, judge windows restart at the current tool
  count, and completed verdicts must still match the active prompt before they
  can notify or inject. The scope-analysis prompt also preserves operative
  actions in phrases such as “figure out how to construct and evaluate” and
  directs the model not to invent planning-only or other permission boundaries.

## [0.1.3] - 2026-08-03

### Fixed

- Claude-backed summarize and judge jobs now remove inherited
  `CLAUDE_CODE_SIMPLE` before invoking Claude, so OAuth/keychain credentials
  remain available in live hook-spawned workers as well as direct model probes.
  Custom `model_command` invocations receive the same OAuth-safe recursion
  guards, and failures now report the child status plus non-empty stderr or
  stdout instead of an empty error.

### Added

- A failing model runner is no longer silent. Background summarize/judge
  outcomes are recorded in `~/.scopey/model_health.json`; `scopey doctor` now
  fails a `model jobs` check when calls fail persistently, `scopey status`
  prints a MODEL UNAVAILABLE banner when the stored scope is only the
  fallback echo of the latest prompt, `scopey models --verify` reports live
  job outcomes so a green probe cannot mask a broken production path, and a
  desktop notification fires after repeated failures
  (`notify_on_model_fallback`, default `true`). The health file keeps a
  bounded list of recent job outcomes — `model_health_history` (default 50,
  0 disables) truncates the oldest on every write, and every entry's error
  text is clipped — so the file is bounded by construction; corrupt files
  read as empty and are rewritten on the next outcome.
- `make e2e-local`: opt-in end-to-end tests that drive a real hook through a
  detached worker against locally authenticated `claude`/`codex` CLIs. One
  test covers the poisoned `CLAUDE_CODE_SIMPLE=1` flow (failing if scope
  extraction falls back instead of using OAuth credentials); a clean
  10-session concurrency burst per runner requires every session to spawn its
  own worker, invoke its cheap sub-model for real, log the full expected
  event trail, and leave each artifact — session JSONL, worker log, by-id
  store, health file — exactly where it belongs. Model-health updates are now
  flock-serialized so concurrent workers cannot lose counts, letting the
  tests assert exact totals. All scopey state stays in a temp home.

## [0.1.2] - 2026-08-02

### Fixed

- Hook events fired inside a subagent session are ignored, so delegated work no
  longer receives scope reminders and course corrections meant for the
  conversation between the user and the top-level agent. Because subagent
  events reuse the parent's session id, their tool calls had also been
  inflating the parent's tool count and shifting judge and reminder cadence.
  Claude Code and Codex subagents are recognized by the `agent_id` their hooks
  carry, OpenCode by its child sessions, and Pi by its subagent event markers.
  Top-level sessions started with `--agent` keep their previous behavior.
- Codex names its multi-agent tools without a separator, so a blocking
  `collaborationwait_agent` call escaped the noise list and counted as real
  tool activity.

### Added

- `ignore_subagents` restores the previous behavior when set to `false`.
- `log_raw_events` records each hook's raw stdin payload in the session log for
  debugging. Payloads include prompts, so it carries the same sensitivity as
  the rest of `~/.scopey`.

## [0.1.1] - 2026-07-31

### Changed

- Hook and extension installers now invoke `scopey` through `PATH` instead of
  embedding the setup process's absolute executable path.
- Manually dispatched releases now accept a patch, minor, or major bump and
  persist the selected version before building and publishing it.

## [0.1.0] - 2026-07-31

### Added

- Prebuilt macOS and Linux binaries for Intel and Apple/ARM systems, published
  by a manually dispatchable or tag-triggered GitHub release workflow.
- Installation through the public `ArchAstro/tools` Homebrew tap.
- Automatic Homebrew formula updates after successful GitHub releases.
- Cross-session insights with session, date, harness, cwd, and verdict filters.
- Structured scope-transition logging and authoritative scope mutations.
- Harness integrations for Claude Code, Codex, Grok, Pi, and OpenCode.
- Local formatting hooks and public CI checks.

### Security

- Documented the sensitivity of stored prompts, transcripts, hook
  configuration, and custom model commands.
