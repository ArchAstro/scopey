# Changelog

All notable changes to Scopey will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project intends to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
