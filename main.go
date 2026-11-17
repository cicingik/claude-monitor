// Command claude-monitor is a read-only terminal UI that watches Claude Code
// transcripts and shows what each session and subagent is doing.
package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/cicingik/claude-monitor/internal/scan"
	"github.com/cicingik/claude-monitor/internal/ui"
)

func main() {
	home, _ := os.UserHomeDir()
	root := flag.String("root", filepath.Join(home, ".claude", "projects"), "Claude Code projects directory")
	since := flag.Duration("since", 24*time.Hour, "only sessions whose transcript changed within this window (0 = all)")
	flag.Parse()

	s := scan.New(*root, *since)
	if _, err := tea.NewProgram(ui.New(s)).Run(); err != nil {
		fmt.Fprintln(os.Stderr, "claude-monitor:", err)
		os.Exit(1)
	}
}
