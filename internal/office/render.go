package office

import (
	"image"
	"image/color"
	"sort"
	"time"

	"github.com/cicingik/claude-monitor/internal/pixel"
	"github.com/cicingik/claude-monitor/internal/scan"
)

// Background is the color behind the scene (and of transparent pixels).
var Background = cFrame

const frameRate = 125 * time.Millisecond

// Render draws sc as of t. t only drives animation, so a fixed t gives a
// fixed frame.
func Render(sc Scene, t time.Time) *image.RGBA {
	img := pixel.New(sc.W, sc.H)
	pixel.Fill(img, img.Bounds(), cFrame)
	g := sc.g
	if len(g.units) == 0 {
		return img
	}
	ms := t.UnixMilli()
	drawRooms(img, g)
	for i, u := range sc.Desks {
		drawDesk(img, u, sc.DeskInfo[i], ms)
	}

	actors := append([]Actor(nil), sc.Actors...)
	sort.SliceStable(actors, func(i, j int) bool { return actors[i].Pos.Y < actors[j].Pos.Y })
	frames := make([]*image.RGBA, len(actors))
	for i, a := range actors {
		frames[i] = drawActor(img, a, t)
	}
	if !g.sofaBottom.Empty() { // its backrest hides the lower bodies of the people on it
		drawSofa(img, image.Rect(g.sofaBottom.Min.X, g.sofaBottom.Min.Y+4, g.sofaBottom.Max.X, g.sofaBottom.Max.Y), true)
	}
	for _, l := range sc.Links {
		drawLink(img, l, ms)
	}
	for i, a := range actors {
		drawOverlay(img, a, frames[i], ms)
	}
	return img
}

// ---- rooms -----------------------------------------------------------------

func drawRooms(img *image.RGBA, g geom) {
	// floors
	for y := g.work.Min.Y; y < g.work.Max.Y; y++ {
		for x := g.work.Min.X; x < g.work.Max.X; x++ {
			pixel.Set(img, x, y, woodAt(x, y))
		}
	}
	for y := g.blue.Min.Y; y < g.blue.Max.Y; y++ {
		for x := g.blue.Min.X; x < g.blue.Max.X; x++ {
			c := cBlue
			if (x/3+y/2)%5 == 0 {
				c = cBlueAlt
			}
			pixel.Set(img, x, y, c)
		}
	}
	for y := g.kitchen.Min.Y; y < g.kitchen.Max.Y; y++ {
		for x := g.kitchen.Min.X; x < g.kitchen.Max.X; x++ {
			c := cCheckA
			if ((x-g.kitchen.Min.X)/4+(y-g.kitchen.Min.Y)/4)%2 == 1 {
				c = cCheckB
			}
			pixel.Set(img, x, y, c)
		}
	}
	// back walls
	for _, r := range []image.Rectangle{
		image.Rect(border, 0, g.divX, g.wallH),
		image.Rect(g.divX+divW, 0, g.w-border, g.wallH),
	} {
		pixel.Fill(img, r, cWallFace)
		pixel.Fill(img, image.Rect(r.Min.X, r.Min.Y, r.Max.X, 2), cWallCap)
		pixel.Fill(img, image.Rect(r.Min.X, r.Max.Y-3, r.Max.X, r.Max.Y-1), cWallLow)
		pixel.Fill(img, image.Rect(r.Min.X, r.Max.Y-1, r.Max.X, r.Max.Y), cBaseboard)
		shadeRow(img, r.Min.X, r.Max.X, r.Max.Y)
	}
	// dividing wall with a doorway, faces at the bottom of each segment
	pixel.Fill(img, image.Rect(g.divX, 0, g.divX+divW, g.gapY0), cWallCap)
	pixel.Fill(img, image.Rect(g.divX, g.gapY0-6, g.divX+divW, g.gapY0), cWallFace)
	pixel.Fill(img, image.Rect(g.divX, g.gapY1, g.divX+divW, g.h-border), cWallCap)
	pixel.Fill(img, image.Rect(g.divX, g.h-border-6, g.divX+divW, g.h-border), cWallFace)
	for x := g.divX - 1; x < g.divX+divW+1; x++ { // doorway threshold
		for y := g.gapY0; y < g.gapY1; y++ {
			if x >= g.divX && x < g.divX+divW {
				pixel.Set(img, x, y, shade(woodAt(x, y), 0.8))
			}
		}
	}

	drawWallDecor(img, g)

	// lounge furniture
	if !g.sofaTop.Empty() {
		drawSofa(img, g.sofaTop, false)
	}
	if !g.sofaLeft.Empty() {
		drawSideSofa(img, g.sofaLeft, false)
		drawSideSofa(img, g.sofaRight, true)
	}
	if !g.sofaBottom.Empty() {
		drawSofa(img, g.sofaBottom, true)
	}
	drawTable(img, g.table)
	if !g.counter.Empty() {
		drawCounter(img, g.counter)
	}
	// floor plants
	pb := plantArt.Bounds()
	if g.lounge.Dx() >= 40 && g.blue.Dy() >= 40 {
		pixel.Draw(img, plantArt, image.Pt(g.lounge.Min.X+1, g.lounge.Min.Y-3))
		pixel.Draw(img, plantArt, image.Pt(g.lounge.Max.X-pb.Dx()-1, g.lounge.Min.Y-3))
	}
	if g.work.Dy() >= 50 && g.units[0].Min.X-g.work.Min.X >= pb.Dx() {
		pixel.Draw(img, plantArt, image.Pt(g.work.Min.X+1, g.work.Max.Y-pb.Dy()))
	}
}

func woodAt(x, y int) color.RGBA {
	row := y / 4
	if y%4 == 3 {
		return cWoodLine
	}
	if (x+row*11)%23 == 0 {
		return cWoodLine
	}
	if row%2 == 1 {
		return cWoodAlt
	}
	return cWood
}

func shadeRow(img *image.RGBA, x0, x1, y int) {
	for x := x0; x < x1; x++ {
		if (image.Point{x, y}).In(img.Bounds()) {
			c := img.RGBAAt(x, y)
			pixel.Set(img, x, y, shade(c, 0.8))
		}
	}
}

func drawWallDecor(img *image.RGBA, g geom) {
	face := image.Rect(border, 2, g.divX, g.wallH-1)
	if face.Dy() < 6 {
		return
	}
	// work room: shelves, a clock and the exit door
	shelfY := face.Max.Y - 8
	room := face.Dx()
	drawShelf(img, image.Rect(face.Min.X+6, shelfY, face.Min.X+28, shelfY+7), 1)
	if room > 90 {
		drawShelf(img, image.Rect(face.Min.X+room/2+2, shelfY, face.Min.X+room/2+24, shelfY+7), 5)
	}
	if room > 60 {
		pixel.Draw(img, clockArt, image.Pt(face.Min.X+room/2-12, face.Min.Y+1))
	}
	drawDoor(img, g.exitDoor)
	// lounge: framed pictures
	lf := image.Rect(g.divX+divW, 2, g.w-border, g.wallH-1)
	cx := (lf.Min.X + lf.Max.X) / 2
	drawPicture(img, image.Rect(cx-8, lf.Min.Y+1, cx+8, lf.Min.Y+8), 0)
	if lf.Dx() >= 44 {
		drawPicture(img, image.Rect(cx-20, lf.Min.Y+2, cx-12, lf.Min.Y+8), 1)
		drawPicture(img, image.Rect(cx+12, lf.Min.Y+2, cx+20, lf.Min.Y+8), 2)
	}
}

func drawShelf(img *image.RGBA, r image.Rectangle, seed int) {
	books := []color.RGBA{rgb(0xd64545), rgb(0xe7b84a), rgb(0x4a86d6), rgb(0x5fbf6a), rgb(0xececec), rgb(0xa35ad6), rgb(0xe07a3a)}
	x := r.Min.X + 1
	for i := seed; x < r.Max.X-1; i++ {
		w := 1 + i%2
		h := 3 + (i*7)%3
		c := books[(i*3)%len(books)]
		pixel.Fill(img, image.Rect(x, r.Max.Y-2-h, x+w, r.Max.Y-2), c)
		pixel.Set(img, x, r.Max.Y-2-h, shade(c, 1.15))
		x += w
		if i%5 == 4 {
			x++
		}
	}
	pixel.Fill(img, image.Rect(r.Min.X, r.Max.Y-2, r.Max.X, r.Max.Y-1), rgb(0xa0703f))
	pixel.Fill(img, image.Rect(r.Min.X, r.Max.Y-1, r.Max.X, r.Max.Y), rgb(0x5e3b22))
}

func drawDoor(img *image.RGBA, r image.Rectangle) {
	if r.Empty() {
		return
	}
	pixel.Fill(img, r, rgb(0x3a2416))
	in := r.Inset(1)
	in.Max.Y = r.Max.Y
	pixel.Fill(img, in, rgb(0x7a4a2a))
	pixel.Fill(img, image.Rect(in.Min.X+1, in.Min.Y+1, in.Max.X-1, in.Min.Y+4), rgb(0x8c5832))
	pixel.Fill(img, image.Rect(in.Min.X+1, in.Min.Y+5, in.Max.X-1, in.Max.Y-1), rgb(0x8c5832))
	pixel.Set(img, in.Max.X-2, in.Min.Y+in.Dy()/2+1, cGold)
	// exit sign
	pixel.Fill(img, image.Rect(r.Min.X+2, r.Min.Y-2, r.Max.X-2, r.Min.Y), rgb(0x3fbf5f))
}

func drawPicture(img *image.RGBA, r image.Rectangle, kind int) {
	pixel.Fill(img, r, rgb(0x8a6236))
	in := r.Inset(1)
	pixel.Fill(img, in, rgb(0x7ab8e0))
	switch kind {
	case 0: // hills and a sun
		pixel.Fill(img, image.Rect(in.Min.X, in.Max.Y-2, in.Max.X, in.Max.Y), rgb(0x5fbf6a))
		pixel.Fill(img, image.Rect(in.Min.X+2, in.Max.Y-3, in.Min.X+7, in.Max.Y-2), rgb(0x4aa858))
		pixel.Fill(img, image.Rect(in.Max.X-4, in.Min.Y+1, in.Max.X-2, in.Min.Y+3), cGold)
	case 1: // portrait
		pixel.Fill(img, in, rgb(0x3a2a44))
		pixel.Fill(img, image.Rect(in.Min.X+2, in.Min.Y+1, in.Max.X-2, in.Min.Y+3), rgb(0xe8b48a))
		pixel.Fill(img, image.Rect(in.Min.X+1, in.Min.Y+3, in.Max.X-1, in.Max.Y), rgb(0xd64c74))
	default: // abstract
		pixel.Fill(img, in, rgb(0xf0e6d0))
		pixel.Fill(img, image.Rect(in.Min.X+1, in.Min.Y+1, in.Min.X+4, in.Max.Y-1), rgb(0xd64545))
		pixel.Fill(img, image.Rect(in.Min.X+3, in.Min.Y+2, in.Max.X-1, in.Max.Y), rgb(0x4a86d6))
	}
}

var (
	cSofa    = rgb(0xb8394f)
	cSofaDk  = rgb(0x8c2a3e)
	cSofaHi  = rgb(0xd8546b)
	cSofaOut = rgb(0x4a1621)
)

// drawSofa draws a horizontal sofa; facingUp puts the backrest at the bottom.
func drawSofa(img *image.RGBA, r image.Rectangle, facingUp bool) {
	pixel.Fill(img, r, cSofaOut)
	in := r.Inset(1)
	back, seat := in, in
	if facingUp {
		back.Min.Y = in.Max.Y - 3
		seat.Max.Y = back.Min.Y
	} else {
		back.Max.Y = in.Min.Y + 4
		seat.Min.Y = back.Max.Y
	}
	pixel.Fill(img, back, cSofaDk)
	pixel.Fill(img, seat, cSofa)
	pixel.Fill(img, image.Rect(seat.Min.X, seat.Min.Y, seat.Max.X, seat.Min.Y+1), cSofaHi)
	mid := (in.Min.X + in.Max.X) / 2 // seam between the two cushions
	pixel.Fill(img, image.Rect(mid, in.Min.Y, mid+1, in.Max.Y), cSofaOut)
	for _, x := range []int{in.Min.X, in.Max.X - 3} { // arms
		arm := image.Rect(x, in.Min.Y, x+3, in.Max.Y)
		pixel.Fill(img, arm, cSofa)
		pixel.Fill(img, image.Rect(x, arm.Min.Y, x+3, arm.Min.Y+1), cSofaHi)
		pixel.Fill(img, image.Rect(x+3-boolInt(x == in.Min.X)*3, arm.Min.Y, x+4-boolInt(x == in.Min.X)*3, arm.Max.Y), cSofaOut)
	}
	if !facingUp {
		pixel.Fill(img, image.Rect(in.Min.X, in.Max.Y-1, in.Max.X, in.Max.Y), cSofaDk)
	}
}

// drawSideSofa draws a vertical sofa against the left (or right) wall.
func drawSideSofa(img *image.RGBA, r image.Rectangle, right bool) {
	pixel.Fill(img, r, cSofaOut)
	in := r.Inset(1)
	back, seat := in, in
	if right {
		back.Min.X = in.Max.X - 3
		seat.Max.X = back.Min.X
	} else {
		back.Max.X = in.Min.X + 3
		seat.Min.X = back.Max.X
	}
	pixel.Fill(img, back, cSofaDk)
	pixel.Fill(img, seat, cSofa)
	pixel.Fill(img, image.Rect(in.Min.X, in.Min.Y, in.Max.X, in.Min.Y+3), cSofa)
	pixel.Fill(img, image.Rect(in.Min.X, in.Min.Y, in.Max.X, in.Min.Y+1), cSofaHi)
	pixel.Fill(img, image.Rect(in.Min.X, in.Min.Y+3, in.Max.X, in.Min.Y+4), cSofaOut)
	pixel.Fill(img, image.Rect(in.Min.X, in.Max.Y-3, in.Max.X, in.Max.Y), cSofa)
	pixel.Fill(img, image.Rect(in.Min.X, in.Max.Y-4, in.Max.X, in.Max.Y-3), cSofaOut)
	pixel.Fill(img, image.Rect(in.Min.X, in.Max.Y-1, in.Max.X, in.Max.Y), cSofaDk)
}

func drawTable(img *image.RGBA, r image.Rectangle) {
	if r.Empty() {
		return
	}
	box(img, r, cDeskTop, cDeskHi, cDeskFront)
	pixel.Draw(img, mugArt, image.Pt(r.Min.X+3, r.Min.Y+1))
}

func drawCounter(img *image.RGBA, r image.Rectangle) {
	box(img, r, cDeskTop, cDeskHi, cDeskFront)
	pixel.Draw(img, coffeeArt, image.Pt(r.Min.X+3, r.Min.Y-3))
	pixel.Draw(img, mugArt, image.Pt(r.Max.X-6, r.Min.Y+1))
}

// box draws a 3/4-view block: a top surface, a 2px front face and an outline.
func box(img *image.RGBA, r image.Rectangle, top, hi, front color.RGBA) {
	pixel.Fill(img, r, cDeskLine)
	in := r.Inset(1)
	pixel.Fill(img, in, top)
	pixel.Fill(img, image.Rect(in.Min.X, in.Min.Y, in.Max.X, in.Min.Y+1), hi)
	pixel.Fill(img, image.Rect(in.Min.X, in.Max.Y-2, in.Max.X, in.Max.Y), front)
}

// ---- desks -----------------------------------------------------------------

func drawDesk(img *image.RGBA, u image.Rectangle, d DeskInfo, ms int64) {
	x, y := u.Min.X, u.Min.Y
	pixel.Draw(img, chairArt, image.Pt(x+7, y+24))
	top, hi, front := cDeskTop, cDeskHi, cDeskFront
	if d.Lead {
		top, hi, front = cLeadTop, cLeadHi, cLeadFront
	}
	desk := image.Rect(x+2, y+9, x+22, y+18)
	pixel.Fill(img, image.Rect(desk.Min.X+1, desk.Max.Y, desk.Max.X-1, desk.Max.Y+2), shade(woodAt(x, y+19), 0.7)) // shadow
	box(img, desk, top, hi, front)
	for _, lx := range []int{desk.Min.X + 1, desk.Max.X - 3} {
		pixel.Fill(img, image.Rect(lx, desk.Max.Y, lx+2, desk.Max.Y+2), cLegs)
	}
	if d.Lead {
		pixel.Fill(img, image.Rect(x+10, y+15, x+14, y+16), cGold)
		pixel.Draw(img, deskPlantArt, image.Pt(x+3, y+7))
	}
	pixel.Draw(img, monitorArt, image.Pt(x+7, y+2))
	drawScreen(img, monitorScreen.Add(image.Pt(x+7, y+2)), d.Screen, ms, x)
	pixel.Draw(img, keyboardArt, image.Pt(x+8, y+12))
	pixel.Fill(img, image.Rect(x+17, y+13, x+19, y+14), rgb(0xdcd9cf))

	if d.Owner != "" { // token meter
		m := image.Rect(x+4, y, x+20, y+2)
		pixel.Fill(img, m, cMeterBg)
		n := int(float64(m.Dx()-2)*d.Meter + 0.5)
		if d.Meter > 0 {
			n = max(n, 1)
		}
		c := cCheck
		switch {
		case d.Meter > 0.66:
			c = rgb(0xe8644a)
		case d.Meter > 0.33:
			c = cGold
		}
		pixel.Fill(img, image.Rect(m.Min.X+1, m.Min.Y, m.Min.X+1+n, m.Max.Y), c)
	}
}

func drawScreen(img *image.RGBA, r image.Rectangle, s Screen, ms int64, seed int) {
	switch s {
	case ScreenOff:
		pixel.Fill(img, r, cScreenOff)
		pixel.Set(img, r.Min.X, r.Min.Y, rgb(0x3a5245))
	case ScreenOn:
		pixel.Fill(img, r, cScreenOn)
		for i, w := range []int{4, 2, 3} {
			pixel.Fill(img, image.Rect(r.Min.X+1, r.Min.Y+i, r.Min.X+1+w, r.Min.Y+i+1), rgb(0x6fd29a))
		}
	case ScreenBusy:
		f := int(ms/int64(frameRate)) + seed
		bg := cScreenHot
		if f%7 == 0 {
			bg = shade(cScreenHot, 0.8) // flicker
		}
		pixel.Fill(img, r, bg)
		for i := range r.Dy() {
			w := 1 + (f/2+i*3+seed)%(r.Dx()-1)
			pixel.Fill(img, image.Rect(r.Min.X, r.Min.Y+i, r.Min.X+w, r.Min.Y+i+1), cScreenTxt)
		}
		if f%2 == 0 { // cursor
			pixel.Set(img, r.Max.X-1, r.Max.Y-1, cWhite)
		}
	}
}

// ---- actors ----------------------------------------------------------------

// frameOf returns the image an actor shows at t.
func frameOf(a Actor, t time.Time) (*image.RGBA, image.Point) {
	ms := t.UnixMilli()
	phase := int64(hash(a.AgentID) % 997)
	if a.Sprite != nil {
		return a.Sprite.Frame(time.Duration(t.UnixNano())), image.Point{}
	}
	l := lookFor(a)
	var off image.Point
	switch a.Anim {
	case AnimWalk, AnimLeave:
		f := int((ms + phase) / 160)
		return character(l, Stand, f, false), image.Pt(0, -(f & 1))
	case AnimTyping:
		return character(l, SitBack, int((ms+phase)/180), false), off
	case AnimSleep:
		if (ms+phase)/1200%2 == 1 {
			off.Y = 1
		}
		return character(l, a.Pose, 0, true), off
	}
	if a.Pose == SitBack && (ms+phase)/700%2 == 1 {
		off.Y = -1 // idle bob at the desk
	}
	return character(l, a.Pose, 0, false), off
}

func drawActor(img *image.RGBA, a Actor, t time.Time) *image.RGBA {
	f, off := frameOf(a, t)
	if f == nil {
		return nil
	}
	b := f.Bounds()
	at := image.Pt(a.Pos.X-b.Dx()/2, a.Pos.Y-b.Dy()).Add(off)
	// soft floor shadow under standing or walking actors
	if a.Pose == Stand || a.Anim == AnimWalk || a.Anim == AnimLeave {
		for x := a.Pos.X - 3; x < a.Pos.X+3; x++ {
			if (image.Point{x, a.Pos.Y}).In(img.Bounds()) {
				pixel.Set(img, x, a.Pos.Y, shade(img.RGBAAt(x, a.Pos.Y), 0.7))
			}
		}
	}
	if a.Status == scan.Done && a.Anim != AnimWalk {
		pixel.DrawTinted(img, f, at, rgb(0x6a7890), 0.35)
	} else {
		pixel.Draw(img, f, at)
	}
	return f
}

func drawOverlay(img *image.RGBA, a Actor, f *image.RGBA, ms int64) {
	if f == nil {
		return
	}
	h := f.Bounds().Dy()
	top := image.Pt(a.Pos.X, a.Pos.Y-h)
	switch {
	case a.Anim == AnimTyping:
		at := top.Add(image.Pt(3, -6))
		pixel.Draw(img, bubbleArt, at)
		for i := range int(ms / 300 % 4) {
			pixel.Set(img, at.X+2+i*2, at.Y+2, rgb(0x2a2a33))
		}
	case a.Anim == AnimSleep:
		p := int(ms / 400 % 6)
		drawZ(img, top.Add(image.Pt(4, 2-p)))
	case a.Status == scan.Done && a.Anim != AnimWalk:
		pixel.Draw(img, checkArt, top.Add(image.Pt(-3, -6)))
	}
	if a.Selected && ms/300%3 != 2 {
		b := f.Bounds()
		at := image.Pt(a.Pos.X-b.Dx()/2-1, a.Pos.Y-b.Dy()-1)
		pixel.Draw(img, pixel.Outline(f, cSelect), at)
		bob := int(ms / 250 % 2)
		pixel.Draw(img, arrowArt, top.Add(image.Pt(-3, -7-bob)))
	}
}

func drawZ(img *image.RGBA, p image.Point) {
	for _, d := range []image.Point{{0, 0}, {1, 0}, {2, 0}, {1, 1}, {0, 2}, {1, 2}, {2, 2}} {
		pixel.Set(img, p.X+d.X, p.Y+d.Y, cZ)
	}
}

// ---- links -----------------------------------------------------------------

func drawLink(img *image.RGBA, l Link, ms int64) {
	c, ok := LinkColor[l.Kind]
	if !ok {
		c = cWhite
	}
	off := int(ms / 100)
	n := 0
	pixel.Line(l.From, l.To, func(i int, p image.Point) {
		n = i
		if (i+4-off%4)%4 < 2 {
			pixel.Set(img, p.X, p.Y, c)
		}
	})
	if n == 0 {
		return
	}
	// an envelope travels from sender to receiver every 1.6s
	k := float64(ms%1600) / 1600
	p := image.Pt(l.From.X+int(float64(l.To.X-l.From.X)*k), l.From.Y+int(float64(l.To.Y-l.From.Y)*k))
	env := envelopeArt.Bounds()
	at := p.Sub(image.Pt(env.Dx()/2, env.Dy()/2))
	pixel.Draw(img, envelopeArt, at)
	pixel.Fill(img, image.Rect(at.X, at.Y, at.X+env.Dx(), at.Y+1), c)
}
