#!/usr/bin/env python3
"""Keep the root CHANGELOG.md one section per release: split it from the tags, cut a release, check it (#982)."""

# Usage:
#   changelog.py split CHANGELOG TAG...        rebuild CHANGELOG's release sections from the file at each TAG
#   changelog.py release CHANGELOG VERSION DATE
#                                              turn `## Unreleased` into `## [VERSION] - DATE` and open a new,
#                                              empty `## Unreleased` (the release-time step, docs/RELEASING.md)
#   changelog.py check CHANGELOG CARGO_TOML    exit 1 unless CHANGELOG holds the structure below
#   changelog.py --self-test
# Exit status: 0 done / the check holds, 1 refused or a check failed (each reason on stderr), 2 a usage error.
#
# WHY (#982). cargo-dist builds the GitHub release body from this file (dist-workspace.toml, the generated
# release.yml). Its lookup (axoproject `changelog_for_version`, cargo-dist 0.32.0) takes the section titled with the
# exact version, then the version without its prerelease suffix, then, for a prerelease, `Unreleased`. Until #982
# the file only ever had `## Unreleased`, so every release's notes were all history to date.
#
# THE STRUCTURE `check` HOLDS THE FILE TO:
#   - the first section is `## Unreleased`, and it is the only one;
#   - every other section is `## [<semver>] - <YYYY-MM-DD>`, newest first: versions strictly descending by SemVer
#     precedence, dates never increasing;
#   - the workspace version (`[workspace.package] version` in CARGO_TOML) has its own section. The release PR bumps
#     that version, so it fails this check until its `Unreleased` is renamed with `release`: the next release cannot
#     ship without exactly its own section;
#   - subheads are `### <one of SUBHEADS>`, entries are `- ` bullets (continued by indented lines) under a subhead,
#     and no entry appears twice, so an entry belongs to exactly one release;
#   - a release section is not empty (entries or a note);
#   - the file ends with exactly the compare links of its releases, newest first (`link_lines`).
#
# SPLIT: a release's entries are the bullets in the file at its tag that are absent from the file at the previous
# tag, compared as whole bullets, never as lines. A bullet keeps its text and subhead from the current file, and
# `split` refuses (rather than guesses) when a bullet at a tag is not in the current file verbatim, under the same
# subhead: a later rewording has to be resolved by hand. Text before a section's first subhead (a note) is kept;
# a release with neither entries nor a note gets EMPTY_NOTE. Re-running `split` on its own output changes nothing.
import datetime
import os
import re
import subprocess
import sys
import tempfile
import tomllib

REPO_URL = "https://github.com/vig-os/tessera"
# Every subhead the file may use, in the order a section lists them. `Format break` leads a release that breaks the
# on-disk format (#917); the rest are Keep a Changelog's.
SUBHEADS = [
    "Format break",
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
]
# The subheads a fresh `## Unreleased` opens with, empty, for contributors to add to (the file's convention).
UNRELEASED_SUBHEADS = ["Added", "Changed", "Removed", "Fixed", "Security"]
EMPTY_NOTE = [
    "This file recorded no entries for this release. Its changes are listed per crate, in",
    "`tessera/crates/<crate>/CHANGELOG.md`.",
]
UNRELEASED = "Unreleased"

SEMVER = re.compile(
    r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)
RELEASE_HEADING = re.compile(
    r"## \[(?P<version>[^\]]+)\] - (?P<date>\d{4}-\d{2}-\d{2})"
)
LINK_LINE = re.compile(r"\[[^\]]+\]: \S+")


class Refused(Exception):
    """A refusal or a failed check, with its reason."""


class Section:
    def __init__(self, version, date=None):
        self.version = version  # UNRELEASED or a SemVer string
        self.date = date  # YYYY-MM-DD, None for Unreleased
        self.note = []  # lines before the first subhead
        self.groups = {}  # subhead -> [bullet text]; insertion order = file order

    def bullets(self):
        return [(sub, b) for sub, bs in self.groups.items() for b in bs]


def semver_key(version):
    """SemVer 2.0.0 §11 precedence as a sort key; Refused when VERSION is not SemVer."""
    m = SEMVER.fullmatch(version)
    if not m:
        raise Refused(f"`{version}` is not a SemVer version")
    core = tuple(int(x) for x in m.groups()[:3])
    if m.group(4) is None:
        return core + ((1,),)  # a release ranks above all of its prereleases
    ids = tuple(
        (0, int(i), "") if i.isdigit() else (1, 0, i) for i in m.group(4).split(".")
    )
    return core + ((0,) + ids,)


def parse(text, where="CHANGELOG"):
    """(header lines, [Section], [link lines]); Refused on any line outside the structure."""
    header, sections, links = [], [], []
    sec, sub, bullet = None, None, None
    for n, line in enumerate(text.split("\n"), 1):
        at = f"{where}:{n}"
        if line.startswith("## "):
            if line == f"## {UNRELEASED}":
                sec = Section(UNRELEASED)
            else:
                m = RELEASE_HEADING.fullmatch(line)
                if not m:
                    raise Refused(
                        f"{at}: `{line}` is neither `## {UNRELEASED}` nor `## [<version>] - <YYYY-MM-DD>`"
                    )
                semver_key(m["version"])
                try:
                    datetime.date.fromisoformat(m["date"])
                except ValueError:
                    raise Refused(f"{at}: `{m['date']}` is not a date") from None
                sec = Section(m["version"], m["date"])
            if links:
                raise Refused(f"{at}: a section after the links")
            sections.append(sec)
            sub, bullet = None, None
        elif sec is None:
            header.append(line)
        elif LINK_LINE.fullmatch(line):
            links.append(line)
            sub, bullet = None, None
        elif links and line:
            raise Refused(f"{at}: text after the links")
        elif line.startswith("### "):
            sub = line[4:]
            if sub not in SUBHEADS:
                raise Refused(
                    f"{at}: unknown subhead `{sub}` (one of: {', '.join(SUBHEADS)})"
                )
            if sub in sec.groups:
                raise Refused(f"{at}: subhead `{sub}` twice in one section")
            sec.groups[sub] = []
            bullet = None
        elif line.startswith("- "):
            if sub is None:
                raise Refused(f"{at}: an entry outside a subhead")
            bullet = [line]
            sec.groups[sub].append(bullet)
        elif line.startswith("  ") and bullet is not None:
            bullet.append(line)
        elif not line:
            bullet = None
        elif sub is None:
            sec.note.append(line)
        else:
            raise Refused(
                f"{at}: `{line[:60]}` is neither an entry nor its indented continuation"
            )
    while header and not header[-1]:
        header.pop()
    for s in sections:
        while s.note and not s.note[-1]:
            s.note.pop()
        for g in s.groups:
            s.groups[g] = ["\n".join(b) for b in s.groups[g]]
    return header, sections, links


def link_lines(sections):
    """The compare link of every release section, newest first; the oldest links to its tag."""
    versions = [s.version for s in sections if s.version != UNRELEASED]
    out = []
    for i, v in enumerate(versions):
        if i + 1 < len(versions):
            out.append(f"[{v}]: {REPO_URL}/compare/v{versions[i + 1]}...v{v}")
        else:
            out.append(f"[{v}]: {REPO_URL}/releases/tag/v{v}")
    return out


def render(header, sections):
    out = header + [""]
    for s in sections:
        out += [
            f"## {UNRELEASED}"
            if s.version == UNRELEASED
            else f"## [{s.version}] - {s.date}",
            "",
        ]
        if s.note:
            out += s.note + [""]
        for sub in SUBHEADS:
            bullets = s.groups.get(sub)
            if bullets is None or (not bullets and s.version != UNRELEASED):
                continue
            out += [f"### {sub}", ""]
            if bullets:
                out += bullets + [""]
    out += link_lines(sections)
    while out and not out[-1]:
        out.pop()
    return "\n".join(out) + "\n"


def git(cwd, *args):
    r = subprocess.run(["git", "-C", cwd, *args], capture_output=True, text=True)
    if r.returncode != 0:
        raise Refused(f"git {' '.join(args)}: {r.stderr.strip()}")
    return r.stdout


def first_line(bullet):
    return bullet.split("\n", 1)[0][:100]


def split(path, tags):
    """Rewrite PATH with one section per TAG (oldest first) and `Unreleased` for the rest."""
    cwd = os.path.dirname(os.path.abspath(path))
    rel = git(
        cwd, "ls-files", "--full-name", "--error-unmatch", os.path.basename(path)
    ).strip()
    with open(path, encoding="utf-8") as f:
        header, current, _ = parse(f.read(), path)
    pool, order = {}, {}
    for s in current:
        for sub, b in s.bullets():
            if b in pool:
                raise Refused(f"{path}: an entry appears twice: {first_line(b)}")
            pool[b] = sub
            order[b] = len(order)
    if not tags:
        raise Refused("split needs at least one tag")
    versions = [t[1:] if t.startswith("v") else t for t in tags]
    keys = [semver_key(v) for v in versions]
    if keys != sorted(set(keys)):
        raise Refused(f"tags must be given oldest first, each once: {' '.join(tags)}")
    known = set(versions) | {UNRELEASED}
    for s in current:
        if s.version not in known:
            raise Refused(
                f"{path}: section [{s.version}] is not one of the given tags; split would drop it"
            )

    release_of, previous, seen = {}, set(), set()
    for tag, version in zip(tags, versions):
        _, at_tag, _ = parse(git(cwd, "show", f"{tag}:{rel}"), f"{tag}:{rel}")
        here = {}
        for s in at_tag:
            for sub, b in s.bullets():
                if b not in pool:
                    raise Refused(
                        f"{tag}: an entry is not in {path} verbatim (reworded or removed since): "
                        f"{first_line(b)}"
                    )
                if pool[b] != sub:
                    raise Refused(
                        f"{tag}: an entry is under `{sub}` at the tag and `{pool[b]}` now: {first_line(b)}"
                    )
                here[b] = sub
        for b in here:
            if b in previous:
                continue
            if b in seen:
                raise Refused(
                    f"{tag}: an entry released before is back after being removed: {first_line(b)}"
                )
            release_of[b] = version
        gone = previous - set(here)
        if gone:
            raise Refused(
                f"{tag}: {len(gone)} entr{'y' if len(gone) == 1 else 'ies'} of the previous tag "
                f"removed before this one, first: {first_line(sorted(gone, key=order.get)[0])}"
            )
        seen |= set(here)
        previous = set(here)

    old = {s.version: s for s in current}
    sections = [Section(UNRELEASED)]
    for tag, version in reversed(list(zip(tags, versions))):
        sections.append(
            Section(version, git(cwd, "log", "-1", "--format=%cs", tag).strip())
        )
    by_version = {s.version: s for s in sections}
    for s in sections:
        if s.version in old:
            s.note = old[s.version].note
        if s.version == UNRELEASED:
            s.groups = {sub: [] for sub in UNRELEASED_SUBHEADS}
    for b in sorted(pool, key=order.get):
        s = by_version[release_of.get(b, UNRELEASED)]
        s.groups.setdefault(pool[b], []).append(b)
    for s in sections[1:]:
        if not s.bullets() and not s.note:
            s.note = list(EMPTY_NOTE)
    with open(path, "w", encoding="utf-8") as f:
        f.write(render(header, sections))


def release(path, version, date):
    """Rename PATH's `Unreleased` to `[VERSION] - DATE` and open a fresh, empty `Unreleased` above it."""
    try:
        datetime.date.fromisoformat(date)
    except ValueError:
        raise Refused(f"`{date}` is not a YYYY-MM-DD date") from None
    with open(path, encoding="utf-8") as f:
        header, sections, _ = parse(f.read(), path)
    if not sections or sections[0].version != UNRELEASED:
        raise Refused(f"{path}: the first section is not `## {UNRELEASED}`")
    cut = sections[0]
    if not cut.bullets():
        raise Refused(f"{path}: `## {UNRELEASED}` has no entries to release")
    newest = next((s for s in sections[1:]), None)
    if newest is not None and semver_key(version) <= semver_key(newest.version):
        raise Refused(
            f"{version} is not newer than the newest release in {path}, {newest.version}"
        )
    if newest is not None and date < newest.date:
        raise Refused(f"{date} is before {newest.version}'s date, {newest.date}")
    cut.version, cut.date = version, date
    cut.groups = {sub: bs for sub, bs in cut.groups.items() if bs}
    fresh = Section(UNRELEASED)
    fresh.groups = {sub: [] for sub in UNRELEASED_SUBHEADS}
    with open(path, "w", encoding="utf-8") as f:
        f.write(render(header, [fresh] + sections))


def workspace_version(cargo_toml):
    with open(cargo_toml, "rb") as f:
        data = tomllib.load(f)
    try:
        return data["workspace"]["package"]["version"]
    except KeyError:
        raise Refused(f"{cargo_toml}: no [workspace.package] version") from None


def check(path, cargo_toml):
    """Refused, naming every violation, unless PATH holds the structure in this file's header."""
    with open(path, encoding="utf-8") as f:
        _, sections, links = parse(f.read(), path)
    errors = []
    if not sections or sections[0].version != UNRELEASED:
        errors.append(f"the first section must be `## {UNRELEASED}`")
    if sum(s.version == UNRELEASED for s in sections) > 1:
        errors.append(f"`## {UNRELEASED}` appears more than once")
    released = [s for s in sections if s.version != UNRELEASED]
    for newer, older in zip(released, released[1:]):
        if semver_key(newer.version) <= semver_key(older.version):
            errors.append(
                f"[{newer.version}] is listed above [{older.version}] but is not newer"
            )
        if newer.date < older.date:
            errors.append(
                f"[{newer.version}] ({newer.date}) is dated before [{older.version}] ({older.date})"
            )
    for s in released:
        if not s.bullets() and not s.note:
            errors.append(f"[{s.version}] is empty")
    version = workspace_version(cargo_toml)
    if version not in {s.version for s in released}:
        errors.append(
            f"no `## [{version}] - <date>` section for the workspace version ({cargo_toml}); on the release PR, run "
            f"`scripts/changelog.py release {path} {version} <date>` (docs/RELEASING.md)"
        )
    seen = set()
    for s in sections:
        for _, b in s.bullets():
            if b in seen:
                errors.append(f"an entry appears twice: {first_line(b)}")
            seen.add(b)
    expected = link_lines(sections)
    if links != expected:
        errors.append(
            "the links at the end must be exactly:\n    " + "\n    ".join(expected)
        )
    if errors:
        raise Refused("\n".join(f"{path}: {e}" for e in errors))


# ── self-test ────────────────────────────────────────────────────────────────────────────────────────────────────

HEADER = "# Changelog\n\nAll notable changes.\n"
A, B, C, D = ("- a: one\n  continued", "- b: two", "- c: three", "- d: four")


def _unreleased(**groups):
    out = HEADER + "\n## Unreleased\n"
    for sub in UNRELEASED_SUBHEADS:
        out += f"\n### {sub}\n"
        if groups.get(sub):
            out += "\n" + "\n".join(groups[sub]) + "\n"
    return out


def _self_test():
    # The throwaway repo's git calls must not reach an outer repository (a git hook sets GIT_DIR and
    # GIT_INDEX_FILE, which override `git -C`).
    for var in [v for v in os.environ if v.startswith("GIT_")]:
        del os.environ[var]
    failures = []

    def expect(name, cond):
        verdict = "ok  " if cond else "FAIL"
        print(f"{verdict} {name}")  # guardrails-ok: the self-test's report
        if not cond:
            failures.append(name)

    def refused(name, fn, needle):
        try:
            fn()
        except Refused as e:
            expect(f"{name} (refused: {needle})", needle in str(e))
            return
        expect(f"{name} (refused: {needle})", False)

    with tempfile.TemporaryDirectory() as tmp:
        repo = os.path.join(tmp, "repo")
        os.mkdir(repo)
        cl = os.path.join(repo, "CHANGELOG.md")
        cargo = os.path.join(repo, "Cargo.toml")

        def g(*args):
            git(
                repo,
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                *args,
            )

        def put(text, version, tag=None, date="2026-01-01"):
            with open(cl, "w", encoding="utf-8") as f:
                f.write(text)
            with open(cargo, "w", encoding="utf-8") as f:
                f.write(f'[workspace.package]\nversion = "{version}"\n')
            g("add", "-A")
            env_date = f"{date}T12:00:00+00:00"
            os.environ["GIT_COMMITTER_DATE"] = os.environ["GIT_AUTHOR_DATE"] = env_date
            g("commit", "-q", "-m", tag or "dev")
            if tag:
                g("tag", tag)

        def read():
            with open(cl, encoding="utf-8") as f:
                return f.read()

        g("init", "-q")
        put(_unreleased(), "0.1.0-alpha.1", "v0.1.0-alpha.1", "2026-01-01")
        put(
            _unreleased(Added=[A], Fixed=[B]),
            "0.1.0-alpha.2",
            "v0.1.0-alpha.2",
            "2026-02-01",
        )
        put(
            _unreleased(Added=[A, C], Fixed=[B]),
            "0.1.0-alpha.3",
            "v0.1.0-alpha.3",
            "2026-03-01",
        )
        put(
            _unreleased(Added=[A, C, D], Fixed=[B]), "0.1.0-alpha.3", None, "2026-03-05"
        )
        tags = ["v0.1.0-alpha.1", "v0.1.0-alpha.2", "v0.1.0-alpha.3"]
        base = f"{REPO_URL}/compare"

        refused(
            "check before the split: the workspace version has no section",
            lambda: check(cl, cargo),
            "no `## [0.1.0-alpha.3] - <date>` section",
        )
        split(cl, tags)
        want = (
            HEADER
            + "\n## Unreleased\n\n### Added\n\n"
            + D
            + "\n\n### Changed\n\n### Removed\n\n### Fixed\n\n"
            "### Security\n\n## [0.1.0-alpha.3] - 2026-03-01\n\n### Added\n\n"
            + C
            + "\n\n"
            "## [0.1.0-alpha.2] - 2026-02-01\n\n### Added\n\n"
            + A
            + "\n\n### Fixed\n\n"
            + B
            + "\n\n"
            "## [0.1.0-alpha.1] - 2026-01-01\n\n" + "\n".join(EMPTY_NOTE) + "\n\n"
            f"[0.1.0-alpha.3]: {base}/v0.1.0-alpha.2...v0.1.0-alpha.3\n"
            f"[0.1.0-alpha.2]: {base}/v0.1.0-alpha.1...v0.1.0-alpha.2\n"
            f"[0.1.0-alpha.1]: {REPO_URL}/releases/tag/v0.1.0-alpha.1\n"
        )
        got = read()
        expect(
            "split: one section per tag, entries diffed by bullet, empty release noted, links",
            got == want,
        )
        if got != want:
            print(got)  # guardrails-ok: the self-test shows the output it rejected
        check(cl, cargo)
        expect("check: the split file passes", True)
        split(cl, tags)
        expect("split: re-running on its own output changes nothing", read() == want)

        # A note in a release section survives a re-split.
        with open(cl, "w", encoding="utf-8") as f:
            f.write(
                want.replace(
                    "## [0.1.0-alpha.2] - 2026-02-01\n",
                    "## [0.1.0-alpha.2] - 2026-02-01\n\nA note.\n",
                )
            )
        split(cl, tags)
        expect(
            "split: a section's note is kept",
            "## [0.1.0-alpha.2] - 2026-02-01\n\nA note.\n\n### Added" in read(),
        )

        # The release-PR case: the version is bumped and Unreleased not yet renamed.
        with open(cl, "w", encoding="utf-8") as f:
            f.write(want)
        with open(cargo, "w", encoding="utf-8") as f:
            f.write('[workspace.package]\nversion = "0.1.0-alpha.4"\n')
        refused(
            "check on a release PR before the rename",
            lambda: check(cl, cargo),
            "no `## [0.1.0-alpha.4] - <date>` section for the workspace version",
        )
        release(cl, "0.1.0-alpha.4", "2026-04-01")
        got = read()
        expect(
            "release: Unreleased becomes the version's section, without its empty subheads",
            "## [0.1.0-alpha.4] - 2026-04-01\n\n### Added\n\n"
            + D
            + "\n\n## [0.1.0-alpha.3]"
            in got,
        )
        expect(
            "release: a fresh, empty Unreleased opens above it",
            got.startswith(
                HEADER
                + "\n## Unreleased\n\n### Added\n\n### Changed\n\n### Removed\n\n### Fixed\n\n"
                "### Security\n\n## [0.1.0-alpha.4]"
            ),
        )
        expect(
            "release: its compare link is added",
            f"[0.1.0-alpha.4]: {base}/v0.1.0-alpha.3...v0.1.0-alpha.4\n[0.1.0-alpha.3]"
            in got,
        )
        check(cl, cargo)
        expect("check: passes after the release step", True)
        refused(
            "release with nothing unreleased",
            lambda: release(cl, "0.1.0-alpha.5", "2026-05-01"),
            "no entries",
        )
        with open(cl, "w", encoding="utf-8") as f:
            f.write(want)
        refused(
            "release of an older version",
            lambda: release(cl, "0.1.0-alpha.2", "2026-05-01"),
            "is not newer",
        )
        refused(
            "release of the newest version again",
            lambda: release(cl, "0.1.0-alpha.3", "2026-05-01"),
            "is not newer",
        )
        refused(
            "release dated before the newest",
            lambda: release(cl, "0.1.0-alpha.4", "2026-02-01"),
            "is before",
        )
        refused(
            "release with a bad date",
            lambda: release(cl, "0.1.0-alpha.4", "2026-13-01"),
            "not a YYYY-MM-DD",
        )

        # check: each violation is named.
        with open(cargo, "w", encoding="utf-8") as f:
            f.write('[workspace.package]\nversion = "0.1.0-alpha.3"\n')

        def check_text(text):
            with open(cl, "w", encoding="utf-8") as f:
                f.write(text)
            return lambda: check(cl, cargo)

        refused(
            "check: a missing link",
            check_text(
                want.replace(
                    f"[0.1.0-alpha.1]: {REPO_URL}/releases/tag/v0.1.0-alpha.1\n", ""
                )
            ),
            "the links at the end",
        )
        refused(
            "check: a wrong link",
            check_text(
                want.replace(
                    "v0.1.0-alpha.1...v0.1.0-alpha.2", "v0.1.0-alpha.1...v0.1.0-alpha.3"
                )
            ),
            "the links at the end",
        )
        swapped = (
            want.replace("## [0.1.0-alpha.3] - 2026-03-01", "## [X]")
            .replace(
                "## [0.1.0-alpha.2] - 2026-02-01", "## [0.1.0-alpha.3] - 2026-03-01"
            )
            .replace("## [X]", "## [0.1.0-alpha.2] - 2026-02-01")
        )
        refused(
            "check: releases out of order",
            check_text(swapped),
            "is listed above [0.1.0-alpha.3] but is not newer",
        )
        refused(
            "check: dates out of order",
            check_text(
                want.replace(
                    "## [0.1.0-alpha.3] - 2026-03-01", "## [0.1.0-alpha.3] - 2026-01-15"
                )
            ),
            "is dated before",
        )
        refused(
            "check: an entry in two sections",
            check_text(
                want.replace(
                    "### Fixed\n\n### Security",
                    "### Fixed\n\n" + C + "\n\n### Security",
                )
            ),
            "an entry appears twice",
        )
        refused(
            "check: Unreleased not first",
            check_text(
                want.replace("## Unreleased\n", "", 1).replace(
                    "## [0.1.0-alpha.1] - 2026-01-01\n",
                    "## [0.1.0-alpha.1] - 2026-01-01\n\n## Unreleased\n",
                )
            ),
            "the first section must be",
        )
        refused(
            "check: an empty release",
            check_text(want.replace("\n".join(EMPTY_NOTE) + "\n\n", "")),
            "is empty",
        )
        refused(
            "check: a heading without a date",
            check_text(
                want.replace("## [0.1.0-alpha.2] - 2026-02-01", "## [0.1.0-alpha.2]")
            ),
            "is neither `## Unreleased` nor",
        )
        refused(
            "check: an unknown subhead",
            check_text(want.replace("### Changed", "### Misc")),
            "unknown subhead",
        )
        refused(
            "check: an entry outside a subhead",
            check_text(
                want.replace(
                    "## [0.1.0-alpha.3] - 2026-03-01\n\n",
                    "## [0.1.0-alpha.3] - 2026-03-01\n\n- stray\n\n",
                )
            ),
            "an entry outside a subhead",
        )

        # split refuses what it would have to guess.
        def split_text(text):
            with open(cl, "w", encoding="utf-8") as f:
                f.write(text)
            return lambda: split(cl, tags)

        refused(
            "split: an entry reworded since its tag",
            split_text(_unreleased(Added=["- a: one, reworded", C, D], Fixed=[B])),
            "not in",
        )
        refused(
            "split: an entry moved to another subhead",
            split_text(_unreleased(Added=[A, C, D, B])),
            "is under",
        )
        refused(
            "split: tags out of order",
            lambda: split(cl, list(reversed(tags))),
            "oldest first",
        )
        refused(
            "split: a section that is not a tag",
            split_text(
                want.replace(
                    "## [0.1.0-alpha.1] - 2026-01-01", "## [0.0.9] - 2025-12-01"
                )
            ),
            "is not one of the given tags",
        )

    expect(
        "semver: prereleases rank below their release, numeric ids numerically",
        semver_key("0.1.0-alpha.2")
        < semver_key("0.1.0-alpha.10")
        < semver_key("0.1.0-beta")
        < semver_key("0.1.0")
        < semver_key("0.1.1"),
    )
    summary = "FAILED" if failures else "passed"
    print(f"{summary}: {len(failures)} failure(s)")  # guardrails-ok: the report
    return 1 if failures else 0


def main(argv):
    try:
        if argv[1:] == ["--self-test"]:
            return _self_test()
        if len(argv) >= 4 and argv[1] == "split":
            split(argv[2], argv[3:])
        elif len(argv) == 5 and argv[1] == "release":
            release(argv[2], argv[3], argv[4])
        elif len(argv) == 4 and argv[1] == "check":
            check(argv[2], argv[3])
            print(f"{argv[2]}: ok")  # guardrails-ok: the check's verdict
        else:
            print(  # guardrails-ok: the usage message
                __doc__
                + "\nusage: changelog.py split CHANGELOG TAG... | release CHANGELOG VERSION DATE | "
                "check CHANGELOG CARGO_TOML | --self-test",
                file=sys.stderr,
            )
            return 2
    except Refused as e:
        print(e, file=sys.stderr)  # guardrails-ok: the refusal's reason
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
