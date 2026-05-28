# oby

A live, per-agent activity feed for [Claude Code](https://claude.com/claude-code) — observe what your agents are *actually* doing, including the stdout/stderr they discard inside shell pipelines (`2>/dev/null`, `| grep`, `| head`), without spending a single agent token on it.

A wrapper (`alias claude="oby claude"`) owns your terminal; a hotkey toggles between your normal claude session and a full-screen activity feed. Each subagent gets its own routed stream, so parallel dispatch stays legible.

## Status

**Design phase.** Architecture is documented in [`docs/architecture.md`](docs/architecture.md). Implementation has not started — the design has been empirically verified against Claude Code `2.1.142` (see Appendix A of the architecture doc), but no code lives here yet.

## How it works (briefly)

- A `PreToolUse` hook rewrites Bash commands to tee the bytes that would have been discarded into a per-agent unix socket — the agent's tool result stays byte-identical.
- Multi-statement scripts get **execution tracing** (`set -x` via `BASH_XTRACEFD`) so you see *which* commands ran, not just their output — useful when an agent writes a one-shot loop that touches many files without echoing anything.
- The harness-injected `agent_id` field routes each subagent's commands to its own stream. Main agent and concurrent subagents stay cleanly separated.
- The wrapper owns the terminal: claude in one view, the activity feed in the other, one hotkey to swap.
- A small plugin trait (`Capturer`) lets each observed tool — Bash, Read, Edit, Grep, Task, … — declare its own renderer in one file in the source tree. Adding a new capturer is one PR + one line in the registry.

## Non-goals (for now)

Web UI, cross-session persistence, external user-installable plugins, and Windows support are all deferred. The architecture is intentionally compatible with each (see §16 of the design doc); none are in the initial scope.

## License

MIT. See [`LICENSE`](LICENSE).
