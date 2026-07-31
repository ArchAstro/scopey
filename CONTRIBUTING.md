# Contributing to Scopey

Thanks for helping improve Scopey. Bug reports, documentation fixes, tests, and
focused feature pull requests are welcome.

## Before filing an issue

- Search existing issues first.
- Run `scopey doctor` and include the relevant, redacted output.
- Never attach raw files from `~/.scopey/`, agent transcripts, credentials, or
  proprietary prompts. Create a minimal synthetic reproduction instead.
- Use a private security report for vulnerabilities rather than a public issue.

## Development setup

Scopey uses stable Rust. Agent CLIs are needed for live model probes, but the
unit and integration test suites must pass without Claude, Codex, Grok, Pi, or
OpenCode installed.

```bash
git clone https://github.com/ArchAstro/scopey.git
cd scopey
cargo build
cargo test --locked --all-targets --all-features
```

Install [`pre-commit`](https://pre-commit.com/) and enable the repository hook:

```bash
make install-pre-commit
```

Before opening a pull request, run:

```bash
make release-check
cargo package --locked --allow-dirty
```

## Pull requests

- Keep changes focused and explain user-visible behavior.
- Add or update tests for behavioral changes.
- Update the README and CLI help when flags, defaults, or workflows change.
- Preserve backward compatibility for config and session data when practical;
  document migrations when it is not.
- Do not commit generated session data, logs, credentials, or build artifacts.

By contributing, you agree that your contribution is licensed under the MIT
License included in this repository.
