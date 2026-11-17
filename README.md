# claude-monitor

A terminal UI that watches Claude Code agents. It shows what each session and
subagent is doing, how they interact (spawn, message, handback), and how many
tokens each one used.

By default it draws each session as a pixel-art office: every agent is a small
animated character at a desk or in the lounge. Press `v` for the classic tree
and tabs.

![The office view, rendered from the built-in demo data](docs/office.png)

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
claude-monitor --demo          # the office with built-in synthetic data, no transcripts read
claude-monitor --snapshot office.png --session <id> --scale 4   # one frame to a PNG, exit
```

| Flag         | Default                              | Meaning                                                        |
|--------------|--------------------------------------|----------------------------------------------------------------|
| `--root`     | `~/.claude/projects`                 | Claude Code projects directory                                 |
| `--since`    | `24h`                                | Skip sessions whose transcript file is older than this (0 = all) |
| `--dump`     | off                                  | Print the tree, edges and per-model usage as text, then exit   |
| `--sprites`  | `$XDG_CONFIG_HOME/claude-monitor/sprites` (else `~/.config/claude-monitor/sprites`) | Directory of custom character GIF/PNG files |
| `--snapshot` | off                                  | Scan once, render one office frame to this PNG, then exit      |
| `--session`  | most recently active                 | Session shown by `--snapshot`                                  |
| `--scale`    | `4`                                  | Pixel scale of the `--snapshot` PNG                            |
| `--demo`     | off                                  | Use a built-in synthetic snapshot instead of scanning; works with the TUI, `--dump` and `--snapshot` |

The TUI rescans every second. Each scan reads only the lines appended since the
last one, so polling stays cheap on large transcripts.

`--snapshot` lays the office out as a 160x45 terminal would show it, at the
time of the scan, and scales it up nearest-neighbour. `--demo --snapshot` always
renders the same frame.

## Office view

The default view. One office per session; the header shows which session
(`◀ 2/7 ▶`) and starts on the most recently active one.

- **Work room** (wood floor): the session's main agent sits at the lead desk
  (darker wood, a plant and a gold plaque); each subagent owns a desk with a
  CRT, keyboard and green chair. A token meter above each desk is that agent's
  tokens relative to the busiest agent in the session.
- **Lounge** (blue floor): sofas around a coffee table, and a checker kitchen
  strip with a coffee counter.
- **Back wall**: shelves, a clock and the exit door.

| Status    | Where and how                                                    |
|-----------|------------------------------------------------------------------|
| running   | at its desk, typing; the screen flickers green; a speech bubble  |
| active    | at its desk, idle bob; the screen is on                          |
| idle      | walks to the lounge and dozes on a sofa (`z`)                    |
| done      | in the lounge, dimmed, with a `✓` above its head                 |
| done > 10 min | walks out the exit door; counted in the footer as "left"     |

New agents walk in from the exit door. Agents that change place walk there,
through the doorway between the rooms.

Edges younger than 90 seconds are drawn as a moving dotted line with a letter
travelling from sender to receiver: yellow for spawn, cyan for message, green
for handback. Older edges are only listed in the strip.

Under the canvas:

- **Strip** (4 lines): the selected agent's description, type, model, status
  and current tool (`now:`); its tokens (in, out, cache read and write), turns
  and last activity; its edges, newest first, with live ones in their link
  color; and `busy`, the text of every speech bubble (the running tool).
- **Footer**: the session's total tokens and its totals per model, and how many
  agents left.

When a session has more agents than desks or lounge seats, the office pages:
the header shows `page 1/2` and selecting an agent shows its page. Sprites
never overlap. Below 80x24 a one-line message replaces the canvas.

Pixels are drawn with the upper-half block `▀`, two pixels per cell, in 24-bit
color; terminals without truecolor get the nearest 256 colors.

### Office keys

| Key                          | Action                                   |
|------------------------------|------------------------------------------|
| `↑`/`↓`, `k`/`j`, `tab`      | select the previous or next agent        |
| `[` `]`, `←` `→`             | previous or next session                 |
| `enter`                      | open the selected agent in the tree view |
| `v`                          | switch to the tree view                  |
| `r`                          | drop all state and rescan                |
| `q`, `ctrl+c`                | quit                                     |

## Custom characters

Put your own `.gif` or `.png` characters in the sprites directory
(`~/.config/claude-monitor/sprites` by default, or `--sprites <dir>`). Each
agent picks, in order:

1. the file named after its agent type: `general-purpose.gif`, `Explore.png`,
   `my-agent.gif`, and `session.gif` for the main agent;
2. otherwise one of the files not named after an agent type in use, chosen by a
   stable hash of the agent id;
3. otherwise a built-in character.

Animated GIFs keep their frame delays, disposal and transparency. Transparent
margins are cropped and the image is scaled nearest-neighbour to the character
height (16 pixels, at most 12 wide). A missing directory or a broken file never
stops the monitor; the help bar shows a one-line note instead.

These files stay on your machine. Keep them in the sprites directory, not in
this repository: `sprites/` is in `.gitignore`.

## Art and licensing

All built-in art (floors, walls, furniture and characters) is original, drawn
in Go code as pixel maps in `internal/office/art.go` and `render.go`. No image
files are bundled. `docs/office.png` is rendered from the built-in demo data.
Custom sprites are loaded only from your local directory at run time and are
never copied anywhere; only use art you have the right to use.

## Tree view

Press `v` to switch between the office and the tree.

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

### Tree keys

| Key               | Action                     |
|-------------------|----------------------------|
| `↑`/`↓`, `k`/`j`  | move the selection         |
| `enter`, `space`  | collapse or expand a node  |
| `tab`             | next tab                   |
| `1` `2` `3`       | Activity, Interactions, Usage |
| `v`               | switch to the office view  |
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
go test ./internal/pixel ./internal/office -bench . -run '^$'   # frame time
```

Scanner tests run on synthetic transcripts in `internal/scan/testdata`.
`BenchmarkFrame160x45` times one full office frame (layout, render and
encode) for a 160x45 terminal.

Regenerate the screenshot from demo data only, with no custom sprites:

```sh
go build -o bin/claude-monitor . && mkdir -p /tmp/no-sprites &&
  ./bin/claude-monitor --demo --sprites /tmp/no-sprites --snapshot docs/office.png --scale 4
```
