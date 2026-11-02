#!/usr/bin/env bash
# Which `nix flake check` legs run for an event, and whether exactly those passed (#725).
# nix-check.yml's `plan` job asks it which legs to run; its `gate` job (the required `nix flake
# check` context) asks it whether the run had exactly those legs, each green.
#
# THE RULE (owner decision on #725, 2026-11-02): the aarch64-linux leg is the slowest (46-82 min
# hosted), so it is off the per-PR path.
#
#   event                                   legs
#   pull_request, push to a branch          x86_64-linux
#   schedule (nightly, on dev)              x86_64-linux aarch64-linux
#   push of a release tag                   x86_64-linux aarch64-linux
#   workflow_dispatch                       x86_64-linux aarch64-linux
#   anything else                           none: the plan fails, so the gate fails
#
# A schedule on any ref but dev fails too: GitHub runs a schedule on the DEFAULT branch, which is
# dev today, and a nightly that silently moved to another branch would check the wrong code. The
# release-tag pattern is nix-check.yml's, not repeated here: any tag push reaching this script is
# one that filter admitted.
#
# THE GATE IS ASSERTED BY NAME, NEVER OVER WHAT HAPPENED TO RUN (#722/#723). It works the legs out
# again from the event and ref, with the same rule, rather than trusting the plan job's output, and
# is given every leg's job result by name. It fails unless:
#   - the plan job succeeded and named exactly the legs the rule gives (a lost or mangled output
#     would otherwise skip the aarch64 job and leave the gate checking a shorter set);
#   - it got a result for exactly the known legs, each once, and no other name;
#   - every planned leg is `success` and every unplanned one `skipped` (a leg that ran when the
#     plan said not to means the workflow and this rule have drifted apart).
# A predicate over the legs that happened to run would pass on none of them, which is how a gate
# reads "no aarch64 leg" as green on a nightly whose aarch64 job never started.
#
# Usage: nix-check-legs.sh plan EVENT REF           prints `legs=…` and `run-<leg>=true|false`
#                                                   lines for $GITHUB_OUTPUT
#        nix-check-legs.sh gate EVENT REF PLAN_RESULT PLANNED LEG=RESULT…
#                                                   PLAN_RESULT the plan job's result, PLANNED its
#                                                   `legs` output, one LEG=RESULT per known leg
#        nix-check-legs.sh --self-test
# Exit status: 0 when the plan is made / the gate holds, 1 when not (the reason on stderr), 2 on a
# usage error.
set -uo pipefail

# Every leg nix-check.yml can run, by name: each is a job whose name is `nix flake check (<leg>)`.
KNOWN_LEGS="x86_64-linux aarch64-linux"
# The leg that runs for every event, so a plan can never be empty.
ALWAYS_LEG="x86_64-linux"
SCHEDULE_REF="refs/heads/dev"

# 0 when $1 is one of the space-separated words in $2. Whole-word equality, never a substring.
is_one_of() {
  local w
  for w in $2; do
    [ "$w" = "$1" ] && return 0
  done
  return 1
}

plan() {
  local event="$1" ref="$2" legs leg
  case "$event" in
    pull_request) legs="$ALWAYS_LEG" ;;
    push)
      case "$ref" in
        refs/heads/*) legs="$ALWAYS_LEG" ;;
        refs/tags/*) legs="$KNOWN_LEGS" ;;
        *) echo "plan: push to '$ref' is neither a branch nor a tag" >&2; return 1 ;;
      esac
      ;;
    schedule)
      if [ "$ref" != "$SCHEDULE_REF" ]; then
        echo "plan: a schedule must run on $SCHEDULE_REF, not '$ref'" >&2
        return 1
      fi
      legs="$KNOWN_LEGS"
      ;;
    workflow_dispatch) legs="$KNOWN_LEGS" ;;
    *) echo "plan: no rule for event '$event'; add one to $0 deliberately" >&2; return 1 ;;
  esac
  echo "legs=$legs"
  for leg in $KNOWN_LEGS; do
    if is_one_of "$leg" "$legs"; then echo "run-$leg=true"; else echo "run-$leg=false"; fi
  done
}

# The words of $1, sorted and space-joined, so two leg lists compare as sets.
as_set() {
  tr ' ' '\n' <<< "$1" | grep . | sort | tr '\n' ' '
}

gate() {
  local event="$1" ref="$2" plan_result="$3" planned="$4" legs pair name result seen="" status=0 leg
  shift 4
  if [ "$plan_result" != success ]; then
    echo "gate: the plan job did not succeed (result '$plan_result'), so no leg set is known" >&2
    return 1
  fi
  # Every rule of `plan` names at least $ALWAYS_LEG, and an event without a rule fails here.
  if ! legs=$(plan "$event" "$ref" | sed -n 's/^legs=//p'); then
    echo "gate: no legs for event '$event' on '$ref'" >&2
    return 1
  fi
  if [ "$(as_set "$planned")" != "$(as_set "$legs")" ]; then
    echo "gate: the plan job named legs [$planned], but the rule gives [$legs] for $event on $ref" >&2
    return 1
  fi
  for pair in "$@"; do
    name="${pair%%=*}"
    result="${pair#*=}"
    if [ "$name" = "$pair" ] || ! is_one_of "$name" "$KNOWN_LEGS"; then
      echo "gate: '$pair' is not LEG=RESULT for a known leg (known: $KNOWN_LEGS)" >&2
      return 1
    fi
    if is_one_of "$name" "$seen"; then
      echo "gate: two results for $name" >&2
      return 1
    fi
    seen="$seen $name"
    if is_one_of "$name" "$legs"; then
      if [ "$result" = success ]; then
        echo "gate: $name: success (planned)"
      else
        echo "::error title=nix flake check::the $name leg did not pass (result '$result')"
        status=1
      fi
    elif [ "$result" = skipped ]; then
      echo "gate: $name: skipped (not planned for this event)"
    else
      echo "::error title=nix flake check::the $name leg was not planned for this event but its result is '$result'"
      status=1
    fi
  done
  for leg in $KNOWN_LEGS; do
    if ! is_one_of "$leg" "$seen"; then
      echo "gate: no result was given for $leg" >&2
      status=1
    fi
  done
  [ "$status" -eq 0 ] && echo "gate: planned legs [$legs] all passed"
  return "$status"
}

# ── Self-test: every rule of `plan` and every refusal of `gate`, with no GitHub involved. ──────────
self_test() {
  local status=0 ran=0
  # expect NAME WANT_STATUS WANT_OUTPUT CMD…: runs CMD, asserts its exit status and, when
  # WANT_OUTPUT is non-empty, that its stdout+stderr contains that line fragment.
  expect() {
    local name="$1" want="$2" want_out="$3" got out
    shift 3
    ran=$((ran+1))
    out=$("$@" 2>&1)
    got=$?
    if [ "$got" -ne "$want" ]; then
      echo "self-test: $name: exit $got, expected $want; output:" >&2
      printf '%s\n' "$out" >&2
      status=1
    elif [ -n "$want_out" ] && ! grep -qF -- "$want_out" <<< "$out"; then
      echo "self-test: $name: output lacks '$want_out'; output:" >&2
      printf '%s\n' "$out" >&2
      status=1
    fi
  }
  local both="x86_64-linux aarch64-linux"

  # plan: the per-PR path is x86_64 only; the aarch64 leg runs nightly, on release tags and by hand.
  expect "pull_request: x86_64 only" 0 "legs=x86_64-linux" plan pull_request refs/pull/1/merge
  expect "pull_request: no aarch64" 0 "run-aarch64-linux=false" plan pull_request refs/pull/1/merge
  expect "push to main: x86_64 only" 0 "run-aarch64-linux=false" plan push refs/heads/main
  expect "push of a tag: both" 0 "legs=$both" plan push refs/tags/v0.1.0
  expect "schedule on dev: both" 0 "run-aarch64-linux=true" plan schedule refs/heads/dev
  expect "schedule on main: refused" 1 "a schedule must run on refs/heads/dev, not 'refs/heads/main'" \
    plan schedule refs/heads/main
  expect "workflow_dispatch: both" 0 "run-aarch64-linux=true" plan workflow_dispatch refs/heads/ci/x
  expect "workflow_dispatch: x86_64 too" 0 "run-x86_64-linux=true" plan workflow_dispatch refs/heads/ci/x
  expect "unknown event: refused" 1 "no rule for event 'merge_group'" plan merge_group refs/heads/dev
  expect "push to neither: refused" 1 "is neither a branch nor a tag" plan push refs/notes/x

  # gate: exactly the planned legs, by name.
  local pr="pull_request refs/pull/1/merge" night="schedule refs/heads/dev"
  # shellcheck disable=SC2086  # $pr / $night are two words each: EVENT REF.
  {
  expect "PR, x86 green, arm skipped" 0 "all passed" \
    gate $pr success x86_64-linux x86_64-linux=success aarch64-linux=skipped
  expect "nightly, both green" 0 "all passed" \
    gate $night success "$both" x86_64-linux=success aarch64-linux=success
  expect "plan order does not matter" 0 "all passed" \
    gate $night success "aarch64-linux x86_64-linux" x86_64-linux=success aarch64-linux=success
  expect "nightly, arm failed" 1 "the aarch64-linux leg did not pass (result 'failure')" \
    gate $night success "$both" x86_64-linux=success aarch64-linux=failure
  # The vacuous case this gate exists for: a nightly whose aarch64 job never ran is not green.
  expect "nightly, arm skipped" 1 "the aarch64-linux leg did not pass (result 'skipped')" \
    gate $night success "$both" x86_64-linux=success aarch64-linux=skipped
  # ... and a plan output that lost the aarch64 leg does not make it green either.
  expect "nightly, plan output short" 1 "the plan job named legs [x86_64-linux], but the rule gives" \
    gate $night success x86_64-linux x86_64-linux=success aarch64-linux=skipped
  expect "nightly, plan output empty" 1 "the plan job named legs [], but the rule gives" \
    gate $night success "" x86_64-linux=success aarch64-linux=skipped
  expect "PR, plan output too long" 1 "the plan job named legs [$both], but the rule gives [x86_64-linux]" \
    gate $pr success "$both" x86_64-linux=success aarch64-linux=success
  # A substring of a known leg must not count as it.
  expect "substring planned leg" 1 "the plan job named legs [x86_64]" \
    gate $pr success x86_64 x86_64-linux=success aarch64-linux=skipped
  expect "PR, x86 cancelled" 1 "the x86_64-linux leg did not pass (result 'cancelled')" \
    gate $pr success x86_64-linux x86_64-linux=cancelled aarch64-linux=skipped
  expect "PR, arm ran unplanned" 1 "was not planned for this event but its result is 'success'" \
    gate $pr success x86_64-linux x86_64-linux=success aarch64-linux=success
  expect "plan failed" 1 "the plan job did not succeed (result 'failure')" \
    gate $pr failure "" x86_64-linux=success aarch64-linux=skipped
  expect "event without a rule" 1 "no legs for event 'merge_group'" \
    gate merge_group refs/heads/dev success x86_64-linux x86_64-linux=success aarch64-linux=skipped
  expect "result missing" 1 "no result was given for aarch64-linux" \
    gate $pr success x86_64-linux x86_64-linux=success
  expect "no results" 1 "no result was given for x86_64-linux" gate $pr success x86_64-linux
  expect "duplicate result" 1 "two results for x86_64-linux" \
    gate $pr success x86_64-linux x86_64-linux=failure x86_64-linux=success aarch64-linux=skipped
  expect "unknown result name" 1 "is not LEG=RESULT for a known leg" \
    gate $pr success x86_64-linux x86_64-linux=success aarch64-linux=skipped arm=success
  expect "malformed result" 1 "is not LEG=RESULT" \
    gate $pr success x86_64-linux x86_64-linux aarch64-linux=skipped
  }

  # The exact number of cases, so a case deleted by accident fails here rather than shrinking the test.
  if [ "$ran" -ne 28 ]; then
    echo "self-test: ran $ran cases, expected 28" >&2
    status=1
  fi
  [ "$status" -eq 0 ] && echo "self-test: $ran cases OK"
  return "$status"
}

case "${1:-}" in
  --self-test) self_test; exit ;;
  plan)
    [ "$#" -eq 3 ] || { echo "usage: $0 plan EVENT REF" >&2; exit 2; }
    plan "$2" "$3"
    ;;
  gate)
    [ "$#" -ge 5 ] || { echo "usage: $0 gate EVENT REF PLAN_RESULT PLANNED LEG=RESULT…" >&2; exit 2; }
    shift
    gate "$@"
    ;;
  *) echo "usage: $0 plan EVENT REF | gate EVENT REF PLAN_RESULT PLANNED LEG=RESULT… | --self-test" >&2; exit 2 ;;
esac
