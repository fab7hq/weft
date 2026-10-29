# RingFrame

> **Warp and weft.** The warp is the fixed threads already on the loom, held
> under tension. That is this: the record everything else is measured against.
> [Weft](../..) is the thread carried across them.

RingFrame is Weft's engine. It records what a person asked of a coding agent,
judges what came back, and seals their decision. Three acts over one
append-only ledger in `.fab7/rf/`:

- **Ask** — the person's words, kept as they wrote them, with the practice
  rules they chose and their harness's route rules. Sending it from Weft is
  the confirmation.
- **Eval** — the change over the open Asks, judged by harness sessions Weft
  starts and feeds one task at a time. Every citation is checked, and the
  verdict is computed, not judged.
- **Seal** — the decision that closes the Asks, with a receipt that can be
  re-verified later.

## Weft links it

This crate lives in Weft's repository. weftd links it and calls it through
`ringframe::api`. Only this crate writes `.fab7/rf/`: Weft shows what it
wrote and never writes there itself. A person uses RingFrame through Weft.

## The `ringframe` command

The binary is built from this crate and installs beside `weft`. It holds what
Eval's judges run (`eval task`, `eval submit`, and the lookups `eval window`,
`eval grep`, `eval commits`), and commands to inspect and check a project's
record:

```sh
ringframe ask list --json          # every Ask, oldest first
ringframe eval show --eval <id>    # an Eval's report
ringframe ledger verify --json     # re-check every event and every artifact
```

`ringframe --help` lists them all.

## Configuration is not here

The harness profiles and the rule catalogs are configuration, synced from the
[fab7 marketplace](https://github.com/fab7hq/fab7) into `~/.fab7/rf/`. This
crate ships none of it: with no configuration it refuses to compile an Ask
rather than invent one.

## License

Apache-2.0
