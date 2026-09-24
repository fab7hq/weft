# The canonical-JSON corpus

RingFrame's evidence rests on one function:

```python
json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
```

Every brief, intent, judgement, ledger event, `response_sha256` and config
digest is a sha256 over its output. If the Rust core differs by one byte,
`source_verified` and `submission: observed` quietly stop matching, and it
looks like a model problem rather than a serialiser one.

`canonical/samples.jsonl` is what the Python core produced, frozen while that
core still existed. It is the reference. The port does not get to decide what
canonical means.

Each line has `name`, `kind` (`json` or `yaml`), `input` as found, `canonical`,
and the `sha256` of the canonical bytes. The envelope is escaped ASCII so that
one sample is one line for every reader, including the samples carrying U+2028.

## What is in it

- **edge** — where canonical JSON implementations actually diverge: float
  rendering, integers past 2^53 and i64, every escape sequence, a codepoint
  sweep across the planes, key ordering, empty keys.
- **artifact**, **event** — real output from this machine, a few of each schema
  RingFrame writes, plus any sample that makes the serialiser choose something
  the edge cases did not.
- **config** — YAML, whose *parsed* form was digested into `profile_sha256`
  while configuration was YAML. It is TOML now, and these rows are kept as
  canonical forms.

Samples are kept for coverage, not volume: the generator drops anything that
renders nothing new.

## Running the gate

    cargo test --test the_corpus_holds

Three tests. Every JSON sample must canonicalise to the reference bytes; every
digest must be of the bytes beside it; canonicalising a canonical form must
change nothing. The YAML rows sit out the first test: this release reads no
YAML, and the other two tests hold those rows.

## Regenerating

Don't. The point of a reference is that it does not move, and the core that
produced it is archived. The generator went with it, to `tools/corpus.py` in
the Python repository, because it only runs against that core.

Adding coverage means adding an edge case there and running it against the
archived core — not producing a new file from whatever this machine happens to
hold.
