package scan

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

const fixtureRoot = "testdata/projects"

func mtime(t *testing.T, path string) time.Time {
	t.Helper()
	fi, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	return fi.ModTime()
}

// fixtureNow is 10s after the newest fixture file was written.
func fixtureNow(t *testing.T) time.Time {
	var newest time.Time
	_ = filepath.WalkDir(fixtureRoot, func(p string, d os.DirEntry, err error) error {
		if err == nil && !d.IsDir() {
			if m := mtime(t, p); m.After(newest) {
				newest = m
			}
		}
		return nil
	})
	return newest.Add(10 * time.Second)
}

func TestAgents(t *testing.T) {
	snap := New(fixtureRoot, 0).Scan(fixtureNow(t))
	tests := []struct {
		name, id, parent, title, typ, model string
		kind                                Kind
		turns                               int
		usage                               Usage
		lastTool                            string
	}{
		{
			// msg_s1 appears on two lines and is counted once; the malformed line
			// before msg_s3 is skipped without losing msg_s3.
			name: "session usage deduped by message.id, malformed line skipped",
			id:   "sess-1", title: "Demo session", model: "claude-opus-5-5", kind: Session,
			turns: 3, usage: Usage{Input: 13, Output: 90, CacheRead: 600, CacheWrite: 5},
			lastTool: "Bash: go test ./...",
		},
		{
			name: "subagent linked to session via meta.toolUseId",
			id:   "a1", parent: "sess-1", title: "Explore code", typ: "Explore",
			model: "claude-sonnet-4-5-20250929", kind: Subagent,
			turns: 3, usage: Usage{Input: 5, Output: 14, CacheWrite: 11},
			lastTool: "SendMessage: to a2",
		},
		{
			name: "nested subagent linked to the subagent holding the tool_use",
			id:   "a2", parent: "a1", title: "Nested look", typ: "general-purpose",
			model: "claude-opus-5-5", kind: Subagent,
			turns: 1, usage: Usage{Input: 4, Output: 8, CacheRead: 16},
		},
		{
			name: "unknown toolUseId falls back to the session",
			id:   "a3", parent: "sess-1", title: "Orphan task", typ: "Plan",
			model: "claude-opus-5-5", kind: Subagent,
			turns: 2, usage: Usage{Input: 1, Output: 1}, // second turn is a zero-usage <synthetic> message
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			a := snap.ByID[tc.id]
			if a == nil {
				t.Fatalf("agent %s missing; have %v", tc.id, keys(snap.ByID))
			}
			if a.ParentID != tc.parent || a.Kind != tc.kind || a.Title != tc.title ||
				a.AgentType != tc.typ || a.Model != tc.model || a.SessionID != "sess-1" || a.Project != "demo" {
				t.Errorf("got parent=%q kind=%v title=%q type=%q model=%q session=%q project=%q",
					a.ParentID, a.Kind, a.Title, a.AgentType, a.Model, a.SessionID, a.Project)
			}
			if a.Turns != tc.turns || a.Usage != tc.usage {
				t.Errorf("turns=%d usage=%+v, want %d %+v", a.Turns, a.Usage, tc.turns, tc.usage)
			}
			if tc.lastTool != "" && a.LastTool != tc.lastTool {
				t.Errorf("lastTool=%q want %q", a.LastTool, tc.lastTool)
			}
		})
	}

	if len(snap.Roots) != 1 || len(snap.Roots[0].Children) != 2 || len(snap.ByID["a1"].Children) != 1 {
		t.Fatalf("tree shape wrong: roots=%d", len(snap.Roots))
	}
	if got := snap.ByModel["claude-opus-5-5"].Output; got != 90+8+1 {
		t.Errorf("ByModel opus output=%d want 99", got)
	}
	if len(snap.ByModel) != 2 {
		t.Errorf("ByModel has %d models, want 2 (no bucket for zero-usage messages): %v", len(snap.ByModel), snap.ByModel)
	}
	if got := snap.ByModel["claude-sonnet-4-5-20250929"].Output; got != 14 {
		t.Errorf("ByModel sonnet output=%d want 14", got)
	}
}

func TestEdges(t *testing.T) {
	snap := New(fixtureRoot, 0).Scan(fixtureNow(t))
	want := []Edge{
		{From: "sess-1", To: "a1", Kind: EdgeSpawn, Label: "Explore code"},
		{From: "a1", To: "a2", Kind: EdgeSpawn, Label: "Nested look"},
		{From: "sess-1", To: "a3", Kind: EdgeSpawn, Label: "Orphan task"},
		{From: "a1", To: "a2", Kind: EdgeMessage, Label: "check main"}, // to: name "helper"
		{From: "a1", To: "a2", Kind: EdgeMessage, Label: "thanks"},     // to: agent id
		{From: "a1", To: "sess-1", Kind: EdgeHandback, Label: "handback"},
	}
	for _, w := range want {
		t.Run(w.Kind+" "+w.From+"->"+w.To+" "+w.Label, func(t *testing.T) {
			for _, e := range snap.Edges {
				if e.From == w.From && e.To == w.To && e.Kind == w.Kind && e.Label == w.Label {
					if e.At.IsZero() {
						t.Errorf("edge has zero time")
					}
					return
				}
			}
			t.Errorf("edge not found in %+v", snap.Edges)
		})
	}
	if len(snap.Edges) != len(want) {
		t.Errorf("got %d edges, want %d", len(snap.Edges), len(want))
	}
}

func TestStatus(t *testing.T) {
	sessPath := filepath.Join(fixtureRoot, "-home-u-demo", "sess-1.jsonl")
	a2Path := filepath.Join(fixtureRoot, "-home-u-demo", "sess-1", "subagents", "agent-a2.jsonl")
	tests := []struct {
		name string
		id   string
		path string
		age  time.Duration // now - file mtime
		want Status
	}{
		{"pending tool_use, fresh file -> running", "sess-1", sessPath, 10 * time.Second, Running},
		{"pending tool_use, written 90s ago -> running", "sess-1", sessPath, 90 * time.Second, Running},
		{"pending tool_use, stale file -> idle", "sess-1", sessPath, 5 * time.Minute, Idle},
		{"no pending tool_use, fresh -> active", "a2", a2Path, 5 * time.Second, Active},
		{"no pending tool_use, 1m old -> idle", "a2", a2Path, time.Minute, Idle},
		{"meta finished -> done", "a1", a2Path, 5 * time.Second, Done},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			snap := New(fixtureRoot, 0).Scan(mtime(t, tc.path).Add(tc.age))
			if got := snap.ByID[tc.id].Status; got != tc.want {
				t.Errorf("status=%v want %v", got, tc.want)
			}
		})
	}
}

func TestSinceFiltersByMtime(t *testing.T) {
	now := mtime(t, filepath.Join(fixtureRoot, "-home-u-demo", "sess-1.jsonl"))
	tests := []struct {
		since time.Duration
		now   time.Time
		want  int
	}{
		{time.Hour, now.Add(time.Minute), 1},
		{time.Hour, now.Add(2 * time.Hour), 0},
		{0, now.Add(1000 * time.Hour), 1},
	}
	for _, tc := range tests {
		if got := len(New(fixtureRoot, tc.since).Scan(tc.now).Roots); got != tc.want {
			t.Errorf("since=%v now+%v: %d sessions, want %d", tc.since, tc.now.Sub(now), got, tc.want)
		}
	}
}

func TestPartialTrailingLineNotConsumed(t *testing.T) {
	root := t.TempDir()
	proj := filepath.Join(root, "-tmp-p")
	if err := os.MkdirAll(proj, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(proj, "s.jsonl")
	l1 := `{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"id":"m1","model":"claude-opus-5-5","content":[],"usage":{"input_tokens":1,"output_tokens":10}}}` + "\n"
	l2 := `{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m2","model":"claude-opus-5-5","content":[],"usage":{"input_tokens":1,"output_tokens":20}}}` + "\n"
	l3 := `{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"id":"m2","model":"claude-opus-5-5","content":[{"type":"text","text":"x"}],"usage":{"input_tokens":1,"output_tokens":20}}}` + "\n"

	sc := New(root, 0)
	steps := []struct {
		name      string
		append    string
		wantTurns int
		wantOut   int64
		wantOff   int64
	}{
		{"complete line plus half a line", l1 + l2[:40], 1, 10, int64(len(l1))},
		{"rest of the line arrives", l2[40:], 2, 30, int64(len(l1) + len(l2))},
		{"same message.id again across scans", l3, 2, 30, int64(len(l1) + len(l2) + len(l3))},
	}
	for _, st := range steps {
		f, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
		if err != nil {
			t.Fatal(err)
		}
		_, _ = f.WriteString(st.append)
		f.Close()
		snap := sc.Scan(time.Now())
		a := snap.ByID["s"]
		if a == nil || a.Turns != st.wantTurns || a.Usage.Output != st.wantOut || sc.files[path].offset != st.wantOff {
			t.Fatalf("%s: agent=%+v offset=%d", st.name, a, sc.files[path].offset)
		}
	}
}

func keys(m map[string]*Agent) []string {
	var out []string
	for k := range m {
		out = append(out, k)
	}
	return out
}

func TestMayBeRelevant(t *testing.T) {
	tests := []struct {
		line string
		want bool
	}{
		{`{"type":"assistant","message":{}}`, true},
		{`{"message":{"role":"user"},"type" : "user"}`, true}, // key order and spacing vary
		{`{"type":"ai-title","aiTitle":"x"}`, true},
		{`{"type":"attachment","attachment":{"type":"text"}}`, false},
		{`{"type":"api-request-blob","body":{"messages":[{"role":"user","content":[{"type":"tool_result"}]}]}}`, false},
		{`{"uuid":"1"}`, false},
	}
	for _, tc := range tests {
		if got := mayBeRelevant([]byte(tc.line)); got != tc.want {
			t.Errorf("mayBeRelevant(%s) = %v, want %v", tc.line, got, tc.want)
		}
	}
}
