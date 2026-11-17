// Command claude-monitor is a read-only terminal UI that watches Claude Code
// transcripts and shows what each session and subagent is doing.
package main

import (
	"flag"
	"fmt"
	"image/png"
	"os"
	"path/filepath"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/cicingik/claude-monitor/internal/office"
	"github.com/cicingik/claude-monitor/internal/pixel"
	"github.com/cicingik/claude-monitor/internal/scan"
	"github.com/cicingik/claude-monitor/internal/ui"
)

// demoTime fixes the --demo --snapshot frame, so the picture is reproducible.
var demoTime = time.Date(2026, 1, 1, 12, 0, 0, 500e6, time.UTC)

func main() {
	home, _ := os.UserHomeDir()
	root := flag.String("root", filepath.Join(home, ".claude", "projects"), "Claude Code projects directory")
	since := flag.Duration("since", 24*time.Hour, "only sessions whose transcript changed within this window (0 = all)")
	dump := flag.Bool("dump", false, "scan once, print the tree, edges and usage as plain text, and exit")
	sprites := flag.String("sprites", spritesDir(home), "directory of custom character .gif/.png files")
	snapshot := flag.String("snapshot", "", "scan once, render one office frame to this PNG file, and exit")
	session := flag.String("session", "", "session id for --snapshot (default: the most recently active)")
	scale := flag.Int("scale", 4, "pixel scale of the --snapshot PNG")
	demo := flag.Bool("demo", false, "show built-in synthetic data instead of scanning transcripts")
	flag.Parse()
	explicit := map[string]bool{}
	flag.Visit(func(f *flag.Flag) { explicit[f.Name] = true })

	s := scan.New(*root, *since)
	scanOnce := func() scan.Snapshot {
		if *demo {
			return office.Demo(demoTime)
		}
		return s.Scan(time.Now())
	}
	cast := office.LoadCast(*sprites, explicit["sprites"])
	switch {
	case *dump:
		ui.Dump(os.Stdout, scanOnce(), *root, *since)
	case *snapshot != "":
		if err := writeSnapshot(*snapshot, scanOnce(), *session, *scale, cast); err != nil {
			fmt.Fprintln(os.Stderr, "claude-monitor:", err)
			os.Exit(1)
		}
	default:
		m := ui.New(s).WithConfig(ui.Config{Office: true, Cast: cast, Demo: *demo})
		if _, err := tea.NewProgram(m).Run(); err != nil {
			fmt.Fprintln(os.Stderr, "claude-monitor:", err)
			os.Exit(1)
		}
	}
}

// writeSnapshot renders the office of one session, as a 160x45 terminal
// would show it at the time of the scan, to a PNG scaled up by scale.
func writeSnapshot(path string, snap scan.Snapshot, session string, scale int, cast *office.Cast) error {
	if a := snap.ByID[session]; session != "" && (a == nil || a.Kind != scan.Session) {
		return fmt.Errorf("session %q not found in the time window", session)
	}
	if session == "" && len(snap.Roots) == 0 {
		return fmt.Errorf("no sessions in the time window")
	}
	size := ui.CanvasSize(160, 45)
	sc := office.Layout(snap, session, size.X, size.Y, snap.ScannedAt)
	cast.Dress(&sc)
	img := office.Render(sc, snap.ScannedAt)
	scale = max(scale, 1)
	img = pixel.Scale(img, img.Bounds().Dx()*scale, img.Bounds().Dy()*scale)
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	if err := png.Encode(f, img); err != nil {
		f.Close()
		return err
	}
	if cast.Note != "" {
		fmt.Fprintln(os.Stderr, cast.Note)
	}
	return f.Close()
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
