package pixel

import (
	"image"
	"image/color"
	"strings"
	"testing"
	"time"
)

func TestEncodeHalfBlock(t *testing.T) {
	red, blue, bg := RGB(0xff0000), RGB(0x0000ff), RGB(0x102030)
	tests := []struct {
		name string
		img  func() *image.RGBA
		want string
	}{
		{
			name: "two stacked pixels make one cell: fg top, bg bottom",
			img: func() *image.RGBA {
				img := New(1, 2)
				Set(img, 0, 0, red)
				Set(img, 0, 1, blue)
				return img
			},
			want: "\x1b[38;2;255;0;0m\x1b[48;2;0;0;255m▀\x1b[0m",
		},
		{
			name: "transparent pixels show the background",
			img:  func() *image.RGBA { return New(1, 2) },
			want: "\x1b[38;2;16;32;48m\x1b[48;2;16;32;48m▀\x1b[0m",
		},
		{
			name: "repeated colors are not re-emitted; odd row pairs with bg",
			img: func() *image.RGBA {
				img := New(2, 3)
				Fill(img, img.Bounds(), red)
				return img
			},
			want: "\x1b[38;2;255;0;0m\x1b[48;2;255;0;0m▀▀\x1b[0m\n" +
				"\x1b[38;2;255;0;0m\x1b[48;2;16;32;48m▀▀\x1b[0m",
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := Encode(tc.img(), bg); got != tc.want {
				t.Errorf("Encode\n got %q\nwant %q", got, tc.want)
			}
		})
	}
}

func TestSpriteFrameByTime(t *testing.T) {
	a, b, c := New(1, 1), New(2, 2), New(3, 3)
	s := &Sprite{Frames: []*image.RGBA{a, b, c}, Delays: []time.Duration{100 * time.Millisecond, 300 * time.Millisecond, 0}}
	tests := []struct {
		at   time.Duration
		want *image.RGBA
	}{
		{0, a},
		{99 * time.Millisecond, a},
		{100 * time.Millisecond, b},
		{399 * time.Millisecond, b},
		{400 * time.Millisecond, c}, // zero delay counts as DefaultDelay
		{499 * time.Millisecond, c},
		{500 * time.Millisecond, a}, // loops
		{1150 * time.Millisecond, b},
		{-50 * time.Millisecond, c},
	}
	for _, tc := range tests {
		if got := s.Frame(tc.at); got != tc.want {
			t.Errorf("Frame(%v) = frame of width %d, want width %d", tc.at, got.Bounds().Dx(), tc.want.Bounds().Dx())
		}
	}
	if (*Sprite)(nil).Frame(time.Second) != nil || (&Sprite{}).Frame(0) != nil {
		t.Error("empty sprite must yield nil")
	}
}

func TestGridAndDraw(t *testing.T) {
	g := Grid(Palette{'x': RGB(0xffffff)}, "x.", ".x")
	dst := New(3, 3)
	Fill(dst, dst.Bounds(), RGB(0x000000))
	Draw(dst, g, image.Pt(1, 1))
	want := []string{"...", ".x.", "..x"}
	for y, row := range want {
		for x, ch := range row {
			got := dst.RGBAAt(x, y) == RGB(0xffffff)
			if got != (ch == 'x') {
				t.Errorf("pixel (%d,%d) white=%v, want %v", x, y, got, ch == 'x')
			}
		}
	}
	ring := Outline(g, RGB(0xff0000))
	if n := strings.Count(Encode(ring, color.RGBA{}), "255;0;0"); n == 0 {
		t.Error("outline drew nothing")
	}
}

// BenchmarkEncode160x45 encodes a full 160×45-cell terminal (160×90 pixels)
// where every pixel differs from its neighbour, the worst case for SGR output.
func BenchmarkEncode160x45(b *testing.B) {
	img := New(160, 90)
	for y := range 90 {
		for x := range 160 {
			Set(img, x, y, color.RGBA{uint8(x), uint8(y), uint8(x ^ y), 255})
		}
	}
	b.ReportAllocs()
	for b.Loop() {
		Encode(img, RGB(0))
	}
}
