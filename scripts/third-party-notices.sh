#!/bin/sh
# Regenerates THIRD_PARTY_NOTICES.md: the license notices of the crates in the release binaries.
#   scripts/third-party-notices.sh          write THIRD_PARTY_NOTICES.md
#   scripts/third-party-notices.sh --check  fail if the committed file is out of date
# Needs cargo-about (cargo install --locked cargo-about --features cli).
set -eu
cd "$(dirname "$0")/.."
out=THIRD_PARTY_NOTICES.md
if [ "${1:-}" = "--check" ]; then
  tmp=$(mktemp)
  trap 'rm -f "$tmp"' EXIT
  cargo about generate --locked --fail about.hbs -o "$tmp"
  diff -u "$out" "$tmp" || {
    echo "$out is out of date: run scripts/third-party-notices.sh and commit the result" >&2
    exit 1
  }
else
  cargo about generate --locked --fail about.hbs -o "$out"
fi
