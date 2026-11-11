#!/usr/bin/env bash
# The `capi-smoke` flake check (flake.nix): the C ABI from a C++ producer's side.
#
#   capi-smoke.sh <tessera-capi source dir> <tessera binary> <readback binary>
#
# Expects cmake, pkg-config, cbindgen and a C++ compiler on PATH, and tessera-capi's and
# nanoarrow's install prefixes on PKG_CONFIG_PATH / CMAKE_PREFIX_PATH (the nix setup hooks do both).
# Runs in the current directory, which it fills with build trees and products.
set -euo pipefail

src=$1
tessera=$2
readback=$3

# 1. The committed header is what cbindgen generates from the source.
cbindgen --quiet --config "$src/cbindgen.toml" --output generated.h "$src/src/lib.rs"
if ! diff -u "$src/include/tessera.h" generated.h; then
  echo "include/tessera.h is stale: regenerate it (see cbindgen.toml)" >&2
  exit 1
fi

# 2. The C++ programs build with CMake against the installed header, pkg-config file and nanoarrow
#    package, once linking the shared library and once the static archive.
for link in shared static; do
  cmake -S "$src/cpp" -B "build-$link" -DTESSERA_LINK=$link -DCMAKE_BUILD_TYPE=Release
  cmake --build "build-$link" -j "${NIX_BUILD_CORES:-2}"
done

# 3. The static build must not need the shared library at run time: no NEEDED entry names it.
#    readelf's output goes to a file first, so a failing readelf can't hide behind the grep.
readelf -d build-static/capi_smoke > static.dynamic
if grep -q 'NEEDED.*libtessera_capi' static.dynamic; then
  echo "capi_smoke (static) still needs libtessera_capi.so" >&2
  exit 1
fi
grep -q 'NEEDED.*libtessera_capi' <(readelf -d build-shared/capi_smoke) || {
  echo "capi_smoke (shared) does not need libtessera_capi.so: the shared link is not what was built" >&2
  exit 1
}

# 4. Each variant writes a product that `tessera verify` accepts and that reads back, value for
#    value, as C++ pushed it, with a known and an unknown row count.
for link in shared static; do
  for rows in known unknown; do
    run=$link-$rows
    mkdir -p "dumps-$run"
    flag=()
    [ "$rows" = unknown ] && flag=(--unknown-rows)
    "build-$link/capi_smoke" "stage-$run" "$run.tsra" "dumps-$run" 200000 9973 "${flag[@]}" > "$run.log"
    cat "$run.log"
    "$tessera" verify "$run.tsra"
    "$readback" "$run.tsra" "dumps-$run"
  done
done

# 5. Every variant sealed the same product.
hashes=$(grep -h '^content_hash' ./*.log | sort -u | wc -l)
if [ "$hashes" -ne 1 ]; then
  echo "the 4 variants sealed $hashes different products" >&2
  exit 1
fi

# 6. #802: one hash across producer threads; arrival order moves it; claimed keys back-pressure;
#    a stall is an error.
mkdir threads
build-shared/capi_threads threads
