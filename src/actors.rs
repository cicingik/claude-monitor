//! The agents on the office picture, all sessions in one room: the places
//! they take (work: the manager's desk, nine open-plan desks, the meeting
//! room's chairs; rest: seats in the lounges, the kitchen and the manager's
//! sofa), the entrance and the corridors between them, as fractions of
//! office.png; who sits where (`layout`: the agents of active sessions, the
//! residents, by the rules below); how they walk there, and how agents of
//! inactive sessions drop in as visitors (`Walker`); and drawing them with
//! their marks and links (`paint`), and the pet that strolls round the
//! lobby (`Pet`, `paint_pet`).

mod graph;
mod paint;
mod walk;

use std::collections::{HashMap, HashSet};

use crate::{
    scan::{Agent, EdgeKind, Kind, SEC, Snapshot, Status},
    scene::{Point, Pose, WALK_SPEED},
};

pub use graph::GRAPH;
pub use paint::{Item, PET, link_rgb, paint, paint_pet};
pub use walk::Walker;

/// How long a done agent stays before it leaves the office.
pub const LEAVE_AFTER: i64 = 600 * SEC;
/// How long an edge is drawn between two agents.
pub const LINK_TTL: i64 = 90 * SEC;
/// A session is active, its agents residents of the office, while one of
/// them is running or active, or is not done and was active this recently.
pub const ACTIVE_WINDOW: i64 = 300 * SEC;
/// The most visitors in the office at once.
pub const MAX_VISITORS: usize = 2;
/// How fast visitors stroll: slower than residents heading somewhere.
const VISITOR_SPEED: f64 = 0.6 * WALK_SPEED;
/// Seconds from one visitor's chance to come in to the next, at least and at most.
const VISIT_EVERY: (f64, f64) = (4.0, 12.0);
/// Seconds a visitor may wait at a stop, at least and at most.
const VISIT_WAIT: (f64, f64) = (1.0, 3.0);
/// Seconds the pet waits at each of its stops, at least and at most.
const PET_WAIT: (f64, f64) = (1.0, 4.0);
/// How fast the pet strolls: slower than visitors, who so pass it.
const PET_SPEED: f64 = 0.5 * VISITOR_SPEED;

/// What a character is doing, which picks its motion and marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Anim {
    Typing, // running: at its desk, bobbing, a speech bubble
    Idle,   // active at its desk, or done in the lounge
    Sleep,  // idle in the lounge: still, with a z
    Walk,
}

/// One agent placed in the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Actor {
    pub id: String,
    pub kind: String, // agent type, "session" for the lead: picks the sprite
    pub status: Status,
    pub node: usize, // its place
    pub pos: Point,  // its feet; the place, or on the way there
    pub anim: Anim,
    pub left: bool, // facing left
    pub selected: bool,
    pub lead: bool,
    pub sprite: usize,  // index into the Cast
    pub visitor: bool, // of an inactive session, or with no place: walks a round and leaves
}

/// An edge drawn between two actors.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

/// Everything drawn in the office at one moment.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub actors: Vec<Actor>,
    pub links: Vec<Link>,
    pub roster: Vec<String>, // the agents drawn that can be selected: residents, then visitors
    pub guests: Vec<Actor>,  // who may visit, first in line first: the first `overflow` are residents with no place
    pub overflow: usize,
    pub sessions: usize, // active sessions
    pub left: usize,     // agents done more than LEAVE_AFTER ago, not drawn
    pub pet: Option<Pose>, // where the pet stands, once walked
}

fn at_desk(s: Status) -> bool {
    matches!(s, Status::Running | Status::Waiting | Status::Active)
}

/// Whether the session root is active at now: a running Claude Code process
/// holds it (a background one may write nothing for hours), or one of its
/// agents is running or active, or is not done and was active within ACTIVE_WINDOW.
pub fn session_active(snap: &Snapshot, root: &str, now: i64) -> bool {
    let mut on = false;
    snap.walk(root, 0, &mut |a, _| on |= a.live || at_desk(a.status) || (a.status != Status::Done && now - a.last_activity <= ACTIVE_WINDOW));
    on
}

/// Places the agents of every active session as of now, the residents:
/// running and active ones at desks, then on the meeting room's chairs; idle
/// and done ones on rest seats, a room at a time; the manager's desk for the
/// most recently active lead at a desk, the other leads before the
/// subagents. Each keeps the place it had in `was` (the last scene) while
/// that still suits it, so two busy sessions do not trade desks on every
/// scan. Residents left without a place, then every agent of the
/// inactive sessions (leads first), are the guests visitors come from.
pub fn layout(snap: &Snapshot, now: i64, was: &Scene) -> Scene {
    let g = &*GRAPH;
    let mut sc = Scene::default();
    let roots: Vec<&Agent> = snap.roots.iter().filter_map(|r| snap.by_id.get(r)).collect();
    let (active, inactive): (Vec<&Agent>, Vec<&Agent>) = roots.into_iter().partition(|r| session_active(snap, &r.id, now));
    sc.sessions = active.len();
    let mut residents = active.clone();
    for root in &active {
        snap.walk(&root.id, 0, &mut |a, depth| match depth {
            0 => {}
            _ if a.status == Status::Done && now - a.last_activity > LEAVE_AFTER => sc.left += 1,
            _ => residents.push(a),
        });
    }

    let had: HashMap<&str, usize> = was.actors.iter().filter(|a| !a.visitor).map(|a| (a.id.as_str(), a.node)).collect();
    let mut place: Vec<Option<usize>> = vec![None; residents.len()];
    let boss = |a: &&Agent| a.kind == Kind::Session && at_desk(a.status);
    let keeps = residents.iter().position(|a| boss(a) && had.get(a.id.as_str()) == Some(&g.lead_desk));
    if let Some(i) = keeps.or_else(|| residents.iter().position(boss)) {
        place[i] = Some(g.lead_desk);
    }
    let mut taken: HashSet<usize> = place.iter().flatten().copied().collect();
    let work: Vec<usize> = g.desks.iter().chain(&g.chairs).copied().collect();
    let suits = |a: &Agent| if at_desk(a.status) { &work } else { &g.seats };
    for (i, a) in residents.iter().enumerate() {
        if let (None, Some(&n)) = (place[i], had.get(a.id.as_str()).filter(|n| suits(a).contains(n) && !taken.contains(n))) {
            place[i] = Some(n);
            taken.insert(n);
        }
    }
    for (i, a) in residents.iter().enumerate() {
        if let (None, Some(&n)) = (place[i], suits(a).iter().find(|n| !taken.contains(n))) {
            place[i] = Some(n);
            taken.insert(n);
        }
    }

    let actor = |a: &Agent, node: usize| {
        let anim = match a.status {
            Status::Running => Anim::Typing,
            Status::Idle => Anim::Sleep,
            Status::Waiting | Status::Active | Status::Done => Anim::Idle,
        };
        let lead = a.kind == Kind::Session;
        let kind = if lead { "session".into() } else { a.agent_type.clone() };
        Actor { id: a.id.clone(), kind, status: a.status, node, pos: g.nodes[node], anim, left: false, selected: false, lead, sprite: 0, visitor: false }
    };
    for (a, p) in residents.iter().zip(&place) {
        match *p {
            Some(n) => {
                sc.roster.push(a.id.clone());
                sc.actors.push(actor(a, n));
            }
            None => sc.guests.push(Actor { anim: Anim::Walk, visitor: true, ..actor(a, g.exit) }),
        }
    }
    sc.overflow = sc.guests.len();
    let mut subs = Vec::new();
    for root in &inactive {
        snap.walk(&root.id, 0, &mut |a, depth| if depth == 0 { sc.guests.push(Actor { anim: Anim::Walk, visitor: true, ..actor(a, g.exit) }) } else { subs.push(a) });
    }
    sc.guests.extend(subs.into_iter().map(|a| Actor { anim: Anim::Walk, visitor: true, ..actor(a, g.exit) }));

    let here: HashSet<&str> = sc.actors.iter().map(|a| a.id.as_str()).collect();
    for e in &snap.edges {
        if here.contains(e.from.as_str()) && here.contains(e.to.as_str()) && e.from != e.to && (now - e.at).max(0) < LINK_TTL {
            sc.links.push(Link { from: e.from.clone(), to: e.to.clone(), kind: e.kind });
        }
    }
    sc
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::graph::{SIZE, apart};
    use crate::scan::{Edge, Usage};
    use crate::scene;
    use image::RgbaImage;

    const NOW: i64 = 1000 * SEC;

    fn agent(id: &str, status: Status, parent: &str) -> Agent {
        Agent {
            id: id.into(),
            parent_id: parent.into(),
            title: String::new(),
            agent_type: "general-purpose".into(),
            model: String::new(),
            project: "p".into(),
            session_id: "s".into(),
            config: "claude".into(),
            kind: if parent.is_empty() { Kind::Session } else { Kind::Subagent },
            status,
            last_activity: NOW,
            last_tool: "Bash: ls".into(),
            last_text: String::new(),
            config_dir: Default::default(),
            cwd: String::new(),
            asking: String::new(),
            turns: 0,
            usage: Usage::default(),
            model_usage: Default::default(),
            events: Vec::new(),
            children: Vec::new(),
            live: false,
        }
    }

    /// One session, "s", running, with subs.
    fn snapshot(subs: &[(&str, Status)]) -> Snapshot {
        sessions(&[("s", Status::Running, 0, subs)])
    }

    /// Sessions of (lead, its status, how long ago its agents were last
    /// active in nanoseconds, its subagents), most recently active first.
    type Session<'a> = (&'a str, Status, i64, &'a [(&'a str, Status)]);
    fn sessions(list: &[Session]) -> Snapshot {
        let mut snap = Snapshot { scanned_at: NOW, ..Snapshot::default() };
        for &(lead, st, ago, subs) in list {
            let mut root = agent(lead, st, "");
            root.last_activity = NOW - ago;
            for &(id, st) in subs {
                root.children.push(id.into());
                snap.by_id.insert(id.into(), Agent { last_activity: NOW - ago, ..agent(id, st, lead) });
            }
            snap.roots.push(lead.into());
            snap.by_id.insert(lead.into(), root);
        }
        snap
    }

    /// Places are CHARACTER_HEIGHT·MAX_ASPECT apart side by side, or a
    /// character height apart above each other, so no two sprites overlap;
    /// the stops where visitors wait are clear of every place too.
    #[test]
    fn places_and_stops_never_overlap() {
        let g = &*GRAPH;
        let places: Vec<usize> = g.places().collect();
        assert_eq!((g.desks.len(), g.chairs.len(), g.seats.len()), (9, 7, 13));
        for (i, &a) in places.iter().enumerate() {
            for &b in &places[i + 1..] {
                let (p, q) = (g.nodes[a], g.nodes[b]);
                assert!(apart(p, q), "places {p:?} and {q:?} overlap");
            }
        }
        assert!(g.stops.len() >= 6, "{} stops", g.stops.len());
        for &s in &g.stops {
            assert!(s != g.exit && places.iter().all(|&p| p != s && apart(g.nodes[s], g.nodes[p])), "stop {:?}", g.nodes[s]);
        }
    }

    /// Every corridor segment stays off the walls of office.png: walls are
    /// the only bright neutral gray in the picture.
    #[test]
    fn corridors_do_not_cross_walls() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/office.png");
        let Ok(img) = image::open(path) else {
            eprintln!("skipped: no {path}");
            return;
        };
        let img = img.to_rgba8();
        let (w, h) = (f64::from(img.width()), f64::from(img.height()));
        let g = &*GRAPH;
        let wall = |x: f64, y: f64| {
            let p = img.get_pixel((x * w) as u32, (y * h) as u32);
            let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
            lo >= 195 && hi - lo <= 16
        };
        // The top-left lounge's, the manager's, the washroom's and the
        // meeting room's doors are painted on the outer face of a wall 18 px
        // thick: going through crosses that.
        const DOORS: [(f64, f64); 4] = [(370.0, 318.0), (911.0, 806.0), (257.0, 500.0), (880.0, 390.0)];
        for (a, near) in g.adj.iter().enumerate() {
            for &b in near {
                let (p, q) = (g.nodes[a], g.nodes[b]);
                let n = ((q.x - p.x).hypot(q.y - p.y) * w) as usize;
                let at = |k: f64| (p.x + (q.x - p.x) * k, p.y + (q.y - p.y) * k);
                let bad = (0..=n).map(|i| at(i as f64 / n.max(1) as f64)).filter(|&(x, y)| wall(x, y)).count();
                let door = (0..=n).map(|i| at(i as f64 / n.max(1) as f64)).any(|(x, y)| DOORS.iter().any(|d| (x * SIZE - d.0).hypot(y * SIZE - d.1) < 15.0));
                let allowed = if door { 24 } else { 2 };
                assert!(bad <= allowed, "corridor {:?} -> {:?} crosses a wall ({bad} px)", (p.x * SIZE, p.y * SIZE), (q.x * SIZE, q.y * SIZE));
            }
        }
    }

    /// Draws the corridors (red), places (desks blue, meeting chairs cyan,
    /// seats green, each seat numbered by the order it fills), the
    /// visitors' stops (magenta), the entrance (yellow) and the pet's stops
    /// (orange rings) over office.png, to look at:
    /// OFFICE_GRAPH_PNG=/tmp/g.png cargo test graph_picture -- --ignored
    /// (OFFICE_GRAPH_BASE=assets/forest.png draws it over another picture)
    #[test]
    #[ignore]
    fn graph_picture() {
        let out = std::env::var("OFFICE_GRAPH_PNG").expect("OFFICE_GRAPH_PNG names the output");
        let base = std::env::var("OFFICE_GRAPH_BASE").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/assets/office.png").into());
        let mut img = image::open(base).unwrap().to_rgba8();
        let (w, h) = (f64::from(img.width()), f64::from(img.height()));
        let g = &*GRAPH;
        let px = |p: Point| (p.x * w, p.y * h);
        for (a, near) in g.adj.iter().enumerate() {
            for &b in near {
                scene::dotted_line(&mut img, px(g.nodes[a]), px(g.nodes[b]), 3, [255, 40, 40], 64);
            }
        }
        let dot = |img: &mut RgbaImage, n: usize, rgb: [u8; 3]| {
            let (x, y) = px(g.nodes[n]);
            scene::draw_art(img, &["###", "###", "###"], x as i64 - 6, y as i64 - 6, 4, |_| Some(rgb));
        };
        for &d in g.desks.iter().chain([&g.lead_desk]) {
            dot(&mut img, d, [60, 120, 255]);
        }
        for &c in &g.chairs {
            dot(&mut img, c, [40, 230, 240]);
        }
        for (i, &s) in g.seats.iter().enumerate() {
            dot(&mut img, s, [60, 220, 90]);
            // Its place in the filling order, in white ticks under it.
            let (x, y) = px(g.nodes[s]);
            scene::draw_art(&mut img, &["#.".repeat(i + 1).as_str()], x as i64 - 6, y as i64 + 10, 3, |c| (c == '#').then_some([255, 255, 255]));
        }
        for &s in &g.stops {
            dot(&mut img, s, [230, 60, 230]);
        }
        dot(&mut img, g.exit, [255, 220, 0]);
        for &s in &g.pet_stops {
            let (x, y) = px(g.nodes[s]);
            scene::draw_art(&mut img, &["#####", "#...#", "#...#", "#...#", "#####"], x as i64 - 10, y as i64 - 10, 4, |c| (c == '#').then_some([255, 140, 0]));
        }
        img.save(out).unwrap();
    }

    #[test]
    fn every_place_is_reachable_from_the_entrance() {
        let g = &*GRAPH;
        for &p in g.desks.iter().chain(&g.chairs).chain(&g.seats).chain([&g.lead_desk]) {
            let (path, len) = g.path(g.exit, p);
            assert!(len.is_finite() && path.first() == Some(&g.exit) && path.last() == Some(&p), "{p}: {path:?}");
        }
    }

    #[test]
    fn sessions_are_active_within_the_window() {
        const HOUR: i64 = 3600 * SEC;
        let snap = sessions(&[
            ("edge", Status::Idle, ACTIVE_WINDOW, &[]),
            ("past", Status::Idle, ACTIVE_WINDOW + 1, &[]),
            ("running", Status::Running, HOUR, &[]),
            ("active", Status::Active, HOUR, &[]),
            ("done-sub", Status::Idle, ACTIVE_WINDOW + 1, &[("d", Status::Done)]),
        ]);
        let on = |id: &str| session_active(&snap, id, NOW);
        assert!(on("edge"), "idle, last active exactly ACTIVE_WINDOW ago");
        assert!(!on("past"), "idle, a nanosecond more");
        assert!(on("running") && on("active"), "running and active always count");
        let mut snap = snap;
        snap.by_id.get_mut("d").unwrap().last_activity = NOW;
        assert!(!session_active(&snap, "done-sub", NOW), "a done agent does not keep its session active");
        snap.by_id.get_mut("d").unwrap().status = Status::Idle;
        assert!(session_active(&snap, "done-sub", NOW), "an idle one active just now does");
        snap.by_id.get_mut("past").unwrap().live = true;
        assert!(session_active(&snap, "past", NOW), "a quiet session a process holds, as a resumed background one");
        let sc = layout(&snap, NOW, &Scene::default());
        assert!(sc.actors.iter().any(|a| a.id == "past" && !a.visitor), "it is drawn in the office");
    }

    /// The nodes of sc's actors, each once.
    fn assert_one_each(sc: &Scene) {
        let nodes: HashSet<usize> = sc.actors.iter().map(|a| a.node).collect();
        assert_eq!(nodes.len(), sc.actors.len(), "two actors share a place: {:?}", sc.actors.iter().map(|a| (&a.id, a.node)).collect::<Vec<_>>());
    }

    #[test]
    fn residents_of_every_active_session_share_the_room() {
        let g = &*GRAPH;
        let snap = sessions(&[
            ("a", Status::Running, SEC, &[("a1", Status::Running), ("a2", Status::Idle), ("a3", Status::Done)]),
            ("b", Status::Active, 2 * SEC, &[("b1", Status::Active)]),
            ("c", Status::Idle, 60 * SEC, &[("c1", Status::Running)]),
            ("z", Status::Idle, 3600 * SEC, &[("z1", Status::Done)]),
        ]);
        let sc = layout(&snap, NOW, &Scene::default());
        let at = |sc: &Scene, id: &str| sc.actors.iter().find(|a| a.id == id).map(|a| (a.node, a.anim));
        assert_eq!(at(&sc, "a"), Some((g.lead_desk, Anim::Typing)), "the most recently active lead at a desk");
        assert_eq!(at(&sc, "b"), Some((g.desks[0], Anim::Idle)), "the other leads first");
        assert_eq!(at(&sc, "c"), Some((g.seats[0], Anim::Sleep)), "an idle lead dozes in the lounge");
        assert_eq!([at(&sc, "a1"), at(&sc, "b1"), at(&sc, "c1")].map(|p| p.unwrap().0), [g.desks[1], g.desks[2], g.desks[3]]);
        assert_eq!((at(&sc, "a2"), at(&sc, "a3")), (Some((g.seats[1], Anim::Sleep)), Some((g.seats[2], Anim::Idle))));
        assert_eq!((at(&sc, "z"), at(&sc, "z1")), (None, None), "an inactive session has no residents");
        assert_eq!((sc.sessions, sc.overflow), (3, 0));
        assert_eq!(sc.roster, ["a", "b", "c", "a1", "a2", "a3", "b1", "c1"]);
        assert_eq!(sc.guests.iter().map(|a| (a.id.as_str(), a.visitor)).collect::<Vec<_>>(), [("z", true), ("z1", true)]);
        assert_one_each(&sc);

        // b becomes the most recently active: the places stay as they are,
        // though a fresh room would give b the manager's desk.
        let mut snap2 = snap.clone();
        snap2.roots.swap(0, 1);
        snap2.by_id.get_mut("b").unwrap().status = Status::Running;
        let kept = layout(&snap2, NOW, &sc);
        assert_eq!((at(&kept, "a").unwrap().0, at(&kept, "b").unwrap().0), (g.lead_desk, g.desks[0]));
        assert_eq!(at(&layout(&snap2, NOW, &Scene::default()), "b").unwrap().0, g.lead_desk);
        // a dozes off: the manager's desk goes to b, a to the lounge.
        snap2.by_id.get_mut("a").unwrap().status = Status::Idle;
        let moved = layout(&snap2, NOW, &kept);
        assert_eq!(at(&moved, "b").unwrap().0, g.lead_desk);
        assert!(g.seats.contains(&at(&moved, "a").unwrap().0));
        assert_eq!(at(&moved, "a1").unwrap().0, g.desks[1], "the rest keep theirs");
        assert_one_each(&moved);
    }

    #[test]
    fn residents_beyond_the_places_become_visitors_first() {
        let g = &*GRAPH;
        let ids: Vec<String> = (0..20).map(|i| format!("r{i:02}")).collect();
        let subs: Vec<(&str, Status)> = ids.iter().map(|id| (id.as_str(), Status::Running)).collect();
        let snap = sessions(&[("x", Status::Running, 0, &subs), ("y", Status::Running, SEC, &[]), ("z", Status::Idle, 3600 * SEC, &[])]);
        let sc = layout(&snap, NOW, &Scene::default());
        // Seventeen work places (the manager's desk, nine desks, seven
        // meeting chairs) for two leads and twenty runners: five have none.
        assert_eq!((sc.actors.len(), sc.overflow), (17, 5));
        assert_eq!(sc.guests.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["r15", "r16", "r17", "r18", "r19", "z"]);
        assert!(sc.actors.iter().all(|a| a.node == g.lead_desk || g.desks.contains(&a.node) || g.chairs.contains(&a.node)));
        assert_one_each(&sc);
        // The first visitor is one of them.
        let mut w = Walker::new(3);
        let mut was = Scene::default();
        for i in 0..400 {
            let mut sc = layout(&snap, NOW, &was);
            w.step(&mut sc, f64::from(i) * 0.05);
            if let Some(v) = sc.actors.iter().find(|a| a.visitor) {
                assert_eq!(v.id, "r15");
                return;
            }
            was = sc;
        }
        panic!("no visitor in 20 s");
    }

    /// The room of office.png a point lies in, by rectangles (pixels of the
    /// 1254 px picture) read off the picture, not off the graph. The copy
    /// room is left out: no door leads into it.
    fn room_of(p: Point) -> Option<&'static str> {
        const ROOMS: [(&str, [f64; 4]); 10] = [
            ("lobby", [380.0, 120.0, 850.0, 345.0]),
            ("lobby", [285.0, 345.0, 870.0, 445.0]),
            ("lobby", [285.0, 445.0, 340.0, 530.0]),
            ("top-left lounge", [45.0, 40.0, 375.0, 335.0]),
            ("washroom", [45.0, 345.0, 265.0, 530.0]),
            ("kitchen", [45.0, 717.0, 285.0, 1165.0]),
            ("open plan", [285.0, 445.0, 870.0, 1165.0]),
            ("meeting room", [860.0, 40.0, 1215.0, 495.0]),
            ("manager's office", [890.0, 515.0, 1215.0, 870.0]),
            ("bottom-right lounge", [920.0, 890.0, 1215.0, 1165.0]),
        ];
        let (x, y) = (p.x * SIZE, p.y * SIZE);
        ROOMS.iter().find(|(_, [x0, y0, x1, y1])| (*x0..*x1).contains(&x) && (*y0..*y1).contains(&y)).map(|(name, _)| *name)
    }

    /// Visitors can wait in every room with a door, and walk there.
    #[test]
    fn every_room_has_a_reachable_stop() {
        let g = &*GRAPH;
        for room in ["lobby", "top-left lounge", "washroom", "kitchen", "open plan", "meeting room", "manager's office", "bottom-right lounge"] {
            let stops: Vec<usize> = g.stops.iter().copied().filter(|&s| room_of(g.nodes[s]) == Some(room)).collect();
            assert!(stops.iter().any(|&s| g.path(g.exit, s).1.is_finite()), "{room}: no reachable stop among {stops:?}");
        }
    }

    /// One idle agent per rest room before any room gets a second, while
    /// the lead works at the manager's desk.
    #[test]
    fn idle_agents_spread_over_the_rest_rooms() {
        let g = &*GRAPH;
        let snap = snapshot(&[("i1", Status::Idle), ("i2", Status::Done), ("i3", Status::Idle), ("i4", Status::Idle)]);
        let sc = layout(&snap, NOW, &Scene::default());
        let room = |id: &str| room_of(sc.actors.iter().find(|a| a.id == id).map(|a| a.pos).unwrap());
        let rooms: Vec<_> = ["i1", "i2", "i3", "i4"].map(room).into_iter().collect();
        assert_eq!(rooms, [Some("top-left lounge"), Some("kitchen"), Some("bottom-right lounge"), Some("manager's office")]);
        assert_eq!(room("s"), Some("manager's office"), "the lead at its desk");
        assert_one_each(&sc);
        // Every rest seat is in a rest room, and the first three are in
        // three rooms.
        let rest: HashSet<_> = g.seats.iter().map(|&s| room_of(g.nodes[s]).unwrap()).collect();
        assert_eq!(rest, ["top-left lounge", "kitchen", "bottom-right lounge", "manager's office"].into());
        let first: HashSet<_> = g.seats[..3].iter().map(|&s| room_of(g.nodes[s])).collect();
        assert_eq!(first.len(), 3);
    }

    /// Twelve running subagents: the nine desks, then three meeting chairs,
    /// every sprite clear of the others.
    #[test]
    fn runners_beyond_the_desks_take_the_meeting_room() {
        let g = &*GRAPH;
        let ids: Vec<String> = (0..12).map(|i| format!("r{i:02}")).collect();
        let subs: Vec<(&str, Status)> = ids.iter().map(|id| (id.as_str(), Status::Running)).collect();
        let sc = layout(&snapshot(&subs), NOW, &Scene::default());
        let nodes: Vec<usize> = sc.actors.iter().filter(|a| a.id.starts_with('r')).map(|a| a.node).collect();
        assert_eq!(nodes[..9], g.desks[..], "the desks first");
        assert_eq!(nodes[9..], g.chairs[..3]);
        assert!(nodes[9..].iter().all(|&n| room_of(g.nodes[n]) == Some("meeting room")));
        assert_eq!((sc.actors.len(), sc.overflow), (13, 0));
        assert_one_each(&sc);
        for (i, a) in sc.actors.iter().enumerate() {
            for b in &sc.actors[i + 1..] {
                assert!(apart(a.pos, b.pos), "{} at {:?} and {} at {:?} overlap", a.id, a.pos, b.id, b.pos);
            }
        }
    }

    /// One minute, 100 ms a step (600 frames), of a room where visitors come
    /// and go, and the pet strolls: where it stands each frame, and the most
    /// frames in a row it overlapped one agent, and stood still overlapping one.
    fn pet_walk(seed: u64) -> (Vec<Pose>, usize, usize) {
        const HOUR: i64 = 3600 * SEC;
        let snap = sessions(&[
            ("a", Status::Running, 0, &[("a1", Status::Running), ("a2", Status::Idle)]),
            ("v1", Status::Idle, HOUR, &[]),
            ("v2", Status::Idle, HOUR, &[]),
            ("v3", Status::Idle, HOUR, &[]),
        ]);
        let mut w = Walker::new(seed);
        let (mut was, mut poses, mut together, mut most) = (Scene::default(), Vec::new(), HashMap::<String, usize>::new(), 0);
        let (mut still, mut most_still) = (0, 0);
        for i in 0..600 {
            let mut sc = layout(&snap, NOW, &was);
            w.step(&mut sc, f64::from(i) * 0.1);
            let pet = sc.pet.expect("the pet is always there");
            together = sc.actors.iter().filter(|a| !apart(a.pos, pet.p)).map(|a| (a.id.clone(), together.get(&a.id).map_or(1, |n| n + 1))).collect();
            most = together.values().copied().fold(most, usize::max);
            still = if !pet.moving && !together.is_empty() { still + 1 } else { 0 };
            most_still = most_still.max(still);
            poses.push(pet);
            was = sc;
        }
        (poses, most, most_still)
    }

    /// The pet stays in the lobby for good, walking only the corridors
    /// between its stops (never to the entrance, never into another room),
    /// waits at three stops or more a minute, and never stands where an
    /// agent is: it makes way at once, and they only pass (slower than
    /// visitors, it is overtaken). Its stops are no agent's place. The same
    /// seed walks it the same way.
    #[test]
    fn the_pet_strolls_round_the_lobby() {
        let g = &*GRAPH;
        assert!(g.pet_stops.iter().all(|s| g.stops.contains(s) && room_of(g.nodes[*s]) == Some("lobby")), "{:?}", g.pet_stops);
        let (poses, most, still) = pet_walk(4);
        let mut waited_at: HashSet<usize> = HashSet::new();
        for (i, ps) in poses.iter().enumerate() {
            assert_eq!(room_of(ps.p), Some("lobby"), "frame {i}: {:?}", ps.p);
            assert!(off_edges(ps.p, |n| g.pet_stops.contains(&n)) < 1e-9, "frame {i}: {:?} off the lobby's corridors", ps.p);
            if !ps.moving {
                waited_at.extend(g.pet_stops.iter().copied().filter(|&s| g.nodes[s] == ps.p));
            }
        }
        assert!(waited_at.len() >= 3, "waited at {waited_at:?}");
        assert!(poses.iter().filter(|ps| ps.moving).count() > 100, "it walks");
        // Over 30 seeds the longest pass took 63 frames, at its pace.
        for (seed, (_, most, still)) in (0..30).map(|seed| (seed, pet_walk(seed))) {
            assert!(still <= 1 && most <= 80, "seed {seed}: overlapped one agent {most} frames in a row, {still} of them standing");
        }
        assert!(most > 0 && still <= 1, "it meets visitors");
        assert_eq!(pet_walk(4).0, poses);
        assert_ne!(pet_walk(5).0, poses);
    }

    /// The distance from p to the nearest corridor between two nodes `on` keeps.
    fn off_edges(p: Point, on: impl Fn(usize) -> bool) -> f64 {
        let g = &*GRAPH;
        let seg = |a: Point, b: Point| {
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let k = (((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            (p.x - a.x - k * dx).hypot(p.y - a.y - k * dy)
        };
        let on = &on;
        g.adj
            .iter()
            .enumerate()
            .filter(|&(a, _)| on(a))
            .flat_map(|(a, near)| near.iter().filter(|&&b| on(b)).map(move |&b| seg(g.nodes[a], g.nodes[b])))
            .fold(f64::INFINITY, f64::min)
    }

    /// The distance from p to the nearest corridor.
    fn off_corridor(p: Point) -> f64 {
        off_edges(p, |_| true)
    }

    /// Five minutes of a room with one resident and six guests, 50 ms a
    /// step: each visit as (arrival time, agent), and how many frames showed
    /// a visitor waiting.
    fn visits(seed: u64) -> (Vec<(f64, String)>, usize) {
        let g = &*GRAPH;
        const HOUR: i64 = 3600 * SEC;
        let mut snap = sessions(&[
            ("a", Status::Running, 0, &[]),
            ("v1", Status::Idle, HOUR, &[("v1a", Status::Done)]),
            ("v2", Status::Idle, HOUR, &[]),
            ("v3", Status::Idle, HOUR, &[]),
            ("v4", Status::Idle, HOUR, &[]),
            ("v5", Status::Idle, HOUR, &[]),
        ]);
        snap.edges.push(Edge { from: "v2".into(), to: "a".into(), kind: EdgeKind::Message, at: NOW, label: String::new() });
        let mut w = Walker::new(seed);
        let (mut was, mut seen, mut arrivals, mut waits) = (Scene::default(), HashMap::<String, Point>::new(), Vec::new(), 0);
        for i in 0..6000 {
            let sec = f64::from(i) * 0.05;
            let mut sc = layout(&snap, NOW, &was);
            w.step(&mut sc, sec);
            let on: Vec<&Actor> = sc.actors.iter().filter(|a| a.visitor).collect();
            assert!(on.len() <= MAX_VISITORS, "{} visitors", on.len());
            assert!(sc.links.is_empty(), "no links to visitors: {:?}", sc.links);
            for v in &on {
                assert!(off_corridor(v.pos) < 1e-9, "{} at {:?}, off the corridors", v.id, v.pos);
                assert!(sc.roster.contains(&v.id), "visitors can be selected");
                if !seen.contains_key(&v.id) {
                    assert_eq!(v.pos, g.nodes[g.exit], "{} comes in at the entrance", v.id);
                    arrivals.push((sec, v.id.clone()));
                }
                if v.anim == Anim::Idle {
                    waits += 1;
                    assert!(g.stops.iter().any(|&s| g.nodes[s] == v.pos), "{} waits at {:?}, not a stop", v.id, v.pos);
                }
            }
            for (id, last) in &seen {
                if !on.iter().any(|v| v.id == *id) {
                    assert_eq!(*last, g.nodes[g.exit], "{id} left from {last:?}, not the entrance");
                }
            }
            seen = on.iter().map(|v| (v.id.clone(), v.pos)).collect();
            was = sc;
        }
        (arrivals, waits)
    }

    #[test]
    fn visitors_come_in_wander_the_corridors_and_leave() {
        let (arrivals, waits) = visits(7);
        let who: Vec<&str> = arrivals.iter().map(|(_, id)| id.as_str()).collect();
        assert!(who.len() >= 12, "{who:?}");
        // Leads first, and nobody twice before everyone had a turn.
        assert_eq!(who[..6], ["v1", "v2", "v3", "v4", "v5", "v1a"]);
        let mut round: Vec<&str> = who[6..12].to_vec();
        round.sort();
        assert_eq!(round, ["v1", "v1a", "v2", "v3", "v4", "v5"], "{who:?}");
        assert!(arrivals.windows(2).all(|p| p[1].0 - p[0].0 >= VISIT_EVERY.0 - 1e-9), "{arrivals:?}");
        assert!(waits > 0, "some wait at a stop");
        // The same seed, the same visits; another seed, others.
        assert_eq!(visits(7).0, arrivals);
        assert_ne!(visits(8).0, arrivals);
    }

    #[test]
    fn a_visitor_whose_session_wakes_walks_on_as_a_resident() {
        let mut snap = sessions(&[("a", Status::Running, 0, &[]), ("v", Status::Idle, 3600 * SEC, &[])]);
        let mut w = Walker::new(1);
        let (mut was, mut t) = (Scene::default(), 0.0);
        let mut last = None;
        while t < 30.0 {
            t += 0.05;
            let mut sc = layout(&snap, NOW, &was);
            w.step(&mut sc, t);
            let v = sc.actors.iter().filter(|a| a.id == "v").collect::<Vec<_>>();
            assert!(v.len() <= 1, "drawn twice");
            match (v.first(), last) {
                (Some(v), None) if v.visitor && v.pos != GRAPH.nodes[GRAPH.exit] => {
                    snap.by_id.get_mut("v").unwrap().status = Status::Running;
                    last = Some(v.pos);
                }
                (Some(v), Some(p)) => {
                    assert!(!v.visitor && (v.pos.x - p.x).hypot(v.pos.y - p.y) <= WALK_SPEED * 0.05 + 1e-9, "jumped from {p:?} to {:?}", v.pos);
                    assert_eq!(v.anim, Anim::Walk);
                    return;
                }
                _ => {}
            }
            was = sc;
        }
        panic!("no visit in 30 s");
    }

    #[test]
    fn layout_puts_runners_at_desks_and_idle_in_the_lounge() {
        let snap = snapshot(&[("a", Status::Running), ("b", Status::Idle), ("c", Status::Done), ("d", Status::Active)]);
        let sc = layout(&snap, NOW, &Scene::default());
        let g = &*GRAPH;
        let at = |id: &str| sc.actors.iter().find(|a| a.id == id).unwrap();
        assert_eq!((at("s").node, at("s").lead), (g.lead_desk, true));
        assert_eq!((at("a").node, at("a").anim), (g.desks[0], Anim::Typing));
        assert_eq!((at("b").node, at("b").anim), (g.seats[0], Anim::Sleep));
        assert_eq!((at("c").node, at("c").anim), (g.seats[1], Anim::Idle));
        assert_eq!((at("d").node, at("d").anim), (g.desks[1], Anim::Idle));
        assert_eq!(sc.roster, ["s", "a", "b", "c", "d"]);

        // Done more than ten minutes ago: left the office.
        let sc = layout(&snap, 2000 * SEC, &Scene::default());
        assert_eq!((sc.left, sc.actors.len()), (1, 4));
    }

    #[test]
    fn newcomers_walk_in_from_the_entrance_along_corridors_and_face_their_way() {
        let g = &*GRAPH;
        let none = Scene::default();
        let snap = snapshot(&[]);
        let mut w = Walker::new(1);
        let mut sc = layout(&snap, NOW, &none);
        w.step(&mut sc, 0.0);
        assert_eq!(sc.actors[0].pos, g.nodes[g.lead_desk], "the first frame places without walking");

        let snap = snapshot(&[("a", Status::Running)]);
        let mut sc = layout(&snap, NOW, &none);
        w.step(&mut sc, 0.1);
        let a = sc.actors.iter().find(|a| a.id == "a").unwrap();
        assert_eq!(a.anim, Anim::Walk);
        assert!((a.pos.y - g.nodes[g.exit].y).abs() < 0.02, "starts at the entrance: {:?}", a.pos);
        // It reaches its desk after walking the corridor's length.
        let (path, len) = g.path(g.exit, g.desks[0]);
        assert!(path.len() > 3);
        let mut t = 0.1;
        let mut lefts = 0;
        while t < 0.1 + len / WALK_SPEED + 1.0 {
            t += 0.05;
            let mut sc = layout(&snap, NOW, &none);
            w.step(&mut sc, t);
            let a = sc.actors.iter().find(|a| a.id == "a").unwrap();
            lefts += usize::from(a.left);
        }
        let mut sc = layout(&snap, NOW, &none);
        w.step(&mut sc, t + 0.05);
        let a = sc.actors.iter().find(|a| a.id == "a").unwrap();
        assert_eq!((a.pos, a.anim), (g.nodes[g.desks[0]], Anim::Typing));
        assert!(lefts > 0, "desk 0 is west of the entrance: it faced left on the way");

        // Gone from the scene: it walks out to the entrance, then vanishes.
        let snap = snapshot(&[]);
        let mut seen_leaving = false;
        for i in 0..400 {
            let mut sc = layout(&snap, NOW, &none);
            w.step(&mut sc, t + 0.1 + f64::from(i) * 0.05);
            seen_leaving |= sc.actors.iter().any(|a| a.id == "a" && a.anim == Anim::Walk);
            if i == 399 {
                assert_eq!(sc.actors.len(), 1, "out of the door");
            }
        }
        assert!(seen_leaving);
    }
}
