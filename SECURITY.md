# Security policy

## Supported versions

Security fixes target the latest 0.1.x release. For unreleased changes, include
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

Eval reads Git state and records model judgements; it does not run project
builds or tests. Skill instructions do not sandbox the host. Host permissions
remain responsible for tool access and effects. Neither an Eval verdict nor
an accepted Seal guarantees correctness or authorizes deployment.
