#!/usr/bin/env bash
# Build the head of the promotion PR that scripts/promotion-plan.sh planned (#854, #925), and create
# its branch. .github/workflows/promote-main.yml runs this with the promotion App's token in GH_TOKEN.
#
#   scripts/promotion-head.sh <repo> <branch> <tag> <tag sha> <main sha> <tree> <head kind>
#   scripts/promotion-head.sh --self-test     prove every rule against a fake `gh` over a throwaway repo
#
# <repo> is `owner/name`; the rest are the plan's `branch`, `tag`, `tag_sha`, `main_sha`, `tree` and
# `head` outputs. OUTPUT: `key=value` lines on stdout, ready for $GITHUB_OUTPUT:
#   head_sha   the commit the branch now points at
#   how        one Markdown sentence for the PR body, saying how the head was built
#
# HEAD KINDS:
#   tag     the head is the tag commit itself (main is its ancestor).
#   merge   the head is a NEW commit made with the Git Data API (`POST /repos/{o}/{r}/git/commits`):
#           tree = the tag's tree, parents = [main's tip, the tag commit], and no author, committer or
#           signature in the request. It merges nothing: no merge is computed, so there is no conflict
#           case. Its tree is the tag's tree by construction, and promotion-plan.sh has already refused
#           a main that holds a change the tag lacks (that change would otherwise vanish from main
#           here, since this commit simply does not contain it).
#
# WHY THE GIT DATA API (#925): every ref is under the `Signed commits` ruleset (`required_signatures`,
# no bypass actors). GitHub signs a commit an App creates through the API when the request is
# "authenticated as the GitHub App or bot and contains no custom author information, custom committer
# information, and no custom signature information" (docs.github.com, About commit signature
# verification, "Signature verification for bots"), which devkit-upgrade.yml relies on for the same
# App. The merges API (`POST /repos/{o}/{r}/merges`), which this replaced, returned an unsigned merge
# commit that the ruleset refused (HTTP 409, run 44571980870).
#
# RULES (each one failing loudly, never a silent fallback): before the branch is created, the head as
# GitHub stores it (`GET .../git/commits/<sha>`) must have exactly <tree>, must be signature-verified
# (`.verification.verified == true`), and for `merge` must have exactly the parents [main, tag]. A head
# that fails a rule gets NO branch: the run creates no ref, so a re-run starts clean (an unreferenced
# commit object is harmless and GitHub collects it). The branch is created once and never moved: if it
# appeared since the workflow checked, the ref create fails and so does the run.
#
# Needs gh and jq; the self-test needs git, and no network.
set -euo pipefail

die() {
  echo "promotion-head: $*" >&2
  exit 1
}

# The commit <sha> as GitHub stores it, reduced to what the rules check:
# {"tree": <sha>, "parents": [<sha>...], "verified": <bool>, "reason": <string>}.
stored_commit() {
  local repo=$1 sha=$2
  gh api "repos/$repo/git/commits/$sha" |
    jq -c '{tree: .tree.sha, parents: [.parents[].sha], verified: .verification.verified,
            reason: .verification.reason}'
}

build() {
  local repo=$1 branch=$2 tag=$3 tag_sha=$4 main_sha=$5 tree=$6 kind=$7 head_sha want_parents got how

  case "$kind" in
    tag)
      head_sha=$tag_sha
      want_parents=""
      how="the \`$tag\` commit itself (main is its ancestor)"
      ;;
    merge)
      # No author, committer or signature: GitHub then signs the commit as the App (see WHY above).
      head_sha=$(
        jq -n --arg tree "$tree" --arg main "$main_sha" --arg tag_sha "$tag_sha" \
          --arg msg "chore: promote $tag to main" \
          --arg body "Merges $tag ($tag_sha) into main ($main_sha). The tree is exactly $tag's tree." \
          '{message: ($msg + "\n\n" + $body + "\n"), tree: $tree, parents: [$main, $tag_sha]}' |
          gh api -X POST "repos/$repo/git/commits" --input - | jq -r '.sha'
      ) || die "GitHub refused to create the promotion commit for $tag (gh's error above); no branch created"
      [ -n "$head_sha" ] && [ "$head_sha" != null ] || die "GitHub's create-commit response has no sha"
      want_parents=$(jq -cn --arg main "$main_sha" --arg tag_sha "$tag_sha" '[$main, $tag_sha]')
      how="a commit with the \`$tag\` tree whose parents are main's tip (\`$main_sha\`) and \`$tag\` (\`$tag_sha\`), made and signed by GitHub"
      ;;
    *) die "unknown head kind '$kind' (expected tag or merge)" ;;
  esac

  got=$(stored_commit "$repo" "$head_sha") || die "could not read back the head $head_sha from GitHub"
  [ "$(jq -r '.tree' <<<"$got")" = "$tree" ] ||
    die "the head $head_sha has tree $(jq -r '.tree' <<<"$got"), not $tag's tree $tree; no branch created"
  if [ -n "$want_parents" ] && [ "$(jq -c '.parents' <<<"$got")" != "$want_parents" ]; then
    die "the head $head_sha has parents $(jq -c '.parents' <<<"$got"), not $want_parents; no branch created"
  fi
  [ "$(jq -r '.verified' <<<"$got")" = true ] ||
    die "the head $head_sha is not signature-verified (reason: $(jq -r '.reason' <<<"$got")); the Signed commits ruleset would refuse it; no branch created"

  jq -n --arg ref "refs/heads/$branch" --arg sha "$head_sha" '{ref: $ref, sha: $sha}' |
    gh api -X POST "repos/$repo/git/refs" --input - >/dev/null ||
    die "GitHub refused to create $branch at $head_sha (gh's error above)"

  printf 'head_sha=%s\nhow=%s\n' "$head_sha" "$how"
}

# The fake `gh` the self-test puts first on PATH: the four calls build() makes, answered from the git
# repository $FAKE_GH_REPO as GitHub would answer them. A commit is verified iff its sha is listed in
# $FAKE_GH_REPO/.verified; POST git/commits lists the commit it makes unless FAKE_GH_SIGN=no.
# FAKE_GH_MANGLE=tree|parents makes POST git/commits store something other than what was asked, to
# prove the read-back rules catch it. Every call is appended to $FAKE_GH_REPO/.calls, and every
# create-commit request body to $FAKE_GH_REPO/.bodies.
write_fake_gh() {
  # The running bash, not `/usr/bin/env bash`: the nix build sandbox has no /usr/bin/env.
  printf '#!%s\n' "$BASH" >"$1"
  cat >>"$1" <<'FAKE'
set -euo pipefail
cd "$FAKE_GH_REPO"
echo "$*" >>.calls
[ "$1" = api ] || { echo "fake gh: unsupported: $*" >&2; exit 2; }
shift
method=GET input=no path=""
while [ $# -gt 0 ]; do
  case "$1" in
    -X) method=$2; shift 2 ;;
    --input) [ "$2" = - ] || exit 2; input=yes; shift 2 ;;
    -*) echo "fake gh: unsupported flag $1" >&2; exit 2 ;;
    *) path=$1; shift ;;
  esac
done
show() { # <sha>: a commit as GitHub's git/commits endpoints return it
  local sha=$1 verified=false
  grep -qxF "$sha" .verified 2>/dev/null && verified=true
  jq -n --arg sha "$sha" --arg tree "$(git rev-parse "$sha^{tree}")" \
    --argjson parents "$(git rev-list --parents -n 1 "$sha" | jq -Rc 'split(" ")[1:]')" \
    --argjson verified "$verified" \
    '{sha: $sha, tree: {sha: $tree}, parents: ($parents | map({sha: .})),
      verification: {verified: $verified, reason: (if $verified then "valid" else "unsigned" end)}}'
}
case "$method $path" in
  "POST repos/"*/git/commits)
    [ "$input" = yes ]
    req=$(cat)
    jq -c . <<<"$req" >>.bodies
    # GitHub's bot signing needs a request with no author, committer or signature.
    [ "$(jq -c 'keys' <<<"$req")" = '["message","parents","tree"]' ] || { echo "fake gh: request carries more than message/tree/parents: $req" >&2; exit 1; }
    tree=$(jq -r '.tree' <<<"$req")
    mapfile -t parents < <(jq -r '.parents[]' <<<"$req")
    case "${FAKE_GH_MANGLE:-}" in
      tree) tree=$(git hash-object -t tree -w /dev/null) ;;
      parents) parents=("${parents[1]}" "${parents[0]}") ;;
    esac
    args=()
    for p in "${parents[@]}"; do args+=(-p "$p"); done
    # GitHub stamps the time it creates the commit, so two identical requests make two commits.
    n=$(($(wc -l <.calls) + 1700000000))
    sha=$(jq -r '.message' <<<"$req" |
      GIT_AUTHOR_DATE="@$n +0000" GIT_COMMITTER_DATE="@$n +0000" git commit-tree "$tree" "${args[@]}")
    [ "${FAKE_GH_SIGN:-yes}" = no ] || echo "$sha" >>.verified
    show "$sha"
    ;;
  "GET repos/"*/git/commits/*) show "${path##*/}" ;;
  "POST repos/"*/git/refs)
    req=$(cat)
    ref=$(jq -r '.ref' <<<"$req")
    # Like GitHub's 422 "Reference already exists": never moves an existing ref.
    git update-ref "$ref" "$(jq -r '.sha' <<<"$req")" "" || { echo "fake gh: Reference already exists (HTTP 422)" >&2; exit 1; }
    jq -n --arg ref "$ref" '{ref: $ref}'
    ;;
  *) echo "fake gh: unsupported call: $method $path" >&2; exit 2 ;;
esac
FAKE
  chmod +x "$1"
}

self_test() {
  local failures=0 R=vig-os/tessera here
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  tmp=$(mktemp -d) # global: the EXIT trap runs after this function has returned
  trap 'rm -rf "$tmp"' EXIT
  mkdir "$tmp/bin"
  write_fake_gh "$tmp/bin/gh"
  PATH="$tmp/bin:$PATH"
  export FAKE_GH_REPO="$tmp/repo"

  # run <description> <expect: ok|fail:<stderr substring>> <args for build>...
  run() {
    local desc=$1 want=$2 got
    shift 2
    if got=$(build "$@" 2>"$tmp/err"); then
      if [ "$want" = ok ]; then
        echo "ok   - $desc -> $(head -n 1 <<<"$got")"
        printf '%s\n' "$got" >"$tmp/out"
        return 0
      fi
      echo "FAIL - $desc: expected an error, got: $(tr '\n' ' ' <<<"$got")"
    elif [ "$want" != ok ] && grep -qF -- "${want#fail:}" "$tmp/err"; then
      echo "ok   - $desc -> fails: $(tail -n 1 "$tmp/err")"
      return 0
    else
      echo "FAIL - $desc: $(cat "$tmp/err")"
    fi
    failures=$((failures + 1))
    # Non-zero only for an expected success, so its follow-up checks are skipped; a failed
    # expected-error case is counted and the self-test carries on.
    [ "$want" != ok ]
  }
  # check <description> <command...>: a property of the result, asserted on the real git objects.
  check() {
    local desc=$1
    shift
    if "$@" >/dev/null 2>&1; then echo "ok   - $desc"; else
      echo "FAIL - $desc"
      failures=$((failures + 1))
    fi
  }
  no_branch() { ! git -C "$FAKE_GH_REPO" rev-parse --verify --quiet "refs/heads/$1"; }
  sha() { git -C "$FAKE_GH_REPO" rev-parse "$1^{commit}"; }
  tree() { git -C "$FAKE_GH_REPO" rev-parse "$1^{tree}"; }
  field() { sed -n "s/^$1=//p" "$tmp/out"; }

  # The repo's shape after the first promotion: main = a merge commit of its own that the next
  # release (on dev) lacks, the case promotion-plan.sh plans as `head=merge`. Repo-local identity and
  # signing config keep a developer's or runner's global config out of the throwaway repo; commits
  # made in it are "verified" only by being listed in .verified, as the fake's GitHub would.
  git init -q -b main "$FAKE_GH_REPO"
  cd "$FAKE_GH_REPO"
  git config user.name self-test
  git config user.email self-test@invalid
  git config commit.gpgsign false
  git config tag.gpgsign false
  printf '%s\n' .calls .bodies .verified >.git/info/exclude
  printf '1\n' >a && git add a && git commit -q -m root
  git switch -q -c dev
  printf '2\n' >a && git commit -q -am d1
  git tag -a v0.1.0-alpha.1 -m alpha.1
  git switch -q main
  git merge -q --no-ff -m "merge alpha.1" v0.1.0-alpha.1
  git switch -q dev
  printf '3\n' >a && printf 'new\n' >b && git add b && git commit -q -am d2
  git tag -a v0.1.0-alpha.2 -m alpha.2
  local tag=v0.1.0-alpha.2 tag_sha main_sha tag_tree head plan
  tag_sha=$(sha $tag) main_sha=$(sha main) tag_tree=$(tree $tag)
  sha v0.1.0-alpha.1 >>.verified # dev's commits are signed (every ref is under Signed commits)
  echo "$tag_sha" >>.verified

  # The head is built from what promotion-plan.sh plans, as the workflow does: the plan's fields.
  pf() { sed -n "s/^$1=//p" <<<"$plan"; }
  plan=$("$BASH" "$here/promotion-plan.sh" "$tag" main) || die "self-test: promotion-plan.sh failed on the test repo"
  check "the plan for a main with a merge commit of its own is head=merge" test "$(pf head)" = merge
  check "the plan's tree is the tag's tree" test "$(pf tree)" = "$tag_tree"

  # merge: a new verified commit, the tag's tree, parents [main, tag], and only then the branch.
  if run "merge: GitHub builds and signs the head" ok \
    "$R" promote-a "$(pf tag)" "$(pf tag_sha)" "$(pf main_sha)" "$(pf tree)" "$(pf head)"; then
    head=$(field head_sha)
    check "  the branch points at the reported head" test "$(sha refs/heads/promote-a)" = "$head"
    check "  the head's tree is exactly the tag's tree" test "$(tree "$head")" = "$tag_tree"
    check "  the head's parents are exactly [main, tag]" \
      test "$(git rev-list --parents -n 1 "$head")" = "$head $main_sha $tag_sha"
    check "  the head is up to date with main (main is its ancestor)" git merge-base --is-ancestor "$main_sha" "$head"
    check "  merging the PR into main gives main exactly the release" \
      test "$(git merge-tree --write-tree main "$head")" = "$tag_tree"
    check "  the commit request carried only message, tree and parents (no author, committer or signature)" \
      test "$(tail -n 1 .bodies | jq -c 'keys')" = '["message","parents","tree"]'
    check "  the PR body sentence names main and the tag" grep -qF "(\`$main_sha\`) and \`$tag\` (\`$tag_sha\`)" "$tmp/out"
  fi

  # tag: the head is the tag commit itself; nothing is created but the branch. Planned against a main
  # (here: its first commit) that is an ancestor of the tag.
  plan=$("$BASH" "$here/promotion-plan.sh" "$tag" main~1) || die "self-test: promotion-plan.sh failed on the test repo"
  check "the plan for a main that is the tag's ancestor is head=tag" test "$(pf head)" = tag
  if run "tag: the head is the tag commit itself" ok \
    "$R" promote-t "$(pf tag)" "$(pf tag_sha)" "$(pf main_sha)" "$(pf tree)" "$(pf head)"; then
    check "  the branch points at the tag commit" test "$(sha refs/heads/promote-t)" = "$tag_sha"
  fi

  # Every rule, failing before any branch exists.
  FAKE_GH_SIGN=no run "merge: GitHub returns an unsigned commit" "fail:is not signature-verified (reason: unsigned)" \
    "$R" promote-u "$tag" "$tag_sha" "$main_sha" "$tag_tree" merge
  check "  no branch was created for the unsigned head" no_branch promote-u
  FAKE_GH_MANGLE=tree run "merge: GitHub stores another tree" "fail:not $tag's tree $tag_tree" \
    "$R" promote-x "$tag" "$tag_sha" "$main_sha" "$tag_tree" merge
  check "  no branch was created for the wrong tree" no_branch promote-x
  FAKE_GH_MANGLE=parents run "merge: GitHub stores the parents in another order" "fail:not [\"$main_sha\",\"$tag_sha\"]" \
    "$R" promote-p "$tag" "$tag_sha" "$main_sha" "$tag_tree" merge
  check "  no branch was created for the wrong parents" no_branch promote-p
  run "tag: an unsigned tag commit" "fail:is not signature-verified" \
    "$R" promote-v "$tag" "$(sha main~1)" "$main_sha" "$(tree main~1)" tag
  check "  no branch was created for the unsigned tag commit" no_branch promote-v
  run "the tree the plan expects is not the head's" "fail:not $tag's tree" \
    "$R" promote-w "$tag" "$tag_sha" "$main_sha" "$(tree main)" tag
  run "the branch appeared since the workflow checked: never moved" "fail:refused to create promote-a" \
    "$R" promote-a "$tag" "$tag_sha" "$main_sha" "$tag_tree" merge
  check "  the existing branch was left where it was" test "$(sha refs/heads/promote-a)" = "$head"
  run "an unknown head kind" "fail:unknown head kind 'rebase'" \
    "$R" promote-k "$tag" "$tag_sha" "$main_sha" "$tag_tree" rebase

  [ "$failures" -eq 0 ] || die "self-test: $failures case(s) failed"
  echo "promotion-head: self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  "" | -*) die "usage: $0 <repo> <branch> <tag> <tag sha> <main sha> <tree> <head kind> | --self-test" ;;
  *)
    [ $# -eq 7 ] || die "usage: $0 <repo> <branch> <tag> <tag sha> <main sha> <tree> <head kind> | --self-test"
    build "$@"
    ;;
esac
