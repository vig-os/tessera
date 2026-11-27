#!/usr/bin/env bash
# File, or add to, the issue for a failed unattended CI run (#725). A pull request's red check is
# seen by its author; a nightly or release-tag run has no one watching it, so its failure has to
# land where the team triages: an open issue labelled `bug` + `area:ci`.
#
# ONE ISSUE PER TITLE. The caller picks a title per kind of run (nix-check.yml: one for the
# nightly, one per release tag). An open issue with these labels and EXACTLY that title gets a
# comment with the new run; otherwise a new issue is created. Titles are compared with jq `==`,
# never `contains`/`inside` (a substring match would pile a release tag's failure onto the
# nightly's issue). When several open issues share the title, the lowest number is used.
#
# Usage: ci-failure-issue.sh TITLE BODY
#        ci-failure-issue.sh --self-test        stubbed `gh`; run by the `ci-failure-issue` check
# Environment: GH_REPO (owner/name, required; also read by `gh`), GH_TOKEN for `gh`.
#   DRY_RUN=1 looks the issue up for real but only prints the comment or issue it would write.
# Exit status: 0 when the issue was written (or, dry, would be), 1 when a `gh` call failed.
set -uo pipefail

LABELS=(bug area:ci)

report() {
  local title="$1" body="$2" found number
  local label_args=()
  for l in "${LABELS[@]}"; do label_args+=(--label "$l"); done

  # `--search` narrows the listing so the result cannot be cut off by `--limit`; the exact match
  # is the jq filter, not the search.
  if ! found=$(gh issue list --repo "$GH_REPO" --state open "${label_args[@]}" \
      --search "\"$title\" in:title" --limit 100 --json number,title); then
    echo "::error title=ci failure issue::could not list the open issues of $GH_REPO"
    return 1
  fi
  if ! number=$(jq -r --arg t "$title" '[.[] | select(.title == $t) | .number] | sort | first // empty' \
      <<< "$found"); then
    echo "::error title=ci failure issue::gh issue list returned something that is not an issue list"
    return 1
  fi

  if [ -n "$number" ]; then
    if [ "${DRY_RUN:-0}" = 1 ]; then
      printf 'dry run: would comment on #%s (%s):\n%s\n' "$number" "$title" "$body"
      return 0
    fi
    gh issue comment "$number" --repo "$GH_REPO" --body "$body" || return 1
    echo "commented on #$number"
  else
    if [ "${DRY_RUN:-0}" = 1 ]; then
      printf 'dry run: would create an issue labelled %s titled "%s":\n%s\n' "${LABELS[*]}" "$title" "$body"
      return 0
    fi
    gh issue create --repo "$GH_REPO" --title "$title" "${label_args[@]}" --body "$body" || return 1
  fi
}

# ── Self-test: `gh` is stubbed; it serves a canned issue list and records every call. ────────────
self_test() {
  local tmp status=0 ran=0
  tmp=$(mktemp -d)
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  mkdir -p "$tmp/bin"
  # `issue list` prints $tmp/list with status $tmp/list_rc; every call is appended to $tmp/calls.
  cat > "$tmp/bin/gh" <<EOF
#!$BASH
printf '%s\n' "\$*" >> "$tmp/calls"
case "\$1 \$2" in
  "issue list") cat "$tmp/list"; exit "\$(cat "$tmp/list_rc")" ;;
  "issue comment"|"issue create") exit 0 ;;
  *) echo "stub gh: unexpected \$*" >&2; exit 2 ;;
esac
EOF
  chmod +x "$tmp/bin/gh"

  # case NAME WANT_STATUS WANT_CALL DRY LIST_RC LIST
  # WANT_CALL is a fragment the LAST recorded gh call must contain; "list" asserts that nothing
  # was written (the last call is the lookup).
  run_case() {
    local name="$1" want="$2" want_call="$3" dry="$4" list_rc="$5" list="$6" got out last
    ran=$((ran+1))
    printf '%s\n' "$list" > "$tmp/list"
    echo "$list_rc" > "$tmp/list_rc"
    : > "$tmp/calls"
    out=$(PATH="$tmp/bin:$PATH" GH_REPO=o/r DRY_RUN="$dry" report "nightly failed" "run 42" 2>&1)
    got=$?
    last=$(tail -n 1 "$tmp/calls")
    if [ "$got" -ne "$want" ]; then
      echo "self-test: $name: exit $got, expected $want; output:" >&2
      printf '%s\n' "$out" >&2
      status=1
    elif ! grep -qF -- "$want_call" <<< "$last"; then
      echo "self-test: $name: last gh call '$last' lacks '$want_call'" >&2
      status=1
    fi
  }
  run_case "no open issue: create, labelled" 0 \
    "issue create --repo o/r --title nightly failed --label bug --label area:ci --body run 42" 0 0 '[]'
  run_case "the lookup is labelled and exact-titled" 0 \
    "issue list --repo o/r --state open --label bug --label area:ci --search \"nightly failed\" in:title" 1 0 '[]'
  run_case "exact title open: comment" 0 "issue comment 7 --repo o/r --body run 42" 0 0 \
    '[{"number":7,"title":"nightly failed"}]'
  # A title that merely CONTAINS the wanted one is another issue.
  run_case "only a longer title: create" 0 "issue create" 0 0 \
    '[{"number":7,"title":"nightly failed on v0.1.0"}]'
  run_case "two exact titles: the lowest" 0 "issue comment 3 " 0 0 \
    '[{"number":9,"title":"nightly failed"},{"number":3,"title":"nightly failed"}]'
  run_case "list fails: nothing written" 1 "issue list" 0 1 '[]'
  run_case "list is not JSON: nothing written" 1 "issue list" 0 0 'not json'
  run_case "dry run, no issue: nothing written" 0 "issue list" 1 0 '[]'
  run_case "dry run, open issue: nothing written" 0 "issue list" 1 0 '[{"number":7,"title":"nightly failed"}]'

  # The exact number of cases, so a case deleted by accident fails here rather than shrinking the test.
  if [ "$ran" -ne 9 ]; then
    echo "self-test: ran $ran cases, expected 9" >&2
    status=1
  fi
  [ "$status" -eq 0 ] && echo "self-test: $ran cases OK"
  return "$status"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit
fi
if [ "$#" -ne 2 ]; then
  echo "usage: $0 TITLE BODY | --self-test" >&2
  exit 2
fi
: "${GH_REPO:?GH_REPO must name the repository (owner/name)}"
report "$1" "$2"
