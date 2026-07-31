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

1. Make sure the merged changes are described under `Unreleased` in
   `CHANGELOG.md`, then run `make release-check` and
   `cargo publish --dry-run --locked` before merging them.
2. In GitHub Actions, run the `Release` workflow from `main` and choose a
   `patch`, `minor`, or `major` bump. The workflow updates `Cargo.toml`,
   `Cargo.lock`, and the changelog, then commits the bump to `main` before it
   builds all four native archives and creates the tag, checksums, and GitHub
   release. Pushing an already-prepared matching `vX.Y.Z` tag remains supported
   as an alternative trigger.
3. The release workflow uses `ARCHASTRO_RELEASE_GITHUB_TOKEN` to push the bump
   commit to this repository and to regenerate and push `Formula/scopey.rb` in
   `ArchAstro/homebrew-tools`. The token owner must be allowed to bypass both
   repositories' pull-request rules.
4. Verify a clean install with `brew install ArchAstro/tools/scopey`, followed
   by `scopey --version`, `scopey setup`, and `scopey doctor`.
5. If publishing to crates.io, run `cargo publish --locked` from the tagged
   commit and verify the docs.rs build.

Release archives contain the `scopey` binary at their root and use these names:

- `scopey-darwin-arm64.tar.gz`
- `scopey-darwin-x64.tar.gz`
- `scopey-linux-arm64.tar.gz`
- `scopey-linux-x64.tar.gz`
