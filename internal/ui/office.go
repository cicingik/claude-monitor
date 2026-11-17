package ui

import (
	"fmt"
	"image"
	"sort"
	"strings"
	"time"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/cicingik/claude-monitor/internal/office"
	"github.com/cicingik/claude-monitor/internal/pixel"
	"github.com/cicingik/claude-monitor/internal/scan"
)

const (
	frameEvery = 125 * time.Millisecond // office animation, separate from the 1s scan poll
	stripRows  = 4
	chromeRows = 1 + stripRows + 2 // header, strip, footer, help
	minW, minH = 80, 24
)

// officeState is the office view's part of the model.
type officeState struct {
	on, animating bool
	session       string // shown session id
	focus         string // selected agent id
	now           time.Time
	scene         office.Scene
	walker        *office.Walker
	page          int
	size          image.Point
}

type frameMsg time.Time

func frameTick() tea.Cmd {
	return tea.Tick(frameEvery, func(t time.Time) tea.Msg { return frameMsg(t) })
}

var officeKeys = struct {
	Prev, Next, PrevSession, NextSession, Enter key.Binding
}{
	Prev:        key.NewBinding(key.WithKeys("up", "k", "shift+tab"), key.WithHelp("↑↓/jk/tab", "select")),
	Next:        key.NewBinding(key.WithKeys("down", "j", "tab")),
	PrevSession: key.NewBinding(key.WithKeys("[", "left"), key.WithHelp("[ ]", "session")),
	NextSession: key.NewBinding(key.WithKeys("]", "right")),
	Enter:       key.NewBinding(key.WithKeys("enter"), key.WithHelp("enter", "open in tree")),
}

func (m Model) enterOffice() (Model, tea.Cmd) {
	m.off.on = true
	m.off.walker.Reset()
	if m.off.focus == "" {
		m.off.focus = m.selected
	}
	if a := m.snap.ByID[m.off.focus]; a != nil {
		m.off.session = a.SessionID
	}
	if m.off.animating {
		return m, nil
	}
	m.off.animating = true
	return m, frameTick()
}

func (m Model) officeKey(msg tea.KeyPressMsg) (Model, tea.Cmd) {
	switch {
	case key.Matches(msg, keys.Quit):
		return m, tea.Quit
	case key.Matches(msg, keys.View):
		m.off.on = false
	case key.Matches(msg, officeKeys.Enter):
		m.off.on = false
		if a := m.snap.ByID[m.off.focus]; a != nil {
			for p := a.ParentID; p != ""; p = m.snap.ByID[p].ParentID {
				delete(m.collapsed, p)
				if m.snap.ByID[p] == nil {
					break
				}
			}
			m.selected = a.ID
			m.syncCursor()
		}
	case key.Matches(msg, officeKeys.Prev):
		m.off.focus = cycle(m.off.scene.Roster, m.off.focus, -1)
	case key.Matches(msg, officeKeys.Next):
		m.off.focus = cycle(m.off.scene.Roster, m.off.focus, 1)
	case key.Matches(msg, officeKeys.PrevSession), key.Matches(msg, officeKeys.NextSession):
		d := 1
		if key.Matches(msg, officeKeys.PrevSession) {
			d = -1
		}
		ids := make([]string, len(m.snap.Roots))
		for i, r := range m.snap.Roots {
			ids[i] = r.ID
		}
		m.off.session = cycle(ids, m.off.session, d)
		m.off.focus = m.off.session
		m.off.walker.Reset()
	case key.Matches(msg, keys.Rescan):
		return m, m.scanCmd(true, false)
	}
	return m, nil
}

// cycle returns the element d steps from cur in ids, wrapping around.
func cycle(ids []string, cur string, d int) string {
	if len(ids) == 0 {
		return cur
	}
	for i, id := range ids {
		if id == cur {
			return ids[((i+d)%len(ids)+len(ids))%len(ids)]
		}
	}
	return ids[0]
}

func (m Model) canvasSize() image.Point { return CanvasSize(m.width, m.height) }

// CanvasSize is the office canvas, in pixels, of a cols×rows terminal.
func CanvasSize(cols, rows int) image.Point { return image.Pt(cols, 2*(rows-chromeRows)) }

// refreshScene lays out the shown session for the current frame. The session
// falls back to the most recently active one, the focus to its lead.
func (m *Model) refreshScene() {
	o := &m.off
	if r := m.snap.ByID[o.session]; r == nil || r.Kind != scan.Session {
		o.session = ""
		if len(m.snap.Roots) > 0 {
			o.session = m.snap.Roots[0].ID
		}
	}
	if a := m.snap.ByID[o.focus]; a == nil || a.SessionID != o.session {
		o.focus = o.session
	}
	size := m.canvasSize()
	sc := office.LayoutFocus(m.snap, o.session, size.X, size.Y, o.now, o.focus)
	if len(sc.Roster) > 0 && !contains(sc.Roster, o.focus) {
		o.focus = sc.Roster[0]
		sc = office.LayoutFocus(m.snap, o.session, size.X, size.Y, o.now, o.focus)
	}
	m.cfg.Cast.Dress(&sc)
	if sc.Page != o.page || size != o.size {
		o.walker.Reset()
		o.page, o.size = sc.Page, size
	}
	o.walker.Step(&sc, o.now)
	o.scene = sc
}

func contains(ids []string, id string) bool {
	for _, x := range ids {
		if x == id {
			return true
		}
	}
	return false
}

func (m Model) officeView() string {
	if !m.scanned {
		return headerStyle.Render("claude-monitor") + "  scanning…"
	}
	if m.width < minW || m.height < minH {
		return fmt.Sprintf("claude-monitor: the office needs at least %dx%d (now %dx%d) · v for the tree · q quits", minW, minH, m.width, m.height)
	}
	canvas := pixel.Encode(office.Render(m.off.scene, m.off.now), office.Background)
	lines := []string{m.officeHeader(), canvas}
	lines = append(lines, m.strip()...)
	lines = append(lines, m.officeFooter(), m.officeHelp())
	for i, l := range lines {
		if i != 1 {
			lines[i] = clip([]string{l}, m.width, 1)
		}
	}
	return strings.Join(lines, "\n")
}

func (m Model) officeHeader() string {
	root := m.snap.ByID[m.off.session]
	if root == nil {
		return headerStyle.Render("claude-monitor") + "  no sessions in the time window"
	}
	idx := 0
	for i, r := range m.snap.Roots {
		if r.ID == root.ID {
			idx = i + 1
		}
	}
	agents, running := 0, 0
	scan.Walk(root, 0, func(a *scan.Agent, _ int) {
		agents++
		if a.Status == scan.Running {
			running++
		}
	})
	page := ""
	if sc := m.off.scene; sc.Pages > 1 {
		page = fmt.Sprintf(" · page %d/%d", sc.Page+1, sc.Pages)
	}
	title := bold.Render(label(root))
	if root.Title != "" {
		title += faint.Render(" · " + root.Project)
	}
	return headerStyle.Render("claude-monitor") + fmt.Sprintf("  ◀ %d/%d ▶ ", idx, len(m.snap.Roots)) + title +
		fmt.Sprintf("  │  agents %d · running %d%s  │  %s", agents, running, page, m.snap.ScannedAt.Local().Format("15:04:05"))
}

// strip is the text under the canvas: the selected agent, its edges and what
// running agents are doing (the speech bubbles' text).
func (m Model) strip() []string {
	out := make([]string, stripRows)
	a := m.snap.ByID[m.off.focus]
	if a == nil {
		out[0] = faint.Render("no agent selected")
		return out
	}
	now := m.off.now
	st := lipgloss.NewStyle().Foreground(statusColor[a.Status])
	out[0] = st.Render("▶ "+a.Status.Glyph()+" ") + bold.Render(label(a)) + "  " +
		fmt.Sprintf("%s · %s · %s", agentType(a), shortModel(a.Model), st.Render(a.Status.String()))
	if a.Status == scan.Running && a.LastTool != "" {
		out[0] += "  " + st.Bold(true).Render("now: "+a.LastTool)
	}
	u := a.Usage
	out[1] = fmt.Sprintf("  tokens %s (in %s · out %s · cache r %s w %s) · turns %d · last activity %s ago",
		humanTokens(u.Total()), humanTokens(u.Input), humanTokens(u.Output), humanTokens(u.CacheRead), humanTokens(u.CacheWrite),
		a.Turns, age(now, a.LastActivity))

	var edges []string
	for i := len(m.snap.Edges) - 1; i >= 0; i-- {
		e := m.snap.Edges[i]
		var s string
		switch a.ID {
		case e.From:
			s = "→ " + e.Kind + " " + m.who(e.To)
		case e.To:
			s = "← " + e.Kind + " " + m.who(e.From)
		default:
			continue
		}
		s += " " + age(now, e.At)
		if now.Sub(e.At) < office.LinkTTL {
			s = lipgloss.NewStyle().Foreground(office.LinkColor[e.Kind]).Render(s)
		}
		edges = append(edges, s)
	}
	out[2] = "  edges " + faint.Render("(none)")
	if len(edges) > 0 {
		out[2] = "  edges " + strings.Join(edges, faint.Render(" · "))
	}

	var busy []string
	for _, x := range m.off.scene.Actors {
		if x.Bubble != "" {
			busy = append(busy, bold.Render(m.who(x.AgentID))+": "+x.Bubble)
		}
	}
	out[3] = "  busy " + faint.Render("nobody is running a tool")
	if len(busy) > 0 {
		out[3] = "  busy " + strings.Join(busy, faint.Render(" · "))
	}
	return out
}

// who is a short name for an agent in the strip.
func (m Model) who(id string) string {
	a := m.snap.ByID[id]
	switch {
	case a == nil:
		return shortID(id)
	case a.Kind == scan.Session:
		return "lead"
	case a.Title != "":
		return scan.Truncate(a.Title, 18)
	}
	return shortID(id)
}

func (m Model) officeFooter() string {
	root := m.snap.ByID[m.off.session]
	if root == nil {
		return ""
	}
	byModel := map[string]scan.Usage{}
	scan.Walk(root, 0, func(a *scan.Agent, _ int) {
		for k, u := range a.ModelUsage {
			byModel[k] = byModel[k].Add(u)
		}
	})
	models := make([]string, 0, len(byModel))
	for k := range byModel {
		models = append(models, k)
	}
	sort.Slice(models, func(i, j int) bool { return byModel[models[i]].Total() > byModel[models[j]].Total() })
	parts := []string{bold.Render("session " + humanTokens(scan.SubtreeUsage(root).Total()))}
	for _, k := range models {
		parts = append(parts, shortModel(k)+" "+humanTokens(byModel[k].Total()))
	}
	s := strings.Join(parts, faint.Render(" · "))
	if n := m.off.scene.Left; n > 0 {
		s += faint.Render(fmt.Sprintf("  │  %d left the office (done > 10m)", n))
	}
	return s
}

func (m Model) officeHelp() string {
	h := m.help.ShortHelpView([]key.Binding{officeKeys.Prev, officeKeys.PrevSession, officeKeys.Enter,
		key.NewBinding(key.WithKeys("v"), key.WithHelp("v", "tree")), keys.Rescan, keys.Quit})
	if c := m.cfg.Cast; c != nil && c.Note != "" {
		h += faint.Render("  │  " + c.Note)
	}
	return h
}
