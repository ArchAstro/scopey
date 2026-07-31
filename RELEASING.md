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
5. Create a signed `vX.Y.Z` tag and GitHub release.
6. If publishing to crates.io, run `cargo publish --locked` from the tagged
   commit and verify the docs.rs build.
