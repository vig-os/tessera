"""#828 M4b measurement seam, applied to the job's source copy of tessera-io's table.rs only (never committed).

Makes the three swept parameters of tessera-table-v2 readable from the environment, each defaulting to the
committed profile constant, so with no variable set the build writes exactly the #828 profile:

- T828_PCO_LEVEL   Pco compression level            (default TABLE_V2_PCO_LEVEL = 8)
- T828_PAGE        Pco values per page              (default TABLE_V2_PCO_VALUES_PER_PAGE = 8192)
- T828_CHUNK_ROWS  rows per numeric chunk / Zstd frame (default ROWS_PER_GROUP = 65536)

Usage: python3 m4b_seam.py <path/to/table.rs>
"""

import sys

path = sys.argv[1]
src = open(path, encoding="utf-8").read()


def replace_once(old: str, new: str) -> None:
    global src
    if src.count(old) != 1:
        sys.exit(f"m4b_seam: expected exactly one {old!r}, found {src.count(old)}")
    src = src.replace(old, new)


replace_once(
    "                    TABLE_V2_PCO_LEVEL,\n                    TABLE_V2_PCO_VALUES_PER_PAGE,\n",
    '                    m4b_env("T828_PCO_LEVEL", TABLE_V2_PCO_LEVEL),\n'
    '                    m4b_env("T828_PAGE", TABLE_V2_PCO_VALUES_PER_PAGE),\n',
)
replace_once(
    "            block_len_multiple: ROWS_PER_GROUP,\n",
    '            block_len_multiple: m4b_env("T828_CHUNK_ROWS", ROWS_PER_GROUP),\n',
)
src += """
/// #828 M4b measurement seam (ksb job source copy only): a swept parameter from the environment.
fn m4b_env(name: &str, default: usize) -> usize {
    std::env::var(name).map_or(default, |v| v.parse().expect(name))
}
"""
open(path, "w", encoding="utf-8").write(src)
sys.stdout.write("m4b_seam: applied\n")
