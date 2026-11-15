#!/usr/bin/env bash
# Plan the pull request that promotes `main` to a published release (#854): `main` tracks the
# latest release. .github/workflows/promote-main.yml runs this after every successful Release run and
# acts on its answer; nothing here touches a remote.
#
#   scripts/promotion-plan.sh <tag> <main-ref> [<expected tag sha>]   print the plan, or fail loudly
#   scripts/promotion-plan.sh --self-test                             prove every rule on throwaway repos
#
# OUTPUT: `key=value` lines on stdout, ready for $GITHUB_OUTPUT:
#   action   `open` (open the promotion PR) or `noop` (main already contains the release; `reason` says so)
#   tag, tag_sha, main_sha
#   head     how the PR head is built: `tag` (the tag commit itself, when main is its ancestor) or `merge`
#            (a merge of the tag into main's tip; see WHY A MERGE below)
#   tree     the tree the PR head has: always the tag's own tree
#   branch   `release/promote-<tag>`
#   title    `chore: promote <tag> to main`
#
# RULES (each one failing loudly, never a silent skip):
#   - the tag is `v<semver>` (scripts/lib/semver.sh) and exists, and when the caller passes the sha
#     the Release run built, the tag still points at it (a moved tag is not the release that ran);
#   - main already containing the tag commit is the one success that opens nothing (`noop`): a re-run
#     of an old Release run, or a promotion merged before this workflow ran;
#   - otherwise the tag must be the HIGHEST `v<semver>` tag in the repository, so main never moves to a
#     release older than the latest;
#   - when main is not an ancestor of the tag, the merge of the two must be clean AND its tree must be
#     exactly the tag's tree. A different tree means main holds a change the release does not, and the
#     PR would publish an unreleased state as "the release": a human decides, this script does not.
#
# WHY A MERGE: main's ruleset requires the PR head to be up to date with main (strict status checks).
# A head that is the bare tag commit is "behind" whenever main has a commit of its own, as it does
# after every promotion merge, so it could only merge after an "Update branch", which pushes a new head
# (dismissing the approval) and restarts the required `nix flake check`. A head that already merges
# the tag into main's tip is up to date from the start and has the tag's tree, which this script checks.
#
# Needs git >= 2.38 (`git merge-tree --write-tree`).
set -euo pipefail

die() {
  echo "promotion-plan: $*" >&2
  exit 1
}

# TAG_RE, semver_sort: shared with scripts/last-release-tag.sh.
# shellcheck source-path=SCRIPTDIR source=lib/semver.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib/semver.sh"

# The highest `v<semver>` tag in the repository (any reachability: a release is a release). Every
# candidate carries the same `v` prefix, so semver_sort orders the tags as they are.
latest_release_tag() {
  local tags
  tags=$(git tag --list 'v*') || die "git tag failed in $PWD (not a git repository?)"
  tags=$(grep -E "$TAG_RE" <<<"$tags" || true)
  [ -n "$tags" ] || return 0
  semver_sort <<<"$tags" | tail -n 1
}

plan() {
  local tag=$1 main_ref=$2 expect_sha=${3:-} tag_sha main_sha tag_tree latest merged head
  [[ "$tag" =~ $TAG_RE ]] || die "'$tag' is not a v<semver> release tag"
  tag_sha=$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}") ||
    die "no tag '$tag' in this repository (fetch tags: actions/checkout needs fetch-depth: 0)"
  if [ -n "$expect_sha" ] && [ "$tag_sha" != "$expect_sha" ]; then
    die "tag $tag points at $tag_sha, but the Release run built $expect_sha: the tag moved, refusing"
  fi
  main_sha=$(git rev-parse --verify --quiet "$main_ref^{commit}") || die "no such main ref: '$main_ref'"
  tag_tree=$(git rev-parse "$tag_sha^{tree}")

  if git merge-base --is-ancestor "$tag_sha" "$main_sha"; then
    printf 'action=noop\ntag=%s\ntag_sha=%s\nmain_sha=%s\n' "$tag" "$tag_sha" "$main_sha"
    printf 'reason=main (%s) already contains %s (%s)\n' "$main_sha" "$tag" "$tag_sha"
    return 0
  fi

  latest=$(latest_release_tag)
  [ "$latest" = "$tag" ] ||
    die "$tag is not the latest release ($latest is): main tracks the latest release, so promote $latest instead"

  if git merge-base --is-ancestor "$main_sha" "$tag_sha"; then
    head=tag
  else
    merged=$(git merge-tree --write-tree --no-messages "$main_sha" "$tag_sha") ||
      die "merging $tag into main ($main_sha) conflicts: main holds changes the release does not; resolve by hand"
    if [ "$merged" != "$tag_tree" ]; then
      echo "promotion-plan: paths where main merged with $tag differs from $tag itself:" >&2
      git diff --stat "$tag_tree" "$merged" >&2
      die "main ($main_sha) holds changes that $tag does not; promoting would publish an unreleased tree as $tag"
    fi
    head=merge
  fi
  printf 'action=open\ntag=%s\ntag_sha=%s\nmain_sha=%s\nhead=%s\ntree=%s\n' \
    "$tag" "$tag_sha" "$main_sha" "$head" "$tag_tree"
  printf 'branch=release/promote-%s\ntitle=chore: promote %s to main\n' "$tag" "$tag"
}

self_test() {
  local failures=0
  tmp=$(mktemp -d) # global: the EXIT trap runs after this function has returned
  trap 'rm -rf "$tmp"' EXIT

  # expect <description> <tag> <main ref> <expected sha or ""> <line>...: every line is in the plan.
  expect() {
    local desc=$1 tag=$2 ref=$3 sha=$4 got line missing=""
    shift 4
    if ! got=$(plan "$tag" "$ref" "$sha" 2>"$tmp/err"); then
      echo "FAIL - $desc: expected a plan, got an error ($(cat "$tmp/err"))"
      failures=$((failures + 1))
      return
    fi
    for line in "$@"; do
      grep -qxF -- "$line" <<<"$got" || missing+=" [$line]"
    done
    if [ -z "$missing" ]; then
      echo "ok   - $desc -> $(head -n 1 <<<"$got")"
    else
      echo "FAIL - $desc: plan lacks$missing; got: $(tr '\n' ' ' <<<"$got")"
      failures=$((failures + 1))
    fi
  }
  # expect_fail <description> <expected stderr substring> <tag> <main ref> [<expected sha>]
  expect_fail() {
    local desc=$1 want=$2 got
    if got=$(plan "$3" "$4" "${5:-}" 2>"$tmp/err"); then
      echo "FAIL - $desc: expected an error, got: $(tr '\n' ' ' <<<"$got")"
      failures=$((failures + 1))
    elif grep -qF -- "$want" "$tmp/err"; then
      echo "ok   - $desc -> fails: $(tail -n 1 "$tmp/err")"
    else
      echo "FAIL - $desc: error lacks '$want': $(cat "$tmp/err")"
      failures=$((failures + 1))
    fi
  }
  commit() { # <file> <content> <message>
    printf '%s\n' "$2" >"$1"
    git add "$1"
    git commit -q -m "$3"
  }
  sha() { git rev-parse "$1^{commit}"; }
  tree() { git rev-parse "$1^{tree}"; }

  # Repo-local config keeps a developer's or runner's global signing config out of the throwaway repo.
  git init -q -b main "$tmp/repo"
  cd "$tmp/repo"
  git config user.name self-test
  git config user.email self-test@invalid
  git config commit.gpgsign false
  git config tag.gpgsign false

  # The repo's shape: dev carries the work and the release tags, main only ever merges a release.
  commit a 1 root
  git switch -q -c dev
  commit a 2 d1
  git tag -a v0.1.0-alpha.1 -m alpha.1

  expect "main is an ancestor of the release: the head is the tag commit" v0.1.0-alpha.1 main "" \
    action=open head=tag "tag_sha=$(sha v0.1.0-alpha.1)" "main_sha=$(sha main)" \
    "tree=$(tree v0.1.0-alpha.1)" branch=release/promote-v0.1.0-alpha.1 "title=chore: promote v0.1.0-alpha.1 to main"
  expect "the caller's sha matches the tag" v0.1.0-alpha.1 main "$(sha v0.1.0-alpha.1)" action=open

  # The promotion PR merged (a merge commit, as GitHub's merge button makes): main now has a commit
  # of its own, which every later release lacks.
  git switch -q main
  git merge -q --no-ff -m "merge alpha.1" v0.1.0-alpha.1
  expect "main already contains the release: nothing to open" v0.1.0-alpha.1 main "" \
    action=noop "reason=main ($(sha main)) already contains v0.1.0-alpha.1 ($(sha v0.1.0-alpha.1))"

  git switch -q dev
  commit a 3 d2
  git tag -a v0.1.0-alpha.2 -m alpha.2
  expect "main has a merge commit of its own: the head is a merge with the tag's tree" v0.1.0-alpha.2 main "" \
    action=open head=merge "tree=$(tree v0.1.0-alpha.2)" "main_sha=$(sha main)"
  expect_fail "a tag that moved since the Release run built it" "the tag moved, refusing" \
    v0.1.0-alpha.2 main "$(sha v0.1.0-alpha.1)"

  # A newer release exists: promoting the older one would leave main behind the latest. alpha.10 sorts
  # above alpha.2 numerically (lexically it would not).
  commit a 4 d3
  git tag -a v0.1.0-alpha.10 -m alpha.10
  expect_fail "an older release while a newer one exists" "is not the latest release (v0.1.0-alpha.10 is)" \
    v0.1.0-alpha.2 main
  expect "the newest release" v0.1.0-alpha.10 main "" action=open head=merge "tree=$(tree v0.1.0-alpha.10)"
  # …while an old release main already contains is still a clean no-op, not an error.
  expect "an old release main already contains" v0.1.0-alpha.1 main "" action=noop

  # main holds a change the release lacks: a clean merge, but not the release's tree.
  git switch -q -c main-extra main
  commit hotfix x "a change made on main only"
  expect_fail "main holds a change the release lacks (clean merge)" "holds changes that v0.1.0-alpha.10 does not" \
    v0.1.0-alpha.10 main-extra
  # main changed the same line as the release: the merge conflicts.
  git switch -q -c main-conflict main
  commit a 99 "a conflicting change on main"
  expect_fail "main conflicts with the release" "conflicts" v0.1.0-alpha.10 main-conflict
  git switch -q dev

  # Inputs that are not a release: rejected before anything is computed.
  git tag nightly-1.2.3
  expect_fail "a tag that is not v<semver>" "is not a v<semver> release tag" nightly-1.2.3 main
  expect_fail "a tag that does not exist" "no tag 'v9.9.9'" v9.9.9 main
  expect_fail "a missing main ref" "no such main ref" v0.1.0-alpha.10 origin/main

  [ "$failures" -eq 0 ] || die "self-test: $failures case(s) failed"
  echo "promotion-plan: self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  "" | -*) die "usage: $0 <tag> <main-ref> [<expected tag sha>] | --self-test" ;;
  *)
    [ $# -ge 2 ] && [ $# -le 3 ] || die "usage: $0 <tag> <main-ref> [<expected tag sha>] | --self-test"
    plan "$@"
    ;;
esac
