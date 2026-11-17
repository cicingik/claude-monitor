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

	"github.com/cicingik/claude-monitor/internal/office"
	"github.com/cicingik/claude-monitor/internal/scan"
	"github.com/cicingik/claude-monitor/internal/ui"
)

func main() {
	home, _ := os.UserHomeDir()
	root := flag.String("root", filepath.Join(home, ".claude", "projects"), "Claude Code projects directory")
	since := flag.Duration("since", 24*time.Hour, "only sessions whose transcript changed within this window (0 = all)")
	dump := flag.Bool("dump", false, "scan once, print the tree, edges and usage as plain text, and exit")
	sprites := flag.String("sprites", spritesDir(home), "directory of custom character .gif/.png files")
	flag.Parse()
	explicit := map[string]bool{}
	flag.Visit(func(f *flag.Flag) { explicit[f.Name] = true })

	s := scan.New(*root, *since)
	if *dump {
		ui.Dump(os.Stdout, s.Scan(time.Now()), *root, *since)
		return
	}
	cast := office.LoadCast(*sprites, explicit["sprites"])
	if _, err := tea.NewProgram(ui.New(s).WithConfig(ui.Config{Office: true, Cast: cast})).Run(); err != nil {
		fmt.Fprintln(os.Stderr, "claude-monitor:", err)
		os.Exit(1)
	}
}

// spritesDir is $XDG_CONFIG_HOME/claude-monitor/sprites, or
// ~/.config/claude-monitor/sprites when XDG_CONFIG_HOME is unset.
func spritesDir(home string) string {
	base := os.Getenv("XDG_CONFIG_HOME")
	if base == "" {
		base = filepath.Join(home, ".config")
	}
	return filepath.Join(base, "claude-monitor", "sprites")
}
