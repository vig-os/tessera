#!/usr/bin/env bash
# Per-check summary of one `nix flake check` leg, so a red leg always names what failed (#483).
# nix-check.yml runs it after the check step, which swallowed nix's exit status so this could run.
#
# Derived from the STORE, not by parsing nix's human-readable output: a check whose output path is
# valid was built and therefore passed. `--keep-going` on the check step is what makes that sound:
# every check is attempted, so a missing output means that check failed rather than that it was
# skipped, unless a substituter already had it (see `cached_in` below). No output format to drift
# against.
#
# THE SET OF CHECKS IS ASSERTED, NOT TRUSTED (#722). The names come from `nix eval`. Counting only
# what it returns would make every verdict a predicate over a possibly-empty collection: an
# evaluation returning `{}` with status 0 used to print "0 passed, 0 failed" and leave the leg
# green. So the evaluated names must equal, exactly and by name, the list in
# .github/nix-flake-checks.txt, whose `count:` line must match its own names. A missing, extra or
# empty set fails the leg, and the error names the difference. Membership is compared with
# `sort`/`comm` on whole lines, never with jq `inside`/`contains` (substring matches on strings).
#
# Usage: nix-check-summary.sh SYSTEM CHECK_RC   writes the table to $GITHUB_STEP_SUMMARY
#        nix-check-summary.sh --self-test        stubbed `nix`; run by the `nix-check-summary` check
#
# Exit status: 1 when the evaluated set differs from the list, when the evaluation fails, or when nix
# passed but the summary counted a failure; otherwise CHECK_RC (1 when empty), so the leg keeps
# nix's own verdict.
# Environment: GITHUB_STEP_SUMMARY (required); NIX_CHECKS_EXPECTED overrides the list's path.
set -uo pipefail

# Non-empty lines of stdin, joined by single spaces.
words() {
  grep . | tr '\n' ' ' | sed 's/ $//'
}

# The expected names, one per line, sorted. Fails (status 1, reason on stderr) when the list is
# unreadable, has no `count:` line or more than one, has a malformed or duplicate name, has a count
# below 1, or a count that differs from the number of names.
expected_names() {
  local file="$1" names count n
  [ -r "$file" ] || { echo "cannot read the expected-check list $file" >&2; return 1; }
  names=$(grep -vE '^(#|count:|$)' "$file" | sort)
  if [ "$(grep -cE '^count:' "$file")" -ne 1 ]; then
    echo "$file must have exactly one 'count:' line" >&2
    return 1
  fi
  count=$(sed -nE 's/^count:[[:space:]]*([0-9]+)[[:space:]]*$/\1/p' "$file")
  if [ -z "$count" ] || [ "$count" -lt 1 ]; then
    echo "$file: the count must be a positive integer" >&2
    return 1
  fi
  if [ -z "$names" ]; then
    echo "$file lists no check names" >&2
    return 1
  fi
  if grep -vqE '^[A-Za-z0-9_-]+$' <<< "$names"; then
    echo "$file: malformed check name(s): $(grep -vE '^[A-Za-z0-9_-]+$' <<< "$names" | words)" >&2
    return 1
  fi
  if [ -n "$(uniq -d <<< "$names")" ]; then
    echo "$file: duplicate check name(s): $(uniq -d <<< "$names" | words)" >&2
    return 1
  fi
  n=$(grep -c . <<< "$names")
  if [ "$n" -ne "$count" ]; then
    echo "$file lists $n check name(s) but says count: $count" >&2
    return 1
  fi
  printf '%s\n' "$names"
}

# The substituter that holds OUTP, or nothing. A cached output counts as a pass:
#
# `nix-store --check-validity` (the caller's first test) is unambiguously LOCAL. Plain
# `nix path-info` may consult substituters, which would report a path as present because a cache
# HAS it rather than because this leg built it, turning a failed check into a green row. Nix 2.34's
# `nix flake check` itself skips every check whose output a substituter already has (it builds
# only `queryMissing`'s `willBuild`: src/nix/flake.cc), so such a check is never built here and
# never downloaded. On ksb the shared cache holds outputs from earlier runs of the SAME
# derivations, and the local test alone showed those checks as "failed or not built" on every ksb
# leg (#688). A check that nix did build and that failed is in no cache, so it stays red. Each
# cached row names the cache.
#
# TRUST ASSUMPTION. Counting a substituted output as a pass is sound only while FORK CODE CANNOT
# WRITE TO THE SHARED ksb BINARY CACHE. A check's output path is input-addressed: it is derived from
# the derivation, which hashes this exact source, so a hit means that derivation was built
# successfully by something with write access, or forged by it. As of 2026-11-02 that holds:
#   - fork PRs never run on ksb: the fork guard on every ksb label in this repo's workflows
#     (scripts/check-ksb-fork-guard.sh), the fork-PR approval setting, and ksb's job-started
#     fork-guard hook that kills a fork's job before its first step (exo-pet/exo-fleet#949). The
#     fork fallback runs GitHub-hosted, which has no ksb cache at all;
#   - ARC runner pods only READ the cache (`http://10.0.2.2:8092/ksb`, signatures required): they
#     hold no attic token, only the JIT runner token and their org's sccache key
#     (exo-pet/exo-fleet#911, "Nix cache: read-only substitution");
#   - the only writers are the `ksb-attic-push` units (`attic watch-store ksb`) on the ksb host and
#     on the seat, each holding a push token kept out of the store (gerchowl/g-fleet
#     hosts/ksb/attic-push.nix; exo-pet/exo-fleet#919).
# Residual: the seat pushes whatever its users build, so fork code a seat user builds by hand would
# reach the cache; CI and the seat sharing one trust domain is open as exo-pet/exo-fleet#911 ("use
# a host-only CI cache key"). Re-check this note if any of those change, above all if ARC pods
# ever get a push token.
cached_in() {
  local outp="$1" sub
  [ -n "$outp" ] || return 0
  for sub in $SUBSTITUTERS; do
    if nix path-info --store "$sub" "$outp" >/dev/null 2>&1; then
      printf '%s\n' "$sub"
      return 0
    fi
  done
}

summarise() {
  local sys="$1" check_rc="$2" S="$GITHUB_STEP_SUMMARY"
  local expected_file="${NIX_CHECKS_EXPECTED:-$(dirname "$0")/../.github/nix-flake-checks.txt}"
  local expected paths_json eval_err eval_rc actual missing extra name outp cache
  local pass=0 fail=0 cached=0 failed_names=""

  echo "## nix flake check — ${sys}" >> "$S"
  echo "" >> "$S"

  # The check step's `$?`, or empty when it never ran (then re-raised as 1). Anything else is a
  # wiring error, not a status to pass through.
  if ! [[ "$check_rc" =~ ^[0-9]*$ ]]; then
    echo "::error title=check summary::CHECK_RC '$check_rc' is not an exit status"
    return 1
  fi

  if ! expected=$(expected_names "$expected_file"); then
    echo ":x: **The expected-check list \`$expected_file\` is invalid**; see the job log." >> "$S"
    echo "::error title=check summary::the expected-check list $expected_file is invalid (reason above)"
    return 1
  fi

  # One evaluation for every check's output path, instead of `attrNames` followed by an `outPath`
  # eval per check: the name and the path cannot disagree because they come from the same attrset.
  eval_err=$(mktemp)
  paths_json=$(nix eval --json ".#checks.${sys}" --apply 'builtins.mapAttrs (_: c: c.outPath)' 2> "$eval_err")
  eval_rc=$?
  if [ "$eval_rc" -ne 0 ] || ! actual=$(jq -r 'if type == "object" then keys[] else error("not an object") end' \
      <<< "$paths_json" 2>/dev/null); then
    {
      echo ":x: **Could not evaluate \`.#checks.${sys}\`** (\`nix eval\` exited \`$eval_rc\`), so no check is"
      echo "accounted for. The job log has nix's error."
    } >> "$S"
    echo "nix eval stderr (tail):"
    tail -n 20 "$eval_err"
    rm -f "$eval_err"
    echo "::error title=check summary::could not evaluate .#checks.${sys} (nix eval exited $eval_rc)"
    return 1
  fi
  rm -f "$eval_err"
  # `{}` evaluates fine and yields no names: the set comparison below fails it, with no special case.
  actual=$(printf '%s\n' "$actual" | grep . | sort)
  missing=$(comm -23 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") | words)
  extra=$(comm -13 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") | words)

  {
    echo "| check | result |"
    echo "|---|---|"
  } >> "$S"

  SUBSTITUTERS=$(nix config show substituters 2>/dev/null)
  while IFS=$'\t' read -r name outp; do
    [ -n "$name" ] || continue
    if [ -n "$outp" ] && nix-store --check-validity "$outp" >/dev/null 2>&1; then
      echo "| \`$name\` | :white_check_mark: passed |" >> "$S"
      pass=$((pass+1))
      continue
    fi
    cache=$(cached_in "$outp")
    if [ -n "$cache" ]; then
      echo "| \`$name\` | :white_check_mark: passed earlier (output in \`$cache\`, so nix skipped it) |" >> "$S"
      pass=$((pass+1)); cached=$((cached+1))
    else
      echo "| \`$name\` | :x: failed or not built |" >> "$S"
      fail=$((fail+1)); failed_names="$failed_names $name"
    fi
  done <<< "$(jq -r 'to_entries[] | "\(.key)\t\(.value)"' <<< "$paths_json")"
  for name in $missing; do
    echo "| \`$name\` | :x: missing from the evaluated checks |" >> "$S"
  done

  {
    echo ""
    echo "**$pass passed ($cached from a binary cache), $fail failed or not built** (\`nix flake check\` exited \`${check_rc:-?}\`)."
    if [ "$fail" -gt 0 ]; then
      echo ""
      echo "Not built:$failed_names"
      echo ""
      echo "A check listed here either failed or never produced an output. With \`--keep-going\`"
      echo "every check is attempted, so the usual reading is a real failure — unless the leg hit"
      echo "its job cap, in which case the tail of the job log says so."
    fi
  } >> "$S"
  echo "summary: $pass passed ($cached of them from a binary cache), $fail failed or not built; check rc=${check_rc:-?}"

  # The evaluated set must be exactly the expected one. This is what turns an empty or short
  # evaluation red: with nothing to count, every other test here passes vacuously.
  if [ -n "$missing" ] || [ -n "$extra" ]; then
    {
      echo ""
      echo ":x: **\`.#checks.${sys}\` is not the expected set** (\`$expected_file\`):"
      [ -z "$missing" ] || echo "missing: \`$missing\`"
      [ -z "$extra" ] || echo "unexpected: \`$extra\`"
    } >> "$S"
    echo "::error title=check set::.#checks.${sys} differs from $expected_file; missing: ${missing:-none}; unexpected: ${extra:-none}"
    return 1
  fi
  # The summary must agree with nix. With `--keep-going`, rc 0 means every check passed, so a red
  # row on a green run is this summary miscounting (as it did for cached checks, #688), and it fails
  # the leg rather than publishing a wrong table. The converse is not asserted: rc != 0 with no red
  # row is legitimate (an evaluation error, the job cap).
  if [ "${check_rc:-1}" -eq 0 ] && [ "$fail" -gt 0 ]; then
    echo "::error title=check summary::nix flake check passed, but the summary counted $fail check(s) as failed or not built:$failed_names"
    return 1
  fi
  # Re-raise the real result: the check step swallowed it so this summary could run.
  return "${check_rc:-1}"
}

# ── Self-test: `nix` and `nix-store` are stubbed, so every case runs without Nix or a build. ──────
#
# Each case gives the stubbed evaluation, which outputs are valid locally or cached, the check
# step's status and the expected-check list, and asserts the exact exit status and, for a failure,
# a line of the output that names the reason.
self_test() {
  local tmp status=0 ran=0
  tmp=$(mktemp -d)
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT
  mkdir -p "$tmp/bin"
  # A stubbed `nix`. `eval` prints $tmp/eval (or nothing) with status $tmp/eval_rc; `config show
  # substituters` names one cache; `path-info --store` finds a path listed in $tmp/cached.
  cat > "$tmp/bin/nix" <<EOF
#!$BASH
case "\$1" in
  eval) [ ! -f "$tmp/eval" ] || cat "$tmp/eval"; exit "\$(cat "$tmp/eval_rc")" ;;
  config) echo "https://cache.example/" ;;
  path-info) grep -qxF "\$4" "$tmp/cached" ;;
  *) echo "stub nix: unexpected \$*" >&2; exit 2 ;;
esac
EOF
  # A stubbed `nix-store --check-validity PATH`: valid when listed in $tmp/valid.
  cat > "$tmp/bin/nix-store" <<EOF
#!$BASH
grep -qxF "\$2" "$tmp/valid"
EOF
  chmod +x "$tmp/bin/nix" "$tmp/bin/nix-store"

  local good="count: 3
alpha
beta
gamma"
  # case NAME WANT_STATUS WANT_OUTPUT CHECK_RC EVAL_RC EVAL_JSON VALID CACHED LIST
  # VALID / CACHED are space-separated check names whose output path (/nix/store/<name>) is valid
  # locally / in the stub cache. An empty WANT_OUTPUT asserts nothing about the output.
  run_case() {
    local name="$1" want="$2" want_out="$3" check_rc="$4" eval_rc="$5" eval_json="$6" valid="$7" cached="$8"
    local list="$9" got out
    ran=$((ran+1))
    rm -f "$tmp/eval"
    [ -z "$eval_json" ] || printf '%s\n' "$eval_json" > "$tmp/eval"
    echo "$eval_rc" > "$tmp/eval_rc"
    tr ' ' '\n' <<< "$valid" | sed 's|^|/nix/store/|' > "$tmp/valid"
    tr ' ' '\n' <<< "$cached" | sed 's|^|/nix/store/|' > "$tmp/cached"
    printf '%s\n' "$list" > "$tmp/expected"
    : > "$tmp/summary"
    out=$(PATH="$tmp/bin:$PATH" GITHUB_STEP_SUMMARY="$tmp/summary" NIX_CHECKS_EXPECTED="$tmp/expected" \
      summarise x86_64-linux "$check_rc" 2>&1)
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
  local all='{"alpha":"/nix/store/alpha","beta":"/nix/store/beta","gamma":"/nix/store/gamma"}'
  run_case "all built" 0 "summary: 3 passed (0 of them" 0 0 "$all" "alpha beta gamma" "" "$good"
  run_case "one cached" 0 "summary: 3 passed (1 of them" 0 0 "$all" "alpha beta" "gamma" "$good"
  run_case "empty set, rc 0" 1 "missing: alpha beta gamma; unexpected: none" 0 0 '{}' "" "" "$good"
  run_case "one missing, rc 0" 1 "missing: gamma; unexpected: none" 0 0 \
    '{"alpha":"/nix/store/alpha","beta":"/nix/store/beta"}' "alpha beta" "" "$good"
  run_case "one extra, rc 0" 1 "missing: none; unexpected: delta" 0 0 \
    '{"alpha":"/nix/store/alpha","beta":"/nix/store/beta","gamma":"/nix/store/gamma","delta":"/nix/store/delta"}' \
    "alpha beta gamma delta" "" "$good"
  # A name that is a substring of an expected one must not count as it (the jq `inside` trap).
  run_case "substring name, rc 0" 1 "missing: gamma; unexpected: amm" 0 0 \
    '{"alpha":"/nix/store/alpha","beta":"/nix/store/beta","amm":"/nix/store/amm"}' "alpha beta amm" "" "$good"
  run_case "eval fails, rc 0" 1 "could not evaluate .#checks.x86_64-linux (nix eval exited 1)" 0 1 "" "" "" "$good"
  run_case "eval prints nothing, rc 0" 1 "missing: alpha beta gamma; unexpected: none" 0 0 "" "" "" "$good"
  run_case "eval not an object, rc 0" 1 "could not evaluate" 0 0 '[]' "" "" "$good"
  run_case "red row on a green run" 1 "summary counted 1 check(s) as failed or not built: gamma" 0 0 \
    "$all" "alpha beta" "" "$good"
  run_case "a check failed" 1 "summary: 2 passed (0 of them from a binary cache), 1 failed" 1 0 \
    "$all" "alpha beta" "" "$good"
  run_case "nix failed, every row green" 1 "" 1 0 "$all" "alpha beta gamma" "" "$good"
  run_case "check step never ran" 1 "" "" 0 "$all" "alpha beta gamma" "" "$good"
  run_case "check status not a number" 1 "CHECK_RC 'abc' is not an exit status" abc 0 "$all" "alpha beta gamma" "" \
    "$good"
  run_case "count disagrees" 1 "lists 3 check name(s) but says count: 4" 0 0 "$all" "alpha beta gamma" "" \
    "count: 4
alpha
beta
gamma"
  run_case "empty list" 1 "the count must be a positive integer" 0 0 '{}' "" "" "count: 0"
  run_case "no count line" 1 "must have exactly one 'count:' line" 0 0 "$all" "alpha beta gamma" "" \
    "alpha
beta
gamma"
  run_case "duplicate name" 1 "duplicate check name(s): beta" 0 0 "$all" "alpha beta gamma" "" "count: 3
alpha
beta
beta"
  # The exact number of cases, so a case deleted by accident fails here rather than shrinking the test.
  if [ "$ran" -ne 18 ]; then
    echo "self-test: ran $ran cases, expected 18" >&2
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
  echo "usage: $0 SYSTEM CHECK_RC | --self-test" >&2
  exit 2
fi
: "${GITHUB_STEP_SUMMARY:?GITHUB_STEP_SUMMARY must name the step summary file}"
summarise "$1" "$2"
