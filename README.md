<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/weft-banner-dark.svg">
    <img alt="Weft" src="assets/weft-banner-light.svg" width="378">
  </picture>
</p>

- **RingFrame** — your agentic workflow in three acts: ask, eval, seal.
  Everything is an Ask.
- **Weft** — RingFrame in your terminal: your agents in panes, your Asks in
  one list.

Named after woven cloth: RingFrame is the warp, the fixed record; Weft is the
thread carried across it.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/fab7hq/weft/main/install.sh | sh
```

Then run `weft` in a repository. Everything else, from setting up your harness
to the whole loop from Ask to Seal, is in the docs at
**[docs.getfab7.com](https://docs.getfab7.com)**. Changes in each version are
in [Releases](https://github.com/fab7hq/weft/releases).

## Development

```sh
cargo build
cargo test
```

The docs, and the scenarios that draw Weft's screens for them, live in
[fab7hq/docs](https://github.com/fab7hq/docs), pinned to a Weft release: a
change to what Weft draws shows up there when the pin moves. `examples/` holds
probes that check one mechanism against a real harness, by hand.

`./bin/reinstall-local` builds both binaries, installs them, syncs the
configuration from a `fab7` checkout beside this one, and reinstalls the
plugins, so a change is tested as users will get it:

```sh
./bin/reinstall-local           # every harness
./bin/reinstall-local <harness>  # one harness
./bin/reinstall-local none      # binaries and configuration only
```

## License

Apache-2.0. Part of [Fab7](https://getfab7.com).
