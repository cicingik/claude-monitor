// Package office turns a scan.Snapshot into a pixel-art office scene: one
// office per session, an actor per agent, desks, a lounge and the recent
// interactions between agents, and renders it to an RGBA image.
package office

import (
	"image"
	"time"

	"github.com/cicingik/claude-monitor/internal/pixel"
	"github.com/cicingik/claude-monitor/internal/scan"
)

// Zone is the part of the office an actor is in.
type Zone int

const (
	Desk Zone = iota
	Lounge
	Exit
)

func (z Zone) String() string { return [...]string{"desk", "lounge", "exit"}[z] }

// Pose is how a built-in character is drawn.
type Pose int

const (
	SitBack  Pose = iota // seated with the back to the viewer: desks, lower sofa
	SitFront             // seated facing the viewer: sofas
	Stand                // standing: kitchen, walking
)

// Animations.
const (
	AnimTyping = "typing"
	AnimIdle   = "idle"
	AnimWalk   = "walk"
	AnimSleep  = "sleep"
	AnimLeave  = "leave"
)

const (
	// CharW×CharH is the largest character; custom sprites are fit into it.
	CharW, CharH = 12, 16
	// LeaveAfter is how long a done agent stays before it leaves the office.
	LeaveAfter = 10 * time.Minute
	// LinkTTL is how long an edge is drawn between two actors.
	LinkTTL = 90 * time.Second
)

// Actor is one agent placed in the scene. Pos is the point just below its
// feet, horizontally centred.
type Actor struct {
	AgentID  string
	Type     string        // agent type, "session" for the lead; picks custom sprites
	Sprite   *pixel.Sprite // nil draws the built-in character
	Pos      image.Point
	Zone     Zone
	Anim     string // typing | idle | walk | sleep | leave
	Bubble   string // LastTool while running
	Selected bool
	Status   scan.Status
	Lead     bool
	Pose     Pose
}

// Bounds is the box the actor may cover.
func (a Actor) Bounds() image.Rectangle {
	return image.Rect(a.Pos.X-CharW/2, a.Pos.Y-CharH, a.Pos.X+CharW/2, a.Pos.Y)
}

// Head is where links attach: just above the head.
func (a Actor) Head() image.Point { return a.Pos.Sub(image.Pt(0, 15)) }

// Link is an interaction drawn between two actors.
type Link struct {
	From, To     image.Point
	FromID, ToID string
	Kind         string // spawn | message | handback
	Age          time.Duration
}

// Screen is the state of a desk's monitor.
type Screen int

const (
	ScreenOff Screen = iota
	ScreenOn
	ScreenBusy
)

// DeskInfo describes the desk at the same index in Scene.Desks.
type DeskInfo struct {
	Owner  string // agent id, "" for a free desk
	Lead   bool
	Meter  float64 // owner's tokens / the session's largest agent, 0..1
	Screen Screen
}

// Scene is everything Render draws, in canvas pixels.
type Scene struct {
	W, H     int
	Actors   []Actor
	Links    []Link
	Desks    []image.Rectangle // desk stations: meter, monitor, desk and chair
	DeskInfo []DeskInfo
	Lounge   image.Rectangle
	Door     image.Point // doorway between the rooms, a walking waypoint
	Exit     image.Point // exit door in the back wall
	Roster   []string    // every agent present in the office, all pages, selection order
	Page     int
	Pages    int
	Left     int // agents done more than LeaveAfter ago, not drawn

	g geom
}

// Actor returns the actor for an agent id.
func (sc *Scene) Actor(id string) (*Actor, bool) {
	for i := range sc.Actors {
		if sc.Actors[i].AgentID == id {
			return &sc.Actors[i], true
		}
	}
	return nil, false
}

// Layout places the agents of one session (default: the most recently active)
// in a w×h pixel office as of now, showing the first page.
func Layout(s scan.Snapshot, session string, w, h int, now time.Time) Scene {
	return LayoutFocus(s, session, w, h, now, "")
}

// LayoutFocus is Layout showing the page that holds focus, marked Selected.
func LayoutFocus(s scan.Snapshot, session string, w, h int, now time.Time, focus string) Scene {
	g := plan(w, h)
	sc := Scene{W: w, H: h, g: g, Lounge: g.lounge, Door: g.door, Exit: g.exit, Pages: 1}
	sc.Desks = append(sc.Desks, g.units...)
	sc.DeskInfo = make([]DeskInfo, len(g.units))
	if len(sc.DeskInfo) > 0 {
		sc.DeskInfo[0].Lead = true
	}

	root := s.ByID[session]
	if root == nil && session == "" && len(s.Roots) > 0 {
		root = s.Roots[0]
	}
	if root == nil || len(g.units) < 2 || len(g.seats) < 2 {
		return sc
	}
	var subs []*scan.Agent
	var peak int64
	scan.Walk(root, 0, func(a *scan.Agent, _ int) {
		peak = max(peak, a.Usage.Total())
		switch {
		case a == root:
		case a.Status == scan.Done && now.Sub(a.LastActivity) > LeaveAfter:
			sc.Left++
		default:
			subs = append(subs, a)
		}
	})
	sc.Roster = []string{root.ID}
	for _, a := range subs {
		sc.Roster = append(sc.Roster, a.ID)
	}

	pages := paginate(subs, len(g.units)-1, len(g.seats)-boolInt(!atDesk(root)))
	sc.Pages = len(pages)
	for i, p := range pages {
		for _, a := range p {
			if a.ID == focus {
				sc.Page = i
			}
		}
	}

	seats := g.seats
	place := func(a *scan.Agent, desk int) {
		act := Actor{AgentID: a.ID, Type: typeOf(a), Status: a.Status, Lead: a == root, Selected: a.ID == focus}
		d := &sc.DeskInfo[desk]
		d.Owner = a.ID
		if peak > 0 {
			d.Meter = float64(a.Usage.Total()) / float64(peak)
		}
		switch a.Status {
		case scan.Running:
			act.Zone, act.Anim, act.Pose, act.Pos, act.Bubble = Desk, AnimTyping, SitBack, deskSeat(g.units[desk]), a.LastTool
			d.Screen = ScreenBusy
		case scan.Active:
			act.Zone, act.Anim, act.Pose, act.Pos = Desk, AnimIdle, SitBack, deskSeat(g.units[desk])
			d.Screen = ScreenOn
		default:
			st := seats[0]
			seats = seats[1:]
			act.Zone, act.Anim, act.Pose, act.Pos = Lounge, AnimIdle, st.pose, st.pos
			if a.Status == scan.Idle {
				act.Anim = AnimSleep
			}
		}
		sc.Actors = append(sc.Actors, act)
	}
	place(root, 0)
	for i, a := range pages[sc.Page] {
		place(a, i+1)
	}

	for _, e := range s.Edges {
		from, ok1 := sc.Actor(e.From)
		to, ok2 := sc.Actor(e.To)
		age := max(now.Sub(e.At), 0)
		if ok1 && ok2 && e.From != e.To && age < LinkTTL {
			sc.Links = append(sc.Links, Link{From: from.Head(), To: to.Head(), FromID: e.From, ToID: e.To, Kind: e.Kind, Age: age})
		}
	}
	return sc
}

// paginate splits agents into pages that fit desks subagent desks and seats
// lounge seats. Every agent owns a desk on its page, even while in the lounge.
func paginate(agents []*scan.Agent, desks, seats int) [][]*scan.Agent {
	pages := [][]*scan.Agent{nil}
	d, s := desks, seats
	for _, a := range agents {
		need := boolInt(!atDesk(a))
		if d == 0 || s < need {
			pages, d, s = append(pages, nil), desks, seats
		}
		pages[len(pages)-1] = append(pages[len(pages)-1], a)
		d, s = d-1, s-need
	}
	return pages
}

func atDesk(a *scan.Agent) bool { return a.Status == scan.Running || a.Status == scan.Active }

func typeOf(a *scan.Agent) string {
	if a.Kind == scan.Session {
		return "session"
	}
	return a.AgentType
}

func boolInt(b bool) int {
	if b {
		return 1
	}
	return 0
}
