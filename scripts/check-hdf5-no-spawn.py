#!/usr/bin/env python3
"""Reject process-launch APIs in crates whose unit tests share HDF5 handles.

Scan all src helpers too: cfg(test) block matching would miss shared functions.
CLI tests/ is exempt: those binaries launch the CLI but do not open HDF5 handles
in the parent. Ingest tests/ is scanned because it can open HDF5 in the parent.
Production source is deliberately covered as it is linked into unit tests.
This is a conservative source ratchet, not a Rust name resolver: new launch
wrappers or generated/external test helpers require review of this policy.
"""

import re
import sys
from pathlib import Path

CRATES = ("tessera-ingest", "tessera-cli")
# Catch imports (including aliases and grouped imports) before a launcher can
# be renamed. Thread/task spawn is safe and is deliberately not banned.
FORBIDDEN = re.compile(
    r"\bCommand\b|\b(?:subprocess|duct|command_fds)\b|\bprocess\s*::\s*\*|"
    r"(?<![.\w])(?:fork|vfork|posix_spawnp?|popen|exec[lv](?:p|e|pe)?|system)\s*\("
    r"|\b(?:libc|nix)\s*::\s*(?:unistd|spawn)\b"
    r"|\buse\b[^;]*\b(?:fork|vfork|posix_spawnp?|popen|system|exec[lv](?:p|e|pe)?)\b"
    r"|\.\s*exec\s*\("
)
# Mask comments and literals so explanatory docs and fixture strings are safe.
LEXEMES = re.compile(
    r'''//[^\n]*|/\*|'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'|r(\#*)"|"(?:\\.|[^"\\])*"''',
    re.DOTALL,
)


def code_only(source):
    """Preserve line positions while masking Rust comments and strings."""
    result = list(source)
    end = 0
    for match in LEXEMES.finditer(source):
        if match.start() < end:
            continue
        end = match.end()
        if match.group() == "/*":
            depth = 1
            while depth and end < len(source):
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
        elif match.group().startswith("r"):
            close = '"' + match.group(1)
            pos = source.find(close, end)
            end = len(source) if pos < 0 else pos + len(close)
        for i in range(match.start(), end):
            if result[i] != "\n":
                result[i] = " "
    return "".join(result)


def check(root):
    failed = False
    for crate in CRATES:
        base = root / "tessera/crates" / crate
        if not (base / "Cargo.toml").is_file() or not (base / "src").is_dir():
            sys.stderr.write(f"missing guarded crate: {crate}\n")
            failed = True
            continue
        files = sorted((base / "src").rglob("*.rs"))
        if crate == "tessera-ingest":
            files += sorted((base / "tests").rglob("*.rs"))
        for path in files:
            source = code_only(path.read_text())
            for match in FORBIDDEN.finditer(source):
                line = source.count("\n", 0, match.start()) + 1
                sys.stderr.write(
                    f"{path}:{line}: process launch API in HDF5 test binary: "
                    f"{match.group()}\n"
                )
                failed = True
    return int(failed)


if __name__ == "__main__":
    sys.exit(check(Path(sys.argv[1]) if len(sys.argv) > 1 else Path(".")))
