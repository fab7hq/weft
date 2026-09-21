#!/usr/bin/env python3
"""Freeze what the Python core's `canonical()` produces, so Rust can match it.

RingFrame's evidence rests on one function:

    json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)

Briefs, intents, judgements, ledger events, hook receipts, `response_sha256`,
config digests. If the Rust core differs by one byte, `source_verified` and
`submission: observed` quietly stop matching, and it looks like a model
problem rather than a serialiser one.

This walks every artifact the Python core has ever written on this machine,
plus a set of edge cases real data may not cover, and records for each: the
bytes as found, the canonical bytes, and their digest. The Rust port reads the
same file and must reproduce every one of them exactly.

    python3 corpus/generate.py [workspace-root]

Run it while the Python core still exists. It is the reference.
"""

import hashlib
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "canonical" / "samples.jsonl"


def canonical(obj) -> bytes:
    """Byte for byte what `ringframe.store.canonical` does."""
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sample(name: str, kind: str, raw: str, obj) -> dict:
    out = canonical(obj)
    return {
        "name": name,
        "kind": kind,
        "input": raw,
        "canonical": out.decode("utf-8"),
        "sha256": sha256(out),
    }


def role_of(obj, path: Path) -> str:
    """What kind of artifact this is. Every one RingFrame writes declares its
    own schema, so there is nothing to guess from a filename full of ids."""
    if isinstance(obj, dict):
        for key in ("schema", "type"):
            if isinstance(obj.get(key), str):
                return obj[key]
    return path.suffix.lstrip(".") or "artifact"


def json_samples(root: Path):
    """Every published artifact and every ledger event on this machine."""
    seen = set()
    for path in sorted(root.rglob(".fab7/rf/**/*.json")):
        raw = path.read_text(encoding="utf-8")
        digest = sha256(raw.encode("utf-8"))
        if digest in seen:
            continue  # the same artifact copied between runs proves nothing twice
        seen.add(digest)
        try:
            obj = json.loads(raw)
        except json.JSONDecodeError:
            continue
        yield sample(f"artifact/{role_of(obj, path)}/{digest[:8]}", "json", raw, obj)

    for path in sorted(root.rglob(".fab7/rf/**/*.jsonl")):
        for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if not line.strip():
                continue
            digest = sha256(line.encode("utf-8"))
            if digest in seen:
                continue
            seen.add(digest)
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            yield sample(f"event/{role_of(obj, path)}/{digest[:8]}", "json", line, obj)


def yaml_samples(root: Path):
    """Config, whose *parsed* form is digested into `profile_sha256`."""
    try:
        import yaml
    except ImportError:
        print("pyyaml not importable; skipping config", file=sys.stderr)
        return
    seen = set()
    for path in sorted(root.rglob("*.yaml")):
        if ".venv" in path.parts or "node_modules" in path.parts:
            continue
        raw = path.read_text(encoding="utf-8")
        digest = sha256(raw.encode("utf-8"))
        if digest in seen:
            continue
        seen.add(digest)
        try:
            obj = yaml.safe_load(raw)
        except yaml.YAMLError:
            continue
        if not isinstance(obj, dict):
            continue
        yield sample(f"config/{obj.get('schema', path.name)}/{digest[:8]}", "yaml", raw, obj)


# Where canonical JSON implementations actually diverge. Real data may cover
# none of these, and each one has broken a port somewhere.
EDGE_CASES = {
    "empty-object": {},
    "empty-array": [],
    "key-order": {"b": 1, "a": 2, "C": 3, "_": 4, "0": 5},
    "nested": {"a": {"b": [{"c": 1}, {"d": [2, 3]}]}},
    "int-zero": {"v": 0},
    "int-negative": {"v": -1},
    "int-large": {"v": 9007199254740993},
    "int-i64-max": {"v": 9223372036854775807},
    "float-whole": {"v": 1.0},
    "float-agreement": {"v": 0.67},
    "float-third": {"v": 0.3333333333333333},
    "float-negative-zero": {"v": -0.0},
    "float-tiny": {"v": 5e-324},
    "float-huge": {"v": 1e308},
    "float-exponent": {"v": 1e30},
    "float-precision": {"v": 0.1},
    "bool-and-null": {"t": True, "f": False, "n": None},
    "string-empty": {"v": ""},
    "string-quote": {"v": 'he said "no"'},
    "string-backslash": {"v": "a\\b"},
    "string-newline": {"v": "a\nb\tc\rd"},
    "string-control": {"v": "\u0000\u0001\u001f"},
    "string-del": {"v": "\u007f"},
    "string-unicode": {"v": "héllo · wörld — ✓"},
    "string-cjk": {"v": "研究"},
    "string-emoji": {"v": "🧵"},
    "string-surrogate-pair": {"v": "\U0001f9f5"},
    "string-line-separator": {"v": "a b c"},
    "string-slash": {"v": "a/b"},
    "key-unicode": {"é": 1, "e": 2, "z": 3},
    "key-empty": {"": 1},
    "array-mixed": [1, 1.0, "1", True, None, {}, []],
    # Every plane, on purpose, because real data covers whichever languages
    # happened to be used and nothing says which those will be.
    "codepoint-sweep": {
        "v": "".join(
            chr(c)
            for c in (
                *range(0x20, 0x7F), 0x80, 0xA0, 0xFF, 0x100, 0x2028, 0x2029, 0x20AC,
                0x3042, 0x4E2D, 0xD7FF, 0xE000, 0xFFFD, 0x10000, 0x1F9F5, 0x10FFFF,
            )
        )
    },
    "escape-sweep": {"v": "".join(chr(c) for c in range(0x00, 0x20)) + '"\\\\/'},
}


# What a canonical form can render differently between implementations: how
# each number comes out, and which escape sequences appear in strings. Those
# are where implementations diverge; everything else is the same few shapes
# repeated, and a corpus of repeats is a corpus of noise.
ESCAPE = re.compile(r"\\(?:u[0-9a-fA-F]{4}|.)")


def renderings(value) -> set:
    """Every way this value makes the serialiser choose."""
    out = set()

    def walk(v):
        if isinstance(v, bool) or v is None:
            out.add(json.dumps(v))
        elif isinstance(v, int):
            # Integers render the same everywhere within a range, so what
            # matters is the range, not the value. Beyond 2^53 and beyond i64
            # they stop being interchangeable.
            m = abs(v)
            out.add(
                "int"
                + ("-neg" if v < 0 else "")
                + ("->u64" if m > 2**64 else "->i64" if m > 2**63 - 1 else "->2^53" if m > 2**53 else "")
            )
        elif isinstance(v, float):
            # A float's rendering is the thing that diverges, so keep it whole.
            out.add(json.dumps(v))
        elif isinstance(v, str):
            rendered = json.dumps(v, ensure_ascii=False)
            out.update(ESCAPE.findall(rendered))
            # Not each codepoint: with ensure_ascii=False every non-ASCII
            # character takes the same path, so one of them proves the lot.
            # The sweep below covers the range on purpose.
            if any(ord(ch) > 127 for ch in v):
                out.add("nonascii")
        elif isinstance(v, dict):
            for k, x in v.items():
                walk(k)
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)

    walk(value)
    return out


def novel(value, seen: set) -> bool:
    """Whether this sample makes the serialiser choose something new."""
    found = renderings(value)
    new = found - seen
    seen |= found
    return bool(new)


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else HERE.parent.parent).resolve()
    samples = [sample(f"edge/{k}", "json", json.dumps(v), v) for k, v in EDGE_CASES.items()]
    # The edge cases go first, so real data is only kept for what they miss.
    seen = set()
    for s in samples:
        novel(json.loads(s["canonical"]), seen)
    # Two reasons to keep a real sample: it makes the serialiser choose
    # something the edge cases did not, or it is a shape the product actually
    # writes. Scalars are covered above; shapes are covered a few of each.
    PER_KIND = 3
    kept = {}
    for s in list(json_samples(root)) + list(yaml_samples(root)):
        value = json.loads(s["canonical"])
        kind = "/".join(s["name"].split("/")[:-1])
        first_of_kind = kept.get(kind, 0) < PER_KIND
        if novel(value, seen) or first_of_kind:
            kept[kind] = kept.get(kind, 0) + 1
            samples.append(s)

    # Self-check: canonicalising the canonical form must be a fixed point, or
    # the reference itself is unstable and nothing downstream means anything.
    for s in samples:
        again = canonical(json.loads(s["canonical"]))
        if again.decode("utf-8") != s["canonical"]:
            print(f"not a fixed point: {s['name']}", file=sys.stderr)
            return 1
        if sha256(s["canonical"].encode("utf-8")) != s["sha256"]:
            print(f"digest disagrees: {s['name']}", file=sys.stderr)
            return 1

    OUT.parent.mkdir(parents=True, exist_ok=True)
    with OUT.open("w", encoding="utf-8") as f:
        for s in samples:
            # The envelope is escaped ASCII on purpose. The canonical bytes it
            # carries may contain U+2028, which is not a JSON line break but is
            # a line break to Python's `splitlines`, to JavaScript and to a few
            # editors. Escaping the transport keeps one sample per line for
            # every reader; the value inside is unchanged.
            f.write(json.dumps(s, sort_keys=True, ensure_ascii=True) + "\n")

    kinds = {}
    for s in samples:
        kinds[s["name"].split("/")[0]] = kinds.get(s["name"].split("/")[0], 0) + 1
    print(f"{len(samples)} samples → {OUT.relative_to(HERE.parent)}")
    for k in sorted(kinds):
        print(f"  {k:<10} {kinds[k]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
