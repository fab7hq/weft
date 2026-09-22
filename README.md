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
written by RingFrame, whose core is the `ringframe` crate in this
repository and ships as its own binary beside `weft`.

## Status

**0.1.0.** The runtime, the input path and the RingFrame interface all work:
agents run in panes, the work list is read from the record, and Weft types a
confirmed prompt for you after asking. One window holds as many projects as
you open.

Early in the sense that matters: the record's shape is settled and the
interface is not. Expect the screens to move.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/fab7hq/weft/main/install.sh | sh
```

It detects the platform, downloads a prebuilt `weft` and `ringframe`, verifies
a checksum, and installs both to `~/.local/bin`. There is no toolchain to
have. Then add the plugin for your harness:

```sh
# Claude Code
/plugin marketplace add fab7hq/fab7 && /plugin install rf@fab7
# Codex
codex plugin marketplace add fab7hq/fab7 && codex plugin add rf@fab7
```

The Python-era `uv tool install ringframe` still gets `ringframe` 0.0.5. It
keeps working and is frozen there; it reads `ringframe.bundle/1` and refuses
this era's configuration by name, which is the boundary working rather than a
failure.

## Working on it

`./bin/reinstall-local` puts this working tree in front of your hosts: it
builds both binaries, installs them, syncs the configuration from the `fab7`
checkout beside this one, and reinstalls the plugins into Claude Code and
Codex. All three move together, which is the point — change a profile and
refresh only the plugins, and the skills change while the core keeps reading
the configuration it last synced from the marketplace.

```sh
./bin/reinstall-local          # both hosts
./bin/reinstall-local none     # binaries and configuration only
```

## Requirements

- macOS or Linux, a terminal
- at least one of `claude` or `codex`

## Build

```sh
cargo build
cargo test
```

## The screen

The sidebar is a tree: **project → harness → action**. A unit hangs under the
harness it was asked of, and says the furthest act that has happened.

```
 ▾ fab7                                         2 open · 1 ●   ⌫
   ▾ codex                                                   1 ●
        health endpoint                              ● EVALED
   ▸ claude-code                                             1
 ▸ ringframe                                    2 open · 1 ●   ⌫
```

`[Enter]` opens what is closed and goes there when it is open — a harness to
its pane, an action to its detail view. The detail view is one unit's whole
story, and it blocks: `ASK [DONE]  EVAL [HAVEN'T RUN]  SEAL [HAVEN'T RUN]`,
each section holding what happened, with `[P]ROCEED` for whatever the next
step is.

## Keys

Weft reserves exactly one chord. Everything else belongs to the agent.

| Key | Action |
| --- | --- |
| `Ctrl+]` | Into the agent, and back again |
| `↑` `↓` | Move |
| `Enter` | Unfold what is closed; open, go there |
| `→` `←` | Unfold · fold, and back everywhere |
| `Space` | The Ask that needs you, opened |
| `Tab` · `1`-`9` | Next agent · that agent |
| `A` `E` `S` | RingFrame's three acts: ask · eval · seal |
| `P` `F` | In the detail view: proceed · follow up |
| `R` `Y` | Ready a harness up · why a pane is waiting |
| `W` | The Weft menu: `O`pen project · `N`ew agent · `B` sidebar · `H`elp · `X` quit |
| `⌫` | Close the selected project, after asking |

In the agent every other key goes straight through, `Esc` included.

**Weft takes no mouse.** Capturing it would hand every click and drag to Weft
instead of to your terminal's own selection, which is what click-drag-copy
over an agent is. Weft is keys only and the mouse stays where you expect it.

To read the screens without running an agent:

```sh
cargo run --example wireframe
```

## Licence

Apache-2.0
