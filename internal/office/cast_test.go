package office

import (
	"bytes"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/cicingik/claude-monitor/internal/pixel"
)

func pngOf(t *testing.T, c color.RGBA) []byte {
	t.Helper()
	img := image.NewRGBA(image.Rect(0, 0, 4, 8))
	for i := 0; i < len(img.Pix); i += 4 {
		img.Pix[i], img.Pix[i+1], img.Pix[i+2], img.Pix[i+3] = c.R, c.G, c.B, c.A
	}
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func spriteDir(t *testing.T, files map[string][]byte) string {
	t.Helper()
	dir := t.TempDir()
	for name, data := range files {
		if err := os.WriteFile(filepath.Join(dir, name), data, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

// colorOf identifies a loaded sprite by its first pixel.
func colorOf(s *pixel.Sprite) string {
	if s == nil {
		return "built-in"
	}
	switch s.Frames[0].RGBAAt(0, 0).R {
	case 1:
		return "general-purpose"
	case 2:
		return "alpha"
	case 3:
		return "zeta"
	case 4:
		return "session"
	}
	return "?"
}

func TestCastResolutionOrder(t *testing.T) {
	dir := spriteDir(t, map[string][]byte{
		"General-Purpose.png": pngOf(t, color.RGBA{1, 0, 0, 255}),
		"alpha.png":           pngOf(t, color.RGBA{2, 0, 0, 255}),
		"zeta.png":            pngOf(t, color.RGBA{3, 0, 0, 255}),
		"broken.gif":          []byte("GIF89a garbage"),
		"notes.txt":           []byte("ignored"),
	})
	c := LoadCast(dir, false)
	if !strings.Contains(c.Note, "3 loaded, 1 skipped (broken.gif)") {
		t.Errorf("note = %q", c.Note)
	}
	scene := func() Scene {
		return Scene{Actors: []Actor{
			{AgentID: "lead", Type: "session"},
			{AgentID: "g1", Type: "general-purpose"},
			{AgentID: "g2", Type: "general-purpose"},
			{AgentID: "e1", Type: "Explore"},
			{AgentID: "e2", Type: "Explore"},
			{AgentID: "x9", Type: ""},
		}}
	}
	sc := scene()
	c.Dress(&sc)
	got := map[string]string{}
	for _, a := range sc.Actors {
		got[a.AgentID] = colorOf(a.Sprite)
	}
	for _, id := range []string{"g1", "g2"} {
		if got[id] != "general-purpose" {
			t.Errorf("%s: %s, want the file named after its type", id, got[id])
		}
	}
	for _, id := range []string{"lead", "e1", "e2", "x9"} {
		if got[id] != "alpha" && got[id] != "zeta" {
			t.Errorf("%s: %s, want a pool file (not general-purpose: that type is in use)", id, got[id])
		}
	}
	again := scene()
	c.Dress(&again)
	for i, a := range again.Actors {
		if a.Sprite != sc.Actors[i].Sprite {
			t.Errorf("%s: sprite changed between frames", a.AgentID)
		}
	}

	// a session.png now wins for the lead
	dir2 := spriteDir(t, map[string][]byte{"session.png": pngOf(t, color.RGBA{4, 0, 0, 255})})
	sc = scene()
	LoadCast(dir2, false).Dress(&sc)
	if got := colorOf(sc.Actors[0].Sprite); got != "session" {
		t.Errorf("lead: %s, want session.png", got)
	}
	if got := colorOf(sc.Actors[1].Sprite); got != "built-in" {
		t.Errorf("general-purpose with only session.png: %s, want built-in", got)
	}
}

func TestCastMissingOrEmptyDir(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "nope")
	tests := []struct {
		dir      string
		explicit bool
		note     string
	}{
		{missing, false, ""},
		{missing, true, "sprites: cannot read"},
		{t.TempDir(), false, ""},
		{t.TempDir(), true, "sprites: no .gif or .png in"},
	}
	for _, tc := range tests {
		c := LoadCast(tc.dir, tc.explicit)
		if (tc.note == "") != (c.Note == "") || !strings.HasPrefix(c.Note, tc.note) {
			t.Errorf("LoadCast(%s, %v).Note = %q, want prefix %q", tc.dir, tc.explicit, c.Note, tc.note)
		}
		sc := Scene{Actors: []Actor{{AgentID: "a", Type: "session"}}}
		c.Dress(&sc)
		if sc.Actors[0].Sprite != nil {
			t.Error("no sprites: actor must stay built-in")
		}
	}
	var nilCast *Cast
	nilCast.Dress(&Scene{Actors: []Actor{{AgentID: "a"}}})
}
