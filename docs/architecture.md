# obi-tee — Architecture

**Date:** 2026-05-27
**Status:** Design. Pre-implementation.

A live, per-agent activity feed for [Claude Code](https://claude.com/claude-code) that observes what your agents are actually doing — including the stdout/stderr they discard inside shell pipelines — without spending a single agent token on it.

---

## 1. Problem

Agents save tokens by hiding output: `cmd 2>/dev/null`, `cmd | grep ERROR`, `cmd | head`, `cmd >/dev/null 2>&1`. For the agent's token budget that's the right thing to do. For a *human* trying to understand what an agent did — whether errors are being swallowed, whether subagents are doing the right thing, where time went — it destroys the signal you need.

A naïve "split the terminal" fix doesn't work: the discard happens *inside* the shell pipeline, before any byte reaches the harness or your terminal. You cannot recover bytes that were never written anywhere. The fix must live at the *command-execution* layer.

## 2. Core insight

The shape of the problem dictates the shape of the solution:

1. **The agent's bytes are destroyed inside the pipeline.** To see them, the command must be changed *before* the discard happens — interception at command-construction time.
2. **Claude Code's `PreToolUse` hook can rewrite a command.** Hooks fire before the command runs, receive its `tool_input`, and can return a modified `tool_input` via `hookSpecificOutput.updatedInput`. This is the interception point.
3. **Subagents are identified natively.** The `PreToolUse` payload includes `agent_id` and `agent_type` *only* when the call originates inside a subagent. Combined with absence-means-main-agent, this is a 100%-reliable, harness-injected routing key. Empirically verified (Appendix A).
4. **The reader, not the file descriptor, provides durability.** Pipes are ephemeral; files give you durability for free; a daemon provides push. For the live transport, a per-agent unix socket draining into the wrapper gets you live + non-blocking + multi-stream without disk. Cross-session persistence is a future addition on top.

Three derived rules that anchor every design choice:

- **Never break the agent.** The agent's tool result must be byte-identical to passthrough.
- **Never block the agent.** Backpressure from the observer must never reach the agent's command.
- **Never require the agent's cooperation.** No "tell agents to self-identify"; the harness already gives us what we need.

## 3. Goals & non-goals

### Goals (PoC)

- Live activity feed of every tool call an agent makes — Bash with full output, plus Read / Edit / Write / Grep / Glob / Task / WebFetch as structured timeline entries.
- Per-agent streams. Main agent and each subagent get their own activity log, routed by `agent_id`. Concurrent subagents stay separate.
- Single-window UX. A wrapper (`alias claude="obi-tee claude"`) owns the terminal; a hotkey toggles between the agent's claude TUI and the activity feed.
- Plugin-based capturers. Each tool's observation logic is one module in the source tree. Adding a new capturer is one PR + one line in the registry.
- Open-source (MIT), single static Rust binary distribution.

### Non-goals (PoC)

- Web UI.
- Cross-session persistence. The activity log lives only as long as the session.
- External user-installable plugins (no plugin ABI; level-2 pluggability only).
- Recovery of output discarded by means the rewriter cannot recognize (eval, exec-redirections, exotic shell constructs). The fallback is "outer wrap only," which still captures non-discarded bytes.

### Future, after PoC

- Cross-session persistence via a split-out daemon and disk-backed history (seam preserved — §16).
- Web viewer as an alternative to the in-terminal toggle (same daemon).

---

## 4. Architecture at a glance

```
┌────────────────────────────────────────────────────────────────────┐
│  Terminal (owned by obi-wrapper)                                   │
│                                                                    │
│  ┌──────────────────────────────────────────────────────────────┐  │
│  │  claude TUI                       (toggled view: activity)   │  │
│  │  (rendered into a pty obi-wrapper allocates)                 │  │
│  └──────────────────────────────────────────────────────────────┘  │
│                                                                    │
│  hotkey ──► swap rendered view (claude ↔ activity feed)            │
└────────────────────────────────────────────────────────────────────┘
                          │
            owns pty +    │ owns terminal + listens on sockets +
            sets env      │ keeps per-agent ring buffers
                          ▼
                ┌──────────────────────┐
                │      obi-wrapper     │
                │  (the wrapper+daemon │
                │   in one process)    │
                └─────────┬────────────┘
                          ▲
                          │ unix sockets
                          │ $OBS_SOCKET_DIR/<agent_key>.sock
                          │
              ┌───────────┴────────────┐
              │                        │
   ┌──────────┴────────┐     ┌─────────┴──────────┐
   │   agent's Bash     │     │   subagent's Bash  │
   │     command        │     │      command       │
   │ ┌────────────────┐ │     │ ┌────────────────┐ │
   │ │ rewritten by   │ │     │ │ rewritten by   │ │
   │ │   obi-hook     │ │     │ │   obi-hook     │ │
   │ └───────┬────────┘ │     │ └───────┬────────┘ │
   │         │          │     │         │          │
   │       │ tee ──►    │     │       │ tee ──►    │
   │         ▼          │     │         ▼          │
   │       obi-tee      │     │       obi-tee      │
   │   (fail-open helper)     │   (fail-open helper)│
   └────────────────────┘     └────────────────────┘
```

`claude` is launched by `obi-wrapper`, which sets `OBS_ACTIVE=1` + `OBS_SOCKET_DIR=…` in env and then execs claude in the allocated pty. `obi-hook` is installed once, globally, in `~/.claude/settings.json`; it is env-gated and no-ops when `OBS_ACTIVE` is unset, so running plain `claude` is byte-for-byte unaffected.

End-to-end data flow for one Bash command:

1. Agent calls Bash with `cmd | grep ERROR`.
2. CC fires PreToolUse hook with the payload (including `agent_id` if subagent).
3. `obi-hook` looks up the Bash capturer, calls `pre_rewrite(ctx, input)`.
4. The Bash capturer returns a rewritten command: `cmd | tee >(obi-tee --agent KEY --tool-use-id TID --stream stdout-piped >/dev/null) | grep ERROR`.
5. `obi-hook` also sends a `DisplayEntry` (the command, headline, agent_key, pending status) to the wrapper-daemon via its control socket.
6. `obi-hook` emits the JSON envelope on stdout to mutate `tool_input.command`. CC runs the rewritten command.
7. Inside the rewritten pipeline, `obi-tee` connects to `$OBS_SOCKET_DIR/<agent_key>.sock`, identifies itself with the tool_use_id, streams bytes.
8. The wrapper appends the bytes to the entry's `LiveStream` body, bound by tool_use_id.
9. Command finishes. PostToolUse fires. The Bash capturer's `render_post` updates status (Ok / Error). The wrapper finalizes the entry.

---

## 5. Components

### 5.1 `obi-wrapper` — the wrapper + daemon, collapsed

A single binary that:

- Owns the real terminal: raw mode, alternate screen.
- Allocates a pty, runs `claude` (with whatever args the user passed) inside it.
- Forwards keystrokes to claude's pty, except a reserved hotkey.
- Listens on a per-session unix socket directory: one socket per agent_key.
- Keeps in-memory ring buffers (per agent_key) of `DisplayEntry`s.
- On hotkey, swaps the rendered view between (a) claude's pty output and (b) a full-screen activity feed for the currently-selected agent.
- On flip-back, sends `SIGWINCH` to claude to force a repaint (standard Ink/React TUI behavior).
- Maintains a per-agent picker in the feed view (one entry per `agent_id`, labeled by `agent_type`).

The wrapper is the listener — no external daemon. This is the **collapsed** topology (PoC). The seam to a future split daemon is in §16.

### 5.2 `obi-hook` — the binary CC invokes

Tiny binary configured in `~/.claude/settings.json`, with one entry per observed tool:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "obi-hook" }] },
      { "matcher": "Read", "hooks": [{ "type": "command", "command": "obi-hook" }] },
      { "matcher": "Edit", "hooks": [{ "type": "command", "command": "obi-hook" }] }
    ],
    "PostToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "obi-hook" }] },
      { "matcher": "Read", "hooks": [{ "type": "command", "command": "obi-hook" }] },
      { "matcher": "Edit", "hooks": [{ "type": "command", "command": "obi-hook" }] }
    ]
  }
}
```

One entry per observed tool (truncated above — full install adds Write, Grep, Glob, Task, WebFetch). `obi-hook` dispatches internally by `tool_name` to the matching capturer; the matchers just select what fires the hook. If CC's current version supports a wildcard matcher (e.g. `".*"` regex), the install can collapse to one entry per event — to be verified during implementation against the target CC version. Explicit per-tool matchers are empirically confirmed to work (RTK uses this form).

On every invocation:

1. **Env-gate.** If `OBS_ACTIVE` is unset, exit 0 immediately.
2. **Parse stdin.** Parse failure → exit 0 (never break the agent).
3. **Look up capturer.** `registry::enabled(&config).find(|c| c.tool_name() == ctx.tool_name)`.
4. **Dispatch.** PreToolUse → `render_pre`, then `pre_rewrite`. PostToolUse → `render_post`.
5. **Send display entries to the wrapper.** Fire-and-forget over the wrapper's control socket. Failure = silent.
6. **Emit rewrite JSON** if PreToolUse and Rewrite returned. Otherwise stdout is empty.

### 5.3 `obi-tee` — the in-pipeline helper

A small Rust binary (~150 lines) used only by the Bash capturer's rewrites:

```
obi-tee --agent KEY --tool-use-id TID --stream NAME [--socket-dir DIR]
```

Reads stdin → connects to `$OBS_SOCKET_DIR/$KEY.sock` → writes a tiny framing header (`tool_use_id`, `stream`, `started_at`) → forwards stdin bytes → closes on EOF.

**Fail-open invariants** (load-bearing — pipefail safety depends on these):

- Connect fails → silently drain stdin to EOF, exit 0.
- Listener disconnects mid-stream → drain remaining stdin, exit 0.
- Any internal error → brief log to stderr (which is itself probably tee'd back to the user!), exit 0.

The agent's command can never see a non-zero exit from `obi-tee`.

### 5.4 `obi-core` — the trait + types

The Rust library every capturer is written against. Contains:

- `HookContext` (mirrors the PreToolUse payload).
- `HookPayload` (ctx + tool_input + optional tool_response).
- `Capturer` trait (the contribution API).
- `DisplayEntry`, `EntryBody`, `EntryStatus`, `DisplayEntryUpdate`.
- `RewriteDecision`.
- `builtin_capturers()` — the registry.

---

## 6. The hook payload (empirically verified)

Verified in CC 2.1.142 via an isolated headless probe (Appendix A). Field availability:

| Field              | Type                              | Main agent | Subagent                  | Use                              |
| ------------------ | --------------------------------- | ---------- | ------------------------- | -------------------------------- |
| `session_id`       | string                            | present    | present (same)            | not a routing key                |
| `transcript_path`  | path                              | present    | present (same)            | not a routing key                |
| `cwd`              | path                              | present    | present                   | useful for display               |
| `hook_event_name`  | `"PreToolUse"` \| `"PostToolUse"` | present    | present                   | dispatch                         |
| `tool_name`        | string                            | present    | present                   | dispatch to capturer             |
| `tool_use_id`      | string                            | present    | present                   | **Pre↔Post correlation**         |
| `tool_input`       | object                            | present    | present                   | tool-specific                    |
| `tool_response`    | object                            | (Post only)| (Post only)               | tool-specific                    |
| `permission_mode`  | string                            | sometimes  | sometimes                 | informational                    |
| `effort`           | object `{level}`                  | present    | present                   | informational                    |
| **`agent_id`**     | string                            | **absent** | present, stable, distinct | **the routing key**              |
| **`agent_type`**   | string                            | **absent** | present                   | label (e.g. `"general-purpose"`) |

The verification dispatched two concurrent general-purpose subagents (ALPHA, BETA), each running two `echo` commands. `agent_id` was stable across each subagent's commands and distinct between them.

### Hook output (for command mutation)

PreToolUse hooks mutate `tool_input` by emitting:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "updatedInput": { "command": "..." }
  }
}
```

Verified both via docs and binary string corroboration (`updatedInput`×198, `hookSpecificOutput`×95, `hookEventName`×56 in the 2.1.142 binary).

---

## 7. Activation & safety model

### 7.1 Env-gate

The hook is installed *globally* in `~/.claude/settings.json` but gated by an env var only `obi-wrapper` sets:

```
obi-wrapper:
  export OBS_ACTIVE=1
  export OBS_SOCKET_DIR=$XDG_RUNTIME_DIR/obi/<session-id>     # macOS fallback: /tmp/obi/<session-id>
  exec claude "$@"
```

When you run plain `claude` (no wrapper), `OBS_ACTIVE` is unset; the hook exits 0 immediately and CC behaves byte-for-byte as if the hook didn't exist. **Installation is one-time global; activation is per-launch.**

### 7.2 Fail-open

Every observer-side path is fail-open. Concretely:

- `obi-hook` exits 0 on every error.
- `obi-tee` exits 0 on every error and drains its stdin to EOF.
- Listener gone, stale socket, race during wrapper startup → `obi-tee` silently consumes bytes.

**Why this is load-bearing.** The canonical rewrite pattern uses `tee | obi-tee` inside the agent's pipeline. Under `set -o pipefail` (which we verified zsh supports), a non-zero exit anywhere in the pipeline propagates to the agent. If `obi-tee` ever exited non-zero, "observability" would convert into "this agent's command now errors when the observer is unhappy." That can never happen.

---

## 8. Routing: `agent_id`

The routing key is:

```rust
ctx.agent_id.as_deref().unwrap_or("main")
```

A stable, harness-injected identifier for which agent issued a tool call. Established by Appendix A:

- Main agent: `agent_id` absent → routes to `"main"`.
- Subagent: `agent_id` is a stable string, identical across all of that subagent's commands.
- Concurrent subagents: distinct `agent_id`s.

For cross-session uniqueness (if ever needed): `(session_id, agent_id)` is the global key. Within-session, `agent_id` alone suffices.

---

## 9. Plugin model: capturers

**Level 2:** in-tree capturers, registry-driven, config-toggleable, no plugin ABI. Adding a new capturer = one file + one line in the registry. Open-source contribution path = PR.

### 9.1 Trait

```rust
pub trait Capturer: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn tool_name(&self) -> &'static str;
    fn subscribes_to(&self) -> &'static [HookEvent] { &[HookEvent::Pre, HookEvent::Post] }

    fn pre_rewrite(&self, _ctx: &HookContext, _input: &Value) -> RewriteDecision {
        RewriteDecision::Passthrough
    }

    fn render_pre(&self, ctx: &HookContext, input: &Value) -> Option<DisplayEntry>;

    fn render_post(
        &self, _ctx: &HookContext, _input: &Value, _response: &Value,
    ) -> Option<DisplayEntryUpdate> { None }
}
```

Three deliberate calls:

- **`tool_input` / `tool_response` stay raw `serde_json::Value`.** Capturers locally derive small typed structs (`#[derive(Deserialize)] struct BashInput { command: String, … }`) for the fields they care about. Keeps the trait object-safe and CC-version-tolerant.
- **`pre_rewrite` is optional and defaults to passthrough.** Only the Bash capturer overrides it. Every other capturer is a pure observer.
- **`RewriteDecision::Rewrite(Value)` hides CC's exact hook-output JSON.** Marshaling to `hookSpecificOutput.updatedInput` lives in `obi-hook`, not the capturer. Shields capturers from CC's schema drift.

### 9.2 Display entry types

```rust
pub struct DisplayEntry {
    pub agent_key:   String,
    pub tool_use_id: String,
    pub tool:        &'static str,
    pub timestamp:   SystemTime,
    pub headline:    String,
    pub body:        EntryBody,
    pub status:      EntryStatus,
}

pub enum EntryBody {
    None,
    Text(String),
    LiveStream { tool_use_id: String },   // bound by id when obi-tee connects
    Diff(DiffBlock),                       // for Edit / Write
}

pub enum EntryStatus { Pending, Ok, Error }

pub struct DisplayEntryUpdate {
    pub tool_use_id: String,
    pub status:      EntryStatus,
    pub append_body: Option<EntryBody>,
}
```

### 9.3 Registry

```rust
pub fn builtin_capturers() -> Vec<Box<dyn Capturer>> {
    vec![
        Box::new(BashCapturer::default()),
        Box::new(ReadCapturer),
        Box::new(EditCapturer),
        Box::new(WriteCapturer),
        Box::new(GrepCapturer),
        Box::new(GlobCapturer),
        Box::new(TaskCapturer),
        Box::new(WebFetchCapturer),
        // contribute new ones here
    ]
}
```

Filtered at startup by `[capture.<name>].enabled` in config. First-match-wins on `tool_name` (no overlap for v1).

### 9.4 Capturer mapping

| Capturer | uses `pre_rewrite` | `EntryBody` shape                                                            |
| -------- | ------------------ | ---------------------------------------------------------------------------- |
| Bash     | yes — discard-tee rewrite (§10) | `LiveStream{tool_use_id}`                                       |
| Read     | no                 | Pre: `None`; Post: `Text("Read 1842 lines, 47 KB")`                          |
| Edit     | no                 | Pre: `Diff(DiffBlock)` derived from `old_string`→`new_string`                |
| Write    | no                 | Pre: `Diff(DiffBlock)` (first-N-line preview); Post: status                  |
| Grep     | no                 | Post: `Text("3 matches in 2 files")`                                         |
| Glob     | no                 | Post: `Text("N paths")`                                                      |
| Task     | no                 | Pre: `Text(prompt[..200])`; Post: subagent result summary                    |
| WebFetch | no                 | Pre: `Text(url)`; Post: response summary                                     |

Task is worth noting: a Task dispatch is logged in the *main agent's* stream (the dispatch is the main agent's tool call). The subagent's own activity is a separate stream that auto-appears under its own `agent_id`. The picker shows both; the human cross-links by time + agent_type.

---

## 10. The Bash capturer's `pre_rewrite` grammar

The one place in the system where real shell knowledge lives. Bounded to this file.

### 10.1 Shell prerequisites

Empirically verified: CC executes Bash-tool commands in the user's login shell (zsh on macOS in 2.1.142). zsh supports the constructs we rely on:

- Process substitution `>(…)`.
- The canonical `cmd | tee >(sink) | filter` shape.
- `2> >(sink)` for stderr.
- `set -o pipefail`.

Bash supports these too (identical syntax). **Fish and POSIX `sh` do not.** The Bash capturer fingerprints the shell at rewrite time (presence of `$BASH_VERSION` or `$ZSH_VERSION`, or `$SHELL` basename) and **passes through unmodified if neither bash nor zsh**. No observability for that command, but no breakage either.

### 10.2 Rewrite shapes

Below, `T="obi-tee --agent $KEY --tool-use-id $TID --stream"`. All process subs end with `>/dev/null` to silence obi-tee's own (empty) stdout.

**Outer wrap (always applied):**

```
{ <inner> ; } > >(tee >($T stdout >/dev/null)) 2> >(tee >($T stderr >/dev/null) >&2)
```

Duplicates the *final* stdout and stderr to obi-tee while preserving the agent's FDs 1/2 byte-identically (the inner `tee` writes to its own stdout, which inherits from the parent shell — landing on the original FD).

**Inner pattern rewrites (when matched):**

| Original                                | Rewrite                                                                                       |
| --------------------------------------- | --------------------------------------------------------------------------------------------- |
| `2>/dev/null`                           | `2> >($T stderr-discarded >/dev/null)`                                                        |
| `>/dev/null` or `1>/dev/null`           | `1> >($T stdout-discarded >/dev/null)`                                                        |
| `&>/dev/null` / `>/dev/null 2>&1`       | both of the above                                                                             |
| `cmd \| grep PAT`                       | `cmd \| tee >($T stdout-piped >/dev/null) \| grep PAT`                                        |
| `cmd \| head/tail/awk/sed/cut/uniq/sort …` | same tee-injection before the filter                                                       |
| `cmd > FILE`                            | `cmd 1> >(tee FILE >($T stdout-to-file >/dev/null) >/dev/null)`                               |
| `cmd >> FILE`                           | same with `tee -a FILE`                                                                       |

Stream tags (`stderr-discarded`, `stdout-piped`, `stdout-to-file`, etc.) let the wrapper label the sub-streams of a single command distinctly in the rendered entry. All share the same `tool_use_id`.

### 10.3 Exit-code preservation

For every pattern above, the agent-visible exit code is unchanged:

- `cmd 2>/dev/null` → `cmd 2> >($T stderr-discarded)`: exit is `cmd`'s.
- `cmd | grep PAT` → `cmd | tee >($T) | grep PAT`: exit is grep's (last stage).
- The outer wrap's group `{ … ; }` returns the inner exit; process subs run alongside.

Under `set -o pipefail`, a failing stage would propagate. **`obi-tee`'s fail-open guarantee (always exit 0) is what makes the tee-injections pipefail-safe.**

### 10.4 Scanner

Not a full shell parser. A shell-aware tokenizer that classifies spans as: word, single-quoted string, double-quoted string, comment, redirection operator, pipe, subshell-open/close, command-substitution span. Walks tokens (skipping anything inside quotes or `$(…)` / backticks) and matches patterns at token boundaries. Common-case crates: `shell-words` for basic tokenization; ~200-line hand-rolled scanner for full fidelity.

### 10.5 Punts (documented behavior)

| Construct                            | Rewriter behavior                                                |
| ------------------------------------ | ---------------------------------------------------------------- |
| `eval "$str"`                        | Outer wrap only; do not recurse into the string                  |
| `exec 2>/dev/null; …`                | Outer wrap only                                                  |
| Heredocs (`<<EOF`)                   | Detect boundary, skip body, rewrite the command line normally    |
| Subshell `(…)` and group `{ …; }`    | Recurse into the inner content                                   |
| `$(…)` / backticks                   | Don't recurse; outer wrap captures their output                  |
| Compound chains (`&&`, `\|\|`, `;`)  | Recurse into each component                                      |
| Anything ambiguous                   | Outer wrap only                                                  |

The unifying rule: **when in doubt, outer wrap only — never break the command.**

---

## 11. Transport: per-agent unix sockets

### 11.1 Why sockets

- Live (no polling).
- Non-blocking from the agent's perspective (the wrapper drains; `obi-tee` is fail-open so backpressure can't reach the agent's command).
- Path-addressed: the socket *path* is the stable per-agent handle that survives the many fresh shells a session spawns.
- Many-writer-one-reader: each command spawns a new `obi-tee` that opens its own connection; the wrapper accepts many concurrent connections.

### 11.2 Layout

```
$OBS_SOCKET_DIR/                    # e.g. $XDG_RUNTIME_DIR/obi/<session-id>/
    main.sock                       # the main agent's stream
    <agent_id_1>.sock               # one per subagent that has run a tool
    <agent_id_2>.sock
    ...
    control.sock                    # obi-hook → wrapper control channel (entries, updates)
```

Socket directory created by `obi-wrapper` on launch and removed on exit (best-effort).

### 11.3 Framing

Per `obi-tee` connection, a small JSON header line followed by raw bytes:

```
{"v":1,"tool_use_id":"toolu_…","stream":"stderr-discarded","started_at":"2026-05-27T22:30:00Z"}
<raw bytes until EOF>
```

Control-channel messages (`obi-hook` → wrapper) are JSON-line framed:

```
{"v":1,"kind":"entry","agent_key":"main","tool_use_id":"…","tool":"bash","headline":"…","body":{"type":"live_stream","tool_use_id":"…"},"status":"pending","timestamp":"…"}
{"v":1,"kind":"update","tool_use_id":"…","status":"ok","append_body":null}
```

Versioned (`v:1`) for forward compatibility.

---

## 12. Display: wrapper-owned window + hotkey toggle

### 12.1 Layout

Single window, two full-screen views, hotkey toggle:

- **View A — claude.** The pty the wrapper allocated for claude is painted to the real terminal. Unchanged from running plain claude.
- **View B — activity feed.** Full-screen rendering of the currently-selected agent's `DisplayEntry` list. Per-agent picker at the bottom (one entry per `agent_id` seen this session, labeled by `agent_type`).

The wrapper reserves exactly one hotkey. Everything else passes through to claude.

### 12.2 Repaint on flip-back

On flipping from View B → View A, the wrapper sends `SIGWINCH` to claude. Claude (Ink/React) repaints on resize. Same trick tmux uses for window switching.

### 12.3 Reserved hotkey

TBD during implementation. Candidates: `Ctrl-G` (rarely used, no conflict with CC's bindings), an F-key, or a tmux-style prefix (`Ctrl-a` then a letter, configurable). The constraint: must not conflict with CC's built-in shortcuts.

---

## 13. Configuration

`~/.config/obi-tee/config.toml`:

```toml
[wrapper]
hotkey = "ctrl-g"               # the toggle key

[capture]
default = "on"                  # default for capturers not explicitly listed

[capture.bash]     enabled = true
[capture.read]     enabled = true
[capture.edit]     enabled = true
[capture.write]    enabled = true
[capture.grep]     enabled = false   # too noisy during exploration
[capture.glob]     enabled = false
[capture.task]     enabled = true
[capture.webfetch] enabled = true

[viewer]
ring_buffer_entries = 500       # per agent
```

Capturer defaults ship `enabled = true` for high-signal tools, `false` for high-volume low-signal ones (grep, glob).

---

## 14. Versioning & contract stability

The `Capturer` trait is the single critical surface. Evolution rules:

- **Patch:** new optional methods with sensible defaults.
- **Minor:** new variants on `EntryBody` (additive; renderers must handle unknown variants by falling back to `Text`).
- **Major:** changing existing method signatures or removing variants.

Because the registry is in-tree, every breaking change to the trait is applied to all built-in capturers in the same PR. There is no out-of-tree ABI to maintain.

---

## 15. Open implementation details

These don't affect the architecture; they're decided during implementation:

- **Exact hotkey choice.** See §12.3.
- **Socket framing details.** JSONL vs length-prefixed binary. JSONL is human-debuggable; binary is faster. Likely JSONL for v1 — debuggability matters more than throughput in observability tooling.
- **Viewer rendering polish.** Colors, status icons, scrollback navigation. Use `ratatui` + `crossterm`.
- **Default config.** Which capturers ship `enabled = true`.
- **Linux / macOS platform parity.** macOS is the primary target; Linux should work; Windows is out of scope.

---

## 16. Out of scope (for now)

Each of these is a real product capability that some users will want. They are deferred to keep PoC scope sane, and the architecture is intentionally compatible with them.

- **Web UI.** Adds a localhost viewer that subscribes to the same socket stream. Requires splitting the daemon (§16.1).
- **Cross-session persistence.** History survives `obi-wrapper` exit; browse past sessions. Requires the split daemon plus on-disk history with rotation.
- **External plugins.** User-installable capturers without rebuilding. Requires a stable plugin ABI (out-of-process is the realistic path).
- **Windows support.** Different pty + unix-socket semantics.

### 16.1 The seam to a split daemon

If/when cross-session persistence or a web UI is added, the wrapper-daemon collapse splits. What stays identical:

- `Capturer` trait, all built-in capturers.
- `obi-hook` and its rewrite logic.
- `obi-tee`'s args, fail-open invariants, socket framing.
- Socket path conventions (with daemon-managed session namespacing).

What gets added:

- A separate `obi-daemon` process that listens on the sockets, owns the ring buffers, and persists history to disk with rotation.
- `obi-wrapper` becomes a thin client that subscribes to the daemon (over the same socket protocol).
- A session index for browsing past sessions.

No PoC code is thrown away in the split. The wrapper's listener code moves into the daemon; the wrapper gains a subscriber.

---

## Appendix A — Empirical verification

Conducted 2026-05-27 against CC `2.1.142` (Mach-O arm64 binary).

### A.1 Hook payload schema

Captured raw PreToolUse payloads in an isolated headless `claude -p` run for both a main-agent Bash call and a subagent (general-purpose) Bash call.

| Field             | Main-agent payload          | Subagent payload                              |
| ----------------- | --------------------------- | --------------------------------------------- |
| `agent_id`        | absent                      | `"a56e70ccdc442bf74"` (stable, unique)        |
| `agent_type`      | absent                      | `"general-purpose"`                           |
| `session_id`      | `"a9db5455…"`               | `"a9db5455…"` (identical)                     |
| `transcript_path` | `…/a9db5455….jsonl`         | identical to main                             |
| `tool_use_id`     | present                     | present (different per call)                  |

### A.2 `agent_id` stability and distinctness

Two concurrent subagents (ALPHA, BETA), each running two `echo` commands:

- ALPHA: `agent_id = ad3aa334ddcf975cb` for both `ALPHA_ONE` and `ALPHA_TWO`. → **stable.**
- BETA: `agent_id = a90c966145305b9d2` for both `BETA_ONE` and `BETA_TWO`. → **stable.**
- ALPHA's id ≠ BETA's id. → **distinct.**

### A.3 Hook output schema for command mutation

`hookSpecificOutput.updatedInput` is the documented field. Corroborated by string presence in the CC binary: `updatedInput`×198, `hookSpecificOutput`×95, `hookEventName`×56.

### A.4 Shell capabilities

CC's Bash tool dispatches into the user's login shell (zsh on macOS in 2.1.142). Verified working in that shell:

- Process substitution `>(…)`.
- `cmd | tee >(sink) | filter` pattern.
- `2> >(sink)` for stderr redirection to a sink.
- `set -o pipefail`.
