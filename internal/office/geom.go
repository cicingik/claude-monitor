package office

import "image"

const (
	border       = 2 // outer frame
	divW         = 4 // wall between the work room and the lounge
	unitW, unitH = 24, 30
)

// seat is a lounge place for one actor.
type seat struct {
	pos  image.Point
	pose Pose
}

// geom is the fixed layout of an office of a given size. Layout and Render
// both derive it from the canvas size, so it is the one source of positions.
type geom struct {
	w, h, wallH, divX int
	work, lounge      image.Rectangle // floors
	blue, kitchen     image.Rectangle // parts of the lounge floor
	gapY0, gapY1      int             // doorway in the dividing wall
	door, exit        image.Point
	exitDoor          image.Rectangle
	units             []image.Rectangle
	seats             []seat

	sofaTop, sofaLeft, sofaRight, sofaBottom, table, counter image.Rectangle
}

func plan(w, h int) geom {
	g := geom{w: w, h: h}
	g.wallH = min(max(h/6, 9), 14)
	loungeW := min(max(w*32/100, 24), 60, w-2*border-divW-2*unitW)
	g.divX = w - border - loungeW - divW
	g.work = image.Rect(border, g.wallH, g.divX, h-border)
	g.lounge = image.Rect(g.divX+divW, g.wallH, w-border, h-border)
	if g.work.Dx() < unitW || g.lounge.Dx() < 2*CharW || g.work.Dy() < 8 {
		return g // too small for an office
	}
	mid := (g.work.Min.Y + g.work.Max.Y) / 2
	g.gapY0, g.gapY1 = mid-8, mid+10
	g.door = image.Pt(g.divX+divW/2, mid+6)
	g.exitDoor = image.Rect(g.divX-14, g.wallH-min(11, g.wallH-2), g.divX-4, g.wallH)
	g.exit = image.Pt(g.exitDoor.Min.X+g.exitDoor.Dx()/2, g.wallH+1)

	// Desk stations: the first row may rise into the wall a little.
	cols := g.work.Dx() / unitW
	rows := max(1, (g.work.Dy()+6)/unitH)
	x0 := g.work.Min.X + (g.work.Dx()-cols*unitW)/2
	y0 := g.work.Min.Y + (g.work.Dy()-rows*unitH)/2
	for r := range rows {
		for c := range cols {
			g.units = append(g.units, image.Rect(x0+c*unitW, y0+r*unitH, x0+(c+1)*unitW, y0+(r+1)*unitH))
		}
	}

	// Lounge: sofas around a coffee table, a kitchen strip at the bottom.
	L := g.lounge
	g.blue = L
	if L.Dy() >= 44 {
		g.kitchen = image.Rect(L.Min.X, L.Max.Y-12, L.Max.X, L.Max.Y)
		g.blue.Max.Y = g.kitchen.Min.Y
	}
	cx := (L.Min.X + L.Max.X) / 2
	g.sofaTop = image.Rect(cx-14, g.blue.Min.Y+5, cx+14, g.blue.Min.Y+15)
	g.seats = append(g.seats,
		seat{image.Pt(cx-6, g.sofaTop.Max.Y-1), SitFront},
		seat{image.Pt(cx+6, g.sofaTop.Max.Y-1), SitFront})
	g.table = image.Rect(cx-8, g.sofaTop.Max.Y+5, cx+8, g.sofaTop.Max.Y+14)
	if g.blue.Dy() >= 40 && L.Dx() >= 3*CharW+8 {
		y := g.table.Min.Y - 5
		g.sofaLeft = image.Rect(L.Min.X+2, y, L.Min.X+11, y+20)
		g.sofaRight = image.Rect(L.Max.X-11, y, L.Max.X-2, y+20)
		g.seats = append(g.seats,
			seat{image.Pt(g.sofaLeft.Min.X+5, g.sofaLeft.Min.Y+15), SitFront},
			seat{image.Pt(g.sofaRight.Min.X+4, g.sofaRight.Min.Y+15), SitFront})
	}
	if g.blue.Max.Y-g.table.Max.Y >= 12 {
		g.sofaBottom = image.Rect(cx-14, g.table.Max.Y+4, cx+14, g.table.Max.Y+12)
		g.seats = append(g.seats,
			seat{image.Pt(cx-6, g.sofaBottom.Min.Y+4), SitBack},
			seat{image.Pt(cx+6, g.sofaBottom.Min.Y+4), SitBack})
	}
	if !g.kitchen.Empty() {
		g.counter = image.Rect(g.kitchen.Max.X-16, g.kitchen.Min.Y-4, g.kitchen.Max.X-1, g.kitchen.Max.Y-2)
		for x := L.Min.X + 8; x+CharW/2 <= g.counter.Min.X; x += CharW + 2 {
			g.seats = append(g.seats, seat{image.Pt(x, g.kitchen.Max.Y-1), Stand})
		}
	}
	return g
}

// deskSeat is where the owner of a desk station sits.
func deskSeat(u image.Rectangle) image.Point { return image.Pt(u.Min.X+unitW/2, u.Min.Y+27) }
