package ui

import (
	"strings"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/cicingik/claude-monitor/internal/scan"
)

func fixtureSnapshot() scan.Snapshot {
	now := time.Date(2026, 1, 1, 10, 0, 0, 0, time.UTC)
	nested := &scan.Agent{ID: "a2nested", ParentID: "a1child", SessionID: "sess-0001", Kind: scan.Subagent,
		Title: "Nested look", AgentType: "general-purpose", Model: "claude-opus-5-5", Status: scan.Active,
		LastActivity: now.Add(-5 * time.Second), Usage: scan.Usage{Output: 800},
		ModelUsage: map[string]scan.Usage{"claude-opus-5-5": {Output: 800}}}
	child := &scan.Agent{ID: "a1child", ParentID: "sess-0001", SessionID: "sess-0001", Kind: scan.Subagent,
		Title: "Explore code", AgentType: "Explore", Model: "claude-sonnet-4-5-20250929", Status: scan.Done,
		LastActivity: now.Add(-2 * time.Minute), Usage: scan.Usage{Input: 1000, Output: 1500},
		ModelUsage: map[string]scan.Usage{"claude-sonnet-4-5-20250929": {Input: 1000, Output: 1500}},
		Children:   []*scan.Agent{nested}}
	sess := &scan.Agent{ID: "sess-0001", SessionID: "sess-0001", Kind: scan.Session, Title: "Demo session",
		Project: "demo", Model: "claude-opus-5-5", Status: scan.Running, LastTool: "Bash: go test ./...",
		LastActivity: now.Add(-3 * time.Second), Turns: 4, Usage: scan.Usage{Input: 10, Output: 90, CacheRead: 1_200_000},
		ModelUsage: map[string]scan.Usage{"claude-opus-5-5": {Input: 10, Output: 90, CacheRead: 1_200_000}},
		Events:     []scan.Event{{At: now, Kind: "tool_use", Tool: "Bash", Summary: "Bash: go test ./..."}},
		Children:   []*scan.Agent{child}}
	return scan.Snapshot{
		Roots:     []*scan.Agent{sess},
		ByID:      map[string]*scan.Agent{sess.ID: sess, child.ID: child, nested.ID: nested},
		ScannedAt: now,
		Edges: []scan.Edge{
			{From: "sess-0001", To: "a1child", Kind: scan.EdgeSpawn, Label: "Explore code", At: now},
			{From: "a1child", To: "a2nested", Kind: scan.EdgeSpawn, Label: "Nested look", At: now},
			{From: "a1child", To: "sess-0001", Kind: scan.EdgeHandback, Label: "handback", At: now},
		},
	}
}

func send(m tea.Model, msgs ...tea.Msg) tea.Model {
	for _, msg := range msgs {
		m, _ = m.Update(msg)
	}
	return m
}

func screen(m tea.Model) string { return ansi.Strip(m.View().Content) }

func TestViewRendersTreeAndTabs(t *testing.T) {
	m := send(New(nil), tea.WindowSizeMsg{Width: 140, Height: 30}, snapshotMsg{snap: fixtureSnapshot()})
	tests := []struct {
		name  string
		keys  []tea.Msg
		want  []string
		avoid []string
	}{
		{
			name: "initial frame: header, tree rows, tab titles, activity",
			want: []string{
				"claude-monitor", "sessions 1 · agents 3 · running 1",
				"▾ ● sess-000 Demo session", "session", "opus-5-5", "1.2M", // session row
				"▾ ✓ Explore code", "Explore", "sonnet-4-5", "2.5k", // child row
				"◐ Nested look", "general-purpose", "800", // nested row
				"1 Activity", "2 Interactions", "3 Usage",
				"now: Bash: go test ./...",
			},
		},
		{
			name: "tab 2 shows edges and the session graph",
			keys: []tea.Msg{tea.KeyPressMsg{Code: '2', Text: "2"}},
			want: []string{"← handback from a1child Explore", "→ spawn    to   a1child Explore", "Session graph",
				`└─spawn "Explore code"─▶ ✓ a1child Explore`, `└─spawn "Nested look"─▶ ◐ a2nested general-purpose`},
		},
		{
			name: "tab key cycles to usage",
			keys: []tea.Msg{tea.KeyPressMsg{Code: tea.KeyTab}},
			want: []string{"By model", "Session total 1.2M", "sonnet-4-5"},
		},
		{
			name:  "down then enter collapses the child",
			keys:  []tea.Msg{tea.KeyPressMsg{Code: '1', Text: "1"}, tea.KeyPressMsg{Code: 'j', Text: "j"}, tea.KeyPressMsg{Code: tea.KeyEnter}},
			want:  []string{"▸ ✓ Explore code"},
			avoid: []string{"Nested look"},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			m = send(m, tc.keys...)
			out := screen(m)
			for _, w := range tc.want {
				if !strings.Contains(out, w) {
					t.Errorf("missing %q in:\n%s", w, out)
				}
			}
			for _, w := range tc.avoid {
				if strings.Contains(out, w) {
					t.Errorf("unexpected %q in:\n%s", w, out)
				}
			}
		})
	}
}

func TestOfficeView(t *testing.T) {
	m := send(New(nil).WithConfig(Config{Office: true}), tea.WindowSizeMsg{Width: 160, Height: 45}, snapshotMsg{snap: fixtureSnapshot()})
	tests := []struct {
		name  string
		keys  []tea.Msg
		want  []string
		avoid []string
	}{
		{
			name: "office is the start view: header, half-block canvas, strip for the lead, footer",
			want: []string{"◀ 1/1 ▶", "sess-000 Demo session", "▀▀▀▀",
				"▶ ● sess-000 Demo session", "session · opus-5-5 · running", "now: Bash: go test ./...",
				"turns 4", "← handback Explore code", "busy lead: Bash: go test ./...", "session 1.2M", "sonnet-4-5 2.5k"},
			avoid: []string{"1 Activity"},
		},
		{
			name: "j selects the next agent",
			keys: []tea.Msg{tea.KeyPressMsg{Code: 'j', Text: "j"}},
			want: []string{"▶ ✓ Explore code", "Explore · sonnet-4-5 · done", "→ spawn Nested look", "← spawn lead"},
		},
		{
			name:  "enter opens the selected agent in the tree",
			keys:  []tea.Msg{tea.KeyPressMsg{Code: tea.KeyEnter}},
			want:  []string{"1 Activity", "✓ Explore code", "Explore · sonnet-4-5 · done"},
			avoid: []string{"▀▀▀▀"},
		},
		{
			name: "v goes back to the office, keeping the selection",
			keys: []tea.Msg{tea.KeyPressMsg{Code: 'v', Text: "v"}},
			want: []string{"▀▀▀▀", "▶ ✓ Explore code"},
		},
		{
			name:  "v again shows the tree",
			keys:  []tea.Msg{tea.KeyPressMsg{Code: 'v', Text: "v"}},
			want:  []string{"1 Activity"},
			avoid: []string{"▀▀▀▀"},
		},
		{
			name:  "a small terminal gets a one-line message",
			keys:  []tea.Msg{tea.KeyPressMsg{Code: 'v', Text: "v"}, tea.WindowSizeMsg{Width: 70, Height: 20}},
			want:  []string{"the office needs at least 80x24 (now 70x20)"},
			avoid: []string{"▀"},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			m = send(m, tc.keys...)
			out := screen(m)
			for _, w := range tc.want {
				if !strings.Contains(out, w) {
					t.Errorf("missing %q in:\n%s", w, out)
				}
			}
			for _, w := range tc.avoid {
				if strings.Contains(out, w) {
					t.Errorf("unexpected %q in:\n%s", w, out)
				}
			}
		})
	}
}
