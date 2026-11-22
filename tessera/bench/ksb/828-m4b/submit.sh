#!/usr/bin/env bash
# Assemble the #828 round-2 ksb job, submit it, and stage the fixture into its results volume.
#
#   FIXTURE=<dir with ge/ and siemens/ (*.npy.zst, events.json, NPY_DECOMPRESSED.sha256)> \
#     ./submit.sh <job-name> [probe|full]
#
# The fixture is private (de-identified #822 data): it never enters the repository or the nix store. It is copied
# into the job's own output directory (host side of the job's /job/out) after submission, ending with a READY
# marker; the job waits for it, verifies it, decompresses it to local scratch and deletes the staged copy. The
# harness is tessera/crates/tessera-io/examples/table_profile.rs with harness-cpu-perf.patch applied.
set -euo pipefail
name=$1
mode=${2:-full}
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd -- "$here/../../../.." && pwd)
job=${JOB_DIR:-/dev/shm/t828/job-$name}
batch_root=${KSB_BATCH_ROOT:-/mnt/HDD/larsgerchow/compute/hdd/batch}
: "${FIXTURE:?set FIXTURE to the fixture directory}"
mkdir -p "$job"
chmod 700 "$job"
cp "$here/flake.nix" "$here/run.py" "$here/m4b_seam.py" "$job/"
# The harness: the committed example plus this directory's measurement patch (CPU time + perf control).
git -C "$repo" show HEAD:tessera/crates/tessera-io/examples/table_profile.rs > "$job/table_profile.rs"
patch --quiet "$job/table_profile.rs" < "$here/harness-cpu-perf.patch"
(cd "$job" && nix flake lock)
ksb run --json --name "$name" --cpu 4 --exclusive-cpus --mem 16Gi --time 180 "path:$job#default" -- "$mode"
out="$batch_root/$name/out"
test -d "$out"
mkdir -p "$out/fixture"
cp -r "$FIXTURE/ge" "$FIXTURE/siemens" "$out/fixture/"
touch "$out/fixture/READY"
echo "fixture staged at $out/fixture"
