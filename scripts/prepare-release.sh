#!/usr/bin/env bash
set -euo pipefail

bump="${1:-}"
release_date="${SCOPEY_RELEASE_DATE:-$(date +%F)}"

case "$bump" in
  patch | minor | major) ;;
  *)
    echo "usage: $0 <patch|minor|major>" >&2
    exit 2
    ;;
esac

current="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
if [[ ! "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  echo "Cargo.toml package version is not semantic: $current" >&2
  exit 1
fi

major="${BASH_REMATCH[1]}"
minor="${BASH_REMATCH[2]}"
patch="${BASH_REMATCH[3]}"
case "$bump" in
  patch) ((patch += 1)) ;;
  minor)
    ((minor += 1))
    patch=0
    ;;
  major)
    ((major += 1))
    minor=0
    patch=0
    ;;
esac
next="${major}.${minor}.${patch}"

if grep -Fq "## [${next}]" CHANGELOG.md; then
  echo "CHANGELOG.md already contains ${next}" >&2
  exit 1
fi
if ! grep -Fq "## [Unreleased]" CHANGELOG.md; then
  echo "CHANGELOG.md has no Unreleased section" >&2
  exit 1
fi

CURRENT_VERSION="$current" NEXT_VERSION="$next" perl -0pi -e '
  s/(\[package\]\nname = "scopey"\nversion = ")\Q$ENV{CURRENT_VERSION}\E("\n)/$1$ENV{NEXT_VERSION}$2/
' Cargo.toml
CURRENT_VERSION="$current" NEXT_VERSION="$next" perl -0pi -e '
  s/(\[\[package\]\]\nname = "scopey"\nversion = ")\Q$ENV{CURRENT_VERSION}\E("\n)/$1$ENV{NEXT_VERSION}$2/
' Cargo.lock

RELEASE_HEADER="## [${next}] - ${release_date}" perl -0pi -e '
  s/## \[Unreleased\]\n/## [Unreleased]\n\n$ENV{RELEASE_HEADER}\n/
' CHANGELOG.md

if ! grep -Fq "version = \"${next}\"" Cargo.toml ||
  ! grep -Fq "## [${next}] - ${release_date}" CHANGELOG.md; then
  echo "failed to prepare ${next}" >&2
  exit 1
fi

printf '%s\n' "$next"
