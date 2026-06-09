# **OB**~~servabilit~~**Y**

A live, per-agent activity feed for [Claude Code](https://claude.com/claude-code) — recovers the stdout and stderr your agents threw away (`2>/dev/null`, …) and shows it in a togglable side panel, without spending an agent token on the lookup.

Fittingly, **oby** is what's left of *observability* after the middle bytes get discarded. The wrapper does the same thing your agents' pipelines do, but keeps the parts that fell on the floor.

## Status

**v0.2.2.** Bash + Read capturers, complete discard-recovery (`2>/dev/null`, `| grep`, `| head`, `| tail`, `> FILE`, `>> FILE`), execution tracing under both bash (`BASH_XTRACEFD=9`) and zsh (PS4 sentinel + awk demuxer), bounded pre-filter capture (configurable context windows for grep/head/tail — see env vars below), multi-agent routing, composes with other PreToolUse hooks, mouse + keyboard scrollback, top-style metrics bar. Empirically verified against Claude Code `2.1.x`. Implementation plan: [`docs/plans/v0.2.2.md`](docs/plans/v0.2.2.md). Architecture: [`docs/architecture.md`](docs/architecture.md).

## How it works

- A `PreToolUse` hook rewrites Bash commands to tee the bytes that would have been discarded into a per-agent unix socket — the agent's tool result stays byte-identical to what it would have been.
- The rewrite now also detects `| grep`, `| head`, `| tail`, and `> FILE` / `>> FILE` redirects. For each, a tee is injected so the bytes the agent's pipeline would have discarded (pre-filter output, post-truncation lines, file content) are captured as side-channel chunks. `set -x` via `BASH_XTRACEFD` adds a per-command trace stream for multi-statement scripts.
- The harness-injected `agent_id` field routes each subagent's commands to its own stream. Main agent and concurrent subagents are cleanly separated.
- The wrapper owns the terminal: claude in one view, the activity feed in the other, one hotkey to swap.
- A small plugin trait (`Capturer`) lets each observed tool declare its own renderer in one file in the source tree. Adding a capturer is one PR + one line in the registry.

## Install

Homebrew (macOS):

```bash
brew install brcourt/tap/oby
```

Cargo:

```bash
cargo install oby-cli oby-hook oby-tee
```

From source:

```bash
git clone https://github.com/brcourt/oby
cd oby
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

Then, in any of those:

```bash
oby install              # writes hook config to ~/.claude/settings.json
oby claude               # launches claude inside the oby wrapper
```

Run plain `claude` (no wrapper) for an unobserved session — the hook env-gates itself and no-ops.

## CLI

| Command | What it does |
|---|---|
| `oby claude [...args]` | Launch Claude inside the wrapper. Args pass through to `claude`. Default subcommand: `oby [args]` is treated as `oby claude [args]`. |
| `oby install` | Write `oby-hook` entries to `~/.claude/settings.json` (PreToolUse / PostToolUse / PostToolUseFailure for Bash & Read, plus SubagentStop). Idempotent. |
| `oby probe latest` | Print the socket dir of the most recent running oby session. |
| `oby probe smoke [--socket-dir DIR]` | Inject synthetic hook traffic (entries, chunks, updates) into a running session. Validates the wrapper end-to-end without needing a live Claude session. |

## Feed view keybinds

When the feed is showing (Ctrl-G from claude):

| Key | Action |
|---|---|
| `Ctrl-G` | Toggle back to the claude session |
| `←` / `→` | Switch between agents (main + each subagent) |
| `↑` / `↓` | Scroll one line |
| `PgUp` / `PgDn` | Scroll 10 lines |
| `Home` / `g` | Jump to oldest entry |
| `End` / `G` | Return to live tail (auto-follow) |
| Mouse wheel | Scroll 3 lines per tick (hold Shift to bypass capture and use terminal-native text selection) |
| `d` or `x` | Delete the selected agent from the picker (refuses to delete `main`). Selects the agent immediately to the right; spam `d` to clear out finished subagents. |
| `q` | Quit oby (also terminates the wrapped claude session) |

The activity title shows `[scrolled +N lines · End/G to tail]` while paused, so you always know whether you're following live or browsing history.

## Status dots in the agent picker

- 🟢 `●` — agent is alive (still might emit events).
- 🔴 `●` — agent is destroyed; its `SubagentStop` hook fired. Safe to delete.

Main is always green while oby is running — it's the session itself, not a subagent.

## Top-bar metrics

The line above the title bar is a `top`-style snapshot for troubleshooting:

```
agents N · entries N · updates N (M orph) · bytes N · conns N · err A/P · fd N · up Nm
```

| Field | Meaning | Watch for |
|---|---|---|
| `agents` | Live agent ring count | Mismatch with subagent count = routing bug |
| `entries` | PreToolUse Entries received | Flat while claude works = hook→wrapper broken |
| `updates (M orph)` | PostToolUse Updates received + orphan count | Many orphans = ordering races |
| `bytes` | Total live bytes received on agent sockets | Flat while Bash runs = `oby-tee` or rewrite broken |
| `conns` | Total agent connections opened | Should grow with bytes |
| `err A/P` | accept_errors / parse_errors | Non-zero A = FD pressure; non-zero P = malformed payloads |
| `fd` | Process FD count | Climbing past ~200 = FD exhaustion incoming (listener self-heals) |
| `up` | Wrapper uptime | Sanity check |

## Environment variables

| Variable | Set by | Effect |
|---|---|---|
| `OBS_ACTIVE` | `oby claude` | Marks a wrapped session. `oby-hook` env-gates on this — runs only when set. |
| `OBS_SOCKET_DIR` | `oby claude` | Path to the per-session unix socket dir (`$XDG_RUNTIME_DIR/obi/<uuid>/` or `/tmp/obi/<uuid>/`). Inherited by claude → bash → `oby-tee`. |
| `OBS_HOOK_LOG` | You | Path to a JSON-lines log file. `oby-hook` appends one line per phase per invocation (`recv`, `pre_entry`, `pre_rewrite`, `post_update`, `send_ok`, `send_connect_err`, etc.). Useful for diagnosing what CC sends and what the hook does with it. Off by default. |
| `OBS_WRAPPER_LOG` | You | Same idea on the wrapper side. Logs every received Entry/Update, agent socket open/close (with bytes), accept errors, parse failures. Off by default. |
| `OBS_COMPOSE_DELAY_MS` | You | Override the 200ms post-compose sleep `oby-hook` uses to win CC's "last hook to finish wins" race against peer PreToolUse rewriters (rtk etc.). Default 200; set lower if you're confident nothing else is composing on the same matcher. |
| `OBS_COMPOSING` | `oby-hook` (internal) | Recursion guard. When `oby-hook` invokes a peer hook for composition, this var is set on the child. If `oby-hook` ever sees it on its own startup, it skips composition. Don't set manually. |

**Pre-filter window sizes (v0.2.2+):**
- `OBS_GREP_BEFORE_LINES` (default `3`) — lines before each grep match in `[stdout-pre-grep]`.
- `OBS_GREP_AFTER_LINES` (default `3`) — lines after each grep match.
- `OBS_HEAD_PEEK_LINES` (default `3`) — lines past head's truncation boundary in `[stdout-pre-head]`.
- `OBS_TAIL_PEEK_LINES` (default `3`) — lines before tail's boundary in `[stdout-pre-tail]`.

Values are clamped to `0..=1000`. Set to `0` to disable a stream's capture window. Read at PreToolUse hook invocation time, so changes take effect for the next command (no need to restart `oby claude`).

## Coexistence with other PreToolUse hooks

If you already have a PreToolUse rewriter installed (e.g. [rtk](https://github.com/anthropics/rtk)), `oby-hook` composes with it automatically. CC runs hooks in parallel and the last to finish wins the `updatedInput` race. `oby-hook` reads `~/.claude/settings.json`, invokes peer hooks itself in array order with the same payload, applies each emitted `updatedInput` to a working copy of `tool_input`, wraps the composed command with its own process substitution, then sleeps `OBS_COMPOSE_DELAY_MS` so its emit reliably wins. Both your existing rewriter AND oby's chunk capture run.

## Diagnostics

When the feed appears stuck, in order of cheapest to most expensive:

1. **Glance at the top-bar metrics.** If `up Nm` is ticking, the run loop is alive. If `entries N` is incrementing, the wrapper is receiving. If `fd N` is climbing past ~200, you're approaching the per-process FD limit — the listener self-heals via accept retry but symptoms will appear first there.
2. **Enable both debug logs and reproduce.**
   ```bash
   OBS_HOOK_LOG=/tmp/oby-hook.log OBS_WRAPPER_LOG=/tmp/oby-wrapper.log oby claude
   ```
   Then compare:
   ```bash
   jq -c 'select(.event | startswith("send"))' /tmp/oby-hook.log   # did delivery succeed?
   tail -50 /tmp/oby-wrapper.log                                    # what did the wrapper see?
   ```
   If the hook log shows `send_connect_err` lines, the wrapper's listener went deaf. If the hook log shows `send_ok` but the wrapper log is silent, the message reached the socket but isn't being read.
3. **Isolate the wrapper from the hook with `oby probe`.**
   ```bash
   # In one terminal:
   oby claude
   # In another:
   oby probe smoke
   ```
   If the synthetic smoke entries render correctly, the wrapper (sockets, ring, TUI) is sound — bug is on the hook side.

## Architecture

Four crates in a Cargo workspace:

| Crate | Responsibility |
|---|---|
| `oby-core` | Trait + types. No I/O. The plugin / wire-format contract. |
| `oby-tee` | In-pipeline helper. Reads stdin, opens a unix socket to the wrapper, streams bytes. Fail-open. |
| `oby-hook` | The binary CC invokes. Env-gates on `OBS_ACTIVE`, parses payloads, dispatches to the matching `Capturer`, composes with peer hooks, marshals the rewrite back to CC. |
| `oby-cli` (binary `oby`) | The wrapper-daemon. Owns the pty, runs claude inside it, listens on per-agent unix sockets and a control socket, paints the TUI, handles the hotkey toggle. |

End-to-end: CC fires PreToolUse → `oby-hook` dispatches to the capturer → capturer optionally rewrites the command to inject `oby-tee` → `oby-tee` streams bytes to a per-agent socket → wrapper's listener appends bytes to that agent's ring buffer → hotkey paints the buffer full-screen.

See [`docs/architecture.md`](docs/architecture.md) for the full design.

## Known limitations (v0.2)

- Only Bash and Read capturers ship. Edit, Write, Grep, Glob, Task, WebFetch tool calls don't show entries in the feed.
- Hotkey hardcoded to Ctrl-G, ring buffer to 500 entries. No TOML config yet (deferred to v0.3+).

## Non-goals (for now)

Web UI, cross-session persistence, external user-installable plugins, and Windows support are all deferred. The architecture is intentionally compatible with each (see §16 of the design doc); none are in the initial scope.

## License

MIT. See [`LICENSE`](LICENSE).
