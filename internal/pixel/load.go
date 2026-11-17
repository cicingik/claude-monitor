package pixel

import (
	"bytes"
	"errors"
	"fmt"
	"image"
	"image/draw"
	"image/gif"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// Limits that keep a stray huge file from eating memory.
const (
	maxFileSize  = 16 << 20
	maxSide      = 2048
	maxGIFFrames = 512
)

// Load reads a GIF (animated or not) or PNG, crops it to the pixels any frame
// uses and scales it with nearest-neighbour sampling to box.Y pixels high, or
// narrower if it would be wider than box.X.
func Load(path string, box image.Point) (*Sprite, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	if len(data) > maxFileSize {
		return nil, fmt.Errorf("larger than %d MB", maxFileSize>>20)
	}
	var s *Sprite
	switch strings.ToLower(filepath.Ext(path)) {
	case ".gif":
		s, err = decodeGIF(data)
	case ".png":
		s, err = decodePNG(data)
	default:
		return nil, errors.New("not a .gif or .png")
	}
	if err != nil {
		return nil, err
	}
	return fit(s, box)
}

func decodePNG(data []byte) (*Sprite, error) {
	cfg, err := png.DecodeConfig(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	if cfg.Width > maxSide || cfg.Height > maxSide {
		return nil, fmt.Errorf("%dx%d is too large", cfg.Width, cfg.Height)
	}
	img, err := png.Decode(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	return Still(toRGBA(img)), nil
}

// decodeGIF composes every frame onto a full canvas, honouring transparency
// and each frame's disposal method, so each Sprite frame is complete.
func decodeGIF(data []byte) (*Sprite, error) {
	g, err := gif.DecodeAll(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	w, h := g.Config.Width, g.Config.Height
	var all image.Rectangle
	for _, f := range g.Image {
		all = all.Union(f.Bounds())
	}
	w, h = max(w, all.Max.X), max(h, all.Max.Y)
	if w > maxSide || h > maxSide || len(g.Image) > maxGIFFrames || len(g.Image) == 0 {
		return nil, fmt.Errorf("%dx%d with %d frames is outside the limits", w, h, len(g.Image))
	}
	canvas := New(w, h)
	s := &Sprite{}
	for i, f := range g.Image {
		var disposal byte
		if i < len(g.Disposal) {
			disposal = g.Disposal[i]
		}
		var before *image.RGBA
		if disposal == gif.DisposalPrevious {
			before = clone(canvas)
		}
		draw.Draw(canvas, f.Bounds(), f, f.Bounds().Min, draw.Over)
		s.Frames = append(s.Frames, clone(canvas))
		var d time.Duration
		if i < len(g.Delay) && g.Delay[i] > 1 { // 0 and 1 mean "as fast as possible": use the default
			d = time.Duration(g.Delay[i]) * 10 * time.Millisecond
		}
		s.Delays = append(s.Delays, d)
		switch disposal {
		case gif.DisposalBackground:
			draw.Draw(canvas, f.Bounds(), image.Transparent, image.Point{}, draw.Src)
		case gif.DisposalPrevious:
			canvas = before
		}
	}
	return s, nil
}

// fit crops s to the union of its frames' opaque pixels and scales it into box.
func fit(s *Sprite, box image.Point) (*Sprite, error) {
	var used image.Rectangle
	for _, f := range s.Frames {
		used = used.Union(opaqueBounds(f))
	}
	if used.Empty() {
		return nil, errors.New("fully transparent")
	}
	h := box.Y
	w := max(1, used.Dx()*h/used.Dy())
	if w > box.X {
		w, h = box.X, max(1, used.Dy()*box.X/used.Dx())
	}
	out := &Sprite{Delays: s.Delays}
	for _, f := range s.Frames {
		out.Frames = append(out.Frames, Scale(f.SubImage(used).(*image.RGBA), w, h))
	}
	return out, nil
}

func opaqueBounds(img *image.RGBA) image.Rectangle {
	var r image.Rectangle
	b := img.Bounds()
	for y := b.Min.Y; y < b.Max.Y; y++ {
		for x := b.Min.X; x < b.Max.X; x++ {
			if img.Pix[img.PixOffset(x, y)+3] > 0 {
				r = r.Union(image.Rect(x, y, x+1, y+1))
			}
		}
	}
	return r
}

func toRGBA(img image.Image) *image.RGBA {
	out := image.NewRGBA(image.Rect(0, 0, img.Bounds().Dx(), img.Bounds().Dy()))
	draw.Draw(out, out.Bounds(), img, img.Bounds().Min, draw.Src)
	return out
}

func clone(img *image.RGBA) *image.RGBA {
	out := &image.RGBA{Pix: append([]byte(nil), img.Pix...), Stride: img.Stride, Rect: img.Rect}
	return out
}
