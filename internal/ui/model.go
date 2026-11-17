// Package ui is the Bubble Tea front end of claude-monitor.
package ui

import (
	"time"

	"charm.land/bubbles/v2/help"
	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"

	"github.com/cicingik/claude-monitor/internal/office"
	"github.com/cicingik/claude-monitor/internal/scan"
)

const pollEvery = time.Second

var tabNames = []string{"Activity", "Interactions", "Usage"}

type keyMap struct {
	Up, Down, Toggle, Tab, Tab1, Tab2, Tab3, Rescan, Quit, View key.Binding
}

var keys = keyMap{
	Up:     key.NewBinding(key.WithKeys("up", "k"), key.WithHelp("↑/k", "up")),
	Down:   key.NewBinding(key.WithKeys("down", "j"), key.WithHelp("↓/j", "down")),
	Toggle: key.NewBinding(key.WithKeys("enter", "space"), key.WithHelp("enter", "collapse/expand")),
	Tab:    key.NewBinding(key.WithKeys("tab"), key.WithHelp("tab", "next tab")),
	Tab1:   key.NewBinding(key.WithKeys("1"), key.WithHelp("1-3", "tab")),
	Tab2:   key.NewBinding(key.WithKeys("2")),
	Tab3:   key.NewBinding(key.WithKeys("3")),
	Rescan: key.NewBinding(key.WithKeys("r"), key.WithHelp("r", "rescan")),
	Quit:   key.NewBinding(key.WithKeys("q", "ctrl+c"), key.WithHelp("q", "quit")),
	View:   key.NewBinding(key.WithKeys("v"), key.WithHelp("v", "office")),
}

func (k keyMap) ShortHelp() []key.Binding {
	return []key.Binding{k.Up, k.Down, k.Toggle, k.Tab, k.Tab1, k.View, k.Rescan, k.Quit}
}

// Model is the Bubble Tea model. The snapshot is replaced wholesale on each scan.
type Model struct {
	scanner   *scan.Scanner
	snap      scan.Snapshot
	scanned   bool
	collapsed map[string]bool
	selected  string // agent id
	cursor    int    // index of selected in rows()
	offset    int    // first visible tree row
	tab       int
	width     int
	height    int
	help      help.Model
	cfg       Config
	off       officeState
}

// Config holds the optional parts of the UI.
type Config struct {
	Office bool         // start in the office view instead of the tree
	Cast   *office.Cast // custom character sprites; nil draws built-in ones
}

// WithConfig applies c to a new model.
func (m Model) WithConfig(c Config) Model {
	m.cfg = c
	m.off.on = c.Office
	m.off.animating = c.Office
	return m
}

// snapshotMsg carries a finished scan. chain marks scans from the poll loop,
// which schedule the next tick; forced rescans do not, so only one loop runs.
type snapshotMsg struct {
	snap  scan.Snapshot
	chain bool
}

type tickMsg struct{}

func New(s *scan.Scanner) Model {
	return Model{scanner: s, collapsed: map[string]bool{}, help: help.New(), width: 100, height: 30,
		off: officeState{walker: &office.Walker{}}}
}

func (m Model) Init() tea.Cmd {
	if m.off.animating {
		return tea.Batch(m.scanCmd(false, true), frameTick())
	}
	return m.scanCmd(false, true)
}

func (m Model) scanCmd(reset, chain bool) tea.Cmd {
	s := m.scanner
	return func() tea.Msg {
		if reset {
			s.Reset()
		}
		return snapshotMsg{snap: s.Scan(time.Now()), chain: chain}
	}
}

func (m Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	switch msg := msg.(type) {
	case snapshotMsg:
		m.snap, m.scanned = msg.snap, true
		m.syncCursor()
		if m.off.now.IsZero() {
			m.off.now = msg.snap.ScannedAt
		}
		if msg.chain {
			cmd = tea.Tick(pollEvery, func(time.Time) tea.Msg { return tickMsg{} })
		}
	case tickMsg:
		cmd = m.scanCmd(false, true)
	case frameMsg:
		if !m.off.on {
			m.off.animating = false
			return m, nil
		}
		m.off.now = time.Time(msg)
		cmd = frameTick()
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
		m.syncCursor()
	case tea.KeyPressMsg:
		if m.off.on {
			m, cmd = m.officeKey(msg)
		} else {
			m, cmd = m.classicKey(msg)
		}
	}
	if m.off.on {
		m.refreshScene()
	}
	return m, cmd
}

func (m Model) classicKey(msg tea.KeyPressMsg) (Model, tea.Cmd) {
	switch {
	case key.Matches(msg, keys.Quit):
		return m, tea.Quit
	case key.Matches(msg, keys.Up):
		m.move(-1)
	case key.Matches(msg, keys.Down):
		m.move(1)
	case key.Matches(msg, keys.Toggle):
		if a := m.snap.ByID[m.selected]; a != nil && len(a.Children) > 0 {
			m.collapsed[a.ID] = !m.collapsed[a.ID]
			m.syncCursor()
		}
	case key.Matches(msg, keys.Tab):
		m.tab = (m.tab + 1) % len(tabNames)
	case key.Matches(msg, keys.Tab1):
		m.tab = 0
	case key.Matches(msg, keys.Tab2):
		m.tab = 1
	case key.Matches(msg, keys.Tab3):
		m.tab = 2
	case key.Matches(msg, keys.Rescan):
		return m, m.scanCmd(true, false)
	case key.Matches(msg, keys.View):
		return m.enterOffice()
	}
	return m, nil
}

type row struct {
	a     *scan.Agent
	depth int
}

// rows flattens the tree, skipping children of collapsed agents.
func (m Model) rows() []row {
	var out []row
	var add func(a *scan.Agent, depth int)
	add = func(a *scan.Agent, depth int) {
		out = append(out, row{a, depth})
		if !m.collapsed[a.ID] {
			for _, c := range a.Children {
				add(c, depth+1)
			}
		}
	}
	for _, r := range m.snap.Roots {
		add(r, 0)
	}
	return out
}

func (m *Model) move(d int) {
	rows := m.rows()
	if len(rows) == 0 {
		return
	}
	m.cursor = min(max(m.cursor+d, 0), len(rows)-1)
	m.selected = rows[m.cursor].a.ID
	m.scrollToCursor()
}

// syncCursor re-finds the selected agent after the rows changed. A hidden or
// vanished selection moves to its nearest visible ancestor, else the first row.
func (m *Model) syncCursor() {
	rows := m.rows()
	idx := map[string]int{}
	for i, r := range rows {
		idx[r.a.ID] = i
	}
	for id := m.selected; id != ""; {
		if i, ok := idx[id]; ok {
			m.cursor, m.selected = i, id
			m.scrollToCursor()
			return
		}
		a := m.snap.ByID[id]
		if a == nil {
			break
		}
		id = a.ParentID
	}
	m.cursor, m.selected = 0, ""
	if len(rows) > 0 {
		m.selected = rows[0].a.ID
	}
	m.scrollToCursor()
}

func (m *Model) scrollToCursor() {
	h := m.treeHeight()
	if m.cursor < m.offset {
		m.offset = m.cursor
	}
	if m.cursor >= m.offset+h {
		m.offset = m.cursor - h + 1
	}
	m.offset = max(m.offset, 0)
}
