# Changelog

All notable changes to Scopey will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project intends to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
