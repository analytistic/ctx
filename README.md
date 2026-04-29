# ctx — Claude Code session toolkit

Inspect, browse, and manage Claude Code sessions from the terminal.

## Commands

| Command | Description |
|---------|-------------|
| `ctx list` | Print session tree overview centered on current position |
| `ctx info` | Show session metadata (path, size, event counts, tokens) |
| `ctx summary` | Show user turn summary with timestamps |
| `ctx insert <text>` | Insert a note into the session tree |
| `ctx rm <uuid>` | Remove an event subtree from the session |
| `ctx export [file]` | Export all events as pretty-printed JSON |
| `ctx tail` | Follow session log in real time |

### `ctx list`

Displays the conversation tree with the current position (`←`) highlighted:

```
Session b210c4f8-ac9…

── user 04/28 21:15 hi 你知道我们这个项目是做啥的吗
   └── Claude [deepseek-v4-flash] 04/28 21:15 [thinking] The user is asking me...

── system 00:55:30 [system]
   └── user 00:55:30 This session is being continued from...
         ┌── [775 levels]
         └── user 00:56:40 /  ←
```

**Options:**
- `-d N` / `--depth N` — tree expansion depth per root (default: 2)
- `-u N` / `--upstream N` — parent-chain levels above current position (default: 3)
- `-l N` / `--max-len N` — max preview characters (default: 80)

## Install

```bash
cargo install --git https://github.com/analytistic/ctx
```

Or download a pre-built binary from [releases](https://github.com/analytistic/ctx/releases).

## Session discovery

`ctx` finds the active session by walking the process tree from the current terminal to locate the running Claude Code process, then reads its session file from `~/.claude/sessions/`.
