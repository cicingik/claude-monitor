// Package pixel draws small RGBA images and encodes them as terminal text in
// which one cell shows two stacked pixels with the upper-half block "▀".
package pixel

import (
	"image"
	"image/color"
)

// RGB is an opaque color.
func RGB(hex uint32) color.RGBA {
	return color.RGBA{uint8(hex >> 16), uint8(hex >> 8), uint8(hex), 255}
}

// New returns a transparent w×h canvas.
func New(w, h int) *image.RGBA { return image.NewRGBA(image.Rect(0, 0, w, h)) }

// Fill paints r (clipped to dst) with c, replacing what was there.
func Fill(dst *image.RGBA, r image.Rectangle, c color.RGBA) {
	r = r.Intersect(dst.Bounds())
	for y := r.Min.Y; y < r.Max.Y; y++ {
		i := dst.PixOffset(r.Min.X, y)
		for x := r.Min.X; x < r.Max.X; x++ {
			dst.Pix[i], dst.Pix[i+1], dst.Pix[i+2], dst.Pix[i+3] = c.R, c.G, c.B, c.A
			i += 4
		}
	}
}

// Set paints one pixel if it is inside dst.
func Set(dst *image.RGBA, x, y int, c color.RGBA) {
	if (image.Point{x, y}).In(dst.Bounds()) {
		i := dst.PixOffset(x, y)
		dst.Pix[i], dst.Pix[i+1], dst.Pix[i+2], dst.Pix[i+3] = c.R, c.G, c.B, c.A
	}
}

// Draw composites src over dst with src's top-left corner at at. Fully
// transparent pixels are skipped and partial alpha is blended.
func Draw(dst, src *image.RGBA, at image.Point) {
	DrawTinted(dst, src, at, color.RGBA{}, 0)
}

// DrawTinted is Draw with every source pixel mixed toward tint by amount
// (0 = unchanged, 1 = tint). It dims or highlights a sprite.
func DrawTinted(dst, src *image.RGBA, at image.Point, tint color.RGBA, amount float64) {
	sb := src.Bounds()
	r := image.Rectangle{at, at.Add(sb.Size())}.Intersect(dst.Bounds())
	k := uint32(amount * 256)
	for y := r.Min.Y; y < r.Max.Y; y++ {
		si := src.PixOffset(sb.Min.X+r.Min.X-at.X, sb.Min.Y+y-at.Y)
		di := dst.PixOffset(r.Min.X, y)
		for x := r.Min.X; x < r.Max.X; x, si, di = x+1, si+4, di+4 {
			a := uint32(src.Pix[si+3])
			if a == 0 {
				continue
			}
			sr, sg, sbl := uint32(src.Pix[si]), uint32(src.Pix[si+1]), uint32(src.Pix[si+2])
			if k > 0 { // premultiplied mix toward tint
				sr = (sr*(256-k) + uint32(tint.R)*a/255*k) >> 8
				sg = (sg*(256-k) + uint32(tint.G)*a/255*k) >> 8
				sbl = (sbl*(256-k) + uint32(tint.B)*a/255*k) >> 8
			}
			inv := 255 - a
			dst.Pix[di] = uint8(sr + uint32(dst.Pix[di])*inv/255)
			dst.Pix[di+1] = uint8(sg + uint32(dst.Pix[di+1])*inv/255)
			dst.Pix[di+2] = uint8(sbl + uint32(dst.Pix[di+2])*inv/255)
			dst.Pix[di+3] = uint8(a + uint32(dst.Pix[di+3])*inv/255)
		}
	}
}

// Line calls fn for each pixel of the straight line from a to b, with the
// pixel's index along the line (0 at a).
func Line(a, b image.Point, fn func(i int, p image.Point)) {
	dx, dy := abs(b.X-a.X), -abs(b.Y-a.Y)
	sx, sy := 1, 1
	if a.X > b.X {
		sx = -1
	}
	if a.Y > b.Y {
		sy = -1
	}
	err := dx + dy
	for i := 0; ; i++ {
		fn(i, a)
		if a == b {
			return
		}
		if e2 := 2 * err; e2 >= dy {
			err += dy
			a.X += sx
		} else {
			err += dx
			a.Y += sy
		}
	}
}

// Outline returns the 1px ring of pixels around src's opaque pixels, painted c,
// on a canvas 1px larger on every side (draw it at sprite origin - (1,1)).
func Outline(src *image.RGBA, c color.RGBA) *image.RGBA {
	sb := src.Bounds()
	out := New(sb.Dx()+2, sb.Dy()+2)
	opaque := func(x, y int) bool {
		p := image.Point{x + sb.Min.X, y + sb.Min.Y}
		return p.In(sb) && src.Pix[src.PixOffset(p.X, p.Y)+3] > 0
	}
	for y := -1; y <= sb.Dy(); y++ {
		for x := -1; x <= sb.Dx(); x++ {
			if opaque(x, y) {
				continue
			}
			if opaque(x-1, y) || opaque(x+1, y) || opaque(x, y-1) || opaque(x, y+1) {
				Set(out, x+1, y+1, c)
			}
		}
	}
	return out
}

// Scale resizes src to w×h with nearest-neighbour sampling.
func Scale(src *image.RGBA, w, h int) *image.RGBA {
	sb := src.Bounds()
	out := New(w, h)
	if sb.Empty() {
		return out
	}
	for y := 0; y < h; y++ {
		sy := sb.Min.Y + y*sb.Dy()/h
		for x := 0; x < w; x++ {
			si := src.PixOffset(sb.Min.X+x*sb.Dx()/w, sy)
			copy(out.Pix[out.PixOffset(x, y):][:4], src.Pix[si:si+4])
		}
	}
	return out
}

func abs(n int) int {
	if n < 0 {
		return -n
	}
	return n
}
