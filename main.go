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
	dump := flag.Bool("dump", false, "scan once, print the tree, edges and usage as plain text, and exit")
	flag.Parse()

	s := scan.New(*root, *since)
	if *dump {
		ui.Dump(os.Stdout, s.Scan(time.Now()), *root, *since)
		return
	}
	if _, err := tea.NewProgram(ui.New(s)).Run(); err != nil {
		fmt.Fprintln(os.Stderr, "claude-monitor:", err)
		os.Exit(1)
	}
}
