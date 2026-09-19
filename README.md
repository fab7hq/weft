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

Early. The runtime and input path work; the RingFrame interface is not built
yet.

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

Weft reserves exactly one key. Everything else belongs to the agent.

| Key | Action |
| --- | --- |
| `Ctrl+Shift+W` | Switch between Weft and the agent |
| `↑` `↓` | Pick an item |
| `Enter` | Open it, or go into the agent |
| `A` `C` `D` | Ask · Check · Decide |
| `H` | Help |
| `X` | Quit |

`Ctrl+Shift+W` needs a terminal that reports modifiers on a Ctrl+letter chord
(Kitty, Ghostty, WezTerm, foot). Elsewhere Weft falls back to `Ctrl+]`, and the
hint bar always names the key that is actually live.

The mouse works everywhere. To select text inside a pane, hold `Shift`.

## Licence

Apache-2.0
