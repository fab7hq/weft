# Security policy

## Supported versions

Security fixes target the latest 0.2.x release. For unreleased changes, include
the source commit when reporting an issue.

## Report privately

Use [GitHub private vulnerability reporting](https://github.com/fab7hq/weft/security/advisories/new).
Include the version or commit, host version, reproduction steps, and impact.
Remove credentials, private prompts, source, and unrelated logs. Do not open a
public issue for an unpatched vulnerability.

## Data and trust

RingFrame stores Ask text, compiled prompts, evaluations, decisions, and hook
metadata under `.fab7/rf/`. Explicit RingFrame invocations are captured in
full; ordinary prompts retain only digests, byte counts, and metadata.
Git-ignore is a sharing default, not encryption. Review records before export.

The CLI uses digests and an append-only ledger to check local consistency.
These are not signatures or protection against someone who can rewrite both
artifacts and ledger. Actor identity is caller-supplied, not authenticated by
RingFrame.

Eval reads Git state and records model judgements, and it runs checks:
builds, tests and linters that an Ask, or a plan step it names, states in
backticks (`cargo test`, `npm test`, `make check`, `pytest`, …). At most four
per Eval, each started by RingFrame itself in the project root as you, with
your environment and network, for up to ten minutes, with no host permission
prompt and no sandbox. A check is a plain argument list, never a shell line,
and only a known runner with a check verb is taken as one; but a build or
test runs the project's own code, so in a repository you do not trust, read
what an Eval will run before you start it. Skill instructions do not sandbox
the host. Host permissions remain responsible for the agents' tool access and
effects. Neither an Eval verdict nor an accepted Seal guarantees correctness
or authorizes deployment.

Weft types into agents only what can be said: a prompt or command read from
a project's files that holds escape sequences or other control characters is
refused, and a session id is used on a resume command line only when it is a
plain id. Weft's Remote is reached through a public TryCloudflare address;
Cloudflare's edge sees its traffic decrypted, and a device gets in only with
an assertion from Fab7's account service and a yes on the machine.
