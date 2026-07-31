# Release checklist

## Before making the repository public

- Confirm ArchAstro owns or has permission to publish the source and
  `assets/scopey.jpg` under the repository's MIT license.
- Confirm the copyright holder in `LICENSE` is correct.
- Review the commit author names and email addresses that will become public.
- Merge the public-readiness and prerequisite pull requests.
- Run `make release-check` and `cargo package --locked` from a clean checkout.

## Immediately after changing visibility

- Enable private vulnerability reporting, secret scanning, and push protection
  in the repository Security settings.
- Protect `main`: require pull requests and the `Lint`, `Test`, `Package`, and
  `Security audit` checks; prevent force pushes and branch deletion.
- Verify issue templates, the security advisory link, Dependabot, badges, and
  the public installation command.
- Add repository topics such as `rust`, `cli`, `coding-agents`, and `developer-tools`.

## Publishing a version

1. Move relevant entries from `Unreleased` in `CHANGELOG.md` into a dated
   version section.
2. Update the version in `Cargo.toml` and run `cargo update -w` if needed.
3. Run `make release-check` and `cargo publish --dry-run --locked`.
4. Merge the release pull request.
5. In GitHub Actions, run the `Release` workflow from `main`. It verifies that
   `Cargo.toml` and this changelog agree, builds all four native archives, then
   creates the tag, checksums, and GitHub release. No repository secret is
   required. Pushing a matching `vX.Y.Z` tag remains supported as an
   alternative trigger.
6. The release workflow uses `ARCHASTRO_RELEASE_GITHUB_TOKEN` to run
   `scripts/update-scopey-formula.sh` and push the generated formula directly
   to `ArchAstro/homebrew-tools/main`. The token owner must be allowed to bypass
   the tap's pull-request rule, matching the ArchAstro CLI release setup.
7. Verify a clean install with `brew install ArchAstro/tools/scopey`, followed
   by `scopey --version`, `scopey setup`, and `scopey doctor`.
8. If publishing to crates.io, run `cargo publish --locked` from the tagged
   commit and verify the docs.rs build.

Release archives contain the `scopey` binary at their root and use these names:

- `scopey-darwin-arm64.tar.gz`
- `scopey-darwin-x64.tar.gz`
- `scopey-linux-arm64.tar.gz`
- `scopey-linux-x64.tar.gz`
