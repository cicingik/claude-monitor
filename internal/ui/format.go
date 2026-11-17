package ui

import (
	"fmt"
	"regexp"
	"strings"
	"time"

	"github.com/cicingik/claude-monitor/internal/scan"
)

// humanTokens renders 1234 as 1.2k and 1234567 as 1.2M.
func humanTokens(n int64) string {
	f := float64(n)
	switch {
	case n < 1_000:
		return fmt.Sprintf("%d", n)
	case n < 10_000:
		return fmt.Sprintf("%.1fk", f/1e3)
	case n < 1_000_000:
		return fmt.Sprintf("%dk", n/1_000)
	case n < 10_000_000:
		return fmt.Sprintf("%.1fM", f/1e6)
	case n < 1_000_000_000:
		return fmt.Sprintf("%dM", n/1_000_000)
	}
	return fmt.Sprintf("%.1fB", f/1e9)
}

var dateSuffix = regexp.MustCompile(`-\d{8}$`)

// shortModel turns claude-sonnet-4-5-20250929 into sonnet-4-5.
func shortModel(m string) string {
	if m == "" {
		return "-"
	}
	return dateSuffix.ReplaceAllString(strings.TrimPrefix(m, "claude-"), "")
}

// age renders the time since t as 5s, 3m, 2h or 4d.
func age(now, t time.Time) string {
	if t.IsZero() {
		return "-"
	}
	d := now.Sub(t)
	switch {
	case d < time.Minute:
		return fmt.Sprintf("%ds", max(0, int(d.Seconds())))
	case d < time.Hour:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh", int(d.Hours()))
	}
	return fmt.Sprintf("%dd", int(d.Hours()/24))
}

// shortID keeps the first 8 characters of an id.
func shortID(id string) string {
	if len(id) > 8 {
		return id[:8]
	}
	return id
}

// label is the display name of an agent: description for subagents,
// short id plus AI title for sessions.
func label(a *scan.Agent) string {
	if a.Kind == scan.Session {
		if a.Title == "" {
			return shortID(a.ID) + " (" + a.Project + ")"
		}
		return shortID(a.ID) + " " + a.Title
	}
	if a.Title == "" {
		return a.ID
	}
	return a.Title
}

func agentType(a *scan.Agent) string {
	if a.Kind == scan.Session {
		return "session"
	}
	if a.AgentType == "" {
		return "agent"
	}
	return a.AgentType
}

func usageDetail(u scan.Usage) string {
	return fmt.Sprintf("in %s · out %s · cache r %s w %s",
		humanTokens(u.Input), humanTokens(u.Output), humanTokens(u.CacheRead), humanTokens(u.CacheWrite))
}

// fit pads or cuts s (plain text) to exactly w cells.
func fit(s string, w int) string {
	if w <= 0 {
		return ""
	}
	s = scan.Truncate(s, w)
	if n := len([]rune(s)); n < w {
		s += strings.Repeat(" ", w-n)
	}
	return s
}

// shortDuration renders 24h0m0s as 24h and 1h30m0s as 1h30m.
func shortDuration(d time.Duration) string {
	s := d.String()
	if strings.HasSuffix(s, "m0s") {
		s = strings.TrimSuffix(s, "0s")
	}
	if strings.HasSuffix(s, "h0m") {
		s = strings.TrimSuffix(s, "0m")
	}
	return s
}
