#!/usr/bin/env bash
# Print the git tag of the last tessera release: the baseline release-plz compares against (#606).
#
#   scripts/last-release-tag.sh <workspace Cargo.toml>   print the tag (stdout), or fail loudly
#   scripts/last-release-tag.sh --self-test              prove the selection on throwaway repos
#
# Tessera's crates are not on crates.io, so release-plz's default baseline (the cargo registry)
# reports every crate as never released and never proposes a release. The release workflow
# therefore checks this tag out and passes its manifest as `--registry-manifest-path`.
#
# The last release is the HIGHEST `v<semver>` tag that is (a) reachable from HEAD and (b) not newer
# than the workspace version in `[workspace.package]`:
#   - Ordering is SemVer, pre-releases included: v0.1.0-alpha.1 < v0.1.0-alpha.2 < v0.1.0-alpha.10
#     < v0.1.0. A lexical sort, and `git tag --sort=v:refname` without `versionsort.suffix`, put
#     v0.1.0 BEFORE its own pre-releases.
#   - The ceiling exists because the tag namespace predates the software releases: `v0.1.0` and
#     `v1.0.0-rc1` are FORMAT milestones from 2026-06 (annotated "format frozen" / "format
#     v1.0-ready"), not software releases. release-plz only ever bumps upward, so no tag above the
#     workspace version can be the previous release of this workspace. The cap stops working once the
#     workspace reaches 0.1.0; the namespace clash itself is #608.
#   - Unreachable tags (a release cut from another line) are not this branch's history.
# No qualifying tag is an error, never a silent fall-back to the registry: that fall-back is the
# exact failure #606 fixes.
set -euo pipefail

die() {
  echo "last-release-tag: $*" >&2
  exit 1
}

# A `v`-prefixed SemVer core with an optional pre-release; build metadata is not used by our tags.
TAG_RE='^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'

# SemVer sort of bare versions on stdin, ascending. GNU `sort -V` follows Debian ordering, where
# `~` sorts before everything, even the end of the string; mapping the pre-release `-` to `~` makes
# 0.1.0~alpha.1 < 0.1.0, which is SemVer's rule. Numeric runs compare numerically (alpha.2 < alpha.10).
# Exact for our `<word>.<number>` pre-releases (alpha.N, beta.N, rc.N); NOT for SemVer's mixed rule
# that numeric identifiers sort below alphanumeric ones (sort -V puts alpha.beta before alpha.1).
semver_sort() {
  sed 's/-/~/' | sort -V | sed 's/~/-/'
}

# The `version` key of the `[workspace.package]` table in the given Cargo.toml.
workspace_version() {
  local manifest=$1 v
  [ -f "$manifest" ] || die "no such manifest: $manifest"
  v=$(awk '
    /^[[:space:]]*\[/ { in_pkg = ($0 ~ /^[[:space:]]*\[workspace\.package\][[:space:]]*$/); next }
    in_pkg && /^[[:space:]]*version[[:space:]]*=/ {
      sub(/^[^=]*=[[:space:]]*"/, ""); sub(/".*$/, ""); print; exit
    }' "$manifest")
  [ -n "$v" ] || die "$manifest has no [workspace.package] version"
  [[ "v$v" =~ $TAG_RE ]] || die "$manifest: workspace version '$v' is not SemVer"
  printf '%s\n' "$v"
}

# Whether version $1 <= version $2 in SemVer order.
semver_le() {
  [ "$(printf '%s\n%s\n' "$1" "$2" | semver_sort | tail -n 1)" = "$2" ]
}

# The tag of the last release, for the repository in the current directory.
resolve() {
  local manifest=$1 ceiling all tags v best=""
  # Explicit `|| exit`: callers may run resolve under `if`, where `set -e` does not apply.
  ceiling=$(workspace_version "$manifest") || exit 1
  all=$(git tag --list 'v*' --merged HEAD) || die "git tag failed in $PWD (not a git repository?)"
  tags=$(grep -E "$TAG_RE" <<<"$all" || true) # no match is not an error: the check below reports it
  [ -n "$tags" ] || die "no release tag (v<semver>) is reachable from HEAD; push the last release's tag (or fetch tags: actions/checkout needs fetch-depth: 0) — refusing to fall back to the crates.io registry (#606)"
  best=$(while read -r v; do
    if semver_le "${v#v}" "$ceiling"; then printf '%s\n' "${v#v}"; fi
  done <<<"$tags" | semver_sort | tail -n 1)
  [ -n "$best" ] || die "no release tag at or below the workspace version $ceiling is reachable from HEAD (found: $(tr '\n' ' ' <<<"$tags"))"
  printf 'v%s\n' "$best"
}

self_test() {
  local failures=0
  tmp=$(mktemp -d) # global: the EXIT trap runs after this function has returned
  trap 'rm -rf "$tmp"' EXIT

  expect() { # <description> <expected tag> <manifest>
    local got
    if got=$(resolve "$3" 2>"$tmp/err") && [ "$got" = "$2" ]; then
      echo "ok   - $1 -> $got"
    else
      echo "FAIL - $1: expected '$2', got '${got:-}' ($(cat "$tmp/err"))"
      failures=$((failures + 1))
    fi
  }
  expect_fail() { # <description> <expected stderr substring> <manifest>
    local got
    if got=$(resolve "$3" 2>"$tmp/err"); then
      echo "FAIL - $1: expected an error, got '$got'"
      failures=$((failures + 1))
    elif grep -qF -- "$2" "$tmp/err"; then
      echo "ok   - $1 -> fails: $(cat "$tmp/err")"
    else
      echo "FAIL - $1: error lacks '$2': $(cat "$tmp/err")"
      failures=$((failures + 1))
    fi
  }
  set_version() { printf '[workspace]\n\n[workspace.package]\nedition = "2021"\nversion = "%s"\n' "$1" >"$tmp/repo/Cargo.toml"; }
  commit() { git -C "$tmp/repo" commit -q --allow-empty -m "$1"; }
  tag() { git -C "$tmp/repo" tag -a "$1" -m "$1"; }

  # Repo-local config keeps a developer's or runner's global signing config out of the throwaway repo.
  git init -q -b main "$tmp/repo"
  git -C "$tmp/repo" config user.name self-test
  git -C "$tmp/repo" config user.email self-test@invalid
  git -C "$tmp/repo" config commit.gpgsign false
  git -C "$tmp/repo" config tag.gpgsign false
  set_version 0.1.0-alpha.1
  cd "$tmp/repo"

  commit root
  expect_fail "no tags at all" "no release tag (v<semver>) is reachable from HEAD" Cargo.toml

  # Pre-release ordering: lexical and plain v:refname sorts both get these wrong.
  tag v0.1.0-alpha.1; commit c1
  tag v0.1.0-alpha.2; commit c2
  tag v0.1.0-alpha.10; commit c3
  set_version 0.1.0
  expect "pre-releases order numerically below the release" v0.1.0-alpha.10 Cargo.toml
  tag v0.1.0; commit c4
  expect "the release sorts above its own pre-releases" v0.1.0 Cargo.toml

  # Tags above the workspace version are not this workspace's previous release (the repo's
  # format-milestone tags v0.1.0 / v1.0.0-rc1 predate the software releases).
  tag v1.0.0-rc1
  expect "a tag above the workspace version is ignored" v0.1.0 Cargo.toml
  set_version 0.0.1
  expect_fail "every tag above the workspace version" "no release tag at or below the workspace version 0.0.1" Cargo.toml

  # Not v<semver>, or not reachable from HEAD: ignored.
  set_version 0.2.0
  tag vNext; tag release-0.1.9; tag v0.1
  git switch -q -c side; commit s1; tag v0.1.5; git switch -q main
  expect "non-SemVer and unreachable tags are ignored" v0.1.0 Cargo.toml

  # Release PR merged (workspace bumped), tag not pushed yet: still the previous release.
  set_version 0.1.0-alpha.2
  git tag -d v0.1.0 v0.1.0-alpha.2 v0.1.0-alpha.10 v1.0.0-rc1 >/dev/null
  expect "bumped but not yet tagged -> the previous release" v0.1.0-alpha.1 Cargo.toml

  # The real repo's tag set at #606: format milestones v0.1.0 and v1.0.0-rc1 above alpha.1.
  set_version 0.1.0-alpha.1
  tag v0.1.0; tag v1.0.0-rc1
  expect "the #606 tag set (format milestones above alpha.1)" v0.1.0-alpha.1 Cargo.toml

  printf '[workspace]\n\n[package]\nversion = "1.0.0"\n' >"$tmp/bare.toml"
  expect_fail "a version outside [workspace.package] is not read" "has no [workspace.package] version" "$tmp/bare.toml"
  expect_fail "a missing manifest" "no such manifest" "$tmp/absent.toml"

  [ "$failures" -eq 0 ] || die "self-test: $failures case(s) failed"
  echo "last-release-tag: self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  "" | -*) die "usage: $0 <workspace Cargo.toml> | --self-test" ;;
  *) resolve "$1" ;;
esac
