#!/usr/bin/env bash
# Run ONE crate's tests at ONE commit as a ksb batch job, off the seat (#957).
#
#   scripts/ksb-test.sh [OPTIONS] <rev> <crate> [FILTERSET]
#   scripts/ksb-test.sh --self-test
#
#   <rev>        any commit in this clone, pushed or not (a branch, a tag, a SHA; resolved to the SHA here)
#   <crate>      a workspace member, e.g. tessera-io (tessera/crates/<crate>/Cargo.toml at <rev>)
#   [FILTERSET]  a nextest filterset, passed as `-E`, e.g. 'test(roundtrip)' or 'binary(cli) & test(=sign)'
#
#   --bins | --lib | --tests | --bin NAME | --test NAME | --features LIST | --no-default-features
#                cargo target/feature selection, passed to `cargo nextest run -p <crate>` as given
#   --threads N  test threads (NEXTEST_TEST_THREADS, RUST_TEST_THREADS, RAYON_NUM_THREADS); default 4
#   --timeout M  minutes the test run may take before it is killed (recorded as exit 124); default 15. The run
#                happens inside ksb's preparation, which ksb bounds itself (its input copy timed out at 1800 s), so
#                build + run beyond that ends as exit 3 instead of 124
#   --rerun      run the tests again although this exact request already has a result (a new job)
#   --name NAME  ksb job name; default tsra-test-<crate>-<sha8>-<digest of the generated job>. A name ksb already
#                knows is reported (or waited on), never resubmitted
#   --dry-run    write the job flake and print the `ksb run` command, submit nothing
#
# HOW IT WORKS. The tests run in crane derivations of the repo's own flake (`legacyPackages.ksbTest`, flake.nix),
# never in the job pod (a pod cargo build is SIGTERMed): one derivation builds the crate's test binaries on top of
# the shared dependency build, a second runs them under nextest and records the exit code, the JUnit report, the
# log and the failing tests' output instead of failing. The job's app copies those files to $KSB_JOB_OUT and exits
# with the recorded code. This script writes a small job flake pinning the commit (a `git+file` input, so ksb
# snapshots it on the submit side and an unpushed commit works) and every git dependency in its Cargo.lock (ksb
# evaluates offline and cannot `builtins.fetchGit`), submits it, waits, and prints the result.
#
# CACHING. Both derivations are content-addressed by their inputs, so the same request on the same commit returns
# the stored result without running anything (a deterministic failure stays failed), and the build derivation is
# shared by every filter and thread count on that commit. --rerun forces a new test run (for a suspected flake); it
# does not rebuild.
#
# EXIT: the test run's own exit code (0 pass; nextest: 100 a test failed, 4 no test matched the filterset; 124 the
# --timeout expired, 137 it expired and the run ignored SIGTERM), 2 a usage error, 3 the job produced no test result
# (an evaluation or compile error, or ksb's own failure such as a copy timeout: its reason and the end of the ksb log
# are printed). Any other code is the job pod's own (ksb killed or evicted the reporting app). Results stay in $KSB_TEST_HOME/<name>/out (default ~/.cache/tessera-ksb-test).
#
# GAPS (CONTRIBUTING.md, "Known gaps"): tests needing private data (the DUPLET fixtures are not a ksb dataset,
# `dataset_refused`) or hardware counters (the job seccomp has no perf_event_open) cannot run here, and the test
# derivation runs on the ksb builder, which bounds the threads but not the memory (#961).
set -euo pipefail

KSB=${KSB:-ksb}
NIX=${NIX:-nix}
KSB_TEST_HOME=${KSB_TEST_HOME:-${XDG_CACHE_HOME:-$HOME/.cache}/tessera-ksb-test}
WAIT_SLICE=600 # seconds per `ksb wait`; the loop goes on until the job is terminal

die() { echo "ksb-test: $*" >&2; exit 2; }

# Temporary directories, removed when the script exits however it exits.
tmp_dirs=()
trap 'rm -rf "${tmp_dirs[@]}"' EXIT
new_tmp() { tmp_dirs+=("$(mktemp -d)"); }

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; }

# The git dependencies of a Cargo.lock (on stdin), one `source` string per line, exactly as the lock spells them.
lock_git_sources() {
  sed -n 's/^source = "\(git+[^"]*\)"$/\1/p' | sort -u
}

# The flake reference that fetches one Cargo.lock git source at its LOCKED revision (the part after `#`).
# github.com uses the tarball fetcher (no clone); any other host is fetched as git.
git_source_flake_ref() {
  local source=$1 url rev
  url=${source#git+}
  rev=${url##*#}
  [[ $url == *"#"* && $rev =~ ^[0-9a-f]{40}$ ]] || { echo "unlocked git source: $source" >&2; return 1; }
  url=${url%%#*}
  url=${url%%\?*}
  if [[ $url =~ ^https://github\.com/([^/]+)/([^/]+)$ ]]; then
    echo "github:${BASH_REMATCH[1]}/${BASH_REMATCH[2]%.git}/$rev"
  else
    echo "git+$url?rev=$rev"
  fi
}

# The clone's object store as a git+file URL: a worktree's commits live in the common git dir.
repo_url() {
  local common
  common=$(git rev-parse --path-format=absolute --git-common-dir)
  [[ $(basename "$common") == .git ]] && common=$(dirname "$common")
  echo "git+file://$common"
}

# Write the job flake (flake.nix + params.json) into $1. Reads the globals set by main.
write_job() {
  local dir=$1 i=0 source ref inputs="" sources_json="{}"
  mkdir -p "$dir"
  while IFS= read -r source; do
    [[ -n $source ]] || continue
    ref=$(git_source_flake_ref "$source") || return 1
    inputs+="    git$i = { url = \"$ref\"; flake = false; };"$'\n'
    sources_json=$(jq -c --arg s "$source" --arg n "git$i" '. + {($s): $n}' <<< "$sources_json")
    i=$((i + 1))
  done <<< "$git_sources"
  jq -n --arg crate "$crate" --argjson cargoArgs "$(printf '%s\n' "${cargo_args[@]}" | jq -R . | jq -sc 'map(select(. != ""))')" \
    --arg filter "$filter" --argjson testThreads "$threads" --argjson timeoutMinutes "$timeout" \
    --arg rerun "$rerun" --argjson gitSources "$sources_json" \
    '{crate: $crate, cargoArgs: $cargoArgs, filter: (if $filter == "" then null else $filter end),
      testThreads: $testThreads, timeoutMinutes: $timeoutMinutes, rerun: (if $rerun == "" then null else $rerun end),
      gitSources: $gitSources}' > "$dir/params.json"
  # ksb copies EVERY locked input to the compute VM before it evaluates, and the dev-shell inputs (vigos: devkit's
  # vig-utils and its uv/gh/claude-code from a second nixpkgs, with home-manager twice; guardrails: the gate
  # toolbelt) are ~300 MB the Rust build never reads. Copying them timed the first job out (`nix copy` > 1800 s),
  # so they follow an empty stub. vigos's overlay is applied to `pkgs`, so the stub has an identity overlay; a
  # ksbTest that came to need either input would fail evaluation on the stub, not build something else. The stub
  # is an ABSOLUTE path input: ksb refuses a relative one ("invalid archived input metadata").
  local stub=$KSB_TEST_HOME/devshell-stub
  mkdir -p "$stub"
  # Shared by concurrent invocations: written whole and renamed into place, so a lock never reads half a file.
  cat > "$stub/flake.nix.$$" <<'EOF'
# Generated by scripts/ksb-test.sh: stands in for tessera's dev-shell-only inputs (vigos, guardrails).
{ outputs = _: { overlays.default = _final: _prev: { }; }; }
EOF
  mv -f "$stub/flake.nix.$$" "$stub/flake.nix"
  cat > "$dir/flake.nix" <<EOF
# Generated by scripts/ksb-test.sh (#957): tessera $sha, $crate. Parameters: params.json.
{
  inputs = {
    tessera.url = "$(repo_url)?rev=$sha";
    devshell-stub.url = "path:$stub";
    tessera.inputs.vigos.follows = "devshell-stub";
    tessera.inputs.guardrails.follows = "devshell-stub";
$inputs  };
  outputs = { tessera, ... }@inputs:
    let
      params = builtins.fromJSON (builtins.readFile ./params.json);
      job = tessera.legacyPackages.x86_64-linux.ksbTest (params // {
        gitSources = builtins.mapAttrs (_: name: inputs.\${name}) params.gitSources;
      });
    in
    {
      apps.x86_64-linux.default = { type = "app"; program = "\${job}/bin/tessera-ksb-test"; };
    };
}
EOF
}

# The digest in the job name: every generated file, so a changed job never reuses an old ksb record (ksb returns the
# existing record for a known name).
job_digest() {
  (cd "$1" && find . -type f -print0 | sort -z | xargs -0 sha256sum) | sha256sum | cut -c1-6
}

# Print the job's result files and return its exit code. $1 = job name, $2 = local result dir.
report() {
  local name=$1 out=$2 status reason code f
  status=$("$KSB" status "$name" --json) || { echo "ksb-test: ksb status $name failed" >&2; return 3; }
  reason=$(jq -r '.reason // ""' <<< "$status")
  code=$(jq -r '.exit_code // "null"' <<< "$status")
  [[ $code == null || $code =~ ^[0-9]+$ ]] || { echo "ksb-test: unreadable status for $name: $status" >&2; return 3; }
  if [[ $code == null || $reason == preparation_failed ]]; then
    echo "ksb-test: job $name produced no test result ($reason)" >&2
    # ksb's own failure (a copy or evaluation timeout) is in next_action, a build failure in the job log.
    jq -r '.next_action // empty' <<< "$status" | cut -c1-2000 >&2
    echo "ksb-test: the end of the job log:" >&2
    "$KSB" logs "$name" | tail -n 80 >&2
    echo "ksb-test: a new commit gets a new job; for an infrastructure failure on this one, --rerun submits again" >&2
    return 3
  fi
  mkdir -p "$out"
  for f in summary.txt failures.txt; do
    "$KSB" out "$name" --path "$f" --dest "$out/$f" > /dev/null 2>&1 || true
  done
  if [[ -f $out/summary.txt ]]; then cat "$out/summary.txt"; else echo "ksb-test: job $name wrote no summary.txt" >&2; fi
  if [[ $code != 0 ]]; then
    if [[ -f $out/failures.txt ]]; then cat "$out/failures.txt"
    else echo "ksb-test: failures.txt not fetched (over ksb out's 1 MiB?); see $(jq -r .output <<< "$status")" >&2
    fi
  fi
  echo "ksb-test: $name exit $code; results: $out (more: ksb out $name --path junit.xml|nextest.log --dest FILE)"
  return "$code"
}

main() {
  local rev="" positional=() name="" dry_run=0 t0
  crate="" filter="" threads=4 timeout=15 rerun="" cargo_args=()
  while (($#)); do
    case $1 in
      --self-test) self_test; return ;;
      -h | --help) usage; return 0 ;;
      --bins | --lib | --tests | --no-default-features) cargo_args+=("$1"); shift ;;
      --bin | --test | --features)
        (($# >= 2)) || die "$1 needs a value"
        cargo_args+=("$1" "$2"); shift 2 ;;
      --threads) [[ ${2:-} =~ ^[1-9][0-9]*$ ]] || die "--threads needs a positive integer"; threads=$2; shift 2 ;;
      --timeout) [[ ${2:-} =~ ^[1-9][0-9]*$ ]] || die "--timeout needs minutes"; timeout=$2; shift 2 ;;
      --rerun) rerun=$(date -u +%Y%m%dT%H%M%S%N); shift ;;
      --name) [[ -n ${2:-} ]] || die "--name needs a value"; name=$2; shift 2 ;;
      --dry-run) dry_run=1; shift ;;
      -*) die "unknown option $1 (--help)" ;;
      *) positional+=("$1"); shift ;;
    esac
  done
  ((${#positional[@]} >= 2 && ${#positional[@]} <= 3)) || { usage >&2; exit 2; }
  rev=${positional[0]} crate=${positional[1]} filter=${positional[2]:-}
  sha=$(git rev-parse --verify --quiet "$rev^{commit}") || die "$rev is not a commit in this clone"
  if ! [[ $crate =~ ^[a-z0-9_-]+$ ]] || ! git cat-file -e "$sha:tessera/crates/$crate/Cargo.toml" 2> /dev/null; then
    die "$crate is not a crate at ${sha:0:12} (no tessera/crates/$crate/Cargo.toml)"
  fi
  git_sources=$(git show "$sha:tessera/Cargo.lock" | lock_git_sources)

  local scratch digest dir
  new_tmp
  scratch=${tmp_dirs[-1]}
  write_job "$scratch"
  digest=$(job_digest "$scratch")
  name=${name:-tsra-test-${crate}-${sha:0:8}-$digest}
  # $dir/job is the submitted flake and nothing else (ksb snapshots the whole directory, so results kept there
  # would make a second submission of the same name a "different submission"); results go to $dir/out.
  dir=$KSB_TEST_HOME/$name
  mkdir -p "$dir/job"
  cp -r "$scratch/." "$dir/job/"
  (cd "$dir/job" && "$NIX" --extra-experimental-features 'nix-command flakes' flake lock 2>&1 | tail -n 3 >&2) \
    || die "nix flake lock failed in $dir/job"
  local cmd=("$KSB" run --json --name "$name" --cpu 1 --mem 1Gi --time 10 "path:$dir/job#default")
  if ((dry_run)); then printf '%q ' "${cmd[@]}"; echo; return 0; fi

  t0=$(date +%s)
  # A name ksb already knows is this exact job (the name carries its digest): report or wait on that record.
  if "$KSB" status "$name" --json > /dev/null 2>&1; then
    echo "ksb-test: $name already exists (the same request); its result, without a new run (--rerun runs again)" >&2
  else
    "${cmd[@]}" > "$dir/submit.json" || die "ksb run failed (see $dir/submit.json)"
    echo "ksb-test: submitted $name (${sha:0:12} $crate ${cargo_args[*]} ${filter:+-E $filter}); waiting" >&2
  fi
  local rc
  while :; do
    rc=0
    "$KSB" wait "$name" --timeout "$WAIT_SLICE" --json > /dev/null || rc=$?
    case $rc in
      0 | 1) break ;;
      124) echo "ksb-test: $name still running after $(($(date +%s) - t0)) s" >&2 ;;
      *) echo "ksb-test: ksb wait $name exited $rc" >&2; return 3 ;;
    esac
  done
  echo "ksb-test: wall time $(($(date +%s) - t0)) s (submit to terminal)" >&2
  rc=0
  report "$name" "$dir/out" || rc=$?
  return "$rc"
}

# The wrapper's own test (pre-commit hook `ksb-test-self-test`): the lock parsing, the job flake, the request
# digest, and the exit code and output of `report` for every job outcome, against a stub `ksb`. A real ksb run is
# what the PR's evidence shows; this keeps the parts that decide what an agent sees from regressing unseen.
self_test() {
  local t status=0 got want
  new_tmp
  t=${tmp_dirs[-1]}
  KSB_TEST_HOME=$t/home
  fail() { echo "self-test: $*" >&2; status=1; }

  # 1. lock parsing: only git sources, deduplicated, verbatim.
  got=$(printf '%s\n' 'source = "registry+https://github.com/rust-lang/crates.io-index"' \
    'source = "git+https://github.com/gerchowl/vortex?rev=5c55cf7fa35bac652849cf1a0e76a1a97035f715#5c55cf7fa35bac652849cf1a0e76a1a97035f715"' \
    'source = "git+https://github.com/gerchowl/vortex?rev=5c55cf7fa35bac652849cf1a0e76a1a97035f715#5c55cf7fa35bac652849cf1a0e76a1a97035f715"' \
    'source = "git+https://example.org/x/y.git?branch=main#0123456789abcdef0123456789abcdef01234567"' | lock_git_sources)
  [[ $(wc -l <<< "$got") == 2 ]] || fail "lock_git_sources returned [$got]"
  # 2. flake refs pin the LOCKED revision.
  got=$(git_source_flake_ref "git+https://github.com/gerchowl/vortex?rev=5c55cf7fa35bac652849cf1a0e76a1a97035f715#5c55cf7fa35bac652849cf1a0e76a1a97035f715")
  [[ $got == github:gerchowl/vortex/5c55cf7fa35bac652849cf1a0e76a1a97035f715 ]] || fail "github ref is $got"
  got=$(git_source_flake_ref "git+https://example.org/x/y.git?branch=main#0123456789abcdef0123456789abcdef01234567")
  [[ $got == "git+https://example.org/x/y.git?rev=0123456789abcdef0123456789abcdef01234567" ]] || fail "git ref is $got"
  git_source_flake_ref "git+https://github.com/a/b?branch=main" 2> /dev/null && fail "accepted an unlocked git source"

  # 3. the job flake: every git source mapped to an input, the parameters as JSON, the commit pinned.
  sha=0123456789abcdef0123456789abcdef01234567 crate=tessera-io filter='test(a) & !test(b)' threads=4 timeout=25
  rerun="" cargo_args=(--features cloud)
  git_sources=$(printf '%s\n' "git+https://github.com/gerchowl/vortex?rev=$sha#$sha")
  write_job "$t/job"
  [[ $(jq -r '.gitSources | to_entries[0] | "\(.key)=\(.value)"' "$t/job/params.json") == "git+https://github.com/gerchowl/vortex?rev=$sha#$sha=git0" ]] \
    || fail "params.json gitSources: $(jq -c .gitSources "$t/job/params.json")"
  [[ $(jq -c '[.crate, .cargoArgs, .filter, .testThreads, .timeoutMinutes, .rerun]' "$t/job/params.json") \
    == '["tessera-io",["--features","cloud"],"test(a) & !test(b)",4,25,null]' ]] || fail "params.json: $(cat "$t/job/params.json")"
  grep -qF "git0 = { url = \"github:gerchowl/vortex/$sha\"; flake = false; };" "$t/job/flake.nix" || fail "no git0 input"
  grep -qF "?rev=$sha\";" "$t/job/flake.nix" || fail "flake.nix does not pin the commit"
  grep -qF 'tessera.inputs.vigos.follows = "devshell-stub";' "$t/job/flake.nix" || fail "vigos is not stubbed"
  grep -qF 'tessera.inputs.guardrails.follows = "devshell-stub";' "$t/job/flake.nix" || fail "guardrails is not stubbed"
  grep -qF "devshell-stub.url = \"path:$KSB_TEST_HOME/devshell-stub\";" "$t/job/flake.nix" || fail "stub is not absolute"
  grep -qF 'overlays.default = _final: _prev: { };' "$KSB_TEST_HOME/devshell-stub/flake.nix" || fail "no identity-overlay stub"
  # 4. the request digest: stable for the same request, new for --rerun.
  local d1 d2
  d1=$(job_digest "$t/job"); write_job "$t/job"; d2=$(job_digest "$t/job")
  [[ $d1 == "$d2" ]] || fail "the same request gave two digests"
  rerun=20261122T000000; write_job "$t/job"
  [[ $d1 != "$(job_digest "$t/job")" ]] || fail "--rerun did not change the digest"
  rerun=""; write_job "$t/job"; echo "# edited" >> "$t/job/flake.nix"
  [[ $d1 != "$(job_digest "$t/job")" ]] || fail "a changed job flake kept the digest"

  # 5. report: the job's exit code is the wrapper's, and a failure prints the failing test's output.
  cat > "$t/ksb" <<'STUB'
#!/usr/bin/env bash
# A stub ksb: the scenario is the job name; status/out/logs answer from canned data.
cmd=$1 name=$2; shift 2
case $name in pass) code=0 reason=succeeded ;; fail) code=100 reason=failed ;; prep) code=null reason=preparation_failed ;; esac
case $cmd in
  status) echo "{\"reason\": \"$reason\", \"exit_code\": $code, \"output\": \"/data/hdd/batch/$name/out\"$([[ $name == prep ]] && echo ', "next_action": "nix copy timed out after 1800 seconds"')}" ;;
  logs) echo "error: could not compile \`tessera-io\` (lib) due to 1 previous error" ;;
  out)
    while (($#)); do case $1 in --path) path=$2; shift 2 ;; --dest) dest=$2; shift 2 ;; *) shift ;; esac; done
    case $path in
      summary.txt) echo "     Summary [   1.000s] 3 tests run: $([[ $name == pass ]] && echo '3 passed' || echo '2 passed, 1 failed'), 0 skipped" > "$dest" ;;
      failures.txt) [[ $name == fail ]] && echo "=== FAIL tessera-io::roundtrip deliberately_fails: assertion failed: 1 == 2" > "$dest" ;;
    esac ;;
esac
STUB
  chmod +x "$t/ksb"
  local saved=$KSB
  KSB=$t/ksb
  local scenario
  for scenario in pass:0 fail:100 prep:3; do
    want=${scenario#*:} scenario=${scenario%%:*} got=0
    report "$scenario" "$t/out-$scenario" > "$t/$scenario.stdout" 2> "$t/$scenario.stderr" || got=$?
    [[ $got == "$want" ]] || fail "report $scenario: exit $got, expected $want"
  done
  grep -q 'assertion failed: 1 == 2' "$t/out-fail/failures.txt" 2> /dev/null || fail "failure output not fetched"
  grep -q 'deliberately_fails: assertion failed' "$t/fail.stdout" || fail "report did not print the failing test's output"
  grep -q '1 failed' "$t/fail.stdout" || fail "report did not print the summary of a failed run"
  grep -q '3 passed' "$t/pass.stdout" || fail "report did not print the summary of a passing run"
  grep -q 'deliberately_fails' "$t/pass.stdout" && fail "report printed failures for a pass"
  grep -q 'could not compile' "$t/prep.stderr" || fail "report did not print the build log"
  grep -q 'nix copy timed out' "$t/prep.stderr" || fail "report did not print ksb's own failure reason"
  KSB=$saved
  ((status == 0)) && echo "self-test: lock parsing, job flake, digest and 3 report outcomes OK"
  return "$status"
}

# `exit` on the same line: bash reads a script as it runs, and agents edit their checkout while a wrapper waits;
# without it, bash would go on to execute whatever text the edited file has past this point once main returns.
main "$@"; exit
