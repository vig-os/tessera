"""The Python provenance surface: pinned source edges (#563), producer and generation (#562).

Run as: python3 provenance.py

Every assertion reads the SEALED manifest back through `tessera.open(...).manifest()`. The builder's
in-memory state is not what a reader sees, so checking it would prove nothing about the file.
"""

import pathlib
import sys
import tempfile

import numpy as np

import tessera

out = pathlib.Path(tempfile.mkdtemp())
VOLUME = np.arange(8, dtype="<i2").tobytes()


def builder(name: str) -> "tessera.Builder":
    b = tessera.Builder("t_product", name, "", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", [2, 2, 2], VOLUME)
    return b


def sealed(b: "tessera.Builder", name: str) -> dict:
    b.pack(str(out / f"{name}.tsra"))
    return tessera.open(out / f"{name}.tsra").manifest()


# --- #563: add_source pins the parent's seal --------------------------------------------------
parent = sealed(builder("parent"), "parent")
parent_seal = parent["manifest_hash"]
assert parent_seal.startswith("blake3:"), parent_seal

pinned = builder("pinned")
pinned.add_source("derived_from", "parent.tsra", content_hash=parent_seal)
got = sealed(pinned, "pinned")["sources"]
assert got == [
    {"role": "derived_from", "reference": "parent.tsra", "content_hash": parent_seal}
], got

# Without the pin the key is ABSENT, not null: a product written by an old caller seals exactly as
# before, so existing products and the conformance corpus keep their hashes.
unpinned = builder("unpinned")
unpinned.add_source("derived_from", "parent.tsra")
got = sealed(unpinned, "unpinned")["sources"]
assert got == [{"role": "derived_from", "reference": "parent.tsra"}], got

# Positional form, same edge (the signature mirrors Rust's `Source::new(..).with_content_hash(..)`).
positional = builder("positional")
positional.add_source("derived_from", "parent.tsra", parent_seal)
assert sealed(positional, "positional")["sources"][0]["content_hash"] == parent_seal


def raises(exc, fn, *needles: str) -> None:
    """`fn()` must raise exactly `exc` (not a subclass of it standing in for a signature regression),
    with every needle in the message."""
    try:
        fn()
    except Exception as e:  # noqa: BLE001 — we assert the exact type below
        assert type(e) is exc, f"expected {exc.__name__}, got {type(e).__name__}: {e}"
        for n in needles:
            assert n in str(e), f"{n!r} not in {e}"
    else:
        raise AssertionError(f"expected {exc.__name__}")


# --- #562: with_producer -------------------------------------------------------------------------
# Without the call the default stamp names the packager; this must not change under the new surface.
default = sealed(builder("default"), "default")
stamp = default["producer"]
# `git_commit` / `git_repo` join the stamp only when the build sets TESSERA_GIT_COMMIT/_REPO
# (Producer::tessera); the CI builds do not, but the stamp is still the default with them.
assert {k: stamp[k] for k in ("tool", "version")} == {
    "tool": "tessera",
    "version": "0.0.0",
}, stamp
assert set(stamp) <= {"tool", "version", "git_commit", "git_repo"}, stamp
assert "generation" not in default, default

full = builder("full-producer")
full.with_producer(
    "anatomist",
    "0.1.0",
    git_commit="0855f5f",
    git_repo="MorePET/anatomist",
    dirty=False,
)
got = sealed(full, "full-producer")["producer"]
assert got == {
    "tool": "anatomist",
    "version": "0.1.0",
    "git_commit": "0855f5f",
    "git_repo": "MorePET/anatomist",
    "dirty": False,
}, got

# Omitted optional fields are ABSENT, not null; a second call replaces the first (Rust semantics).
minimal = builder("minimal-producer")
minimal.with_producer("first", "9")
minimal.with_producer("anatomist", "0.1.0")
got = sealed(minimal, "minimal-producer")["producer"]
assert got == {"tool": "anatomist", "version": "0.1.0"}, got

# --- #562: with_generation -----------------------------------------------------------------------
config = {
    "step": "trees",
    "energy_window_keV": [425, 650],
    "seed": 2**63 - 1,
    "window": (0.5, 1.5),  # a tuple seals as a JSON array
    "dry_run": False,
    "note": None,
    "agent": {"model": "m", "effort": "high", "nested": {"depth": [1, {"k": "v"}]}},
}
both = builder("generation")
both.with_generation(config=config, config_ref="blake3:" + "ab" * 32)
got = sealed(both, "generation")["generation"]
assert got == {
    "config": {**config, "window": [0.5, 1.5]},
    "config_ref": "blake3:" + "ab" * 32,
}, got

only_ref = builder("generation-ref")
only_ref.with_generation(config_ref="blake3:" + "cd" * 32)
assert sealed(only_ref, "generation-ref")["generation"] == {
    "config_ref": "blake3:" + "cd" * 32
}

replaced = builder("generation-replaced")
replaced.with_generation(config={"a": 1})
replaced.with_generation(config={"b": 2})
assert sealed(replaced, "generation-replaced")["generation"] == {"config": {"b": 2}}

# An empty recipe is ABSENT (Generation::is_empty), so accepting it would fake a sealed recipe.
for call in (
    lambda b: b.with_generation(),
    lambda b: b.with_generation(config={}),
    lambda b: b.with_generation(config=None, config_ref=None),
):
    raises(tessera.TesseraError, lambda: call(builder("empty")), "needs a recipe")

# Non-JSON input raises TypeError naming the offending key, at any depth. Nothing is coerced.
for bad, needles in (
    ({1: "x"}, ("config: key 1", "is an int, not a str")),
    ({"agent": {2: "x"}}, ('config["agent"]: key 2',)),
    ({"t": {1, 2}}, ('config["t"]', "set is not a JSON value")),
    ({"a": [1, object()]}, ('config["a"][1]', "object is not a JSON value")),
    ({"nan": float("nan")}, ('config["nan"]', "not finite")),
    ({"big": 2**64}, ('config["big"]', "does not fit in 64 bits")),
    ({"raw": b"x"}, ('config["raw"]', "bytes is not a JSON value")),
    # A lone surrogate is a valid Python str but has no UTF-8 (so no JSON) form.
    ({"s": "\ud800"}, ('config["s"]', "lone surrogate")),
    ({"\ud800": 1}, ("config: key", "lone surrogate")),
):
    raises(TypeError, lambda: builder("bad").with_generation(config=bad), *needles)

# Hostile shapes raise instead of crashing the interpreter: a SIGSEGV is uncatchable and takes a
# long-running pipeline down with it.
cyclic = {}
cyclic["self"] = cyclic
raises(
    ValueError,
    lambda: builder("cycle").with_generation(config=cyclic),
    'config["self"]',
    "circular reference",
)
loop = []
loop.append(loop)
raises(
    ValueError,
    lambda: builder("cycle-list").with_generation(config={"l": loop}),
    'config["l"][0]',
    "circular reference",
)


def nested(depth: int) -> dict:
    root = cur = {}
    for _ in range(depth - 1):
        cur["n"] = {}
        cur = cur["n"]
    return root


# Not a cycle: one dict referenced twice is a DAG and converts like `json.dumps` does.
shared = {"x": 1}
dag = builder("dag")
dag.with_generation(config={"a": shared, "b": [shared, shared]})
got = sealed(dag, "dag")["generation"]["config"]
assert got == {"a": {"x": 1}, "b": [{"x": 1}, {"x": 1}]}, got

# Nesting is capped at a FIXED depth: what the manifest reader parses back. serde_json reads at most
# 127 nested levels, and `config` sits two below the manifest root (manifest -> generation), so 125
# levels of config, counting the config dict itself. Independent of sys.getrecursionlimit(), so raising
# that can neither crash the writer (deep values are dropped and serialised recursively) nor seal a
# product nobody can open (#589).
MAX_CONFIG_DEPTH = 125


def depth_of(config) -> int:
    depth = 1
    while config:
        config, depth = config["n"], depth + 1
    return depth


def nested_list(depth: int) -> dict:
    root = {"l": []}
    cur = root["l"]
    for _ in range(depth - 2):
        cur.append([])
        cur = cur[0]
    return root


old_limit = sys.getrecursionlimit()
sys.setrecursionlimit(10**6)
try:
    for over in (
        nested(MAX_CONFIG_DEPTH + 1),
        nested_list(MAX_CONFIG_DEPTH + 1),
        nested(100_000),
    ):
        try:
            builder("too-deep").with_generation(config=over)
        except tessera.TesseraError as e:
            msg = str(e)
            assert f"deeper than {MAX_CONFIG_DEPTH} levels" in msg, msg
            # The path is shortened: a full one is ~1 KB at this depth and grows with it.
            assert len(msg) < 400, len(msg)
        else:
            raise AssertionError("a config past the reader's depth must raise")

    # Exactly at the limit: seals AND reopens, dicts and lists alike.
    at_limit = builder("at-limit")
    at_limit.with_generation(config=nested(MAX_CONFIG_DEPTH))
    assert (
        depth_of(sealed(at_limit, "at-limit")["generation"]["config"])
        == MAX_CONFIG_DEPTH
    )
    list_limit = builder("list-limit")
    list_limit.with_generation(config=nested_list(MAX_CONFIG_DEPTH))
    sealed(list_limit, "list-limit")
finally:
    sys.setrecursionlimit(old_limit)

# Read back, every level of the config comes out key-sorted (the JCS order the seal hashes), not
# just the top level: what you read doesn't depend on the order you wrote it in.
ordered = builder("ordered")
ordered.with_generation(
    config={"b": {"z": 1, "a": {"y": 2, "c": 3}}, "a": [{"q": 1, "p": 2}]}
)
got = sealed(ordered, "ordered")["generation"]["config"]
assert list(got) == ["a", "b"], list(got)
assert list(got["b"]) == ["a", "z"] and list(got["b"]["a"]) == ["c", "y"], got
assert list(got["a"][0]) == ["p", "q"], got

# Both setters are write-side only: after pack, the existing guard applies.
packed = builder("packed")
packed.pack(str(out / "packed.tsra"))
for call in (
    lambda: packed.with_producer("t", "1"),
    lambda: packed.with_generation(config={"a": 1}),
    lambda: packed.add_source("derived_from", "x", content_hash="blake3:00"),
):
    raises(tessera.TesseraError, call, "builder already packed")

print("provenance: ok")
