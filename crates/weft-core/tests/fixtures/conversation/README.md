# Conversation fixtures

One transcript per harness, as the harness wrote it in HostLab (the local
model, a scripted session: a reply, a shell step that succeeds, one that
fails, a long paste, an interrupt), trimmed: every entry kept in order,
strings past 300 characters cut, the signed-in account's ids replaced. Each
is the release its harness file's `[conversation]` `verified` names, and
`src/conversation.rs` tests it.

| File | Release | Entries drawn | Entries not drawn |
| --- | --- | --- | --- |
| `claude-code.jsonl` | Claude Code 2.1.296 | `user` with `origin.kind = human` (the person); `assistant` text blocks (replies); `assistant` `tool_use` blocks (steps); `user` `tool_result` blocks (outcomes) | thinking, attachments, `system`, `mode`, `permission-mode`, `file-history-snapshot`, `last-prompt`, `ai-title`, the interruption's `user` text |
| `codex.jsonl` | codex-cli 0.162.1 | `event_msg` `item_completed` with `UserMessage`, `AgentMessage`, `CommandExecution` (its status says failed) | `session_meta`, `response_item` (the model's input), `Reasoning`, `turn_context`, `world_state`, `token_count`, `task_started`, `task_complete` |
| `grok.jsonl` | Grok Build 1.0.52 | `session/update` `user_message_chunk`, `agent_message_chunk`, `tool_call` (steps), `tool_call_update` with status completed or failed (outcomes) | `agent_thought_chunk`, `hook_execution`, `turn_completed`, `current_mode_update`, `background_tasks`, a `tool_call_update` with no status |

Antigravity has none: its transcript gives a command's result only as free
text, joined to its call by nothing.

**A new release** is checked by hand by a maintainer: record a session again
with `sandbox/hostlab/probes/transcripts/probe.mjs <lane>`, trim it the same
way, and compare the entry types and fields its `[conversation]` reads. When
they are unchanged, `verified` moves to the new release; when they changed,
the mapping and the fixture change with it.
