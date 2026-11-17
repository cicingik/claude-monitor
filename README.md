# claude-monitor

A terminal UI that watches Claude Code agents. It shows what each session and
subagent is doing, how they interact (spawn, message, handback), and how many
tokens each one used.

It is read-only. It reads transcript files under `~/.claude/projects` and never
calls Claude or any network service.

## Install

Requires Go 1.26 or newer.

```sh
go install github.com/cicingik/claude-monitor@latest
```

Or build from a checkout:

```sh
go build -o bin/claude-monitor .
```

## Usage

```sh
claude-monitor                 # interactive TUI, sessions active in the last 24h
claude-monitor --since 2h      # only sessions whose transcript changed in the last 2h
claude-monitor --since 0       # every session on disk
claude-monitor --dump          # scan once, print plain text, exit
claude-monitor --root /path/to/projects
```

| Flag      | Default              | Meaning                                                        |
|-----------|----------------------|----------------------------------------------------------------|
| `--root`  | `~/.claude/projects` | Claude Code projects directory                                 |
| `--since` | `24h`                | Skip sessions whose transcript file is older than this (0 = all) |
| `--dump`  | off                  | Print the tree, edges and per-model usage as text, then exit   |

The TUI rescans every second. Each scan reads only the lines appended since the
last one, so polling stays cheap on large transcripts.

### Screen

- **Header:** session, agent and running counts, last scan time, time window.
- **Left pane:** a collapsible tree of sessions and their subagents, nested
  subagents included. Each row shows status, title, agent type, model, the
  agent's own tokens and the age of its last activity.
- **Right pane, tab 1 Activity:** the selected agent's details and its last 50
  events, newest first. While it runs, `now:` shows the pending tool call.
- **Tab 2 Interactions:** edges into and out of the selected agent, plus an
  ASCII graph of its session with spawn labels, messages and handbacks.
- **Tab 3 Usage:** a token bar per agent in the session, totals by model and
  the session total.

Status glyphs:

| Glyph | Status  | Rule                                                              |
|-------|---------|-------------------------------------------------------------------|
| `●`   | running | last tool_use has no tool_result yet and the file was written < 2m ago |
| `◐`   | active  | file written < 30s ago                                            |
| `○`   | idle    | otherwise                                                         |
| `✓`   | done    | subagent meta says `finished: true`                               |

## Keys

| Key               | Action                     |
|-------------------|----------------------------|
| `↑`/`↓`, `k`/`j`  | move the selection         |
| `enter`, `space`  | collapse or expand a node  |
| `tab`             | next tab                   |
| `1` `2` `3`       | Activity, Interactions, Usage |
| `r`               | drop all state and rescan  |
| `q`, `ctrl+c`     | quit                       |

## Data sources

All paths are under `--root` (default `~/.claude/projects`). Nothing else under
`~/.claude` is read.

- `<project>/<sessionId>.jsonl`: the main session, one JSON object per line.
  The project directory is the session cwd with `/` replaced by `-`; the UI
  shows the last segment of the cwd.
- `<project>/<sessionId>/subagents/agent-<agentId>.jsonl`: a subagent
  transcript, same schema.
- `<project>/<sessionId>/subagents/agent-<agentId>.meta.json`: `agentType`,
  `description`, `toolUseId`, `finished`.

Line types used: `assistant` (model, usage, `tool_use` and `text` blocks),
`user` (prompts, `tool_result`, handback notices) and `ai-title`. Other types,
unknown fields and malformed lines are skipped.

How the numbers and links are derived:

- **Usage.** Claude Code writes one assistant line per content block and repeats
  the full usage on each, so usage is counted once per `message.id`. Turns are
  distinct `message.id`s. A row's tokens are input + output + cache read +
  cache write for that agent alone.
- **Spawn edge.** A subagent's `meta.toolUseId` is the id of the `Agent` (or
  `Task`) tool_use that started it. Its parent is whichever transcript holds
  that id: the session or another subagent. If none does, the parent is the
  session.
- **Message edge.** A `SendMessage` tool_use, from the agent whose transcript
  holds it to `input.to` (an agent id, or a name given at spawn).
- **Handback edge.** A user line containing `<agent-message from="<agentId>"`,
  from that agent to the transcript owner.

Only short summaries are shown: tool names with a short argument, and at most
60 characters of any prompt or reply. Transcripts can hold secrets, so the
monitor never prints full message text.

## Development

```sh
go vet ./... && go test ./... -count=1
```

Scanner tests run on synthetic transcripts in `internal/scan/testdata`.
