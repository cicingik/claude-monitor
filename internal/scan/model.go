// Package scan reads Claude Code transcripts under ~/.claude/projects and
// builds immutable snapshots of sessions, subagents, their interactions and
// token usage. It only reads files.
package scan

import "time"

// Kind says whether an agent is a top-level session or a spawned subagent.
type Kind int

const (
	Session Kind = iota
	Subagent
)

func (k Kind) String() string {
	if k == Subagent {
		return "subagent"
	}
	return "session"
}

// Status is the liveness of an agent, derived from its transcript and file mtime.
type Status int

const (
	Idle    Status = iota // nothing written for 30s or more
	Active                // transcript written less than 30s ago
	Running               // last tool_use has no tool_result yet and file written less than 2m ago
	Done                  // subagent meta says finished
)

func (s Status) String() string {
	switch s {
	case Running:
		return "running"
	case Active:
		return "active"
	case Done:
		return "done"
	}
	return "idle"
}

// Glyph is the one-character status marker used by the tree and the dump.
func (s Status) Glyph() string {
	switch s {
	case Running:
		return "●"
	case Active:
		return "◐"
	case Done:
		return "✓"
	}
	return "○"
}

// Usage is a token count, deduplicated per API message.
type Usage struct{ Input, Output, CacheRead, CacheWrite int64 }

func (u Usage) Total() int64 { return u.Input + u.Output + u.CacheRead + u.CacheWrite }

func (u Usage) Add(o Usage) Usage {
	return Usage{u.Input + o.Input, u.Output + o.Output, u.CacheRead + o.CacheRead, u.CacheWrite + o.CacheWrite}
}

// Event is one entry of an agent's recent activity. Summary is always short;
// prompt and message text is never carried in full.
type Event struct {
	At            time.Time
	Kind          string // tool_use | text | prompt | handback
	Tool, Summary string
}

// Edge kinds.
const (
	EdgeSpawn    = "spawn"
	EdgeMessage  = "message"
	EdgeHandback = "handback"
)

// Edge is an interaction between two agents.
type Edge struct {
	From, To string
	Kind     string // spawn | message | handback
	At       time.Time
	Label    string
}

// Agent is a session or subagent as of one scan.
type Agent struct {
	ID, ParentID, Title, AgentType, Model, Project, Path string
	SessionID                                            string // ID of the root session (equals ID for sessions)

	Kind         Kind
	Status       Status
	LastActivity time.Time
	LastTool     string // summary of the most recent tool_use
	Turns        int    // distinct message.id
	Usage        Usage  // this agent's own usage, not including children
	ModelUsage   map[string]Usage
	Events       []Event // newest last, at most MaxEvents
	Children     []*Agent
}

// Snapshot is an immutable view built by one Scan.
type Snapshot struct {
	Roots     []*Agent // sessions, most recently active first
	ByID      map[string]*Agent
	Edges     []Edge
	ByModel   map[string]Usage
	ScannedAt time.Time
}

// MaxEvents is how many recent events are kept per agent.
const MaxEvents = 50

// SubtreeUsage sums the usage of a and all its descendants.
func SubtreeUsage(a *Agent) Usage {
	u := a.Usage
	for _, c := range a.Children {
		u = u.Add(SubtreeUsage(c))
	}
	return u
}

// Walk visits a and its descendants depth-first, passing the depth.
func Walk(a *Agent, depth int, fn func(*Agent, int)) {
	fn(a, depth)
	for _, c := range a.Children {
		Walk(c, depth+1, fn)
	}
}
