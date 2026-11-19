#!/usr/bin/env bash
# Every ksb runner label in a workflow must sit behind the fork guard (#688).
#
# vig-os/tessera is PUBLIC, and the vig-os `ksb-ci` runner group admits it. A job whose `runs-on`
# names a `ksb-…` label unconditionally would send a fork PR to ksb, where the runner's job-started
# fork-guard hook SIGKILLs it (exo-pet/exo-fleet#949): the job dies, GitHub cancels it ~10 min later,
# and a required check goes red for every honest outside contributor. So each ksb label must be the
# NON-fork branch of a ternary whose fork branch is a GitHub-hosted label:
#
#     runs-on: ${{ (<FORK PREDICATE>) && 'ubuntu-…' || 'ksb-vig-os-…' }}
#
# with exactly devkit's fork predicate (vig-os/devkit ADR-site-agnostic-projects §2, owner-tested):
# a run is foreign when it carries a pull request whose head is not this repository.
#
# WHAT THIS IS NOT: a security boundary. A fork PR runs the FORK's copy of the workflow files, so a
# fork can delete this guard. Keeping fork code off ksb is the job of the runner side (the fork-guard
# hook) and of the repo's fork-PR approval setting; this gate keeps OUR workflows from routing honest
# fork PRs onto a runner that will kill them, and states the intent where a reviewer reads it.
#
# Rules, per workflow file:
#   1. every non-comment line naming a `ksb-<org>-<size>` label (in any letter case: GitHub matches
#      labels case-insensitively) has the shape above, the predicate verbatim, the hosted label first
#      and the ksb label last;
#   2. no `runs-on: {group: ksb…}` runner-group routing: it bypasses rule 1 (use a guarded label);
#   3. a file that names a ksb label triggers only on an ALLOW-LIST: `push`, `pull_request`,
#      `workflow_dispatch`, `schedule`. Anything else is a design change, not a trigger line:
#      `workflow_run` and `merge_group` carry no head repository the predicate can read (ADR §2);
#      `pull_request_target` carries a readable head repo, but runs with the base repo's secrets for
#      fork PRs, which is how untrusted code gets checked out next to them;
#      `issue_comment` and friends carry no pull request, so the predicate is always false there and
#      ksb's own guard screens only by commenter association.
#
# Out of scope: a label assembled at runtime (`runs-on: ${{ vars.X }}`, `fromJSON(...)`) never
# appears literally, so it cannot be checked here. Route ksb jobs by a literal label only.
#
# Usage: check-ksb-fork-guard.sh [FILE…]      (default: .github/workflows/*.y*ml)
#        check-ksb-fork-guard.sh --self-test  (runs the fixtures in scripts/tests/ksb-fork-guard/)
set -euo pipefail

# The predicate, verbatim from the devkit ADR. Kept as ONE literal so a weakened copy (say, only
# `head.repo.fork`) is a mismatch rather than a judgement call.
PRED='github.event.pull_request && (github.event.pull_request.head.repo.fork || github.event.pull_request.head.repo.full_name != github.repository)'
LABEL_RE="ksb-[a-z0-9]+(-[a-z0-9]+)*-(small|large|xlarge)"
ALLOWED_TRIGGERS=" push pull_request workflow_dispatch schedule "

# The workflow's trigger names, one per line: `on: push`, `on: [push, pull_request]`, or the keys of
# an `on:` block (the first indentation level below it). `true:` is how YAML 1.1 spells a bare `on:`.
triggers() {
  awk '
    /^(on|"on"|true):/ {
      v = $0; sub(/^[^:]*:[ \t]*/, "", v); sub(/[ \t]*#.*$/, "", v)
      if (v != "") { gsub(/[][,]/, " ", v); n = split(v, a, /[ \t]+/); for (i = 1; i <= n; i++) if (a[i] != "") print a[i]; exit }
      inon = 1; next
    }
    inon && /^[^ \t#]/ { exit }
    inon && /^[ \t]+[A-Za-z_]+:/ {
      match($0, /^[ \t]+/); ind = RLENGTH
      if (first == 0) first = ind
      if (ind == first) { k = $0; sub(/^[ \t]+/, "", k); sub(/:.*/, "", k); print k }
    }
  ' "$1"
}

check_file() {
  local f="$1" status=0 named=0 lineno content code trigger
  shopt -s nocasematch
  while IFS= read -r line; do
    lineno="${line%%:*}"
    content="${line#*:}"
    # Comments may name a label freely; only YAML values route jobs.
    code="${content%%#*}"
    [[ "$code" =~ $LABEL_RE ]] || continue
    named=1
    if [[ "$code" != *"\${{ (${PRED}) && 'ubuntu-"* ]]; then
      echo "$f:$lineno: ksb label without the fork guard: expected \${{ (${PRED}) && 'ubuntu-…' || 'ksb-…' }}" >&2
      status=1
    elif ! [[ "$code" =~ \|\|\ \'${LABEL_RE}\'\ \}\} ]]; then
      echo "$f:$lineno: the ksb label must be the LAST (non-fork) branch: … && 'ubuntu-…' || 'ksb-…' }}" >&2
      status=1
    fi
  done < <(grep -niE "$LABEL_RE" "$f" || true)

  # A runner group is `runs-on: {group: …}` or a `group:` key in the mapping under `runs-on:`.
  # `concurrency: group:` is not one, whatever it is called.
  local prev="" n=0
  while IFS= read -r line || [ -n "$line" ]; do
    n=$((n + 1))
    code="${line%%#*}"
    if [[ "$code" =~ group:[[:space:]]*[\'\"]?ksb ]] \
      && { [[ "$code" == *runs-on:* ]] || [[ "$prev" =~ ^[[:space:]]*runs-on:[[:space:]]*$ ]]; }; then
      echo "$f:$n: runner-group routing to ksb bypasses the fork guard: name a guarded ksb label instead" >&2
      status=1
      named=1
    fi
    [[ "$code" =~ [^[:space:]] ]] && prev="$code"
  done < "$f"

  if [ "$named" -eq 1 ]; then
    while IFS= read -r trigger; do
      if [[ "$ALLOWED_TRIGGERS" != *" $trigger "* ]]; then
        echo "$f: trigger '$trigger' in a workflow that routes to ksb: only${ALLOWED_TRIGGERS% } are allowed (see this script's header)" >&2
        status=1
      fi
    done < <(triggers "$f")
  fi
  shopt -u nocasematch
  return "$status"
}

self_test() {
  local dir fixture want got status=0 have
  dir="$(dirname "$0")/tests/ksb-fork-guard"
  # The EXACT fixture set, by name: a count floor would still pass with fixtures deleted, and an empty
  # directory must fail rather than pass vacuously. Add a fixture here when adding its file.
  local expected="bad-issue-comment.yml bad-matrix-list.yml bad-not-last.yml bad-runner-group.yml
bad-swapped.yml bad-unguarded.yml bad-uppercase-label.yml bad-weak-predicate.yml bad-workflow-run.yml
good-guarded.yml good-hosted-only.yml good-inline-on.yml"
  shopt -s nullglob
  local fixtures=("$dir"/*.yml)
  have=$(for fixture in "${fixtures[@]}"; do basename "$fixture"; done | sort | tr '\n' ' ')
  if [ "$have" != "$(tr ' ' '\n' <<< "$expected" | sort | tr '\n' ' ')" ]; then
    echo "self-test: fixture set in $dir is [$have], expected [$(tr '\n' ' ' <<< "$expected")]" >&2
    return 1
  fi
  for fixture in "${fixtures[@]}"; do
    case "$(basename "$fixture")" in
      good-*) want=0 ;;
      bad-*) want=1 ;;
      *) echo "self-test: $fixture is neither good-* nor bad-*" >&2; status=1; continue ;;
    esac
    if check_file "$fixture" 2>/dev/null; then got=0; else got=1; fi
    if [ "$got" -ne "$want" ]; then
      echo "self-test: $(basename "$fixture"): exit $got, expected $want" >&2
      status=1
    fi
  done
  [ "$status" -eq 0 ] && echo "self-test: ${#fixtures[@]} fixtures OK"
  return "$status"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit
fi

shopt -s nullglob
if [ "$#" -gt 0 ]; then files=("$@"); else files=(.github/workflows/*.yml .github/workflows/*.yaml); fi
status=0
for f in "${files[@]}"; do
  check_file "$f" || status=1
done
exit "$status"
