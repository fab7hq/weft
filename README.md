<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/weft-banner-dark.svg">
    <img alt="Weft" src="assets/weft-banner-light.svg" width="378">
  </picture>
</p>

<p align="center">
  <img alt="Weft's walkthrough of one change: each one-line request becomes the prompt RingFrame prepared from it, a Plan, a /goal and a fix, Eval checks the work against the Asks, and Seal records the decision." src="assets/weft-sdlc.gif" width="880">
</p>
<p align="center"><sub>Weft's own screens, drawn from one recorded session; no agent runs.
<a href="https://docs.getfab7.com/weft/getting-started">Get started</a> ·
<a href="https://docs.getfab7.com/weft/sdlc">Map your SDLC</a></sub></p>

Weft is the terminal workspace for the coding agents you already run: your
sessions side by side in panes, and every Ask in one list beside them, from
asked to checked to decided. It has no model of its own.

Named after woven cloth: RingFrame is the warp, the fixed record; Weft is the
thread carried across it.

## What is here

| Part | What it is |
| --- | --- |
| Weft (`weft`, `weftd`) | The terminal UI and the daemon that keeps your agents running |
| RingFrame (`crates/ringframe`) | Ask, Eval and Seal over the record in `.fab7/rf/`, as the `ringframe` binary |
| [Weft on the web](https://weft.getfab7.com) | The same agents from a phone or a browser, from [fab7hq/weft-app](https://github.com/fab7hq/weft-app) |

Weft runs Claude Code, Codex and Antigravity.

## Install

```sh
curl -fsSL https://github.com/fab7hq/weft/releases/latest/download/install.sh | sh
```

Then run `weft` in a repository. Later, `weft update` installs the newest
release, and the running Weft takes it once no agent is working.

## Docs

Setting up your harness, the whole loop from Ask to Seal, and Weft on your
phone are at **[docs.getfab7.com](https://docs.getfab7.com)**.

## Development

```sh
cargo build
cargo nextest run --workspace   # cargo install cargo-nextest; or plain `cargo test`
```

[nextest](https://nexte.st) runs each test in its own process, so RingFrame's
tests that set `HOME` run side by side. `.config/nextest.toml` caps the
integration tests, which each start a weftd, at four at once. Dev builds carry
no debug info (`Cargo.toml`): on macOS it would keep every object file in
`target/debug/deps`, and a build slows once that directory holds hundreds of
thousands. `cargo clean` resets it.

`./bin/reinstall-local` builds both binaries, installs them, syncs RingFrame's
configuration and Weft's harness files from a `fab7` checkout beside this one,
and restarts the daemon, so a change is tested as users will get it. Set up
each harness's plugin from Weft's RingFrame view (`U`).

`crates/weft-core/words.json` holds the words the TUI and the web app share.
`cargo test` fails when it is stale; `WEFT_WRITE_WORDS=1 cargo test -p
weft-core --test words_json` writes it again.

The docs, and the scenarios that draw Weft's screens for them, live in
[fab7hq/docs](https://github.com/fab7hq/docs), pinned to a Weft release.
`examples/` holds probes that check one mechanism against a real harness, by
hand.

## Releases

Tagged `vX.Y.Z`; what changed is in [Releases](https://github.com/fab7hq/weft/releases).
Weft, RingFrame and the `rf` plugins in [fab7hq/fab7](https://github.com/fab7hq/fab7)
share one version.

## License

Apache-2.0. Part of [Fab7](https://getfab7.com).
