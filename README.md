# oby

A live, per-agent activity feed for [Claude Code](https://claude.com/claude-code) — observe what your agents are *actually* doing, including the stdout/stderr they discard inside shell pipelines (`2>/dev/null`, `| grep`, `| head`), without spending a single agent token on it.

A wrapper (`alias claude="oby claude"`) owns your terminal; a hotkey toggles between your normal claude session and a full-screen activity feed. Each subagent gets its own routed stream, so parallel dispatch stays legible.

## Status

**Working PoC.** v0.1 ships Bash + Read capturers + `2>/dev/null` discard-recovery; see [`docs/plans/v0.1.md`](docs/plans/v0.1.md) for the implementation plan. Architecture documented in [`docs/architecture.md`](docs/architecture.md); design verified against Claude Code `2.1.142` (see Appendix A).

## How it works (briefly)

- A `PreToolUse` hook rewrites Bash commands to tee the bytes that would have been discarded into a per-agent unix socket — the agent's tool result stays byte-identical.
- The harness-injected `agent_id` field routes each subagent's commands to its own stream. Main agent and concurrent subagents stay cleanly separated.
- The wrapper owns the terminal: claude in one view, the activity feed in the other, one hotkey to swap.
- A small plugin trait (`Capturer`) lets each observed tool — Bash, Read, Edit, Grep, Task, … — declare its own renderer in one file in the source tree. Adding a new capturer is one PR + one line in the registry.

## Install (development build, v0.1)

```bash
git clone https://github.com/brcourt/oby
cd oby
cargo build --release
export PATH="$PWD/target/release:$PATH"
oby install              # writes hook config into ~/.claude/settings.json
oby claude               # launches claude inside the obi wrapper
```

Press **Ctrl-G** at any time to toggle between the claude session and the activity feed. In the feed: ←/→ switches between agents (main + each subagent), `q` quits, Ctrl-G goes back to claude.

Run plain `claude` (no wrapper) for an unobserved session — the hook env-gates itself and no-ops.

### Coexistence with other PreToolUse hooks

If you already have a PreToolUse rewriter installed (e.g. [rtk](https://github.com/anthropics/rtk)), `oby-hook` composes with it automatically. CC runs hooks in parallel and the last to finish wins the `updatedInput` race — `oby-hook` reads `~/.claude/settings.json`, invokes the peer hooks itself in array order, wraps the composed command with its own process substitution, then sleeps `OBS_COMPOSE_DELAY_MS` (default 200ms) so its emit reliably wins. Both your existing rewriter AND oby's chunk capture run.

## Known limitations (v0.1)

- The Bash capturer only neutralizes `2>/dev/null`. Other inner patterns (`| grep`, `| head`, `> FILE`) ship in v0.2.
- No xtrace / `set -x` — multi-statement scripts only surface outputs, not which command produced them.
- Only Bash and Read capturers ship. Edit, Write, Grep, Glob, Task, WebFetch tool calls don't show entries in the feed.
- Feed scrolling is unimplemented (auto-pins to bottom).
- Hotkey is hardcoded to **Ctrl-G**, ring buffer to 500 entries.

## Non-goals (for now)

Web UI, cross-session persistence, external user-installable plugins, and Windows support are all deferred. Execution tracing (`set -x` via `BASH_XTRACEFD`) and additional inner-pattern rewrites (`| grep`, `| head`, `> FILE`) ship in v0.2. The architecture is intentionally compatible with each (see §16 of the design doc); none are in the initial scope.

## License

MIT. See [`LICENSE`](LICENSE).
