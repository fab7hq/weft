# Weft

> RingFrame is the warp — the fixed threads already on the loom, the record
> everything is built against. Weft is the thread carried across them.

Weft keeps track of what you asked your coding agents for, and what came back.

It runs your real `claude` and `codex` sessions in panes, lets you ask for
something once and send it to whichever agent you choose, and shows one list of
what was asked, what came back, who independently checked it, and what you
decided.

**Weft has no LLM of its own.** Every model call happens in the agent you
already run and configured. Weft drives the agents that think; the record is
written by [RingFrame](https://pypi.org/project/ringframe/).

## Status

Early. The runtime, the input path and the RingFrame interface all work:
agents run in panes, the work list is read from the record, and Weft types a
confirmed prompt for you after asking. It is not packaged yet.

## Requirements

- macOS or Linux, a terminal
- the `ringframe` CLI on `PATH`
- at least one of `claude` or `codex`

## Build

```sh
cargo build
cargo test
```

## Keys

Weft reserves exactly one chord. Everything else belongs to the agent, and
every action on screen shows its key in brackets.

| Key | Action |
| --- | --- |
| `Ctrl+]` | Into the agent, and back again |
| `↑` `↓` | Pick a row |
| `Enter` | Expand the row in place |
| `←` | Back: collapse, close the drawer, cancel |
| `Space` | Jump to the next thing that needs you |
| `Tab` · `1`-`9` | Next agent · that agent |
| `A` `S` | Ask · send the wording RingFrame compiled |
| `C` `D` | Check · decide |
| `P` `J` | Read the wording · read the judges |
| `F` | Ask again about this work |
| `N` `W` | New agent · show or hide the work list |
| `H` `X` | Help · quit |

In the agent every other key goes straight through, `Esc` included.

The mouse works everywhere. To select text inside a pane, hold `Shift`.

To read the screens without running an agent:

```sh
cargo run --example wireframe
```

## Licence

Apache-2.0
