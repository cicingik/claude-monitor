package office

import (
	"time"

	"github.com/cicingik/claude-monitor/internal/scan"
)

// Demo is a synthetic snapshot as of now: one session whose agents cover every
// status, with recent and older edges of each kind. It holds no real data.
func Demo(now time.Time) scan.Snapshot {
	const sid = "demo-0000-office"
	ago := func(d time.Duration) time.Time { return now.Add(-d) }
	opus, sonnet, haiku := "claude-opus-5-5", "claude-sonnet-4-5-20250929", "claude-haiku-4-5-20251001"
	mk := func(id, parent, title, typ, model string, st scan.Status, last time.Duration, tool string, turns int, u scan.Usage) *scan.Agent {
		kind := scan.Subagent
		if parent == "" {
			kind = scan.Session
		}
		ev := []scan.Event{{At: ago(last), Kind: "tool_use", Tool: tool, Summary: tool}}
		if tool == "" {
			ev = nil
		}
		return &scan.Agent{ID: id, ParentID: parent, SessionID: sid, Title: title, AgentType: typ, Model: model,
			Project: "demo", Kind: kind, Status: st, LastActivity: ago(last), LastTool: tool, Turns: turns,
			Usage: u, ModelUsage: map[string]scan.Usage{model: u}, Events: ev}
	}
	lead := mk(sid, "", "Office demo", "", opus, scan.Running, 2*time.Second, "Agent: Write layout tests", 42,
		scan.Usage{Input: 120, Output: 38_000, CacheRead: 2_400_000, CacheWrite: 90_000})
	a1 := mk("a1-explore", sid, "Map the scanner", "Explore", sonnet, scan.Running, 3*time.Second, "Grep: func Scan", 12,
		scan.Usage{Input: 40, Output: 9_000, CacheRead: 610_000, CacheWrite: 40_000})
	a2 := mk("a2-builder", sid, "Build the pixel encoder", "general-purpose", opus, scan.Running, 1*time.Second, "Edit: pixel/encode.go", 30,
		scan.Usage{Input: 90, Output: 31_000, CacheRead: 1_900_000, CacheWrite: 70_000})
	a3 := mk("a3-tester", sid, "Write layout tests", "general-purpose", haiku, scan.Active, 12*time.Second, "Read: scene.go", 8,
		scan.Usage{Input: 30, Output: 4_000, CacheRead: 210_000, CacheWrite: 25_000})
	a4 := mk("a4-reviewer", sid, "Review the README", "my-agent", sonnet, scan.Idle, 3*time.Minute, "Read: README.md", 5,
		scan.Usage{Input: 20, Output: 2_500, CacheRead: 120_000, CacheWrite: 18_000})
	a5 := mk("a5-planner", sid, "Plan the benchmark", "Plan", opus, scan.Done, 2*time.Minute, "", 9,
		scan.Usage{Input: 25, Output: 6_000, CacheRead: 300_000, CacheWrite: 30_000})
	a6 := mk("a6-gifcheck", "a2-builder", "Check GIF disposal", "Explore", haiku, scan.Done, 4*time.Minute, "", 4,
		scan.Usage{Input: 10, Output: 1_200, CacheRead: 60_000, CacheWrite: 9_000})
	a7 := mk("a7-audit", sid, "Old licence audit", "general-purpose", sonnet, scan.Done, 25*time.Minute, "", 6,
		scan.Usage{Input: 15, Output: 3_000, CacheRead: 150_000, CacheWrite: 12_000})
	a2.Children = []*scan.Agent{a6}
	lead.Children = []*scan.Agent{a1, a2, a3, a4, a5, a7}

	s := scan.Snapshot{Roots: []*scan.Agent{lead}, ByID: map[string]*scan.Agent{}, ByModel: map[string]scan.Usage{}, ScannedAt: now}
	scan.Walk(lead, 0, func(a *scan.Agent, _ int) {
		s.ByID[a.ID] = a
		s.ByModel[a.Model] = s.ByModel[a.Model].Add(a.Usage)
	})
	e := func(from, to, kind, label string, d time.Duration) scan.Edge {
		return scan.Edge{From: from, To: to, Kind: kind, Label: label, At: ago(d)}
	}
	s.Edges = []scan.Edge{
		e(sid, "a7-audit", scan.EdgeSpawn, "Old licence audit", 40*time.Minute),
		e("a7-audit", sid, scan.EdgeHandback, "handback", 25*time.Minute),
		e(sid, "a5-planner", scan.EdgeSpawn, "Plan the benchmark", 9*time.Minute),
		e(sid, "a2-builder", scan.EdgeSpawn, "Build the pixel encoder", 8*time.Minute),
		e("a2-builder", "a6-gifcheck", scan.EdgeSpawn, "Check GIF disposal", 7*time.Minute),
		e(sid, "a4-reviewer", scan.EdgeSpawn, "Review the README", 6*time.Minute),
		e("a6-gifcheck", "a2-builder", scan.EdgeHandback, "handback", 4*time.Minute),
		e(sid, "a4-reviewer", scan.EdgeMessage, "Also check the keys table", 3*time.Minute),
		e(sid, "a3-tester", scan.EdgeSpawn, "Write layout tests", 50*time.Second),
		e(sid, "a1-explore", scan.EdgeSpawn, "Map the scanner", 20*time.Second),
		e("a2-builder", "a3-tester", scan.EdgeMessage, "encoder API is ready", 30*time.Second),
		e("a5-planner", sid, scan.EdgeHandback, "handback", 8*time.Second),
	}
	return s
}
