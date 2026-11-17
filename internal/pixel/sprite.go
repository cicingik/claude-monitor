package pixel

import (
	"image"
	"image/color"
	"time"
)

// DefaultDelay is used for frames with no (or a zero) delay.
const DefaultDelay = 100 * time.Millisecond

// Sprite is an animation: Frames[i] shows for Delays[i].
type Sprite struct {
	Frames []*image.RGBA
	Delays []time.Duration
}

// Still is a one-frame sprite.
func Still(img *image.RGBA) *Sprite {
	return &Sprite{Frames: []*image.RGBA{img}, Delays: []time.Duration{DefaultDelay}}
}

func (s *Sprite) delay(i int) time.Duration {
	if i < len(s.Delays) && s.Delays[i] > 0 {
		return s.Delays[i]
	}
	return DefaultDelay
}

// Frame returns the frame showing after elapsed time t, looping forever.
func (s *Sprite) Frame(t time.Duration) *image.RGBA {
	if s == nil || len(s.Frames) == 0 {
		return nil
	}
	var total time.Duration
	for i := range s.Frames {
		total += s.delay(i)
	}
	t %= total
	if t < 0 {
		t += total
	}
	for i := range s.Frames {
		if t < s.delay(i) {
			return s.Frames[i]
		}
		t -= s.delay(i)
	}
	return s.Frames[len(s.Frames)-1]
}

// Size is the largest frame size.
func (s *Sprite) Size() image.Point {
	var p image.Point
	for _, f := range s.Frames {
		p.X, p.Y = max(p.X, f.Bounds().Dx()), max(p.Y, f.Bounds().Dy())
	}
	return p
}

// Palette maps the runes of a pixel grid to colors. Runes it lacks ('.', ' ')
// are transparent.
type Palette map[rune]color.RGBA

// Grid draws a pixel map: one string per row, one rune per pixel.
func Grid(p Palette, rows ...string) *image.RGBA {
	w := 0
	for _, r := range rows {
		w = max(w, len([]rune(r)))
	}
	img := New(w, len(rows))
	for y, r := range rows {
		for x, ch := range []rune(r) {
			if c, ok := p[ch]; ok {
				Set(img, x, y, c)
			}
		}
	}
	return img
}
