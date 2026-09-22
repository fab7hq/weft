# RingFrame

> **Warp and weft.** The warp is the fixed threads already on the loom, held
> under tension. That is this: the record everything else is measured against.
> [Weft](../..) is the thread carried across them.

RingFrame records what was asked of a coding agent, judges what came back, and
seals the decision. Three acts over one append-only ledger in `.fab7/rf/`:

- **Ask** — compile an intent into a prompt, with the directives that apply to
  it, and record that it was confirmed before anything is sent.
- **Eval** — write a facts-only brief over the open Asks and aggregate
  independent judgements into a verdict.
- **Seal** — record the decision that closes them, with a receipt that can be
  re-verified later.

## It is not part of Weft

This crate lives in Weft's repository and is not Weft's ([ADR-0013]). It ships
its own binary, invoked by name, and nothing in Weft depends on this crate —
the skills call `ringframe`, and that string is the contract. Weft renders
what RingFrame wrote and never writes `.fab7/rf/` itself.

[ADR-0013]: https://github.com/fab7hq/weft

## Using it

The binary installs beside `weft`:

```sh
curl -fsSL https://raw.githubusercontent.com/fab7hq/weft/main/install.sh | sh
```

It is driven by skills rather than typed by hand — `rf:ask`, `rf:eval` and
`rf:seal` in Claude Code and Codex, from the [fab7 marketplace]. Twenty-one
commands, each printing one JSON object with `--json`:

```sh
ringframe init                 # prepare this project
ringframe ask list --json      # every compiled Ask, oldest first
ringframe ledger verify --json # re-check every event and every artifact
```

[fab7 marketplace]: https://github.com/fab7hq/fab7

## Configuration is not here

The profiles, the delta catalogs and the skills are configuration, synced from
the marketplace into `~/.fab7/rf/`. This package ships none of it: a core with
no configuration refuses to compile an Ask rather than inventing one.

## Versions

The Rust era starts at 0.1.0. The Python `ringframe` 0.0.5 was the last of the
previous one and stays where it is; it reads `ringframe.bundle/1` and refuses
this era's configuration by name, which is the boundary working rather than a
failure.

## Licence

Apache-2.0
