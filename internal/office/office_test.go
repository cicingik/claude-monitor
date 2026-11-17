package office

import (
	"fmt"
	"image"
	"testing"
	"time"

	"github.com/cicingik/claude-monitor/internal/pixel"
	"github.com/cicingik/claude-monitor/internal/scan"
)

var t0 = time.Date(2026, 1, 1, 12, 0, 0, 0, time.UTC)

// office builds a snapshot with one session (status lead) and one subagent
// per status in subs, each last active ago[i] (default 5s).
func office(lead scan.Status, subs []scan.Status, ago ...time.Duration) scan.Snapshot {
	root := &scan.Agent{ID: "sess", SessionID: "sess", Kind: scan.Session, Status: lead, LastActivity: t0,
		Usage: scan.Usage{Output: 1000}}
	s := scan.Snapshot{Roots: []*scan.Agent{root}, ByID: map[string]*scan.Agent{"sess": root}, ScannedAt: t0}
	for i, st := range subs {
		d := 5 * time.Second
		if i < len(ago) {
			d = ago[i]
		}
		a := &scan.Agent{ID: fmt.Sprintf("a%02d", i), ParentID: "sess", SessionID: "sess", Kind: scan.Subagent,
			AgentType: "general-purpose", Status: st, LastActivity: t0.Add(-d), LastTool: "Bash: ls",
			Usage: scan.Usage{Output: int64(100 * (i + 1))}}
		root.Children = append(root.Children, a)
		s.ByID[a.ID] = a
	}
	return s
}

func mixed(n int) []scan.Status {
	order := []scan.Status{scan.Running, scan.Idle, scan.Active, scan.Done, scan.Running, scan.Done, scan.Idle}
	out := make([]scan.Status, n)
	for i := range out {
		out[i] = order[i%len(order)]
	}
	return out
}

func TestLayoutActorsNeverOverlap(t *testing.T) {
	sizes := [][2]int{{160, 76}, {160, 90}, {80, 34}, {220, 106}, {120, 50}}
	for _, sz := range sizes {
		for _, lead := range []scan.Status{scan.Running, scan.Idle} {
			for n := 0; n <= 24; n++ {
				name := fmt.Sprintf("%dx%d lead=%s n=%d", sz[0], sz[1], lead, n)
				snap := office(lead, mixed(n))
				first := Layout(snap, "sess", sz[0], sz[1], t0)
				if len(first.Roster) != n+1 {
					t.Fatalf("%s: roster %d, want %d", name, len(first.Roster), n+1)
				}
				seen := map[string]int{}
				for _, id := range first.Roster {
					sc := LayoutFocus(snap, "sess", sz[0], sz[1], t0, id)
					a, ok := sc.Actor(id)
					if !ok || !a.Selected {
						t.Fatalf("%s: focus %s not drawn and selected on page %d/%d", name, id, sc.Page, sc.Pages)
					}
					for i, x := range sc.Actors {
						if x.AgentID != "sess" {
							seen[x.AgentID] |= 1 << sc.Page
						}
						if !x.Bounds().Overlaps(sc.Lounge.Union(sc.Desks[0]).Union(sc.Desks[len(sc.Desks)-1])) {
							t.Errorf("%s: %s at %v is outside the office", name, x.AgentID, x.Pos)
						}
						for _, y := range sc.Actors[i+1:] {
							if x.Bounds().Overlaps(y.Bounds()) {
								t.Errorf("%s page %d: %s %v overlaps %s %v", name, sc.Page, x.AgentID, x.Bounds(), y.AgentID, y.Bounds())
							}
						}
					}
				}
				for id, pages := range seen {
					if pages&(pages-1) != 0 {
						t.Errorf("%s: %s drawn on more than one page (mask %b)", name, id, pages)
					}
				}
				if len(seen) != n {
					t.Errorf("%s: %d agents drawn across pages, want %d", name, len(seen), n)
				}
			}
		}
	}
}

func TestLayoutStatusToZone(t *testing.T) {
	snap := office(scan.Idle,
		[]scan.Status{scan.Running, scan.Active, scan.Idle, scan.Done, scan.Done},
		5*time.Second, 5*time.Second, time.Minute, 2*time.Minute, 11*time.Minute)
	sc := Layout(snap, "", 160, 76, t0)
	tests := []struct {
		id     string
		zone   Zone
		anim   string
		bubble string
		screen Screen
	}{
		{"sess", Lounge, AnimSleep, "", ScreenOff},
		{"a00", Desk, AnimTyping, "Bash: ls", ScreenBusy},
		{"a01", Desk, AnimIdle, "", ScreenOn},
		{"a02", Lounge, AnimSleep, "", ScreenOff},
		{"a03", Lounge, AnimIdle, "", ScreenOff},
	}
	for _, tc := range tests {
		a, ok := sc.Actor(tc.id)
		if !ok {
			t.Errorf("%s: not drawn", tc.id)
			continue
		}
		if a.Zone != tc.zone || a.Anim != tc.anim || a.Bubble != tc.bubble {
			t.Errorf("%s: zone %v anim %q bubble %q, want %v %q %q", tc.id, a.Zone, a.Anim, a.Bubble, tc.zone, tc.anim, tc.bubble)
		}
		for i, d := range sc.DeskInfo {
			if d.Owner == tc.id && d.Screen != tc.screen {
				t.Errorf("%s: desk %d screen %v, want %v", tc.id, i, d.Screen, tc.screen)
			}
		}
		if a.Zone == Desk && !a.Bounds().Overlaps(sc.Desks[deskOf(sc, tc.id)]) {
			t.Errorf("%s: sits away from its desk", tc.id)
		}
		if a.Zone == Lounge && !a.Pos.In(sc.Lounge.Inset(-1)) {
			t.Errorf("%s: %v not in the lounge %v", tc.id, a.Pos, sc.Lounge)
		}
	}
	if _, ok := sc.Actor("a04"); ok || sc.Left != 1 {
		t.Errorf("agent done 11m ago: drawn=%v left=%d, want not drawn and left=1", ok, sc.Left)
	}
	if lead := sc.DeskInfo[0]; !lead.Lead || lead.Owner != "sess" {
		t.Errorf("desk 0 = %+v, want the lead's", lead)
	}
	if d := sc.DeskInfo[deskOf(sc, "a04")+1]; d.Meter < 0 || d.Meter > 1 {
		t.Errorf("meter out of range: %v", d.Meter)
	}
}

func deskOf(sc Scene, id string) int {
	for i, d := range sc.DeskInfo {
		if d.Owner == id {
			return i
		}
	}
	return -1
}

func TestLayoutLinksOnlyForRecentEdges(t *testing.T) {
	snap := office(scan.Running, []scan.Status{scan.Running, scan.Idle, scan.Done}, 5*time.Second, time.Minute, 20*time.Minute)
	edge := func(from, to, kind string, ago time.Duration) scan.Edge {
		return scan.Edge{From: from, To: to, Kind: kind, At: t0.Add(-ago)}
	}
	snap.Edges = []scan.Edge{
		edge("sess", "a00", scan.EdgeSpawn, 10*time.Second),
		edge("a00", "a01", scan.EdgeMessage, 89*time.Second),
		edge("a01", "sess", scan.EdgeHandback, 91*time.Second), // too old
		edge("sess", "a01", scan.EdgeSpawn, 10*time.Minute),    // too old
		edge("a02", "sess", scan.EdgeHandback, 5*time.Second),  // a02 has left
		edge("a00", "sess", scan.EdgeHandback, -2*time.Second), // clock skew counts as new
	}
	sc := Layout(snap, "sess", 160, 76, t0)
	var got []string
	for _, l := range sc.Links {
		got = append(got, fmt.Sprintf("%s %s>%s", l.Kind, l.FromID, l.ToID))
		from, _ := sc.Actor(l.FromID)
		if l.From != from.Head() || l.Age >= LinkTTL {
			t.Errorf("link %v: bad anchor or age", l)
		}
	}
	want := []string{"spawn sess>a00", "message a00>a01", "handback a00>sess"}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Errorf("links = %v, want %v", got, want)
	}
}

func TestLayoutUnknownSessionIsAnEmptyOffice(t *testing.T) {
	sc := Layout(office(scan.Running, nil), "nope", 160, 76, t0)
	if len(sc.Actors) != 0 || len(sc.Desks) == 0 {
		t.Errorf("actors %d desks %d, want 0 and some", len(sc.Actors), len(sc.Desks))
	}
	if img := Render(sc, t0); img.Bounds().Dx() != 160 || img.Bounds().Dy() != 76 {
		t.Errorf("render size %v", img.Bounds())
	}
	for _, sz := range [][2]int{{0, 0}, {10, 4}, {79, 30}} { // tiny canvases must not panic
		Render(Layout(office(scan.Running, mixed(5)), "sess", sz[0], sz[1], t0), t0)
	}
}

// BenchmarkFrame160x45 is one full office frame for a 160×45-cell terminal
// used entirely as canvas (160×90 pixels): layout, render and encode.
func BenchmarkFrame160x45(b *testing.B) {
	snap := office(scan.Running, mixed(8))
	snap.Edges = []scan.Edge{
		{From: "sess", To: "a00", Kind: scan.EdgeSpawn, At: t0.Add(-5 * time.Second)},
		{From: "a00", To: "a02", Kind: scan.EdgeMessage, At: t0.Add(-5 * time.Second)},
		{From: "a03", To: "sess", Kind: scan.EdgeHandback, At: t0.Add(-5 * time.Second)},
	}
	b.ReportAllocs()
	for i := 0; b.Loop(); i++ {
		now := t0.Add(time.Duration(i) * frameRate)
		sc := LayoutFocus(snap, "sess", 160, 90, now, "a01")
		pixel.Encode(Render(sc, now), Background)
	}
}

func TestWalkerWalksBetweenPlaces(t *testing.T) {
	var w Walker
	working := office(scan.Running, []scan.Status{scan.Running})
	sc := Layout(working, "sess", 160, 76, t0)
	desk, _ := sc.Actor("a00")
	w.Step(&sc, t0)
	if a, _ := sc.Actor("a00"); a.Pos != desk.Pos || a.Anim != AnimTyping {
		t.Fatalf("first step must place without walking: %+v", a)
	}

	// a00 goes idle: it walks to the lounge through the doorway
	resting := office(scan.Running, []scan.Status{scan.Idle})
	rest := Layout(resting, "sess", 160, 76, t0)
	target, _ := rest.Actor("a00")
	var walked []image.Point
	for i := 1; i <= 80; i++ {
		sc := Layout(resting, "sess", 160, 76, t0)
		w.Step(&sc, t0.Add(time.Duration(i)*frameRate))
		a, _ := sc.Actor("a00")
		walked = append(walked, a.Pos)
		if a.Pos == target.Pos {
			if a.Anim != AnimSleep {
				t.Errorf("arrived with anim %q, want sleep", a.Anim)
			}
			break
		}
		if a.Anim != AnimWalk {
			t.Fatalf("step %d: anim %q while on the way", i, a.Anim)
		}
	}
	if walked[len(walked)-1] != target.Pos {
		t.Fatalf("never arrived: ended at %v, want %v", walked[len(walked)-1], target.Pos)
	}
	door := false
	for _, p := range walked {
		door = door || (p.X >= sc.Door.X-8 && p.X <= sc.Door.X+8 && p.Y == sc.Door.Y)
	}
	if !door {
		t.Errorf("did not pass the doorway %v: %v", sc.Door, walked)
	}

	// a00 vanishes: it walks out with the leave animation, then is gone
	empty := office(scan.Running, nil)
	sc = Layout(empty, "sess", 160, 76, t0)
	w.Step(&sc, t0.Add(81*frameRate))
	if a, ok := sc.Actor("a00"); !ok || a.Anim != AnimLeave || a.Zone != Exit {
		t.Fatalf("vanished actor should walk out: %+v %v", a, ok)
	}
	for i := 82; i < 300; i++ {
		sc = Layout(empty, "sess", 160, 76, t0)
		w.Step(&sc, t0.Add(time.Duration(i)*frameRate))
	}
	if _, ok := sc.Actor("a00"); ok {
		t.Error("actor still drawn after walking out")
	}
}
