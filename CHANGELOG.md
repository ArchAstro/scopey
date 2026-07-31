# Changelog

All notable changes to Scopey will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project intends to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-07-31

### Added

- Prebuilt macOS and Linux binaries for Intel and Apple/ARM systems, published
  by the GitHub release workflow.
- Installation through the public `ArchAstro/tools` Homebrew tap.
- Cross-session insights with session, date, harness, cwd, and verdict filters.
- Structured scope-transition logging and authoritative scope mutations.
- Harness integrations for Claude Code, Codex, Grok, Pi, and OpenCode.
- Local formatting hooks and public CI checks.

### Fixed

- Installed hooks now use Homebrew's stable `opt` path so they continue to
  work after `brew upgrade` removes an older Cellar version.

### Security

- Documented the sensitivity of stored prompts, transcripts, hook
  configuration, and custom model commands.
