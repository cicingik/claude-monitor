package office

import (
	"image"
	"math"
	"time"
)

// walkSpeed is how fast actors walk, in pixels per second.
const walkSpeed = 30.0

// Walker carries actors between the places successive Layouts give them. An
// actor whose place changed walks there, through the doorway when it changes
// rooms; a newcomer walks in from the exit; one that vanished walks out to it.
type Walker struct {
	movers map[string]*mover
	last   time.Time
	primed bool
}

type mover struct {
	x, y  float64
	path  []image.Point // remaining waypoints, the last is the destination
	actor Actor         // as last drawn
	gone  bool          // left the scene, walking out
}

func (m *mover) at() image.Point { return image.Pt(int(math.Round(m.x)), int(math.Round(m.y))) }

func (m *mover) dest() image.Point {
	if len(m.path) == 0 {
		return m.at()
	}
	return m.path[len(m.path)-1]
}

func (m *mover) advance(d float64) {
	for d > 0 && len(m.path) > 0 {
		p := m.path[0]
		dx, dy := float64(p.X)-m.x, float64(p.Y)-m.y
		n := math.Hypot(dx, dy)
		if n <= d {
			m.x, m.y, m.path, d = float64(p.X), float64(p.Y), m.path[1:], d-n
			continue
		}
		m.x, m.y, d = m.x+dx/n*d, m.y+dy/n*d, 0
	}
}

// Reset forgets every actor, so the next Step places them without walking.
func (w *Walker) Reset() { *w = Walker{} }

// Step advances the walk to now and rewrites sc: actors still on their way
// are drawn where they are with the walk (or leave) animation, and links
// follow them.
func (w *Walker) Step(sc *Scene, now time.Time) {
	if w.movers == nil {
		w.movers = map[string]*mover{}
	}
	dt := min(max(now.Sub(w.last).Seconds(), 0), 0.25)
	if w.last.IsZero() {
		dt = 0
	}
	w.last = now
	step := walkSpeed * dt

	here := map[string]bool{}
	for i := range sc.Actors {
		a := &sc.Actors[i]
		here[a.AgentID] = true
		m := w.movers[a.AgentID]
		if m == nil {
			m = &mover{x: float64(a.Pos.X), y: float64(a.Pos.Y)}
			if w.primed && sc.Exit != (image.Point{}) {
				m.x, m.y = float64(sc.Exit.X), float64(sc.Exit.Y)
			}
			w.movers[a.AgentID] = m
		}
		if m.gone || m.dest() != a.Pos {
			m.gone, m.path = false, route(sc, m.at(), a.Pos)
		}
		m.advance(step)
		if p := m.at(); p != a.Pos {
			a.Pos, a.Anim, a.Pose = p, AnimWalk, Stand
		}
		m.actor = *a
	}
	for id, m := range w.movers {
		if here[id] {
			continue
		}
		if !m.gone {
			m.gone, m.path = true, route(sc, m.at(), sc.Exit)
		}
		m.advance(step)
		if m.at() == sc.Exit || len(m.path) == 0 {
			delete(w.movers, id)
			continue
		}
		g := m.actor
		g.Pos, g.Zone, g.Anim, g.Pose, g.Selected, g.Bubble = m.at(), Exit, AnimLeave, Stand, false, ""
		sc.Actors = append(sc.Actors, g)
	}
	w.primed = true

	for i := range sc.Links {
		l := &sc.Links[i]
		if a, ok := sc.Actor(l.FromID); ok {
			l.From = a.Head()
		}
		if a, ok := sc.Actor(l.ToID); ok {
			l.To = a.Head()
		}
	}
}

// route is the path from a to b, through the doorway if they are in
// different rooms.
func route(sc *Scene, a, b image.Point) []image.Point {
	d := sc.Door
	if d == (image.Point{}) || (a.X < d.X) == (b.X < d.X) {
		return []image.Point{b}
	}
	in, out := image.Pt(d.X-8, d.Y), image.Pt(d.X+8, d.Y)
	if a.X > d.X {
		in, out = out, in
	}
	return []image.Point{in, out, b}
}
