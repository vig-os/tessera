"""fd5.quality — description quality validation heuristics.

Checks that fd5 files meet description standards for AI-readability
and FAIR compliance. See white-paper.md § AI-Retrievable (FAIR for AI).
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

import h5py

_MIN_LENGTH = 20

_PLACEHOLDER_PATTERNS: list[re.Pattern[str]] = [
    re.compile(r"\btodo\b", re.IGNORECASE),
    re.compile(r"\btbd\b", re.IGNORECASE),
    re.compile(r"\bfixme\b", re.IGNORECASE),
    re.compile(r"\bplaceholder\b", re.IGNORECASE),
    re.compile(r"\bxxx\b", re.IGNORECASE),
    re.compile(r"^description\b", re.IGNORECASE),
]


@dataclass(frozen=True)
class Warning:
    """A quality warning about a description attribute."""

    path: str
    message: str
    severity: str  # "error" | "warning"


def check_descriptions_file(f: h5py.File) -> list[str]:
    """Check description quality on an open HDF5 file. Returns list of warning messages."""
    warns: list[Warning] = []
    _check_root(f, warns)
    seen: dict[str, str] = {}
    _walk(f, "/", warns, seen)
    return [f"{w.path}: {w.message}" for w in warns]


def check_descriptions(path: Path) -> list[Warning]:
    """Validate description attributes in an fd5 HDF5 file.

    Returns a list of Warning objects. An empty list means all checks pass.
    """
    warnings: list[Warning] = []
    with h5py.File(path, "r") as f:
        _check_root(f, warnings)
        seen: dict[str, str] = {}
        _walk(f, "/", warnings, seen)
    return warnings


def _check_root(f: h5py.File, warnings: list[Warning]) -> None:
    desc = f.attrs.get("description")
    if desc is None:
        warnings.append(
            Warning(
                path="/", message="Missing root description attribute", severity="error"
            )
        )
        return
    desc_str = desc.decode("utf-8") if isinstance(desc, bytes) else str(desc)
    if not desc_str.strip():
        warnings.append(
            Warning(
                path="/", message="Empty root description attribute", severity="error"
            )
        )


def _walk(
    group: h5py.Group,
    prefix: str,
    warnings: list[Warning],
    seen: dict[str, str],
) -> None:
    for key in sorted(group.keys()):
        try:
            item = group[key]
        except (KeyError, OSError):
            continue  # skip external links or broken references
        full_path = f"{prefix}{key}" if prefix == "/" else f"{prefix}/{key}"
        desc = item.attrs.get("description")

        if desc is None:
            warnings.append(
                Warning(
                    path=full_path,
                    message=f"Missing description attribute on {_kind(item)}",
                    severity="error",
                )
            )
        else:
            desc_str = desc.decode("utf-8") if isinstance(desc, bytes) else str(desc)
            _check_quality(full_path, desc_str, warnings, seen)

        if isinstance(item, h5py.Group):
            _walk(item, full_path, warnings, seen)


def _check_quality(
    path: str,
    desc: str,
    warnings: list[Warning],
    seen: dict[str, str],
) -> None:
    if len(desc) < _MIN_LENGTH:
        warnings.append(
            Warning(
                path=path,
                message=f"Short description ({len(desc)} chars, minimum {_MIN_LENGTH})",
                severity="warning",
            )
        )
        return

    for pattern in _PLACEHOLDER_PATTERNS:
        if pattern.search(desc):
            warnings.append(
                Warning(
                    path=path,
                    message="Placeholder text detected in description",
                    severity="warning",
                )
            )
            return

    if desc in seen:
        warnings.append(
            Warning(
                path=path,
                message=f"Duplicate description (same as {seen[desc]})",
                severity="warning",
            )
        )
    else:
        seen[desc] = path


def _kind(item: h5py.HLObject) -> str:
    if isinstance(item, h5py.Dataset):
        return "dataset"
    return "group"
