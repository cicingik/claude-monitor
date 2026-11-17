package ui

import (
	"fmt"
	"io"
	"sort"
	"strings"
	"time"

	"github.com/cicingik/claude-monitor/internal/scan"
)

// Dump writes snap as plain text: the agent tree with status, model and exact
// token counts, then every edge, then usage by model.
func Dump(w io.Writer, snap scan.Snapshot, root string, since time.Duration) {
	running := 0
	for _, a := range snap.ByID {
		if a.Status == scan.Running {
			running++
		}
	}
	window := "all"
	if since > 0 {
		window = shortDuration(since)
	}
	fmt.Fprintf(w, "claude-monitor dump  root=%s  since=%s  scanned=%s\n", root, window, snap.ScannedAt.Format(time.RFC3339))
	fmt.Fprintf(w, "sessions=%d agents=%d running=%d\n\nTREE\n", len(snap.Roots), len(snap.ByID), running)
	for _, r := range snap.Roots {
		scan.Walk(r, 0, func(a *scan.Agent, depth int) {
			u := a.Usage
			fmt.Fprintf(w, "%s%s %s  %s  %s  model=%s  turns=%d  tokens=%s (in=%d out=%d cache_read=%d cache_write=%d)  age=%s  %q\n",
				strings.Repeat("  ", depth), a.Status.Glyph(), a.ID, agentType(a), a.Status, shortModel(a.Model),
				a.Turns, humanTokens(u.Total()), u.Input, u.Output, u.CacheRead, u.CacheWrite,
				age(snap.ScannedAt, a.LastActivity), a.Title)
		})
		if len(r.Children) > 0 {
			t := scan.SubtreeUsage(r)
			fmt.Fprintf(w, "  = session %s with subagents: tokens=%s (out=%d)\n", shortID(r.ID), humanTokens(t.Total()), t.Output)
		}
	}

	fmt.Fprintf(w, "\nEDGES (%d)\n", len(snap.Edges))
	for _, e := range snap.Edges {
		fmt.Fprintf(w, "%-8s %s -> %s  %s  %q\n", e.Kind, e.From, e.To, e.At.Local().Format("2006-01-02 15:04:05"), e.Label)
	}

	fmt.Fprintln(w, "\nUSAGE BY MODEL")
	models := make([]string, 0, len(snap.ByModel))
	var total scan.Usage
	for k, u := range snap.ByModel {
		models = append(models, k)
		total = total.Add(u)
	}
	sort.Strings(models)
	for _, k := range models {
		u := snap.ByModel[k]
		fmt.Fprintf(w, "%-28s tokens=%-6s in=%d out=%d cache_read=%d cache_write=%d\n",
			k, humanTokens(u.Total()), u.Input, u.Output, u.CacheRead, u.CacheWrite)
	}
	fmt.Fprintf(w, "%-28s tokens=%-6s in=%d out=%d cache_read=%d cache_write=%d\n",
		"TOTAL", humanTokens(total.Total()), total.Input, total.Output, total.CacheRead, total.CacheWrite)
}
