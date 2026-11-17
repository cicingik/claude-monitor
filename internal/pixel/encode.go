package pixel

import (
	"image"
	"image/color"
	"strconv"
)

const reset = "\x1b[0m"

// Encode renders img as text: each cell is "▀" with the foreground set to the
// upper pixel and the background to the lower one, in 24-bit color. Rows of
// cells are separated by newlines and each ends with an SGR reset. Transparent
// pixels show bg; an odd last pixel row is paired with bg.
func Encode(img *image.RGBA, bg color.RGBA) string {
	b := img.Bounds()
	w, h := b.Dx(), b.Dy()
	buf := make([]byte, 0, w*(h+1)/2*20)
	for y := 0; y < h; y += 2 {
		if y > 0 {
			buf = append(buf, '\n')
		}
		var fg, bk color.RGBA
		for x := 0; x < w; x++ {
			top := pixelOver(img, b.Min.X+x, b.Min.Y+y, bg)
			bot := bg
			if y+1 < h {
				bot = pixelOver(img, b.Min.X+x, b.Min.Y+y+1, bg)
			}
			if x == 0 || top != fg {
				buf, fg = sgr(buf, "\x1b[38;2;", top), top
			}
			if x == 0 || bot != bk {
				buf, bk = sgr(buf, "\x1b[48;2;", bot), bot
			}
			buf = append(buf, "▀"...)
		}
		buf = append(buf, reset...)
	}
	return string(buf)
}

// pixelOver reads the pixel at (x, y) composited over bg, as an opaque color.
func pixelOver(img *image.RGBA, x, y int, bg color.RGBA) color.RGBA {
	i := img.PixOffset(x, y)
	p := img.Pix[i : i+4 : i+4]
	switch a := uint32(p[3]); a {
	case 255:
		return color.RGBA{p[0], p[1], p[2], 255}
	case 0:
		return bg
	default:
		inv := 255 - a
		return color.RGBA{
			uint8(uint32(p[0]) + uint32(bg.R)*inv/255),
			uint8(uint32(p[1]) + uint32(bg.G)*inv/255),
			uint8(uint32(p[2]) + uint32(bg.B)*inv/255), 255}
	}
}

func sgr(buf []byte, prefix string, c color.RGBA) []byte {
	buf = append(buf, prefix...)
	buf = strconv.AppendUint(buf, uint64(c.R), 10)
	buf = append(buf, ';')
	buf = strconv.AppendUint(buf, uint64(c.G), 10)
	buf = append(buf, ';')
	buf = strconv.AppendUint(buf, uint64(c.B), 10)
	return append(buf, 'm')
}
