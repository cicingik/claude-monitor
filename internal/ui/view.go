package ui

import (
	"fmt"
	"image/color"
	"sort"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/cicingik/claude-monitor/internal/scan"
)

var (
	paneStyle   = lipgloss.NewStyle().Border(lipgloss.RoundedBorder()).BorderForeground(lipgloss.Color("240"))
	headerStyle = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("212"))
	faint       = lipgloss.NewStyle().Faint(true)
	bold        = lipgloss.NewStyle().Bold(true)
	selStyle    = lipgloss.NewStyle().Reverse(true).Bold(true)
	barStyle    = lipgloss.NewStyle().Foreground(lipgloss.Color("63"))
	statusColor = map[scan.Status]color.Color{
		scan.Running: lipgloss.Color("42"),
		scan.Active:  lipgloss.Color("220"),
		scan.Idle:    lipgloss.Color("244"),
		scan.Done:    lipgloss.Color("39"),
	}
)

func glyph(s scan.Status) string {
	return lipgloss.NewStyle().Foreground(statusColor[s]).Render(s.Glyph())
}

func (m Model) View() tea.View {
	content := m.render
	if m.off.on {
		content = m.officeView
	}
	v := tea.NewView(content())
	v.AltScreen = true
	v.WindowTitle = "claude-monitor"
	return v
}

func (m Model) bodyHeight() int { return max(m.height-2, 4) }
func (m Model) treeHeight() int { return m.bodyHeight() - 2 }

func (m Model) render() string {
	bodyH := m.bodyHeight()
	lw := min(max(m.width/2, 30), max(m.width-24, 30))
	rw := max(m.width-lw, 24)
	left := paneStyle.Width(lw).Render(clip(m.treeLines(lw-2), lw-2, bodyH-2))
	right := paneStyle.Width(rw).Render(clip(m.detailLines(rw-2), rw-2, bodyH-2))
	return lipgloss.JoinVertical(lipgloss.Left,
		m.header(),
		lipgloss.JoinHorizontal(lipgloss.Top, left, right),
		m.help.ShortHelpView(keys.ShortHelp()),
	)
}

// clip cuts every line to w cells and pads or cuts the block to h lines.
func clip(lines []string, w, h int) string {
	out := make([]string, h)
	for i := range out {
		if i < len(lines) {
			out[i] = ansi.Truncate(lines[i], w, "…")
		}
	}
	return strings.Join(out, "\n")
}

func (m Model) header() string {
	if !m.scanned {
		return headerStyle.Render("claude-monitor") + "  scanning…"
	}
	running := 0
	for _, a := range m.snap.ByID {
		if a.Status == scan.Running {
			running++
		}
	}
	since := "all"
	if m.scanner != nil && m.scanner.Since > 0 {
		since = shortDuration(m.scanner.Since)
	}
	return headerStyle.Render("claude-monitor") + fmt.Sprintf("  sessions %d · agents %d · running %d  │  scanned %s  │  since %s",
		len(m.snap.Roots), len(m.snap.ByID), running, m.snap.ScannedAt.Local().Format("15:04:05"), since)
}

func (m Model) treeLines(w int) []string {
	rows := m.rows()
	if len(rows) == 0 {
		return []string{faint.Render("no sessions in the time window")}
	}
	var out []string
	for i := m.offset; i < min(m.offset+m.treeHeight(), len(rows)); i++ {
		out = append(out, m.treeRow(rows[i], w, i == m.cursor))
	}
	return out
}

func (m Model) treeRow(r row, w int, sel bool) string {
	a := r.a
	caret := " "
	if len(a.Children) > 0 {
		caret = "▾"
		if m.collapsed[a.ID] {
			caret = "▸"
		}
	}
	prefix := strings.Repeat("  ", r.depth) + caret + " "
	cols := fmt.Sprintf(" %s %6s %4s", fit(shortModel(a.Model), 10), humanTokens(a.Usage.Total()), age(m.snap.ScannedAt, a.LastActivity))
	if w >= 60 {
		cols = " " + fit(agentType(a), 15) + cols
	}
	title := fit(label(a), w-len([]rune(prefix))-2-len([]rune(cols)))
	if sel {
		return selStyle.Render(prefix + a.Status.Glyph() + " " + title + cols)
	}
	return faint.Render(prefix) + glyph(a.Status) + " " + title + faint.Render(cols)
}

func (m Model) detailLines(w int) []string {
	var tabs []string
	for i, n := range tabNames {
		t := fmt.Sprintf(" %d %s ", i+1, n)
		if i == m.tab {
			t = selStyle.Render(t)
		} else {
			t = faint.Render(t)
		}
		tabs = append(tabs, t)
	}
	out := []string{strings.Join(tabs, " "), faint.Render(strings.Repeat("─", w))}
	a := m.snap.ByID[m.selected]
	if a == nil {
		return append(out, faint.Render("nothing selected"))
	}
	switch m.tab {
	case 1:
		return append(out, m.interactions(a)...)
	case 2:
		return append(out, m.usage(a, w)...)
	}
	return append(out, m.activity(a)...)
}

func (m Model) activity(a *scan.Agent) []string {
	out := []string{
		glyph(a.Status) + " " + bold.Render(label(a)),
		fmt.Sprintf("%s · %s · %s · %s", agentType(a), shortModel(a.Model), a.Status, a.Project),
		faint.Render("id " + a.ID),
		fmt.Sprintf("turns %d · tokens %s (%s)", a.Turns, humanTokens(a.Usage.Total()), usageDetail(a.Usage)),
		fmt.Sprintf("last activity %s ago", age(m.snap.ScannedAt, a.LastActivity)),
	}
	if a.Status == scan.Running {
		out = append(out, lipgloss.NewStyle().Foreground(statusColor[scan.Running]).Bold(true).Render("now: "+a.LastTool))
	}
	out = append(out, "", bold.Render("Recent events"))
	if len(a.Events) == 0 {
		out = append(out, faint.Render("  (none)"))
	}
	for i := len(a.Events) - 1; i >= 0; i-- {
		e := a.Events[i]
		out = append(out, faint.Render(e.At.Local().Format("15:04:05"))+" "+fit(eventKind(e.Kind), 8)+" "+e.Summary)
	}
	return out
}

func eventKind(k string) string {
	switch k {
	case "tool_use":
		return "tool"
	case "text":
		return "reply"
	}
	return k
}

// name is the short display form of an agent id used in edges and the graph.
func (m Model) name(id string) string {
	if a := m.snap.ByID[id]; a != nil {
		return shortID(id) + " " + agentType(a)
	}
	return id
}

func (m Model) interactions(a *scan.Agent) []string {
	out := []string{bold.Render("Edges of " + shortID(a.ID))}
	n := 0
	for _, e := range m.snap.Edges {
		var s string
		switch a.ID {
		case e.From:
			s = fmt.Sprintf("→ %-8s to   %s", e.Kind, m.name(e.To))
		case e.To:
			s = fmt.Sprintf("← %-8s from %s", e.Kind, m.name(e.From))
		default:
			continue
		}
		if e.Label != "" && e.Kind != scan.EdgeHandback {
			s += " " + faint.Render("\""+e.Label+"\"")
		}
		out = append(out, "  "+s+" "+faint.Render(e.At.Local().Format("15:04")))
		n++
	}
	if n == 0 {
		out = append(out, faint.Render("  (none)"))
	}
	root := m.snap.ByID[a.SessionID]
	if root == nil {
		return out
	}
	return append(append(out, "", bold.Render("Session graph")), m.graph(root)...)
}

// graph draws the session tree with spawn labels on the branches and each
// agent's outgoing messages and handbacks as annotations.
func (m Model) graph(root *scan.Agent) []string {
	spawnLabel := map[string]string{}
	notes := map[string]map[string]int{}
	for _, e := range m.snap.Edges {
		switch e.Kind {
		case scan.EdgeSpawn:
			spawnLabel[e.To] = e.Label
		case scan.EdgeMessage:
			if notes[e.From] == nil {
				notes[e.From] = map[string]int{}
			}
			notes[e.From]["⇢msg "+shortID(e.To)]++
		case scan.EdgeHandback:
			if notes[e.From] == nil {
				notes[e.From] = map[string]int{}
			}
			notes[e.From]["⇠handback "+shortID(e.To)]++
		}
	}
	annotate := func(id string) string {
		var parts []string
		for k, n := range notes[id] {
			parts = append(parts, fmt.Sprintf("[%s ×%d]", k, n))
		}
		sort.Strings(parts)
		if len(parts) == 0 {
			return ""
		}
		return " " + faint.Render(strings.Join(parts, " "))
	}
	out := []string{glyph(root.Status) + " " + shortID(root.ID) + " session" + annotate(root.ID)}
	var rec func(a *scan.Agent, prefix string)
	rec = func(a *scan.Agent, prefix string) {
		for i, c := range a.Children {
			branch, next := "├─", prefix+"│  "
			if i == len(a.Children)-1 {
				branch, next = "└─", prefix+"   "
			}
			out = append(out, faint.Render(prefix+branch+"spawn \""+spawnLabel[c.ID]+"\"─▶ ")+
				glyph(c.Status)+" "+m.name(c.ID)+annotate(c.ID))
			rec(c, next)
		}
	}
	rec(root, "")
	return out
}

func (m Model) usage(a *scan.Agent, w int) []string {
	root := m.snap.ByID[a.SessionID]
	if root == nil {
		root = a
	}
	total := scan.SubtreeUsage(root)
	out := []string{bold.Render("Session "+shortID(root.ID)) + "  total " + humanTokens(total.Total()), ""}

	var peak int64
	byModel := map[string]scan.Usage{}
	scan.Walk(root, 0, func(x *scan.Agent, _ int) {
		peak = max(peak, x.Usage.Total())
		for k, u := range x.ModelUsage {
			byModel[k] = byModel[k].Add(u)
		}
	})
	barW := max(w-22-8, 4)
	scan.Walk(root, 0, func(x *scan.Agent, depth int) {
		n := 0
		if peak > 0 {
			n = int(float64(barW) * float64(x.Usage.Total()) / float64(peak))
		}
		name := fit(strings.Repeat(" ", depth)+label(x), 20)
		out = append(out, name+"  "+barStyle.Render(strings.Repeat("█", n))+faint.Render(strings.Repeat("░", barW-n))+
			fmt.Sprintf(" %6s", humanTokens(x.Usage.Total())))
	})

	out = append(out, "", bold.Render("By model"))
	models := make([]string, 0, len(byModel))
	for k := range byModel {
		models = append(models, k)
	}
	sort.Slice(models, func(i, j int) bool { return byModel[models[i]].Total() > byModel[models[j]].Total() })
	for _, k := range models {
		u := byModel[k]
		out = append(out, fmt.Sprintf("  %s %7s  %s", fit(shortModel(k), 14), humanTokens(u.Total()), faint.Render(usageDetail(u))))
	}
	return append(out, "", bold.Render("Session total ")+humanTokens(total.Total())+"  "+faint.Render(usageDetail(total)))
}
