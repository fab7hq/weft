# Weft

> **Warp and weft.** The warp is the fixed threads already on the loom, held
> under tension — that is RingFrame, the record everything is measured
> against. The weft is the thread carried across them, and that is this: the
> interface you work through. Neither is cloth on its own.

Weft keeps track of what you asked your coding agents for, and what came back.

It runs your real `claude` and `codex` sessions in panes, lets you ask for
something once and send it to whichever agent you choose, and shows one list of
what was asked, what came back, who independently checked it, and what you
decided.

**Weft has no LLM of its own.** Every model call happens in the agent you
already run and configured. Weft drives the agents that think; the record is
written by RingFrame, whose core is the `ringframe` crate in this repository
and ships as its own binary beside `weft`.

## Status

**0.1.0.** Agents run in panes, the work list is read from the record, and
Weft types a confirmed prompt for you after asking. One window holds as many
projects as you open.

The record's shape is settled; the interface is not. Expect the screens to
move.

## Requirements

- macOS or Linux, and a terminal
- at least one of `claude` or `codex`

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/fab7hq/weft/main/install.sh | sh
```

This detects the platform, downloads prebuilt `weft` and `ringframe` binaries,
verifies their checksum, and installs both to `~/.local/bin` (override with
`WEFT_BIN_DIR`). No toolchain is needed.

Then add the RingFrame plugin to your harness. In Claude Code:

```text
/plugin marketplace add fab7hq/fab7
/plugin install rf@fab7
```

In a shell, for Codex:

```sh
codex plugin marketplace add fab7hq/fab7
codex plugin add rf@fab7
```

## The screen

The sidebar is a tree: **project → harness → action**. Each action hangs under
the harness it was asked of and shows the furthest act that has happened.

```
 ▾ fab7                                         2 open · 1 ●   ⌫
   ▾ codex                                                   1 ●
        health endpoint                              ● EVALED
   ▸ claude-code                                             1
 ▸ ringframe                                    2 open · 1 ●   ⌫
```

`Enter` unfolds what is closed and goes to what is open: a harness to its
pane, an action to its detail view. The detail view is one action's whole
story, and it blocks — `ASK [DONE]  EVAL [HAVEN'T RUN]  SEAL [HAVEN'T RUN]`,
each section holding what happened, with `[P]ROCEED` for the next step.

## Keys

Weft reserves exactly one chord. Everything else belongs to the agent.

| Key | Action |
| --- | --- |
| `Ctrl+]` | Into the agent, and back again |
| `↑` `↓` | Move |
| `Enter` | Unfold what is closed; open, go there |
| `→` `←` | Unfold · fold, and back everywhere |
| `Space` | Jump to the next thing that needs you |
| `Tab` · `1`-`9` | Next agent · that agent |
| `A` `E` `S` | RingFrame's three acts: ask · eval · seal |
| `P` `F` | In the detail view: proceed · follow up |
| `R` `Y` | Ready a harness · why a pane is waiting |
| `O` `N` `B` | Open a project · new agent · toggle the sidebar |
| `H` `X` | Help · quit |
| `W` | The Weft menu, holding the operations above |
| `⌫` | Close the selected project, after asking |

Inside the agent every key goes straight through, `Esc` included.

**Weft takes no mouse.** Capturing it would take every click and drag away
from your terminal's own selection, which is how you copy text out of an
agent. Weft is keys only, and the mouse stays where you expect it.

## Development

```sh
cargo build
cargo test
cargo run --example wireframe   # the screens, without running an agent
```

`./bin/reinstall-local` puts this working tree in front of your hosts. It
builds both binaries, installs them, syncs the configuration from the `fab7`
checkout beside this one, and reinstalls the plugins. The three have to move
together: refresh only the plugins and the skills change while the core keeps
reading the configuration it last synced.

```sh
./bin/reinstall-local           # both hosts
./bin/reinstall-local claude    # one host (or codex)
./bin/reinstall-local none      # binaries and configuration only
```

## License

Apache-2.0
