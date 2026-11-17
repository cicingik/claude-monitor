package pixel

import (
	"bytes"
	"image"
	"image/color"
	"image/gif"
	"image/png"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// tinyGIF is a 4×4 animation: red, then blue top-left (cleared afterwards),
// then a blue corner (restored afterwards), then an empty frame.
func tinyGIF(t *testing.T) []byte {
	t.Helper()
	pal := color.Palette{color.RGBA{}, color.RGBA{255, 0, 0, 255}, color.RGBA{0, 0, 255, 255}}
	frame := func(r image.Rectangle, idx uint8) *image.Paletted {
		p := image.NewPaletted(r, pal)
		for i := range p.Pix {
			p.Pix[i] = idx
		}
		return p
	}
	g := &gif.GIF{
		Image:    []*image.Paletted{frame(image.Rect(0, 0, 4, 4), 1), frame(image.Rect(0, 0, 2, 2), 2), frame(image.Rect(3, 3, 4, 4), 2), frame(image.Rect(1, 1, 2, 2), 0)},
		Delay:    []int{10, 20, 0, 30},
		Disposal: []byte{gif.DisposalNone, gif.DisposalBackground, gif.DisposalPrevious, gif.DisposalNone},
		Config:   image.Config{ColorModel: pal, Width: 4, Height: 4},
	}
	var buf bytes.Buffer
	if err := gif.EncodeAll(&buf, g); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func write(t *testing.T, name string, data []byte) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(p, data, 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

func TestLoadGIFComposesFramesWithDisposal(t *testing.T) {
	s, err := Load(write(t, "a.gif", tinyGIF(t)), image.Pt(20, 8))
	if err != nil {
		t.Fatal(err)
	}
	if len(s.Frames) != 4 || s.Size() != image.Pt(8, 8) {
		t.Fatalf("frames %d size %v, want 4 frames scaled to 8x8", len(s.Frames), s.Size())
	}
	wantDelays := []time.Duration{100 * time.Millisecond, 200 * time.Millisecond, 0, 300 * time.Millisecond}
	for i, d := range wantDelays {
		if s.Delays[i] != d {
			t.Errorf("delay %d = %v, want %v", i, s.Delays[i], d)
		}
	}
	red, blue, clear := color.RGBA{255, 0, 0, 255}, color.RGBA{0, 0, 255, 255}, color.RGBA{}
	// pixel (x, y) of the 4×4 source is (2x, 2y) after scaling.
	tests := []struct {
		frame int
		x, y  int
		want  color.RGBA
	}{
		{0, 0, 0, red}, {0, 3, 3, red},
		{1, 0, 0, blue}, {1, 1, 1, blue}, {1, 2, 2, red}, // drawn over the red frame
		{2, 0, 0, clear}, {2, 1, 1, clear}, {2, 3, 3, blue}, {2, 2, 0, red}, // frame 1 disposed to transparent
		{3, 3, 3, red}, {3, 0, 0, clear}, {3, 1, 1, clear}, // frame 2 disposed to previous; frame 3 draws only transparency
	}
	for _, tc := range tests {
		if got := s.Frames[tc.frame].RGBAAt(2*tc.x, 2*tc.y); got != tc.want {
			t.Errorf("frame %d (%d,%d) = %v, want %v", tc.frame, tc.x, tc.y, got, tc.want)
		}
	}
	if s.Frame(150*time.Millisecond) != s.Frames[1] {
		t.Error("Frame(150ms) should be frame 1")
	}
}

func TestLoadPNGCropsAndFitsTheBox(t *testing.T) {
	img := image.NewNRGBA(image.Rect(0, 0, 40, 40)) // a 10×20 opaque block in transparent padding
	for y := 10; y < 30; y++ {
		for x := 15; x < 25; x++ {
			img.Set(x, y, color.NRGBA{0, 200, 0, 255})
		}
	}
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		t.Fatal(err)
	}
	path := write(t, "b.PNG", buf.Bytes())
	for _, tc := range []struct{ box, want image.Point }{
		{image.Pt(12, 16), image.Pt(8, 16)}, // fits the height
		{image.Pt(4, 16), image.Pt(4, 8)},   // too wide: fits the width
	} {
		s, err := Load(path, tc.box)
		if err != nil {
			t.Fatal(err)
		}
		if got := s.Size(); got != tc.want || len(s.Frames) != 1 {
			t.Errorf("box %v: size %v frames %d, want %v and 1", tc.box, got, len(s.Frames), tc.want)
		}
	}
}

func TestLoadRejectsBadFiles(t *testing.T) {
	for _, name := range []string{"junk.gif", "junk.png", "x.txt", "empty.gif"} {
		data := []byte("definitely not an image")
		if name == "empty.gif" {
			data = nil
		}
		if s, err := Load(write(t, name, data), image.Pt(12, 16)); err == nil || s != nil {
			t.Errorf("%s: want an error, got %v", name, s)
		}
	}
	if _, err := Load(filepath.Join(t.TempDir(), "missing.gif"), image.Pt(12, 16)); err == nil {
		t.Error("missing file: want an error")
	}
}
