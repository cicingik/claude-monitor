package scan

import (
	"encoding/json"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

const (
	runningWindow = 2 * time.Minute
	activeWindow  = 30 * time.Second
)

// meta is the subset of agent-<id>.meta.json the monitor uses.
type meta struct {
	AgentType   string `json:"agentType"`
	Description string `json:"description"`
	ToolUseID   string `json:"toolUseId"`
	SpawnDepth  int    `json:"spawnDepth"`
	Finished    *bool  `json:"finished"`
}

// Scanner keeps per-file parse state between scans so each Scan only reads
// lines appended since the previous one. It is safe for concurrent use.
type Scanner struct {
	Root  string
	Since time.Duration // sessions whose file is older than this are skipped; <=0 keeps all

	mu    sync.Mutex
	files map[string]*fileState
}

func New(root string, since time.Duration) *Scanner {
	return &Scanner{Root: root, Since: since, files: map[string]*fileState{}}
}

// Reset drops all parse state so the next Scan re-reads every file.
func (s *Scanner) Reset() {
	s.mu.Lock()
	s.files = map[string]*fileState{}
	s.mu.Unlock()
}

// Scan reads new transcript lines and returns a fresh snapshot.
func (s *Scanner) Scan(now time.Time) Snapshot {
	s.mu.Lock()
	defer s.mu.Unlock()

	snap := Snapshot{ByID: map[string]*Agent{}, ByModel: map[string]Usage{}, ScannedAt: now}
	seen := map[string]bool{}
	projects, _ := os.ReadDir(s.Root)
	for _, p := range projects {
		if !p.IsDir() {
			continue
		}
		projDir := filepath.Join(s.Root, p.Name())
		entries, _ := os.ReadDir(projDir)
		for _, e := range entries {
			if e.IsDir() || !strings.HasSuffix(e.Name(), ".jsonl") {
				continue
			}
			info, err := e.Info()
			if err != nil || (s.Since > 0 && now.Sub(info.ModTime()) > s.Since) {
				continue
			}
			s.session(&snap, seen, projDir, p.Name(), strings.TrimSuffix(e.Name(), ".jsonl"), info, now)
		}
	}
	for path := range s.files {
		if !seen[path] {
			delete(s.files, path)
		}
	}
	sort.SliceStable(snap.Roots, func(i, j int) bool {
		return snap.Roots[i].LastActivity.After(snap.Roots[j].LastActivity)
	})
	sort.SliceStable(snap.Edges, func(i, j int) bool { return snap.Edges[i].At.Before(snap.Edges[j].At) })
	for _, a := range snap.ByID {
		for m, u := range a.ModelUsage {
			snap.ByModel[m] = snap.ByModel[m].Add(u)
		}
	}
	return snap
}

// load advances the parse state of path to the current end of file.
func (s *Scanner) load(seen map[string]bool, path string, info fs.FileInfo) *fileState {
	seen[path] = true
	st := s.files[path]
	if st == nil || info.Size() < st.offset { // new, or truncated and rewritten
		st = newFileState()
		s.files[path] = st
	}
	if info.Size() > st.offset {
		_ = st.readFrom(path) // unreadable: keep what was parsed
	}
	return st
}

type member struct {
	a  *Agent
	st *fileState
	m  meta
}

func (s *Scanner) session(snap *Snapshot, seen map[string]bool, projDir, projName, sid string, info fs.FileInfo, now time.Time) {
	path := filepath.Join(projDir, sid+".jsonl")
	st := s.load(seen, path, info)
	root := build(st, sid, path, info.ModTime(), now, false)
	root.Kind, root.SessionID, root.Title = Session, sid, Truncate(st.title, 60)
	root.Project = projectName(st.cwd, projName)

	members := map[string]*member{sid: {a: root, st: st}}
	order := []string{sid}

	subDir := filepath.Join(projDir, sid, "subagents")
	entries, _ := os.ReadDir(subDir)
	for _, e := range entries {
		name := e.Name()
		if e.IsDir() || !strings.HasPrefix(name, "agent-") || !strings.HasSuffix(name, ".jsonl") {
			continue
		}
		info, err := e.Info()
		if err != nil {
			continue
		}
		id := strings.TrimSuffix(strings.TrimPrefix(name, "agent-"), ".jsonl")
		p := filepath.Join(subDir, name)
		var m meta
		if b, err := os.ReadFile(filepath.Join(subDir, "agent-"+id+".meta.json")); err == nil {
			_ = json.Unmarshal(b, &m)
		}
		sst := s.load(seen, p, info)
		a := build(sst, id, p, info.ModTime(), now, m.Finished != nil && *m.Finished)
		a.Kind, a.SessionID, a.Project = Subagent, sid, root.Project
		a.Title, a.AgentType = Truncate(m.Description, 60), m.AgentType
		members[id] = &member{a: a, st: sst, m: m}
		order = append(order, id)
	}

	// Which transcript holds each Agent/Task tool_use, and spawn names.
	owner := map[string]string{}
	byName := map[string]string{}
	for _, id := range order {
		for tid := range members[id].st.spawns {
			owner[tid] = id
		}
	}
	for _, id := range order[1:] {
		mb := members[id]
		mb.a.ParentID = sid
		if o, ok := owner[mb.m.ToolUseID]; ok && o != id {
			mb.a.ParentID = o
		}
		if sp, ok := members[mb.a.ParentID].st.spawns[mb.m.ToolUseID]; ok && sp.Name != "" {
			byName[sp.Name] = id
		}
	}
	for _, id := range order[1:] { // break any cycle by reattaching to the session
		if cyclic(members, id) {
			members[id].a.ParentID = sid
		}
	}
	for _, id := range order[1:] {
		mb := members[id]
		parent := members[mb.a.ParentID].a
		parent.Children = append(parent.Children, mb.a)
		sp, ok := members[mb.a.ParentID].st.spawns[mb.m.ToolUseID]
		e := Edge{From: parent.ID, To: id, Kind: EdgeSpawn, At: sp.At, Label: Truncate(sp.Description, 40)}
		if !ok || e.At.IsZero() {
			e.At = mb.st.firstAt
		}
		if e.Label == "" {
			e.Label = Truncate(mb.m.Description, 40)
		}
		snap.Edges = append(snap.Edges, e)
	}
	for _, id := range order {
		mb := members[id]
		sort.SliceStable(mb.a.Children, func(i, j int) bool {
			return members[mb.a.Children[i].ID].st.firstAt.Before(members[mb.a.Children[j].ID].st.firstAt)
		})
		for _, sd := range mb.st.sends {
			to := sd.To
			if _, ok := members[to]; !ok {
				if n, ok := byName[to]; ok {
					to = n
				}
			}
			snap.Edges = append(snap.Edges, Edge{From: id, To: to, Kind: EdgeMessage, At: sd.At, Label: sd.Label})
		}
		for _, hb := range mb.st.handbacks {
			snap.Edges = append(snap.Edges, Edge{From: hb.To, To: id, Kind: EdgeHandback, At: hb.At, Label: "handback"})
		}
		snap.ByID[id] = mb.a
	}
	snap.Roots = append(snap.Roots, root)
}

func cyclic(members map[string]*member, id string) bool {
	visited := map[string]bool{}
	for cur := id; ; {
		if visited[cur] {
			return true
		}
		visited[cur] = true
		mb, ok := members[cur]
		if !ok || mb.a.ParentID == "" {
			return false
		}
		cur = mb.a.ParentID
	}
}

// build turns parse state into an Agent with status, usage and events.
func build(st *fileState, id, path string, mtime, now time.Time, finished bool) *Agent {
	a := &Agent{
		ID: id, Path: path, Model: st.model, LastTool: st.lastTool,
		Turns: len(st.usage), LastActivity: st.lastAt,
		ModelUsage: map[string]Usage{},
		Events:     append([]Event(nil), st.events...),
	}
	if a.LastActivity.IsZero() {
		a.LastActivity = mtime
	}
	for mid, u := range st.usage {
		a.Usage = a.Usage.Add(u)
		m := st.msgModel[mid]
		if m == "" {
			m = "unknown"
		}
		a.ModelUsage[m] = a.ModelUsage[m].Add(u)
	}
	age := now.Sub(mtime)
	switch {
	case finished:
		a.Status = Done
	case st.pending() && age < runningWindow:
		a.Status = Running
	case age < activeWindow:
		a.Status = Active
	default:
		a.Status = Idle
	}
	return a
}

// projectName is the last segment of the session cwd, or of the encoded dir name.
func projectName(cwd, dir string) string {
	if cwd != "" {
		return filepath.Base(cwd)
	}
	if i := strings.LastIndex(dir, "-"); i >= 0 && i < len(dir)-1 {
		return dir[i+1:]
	}
	return dir
}
