//! The agents on the office picture, all sessions in one room: the places
//! they take (work: the manager's desk, nine open-plan desks, the meeting
//! room's chairs; rest: seats in the lounges, the kitchen and the manager's
//! sofa), the entrance and the corridors between them, as fractions of
//! office.png; who sits where (`layout`: the agents of active sessions, the
//! residents, by the rules below); how they walk there, and how agents of
//! inactive sessions drop in as visitors (`Walker`); and drawing them with
//! their marks and links (`paint`), and the pet that strolls round the
//! lobby (`Pet`, `paint_pet`).

use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::{DefaultHasher, Hash, Hasher},
    sync::LazyLock,
    time::Duration,
};

use image::RgbaImage;

use crate::{
    cast::fnv32a,
    scan::{Agent, EdgeKind, Kind, SEC, Snapshot, Status},
    scene::{self, CHARACTER_HEIGHT, Layout, Point, Pose, Scaled, WALK_SPEED},
};

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

/// The places and corridors, built once from office.png measurements.
pub struct Graph {
    pub nodes: Vec<Point>, // fractions of the office
    adj: Vec<Vec<usize>>,
    pub exit: usize,
    pub lead_desk: usize,
    pub desks: Vec<usize>,  // the open plan's desks, row by row
    pub chairs: Vec<usize>, // the meeting room's: work places once the desks are taken
    pub seats: Vec<usize>,  // rest seats, in the order they fill: one room after another
    pub stops: Vec<usize>,  // where visitors may wait: no place, clear of every place
    pub pet_stops: Vec<usize>, // the lobby's nodes the pet strolls between: not the entrance
}

/// The picture the measurements were taken on is 1254×1254.
const SIZE: f64 = 1254.0;

pub static GRAPH: LazyLock<Graph> = LazyLock::new(|| {
    let mut g = Graph { nodes: Vec::new(), adj: Vec::new(), exit: 0, lead_desk: 0, desks: Vec::new(), chairs: Vec::new(), seats: Vec::new(), stops: Vec::new(), pet_stops: Vec::new() };
    let mut n = |x: f64, y: f64| {
        g.nodes.push(Point { x: x / SIZE, y: y / SIZE });
        g.adj.push(Vec::new());
        g.nodes.len() - 1
    };
    // Lobby: the glass doors at the top, around the reception desk.
    let exit = n(630.0, 118.0);
    let (lob_n, lob_nw, lob_ne) = (n(630.0, 190.0), n(455.0, 190.0), n(805.0, 190.0));
    let (lob_sw, lob_s, lob_se) = (n(455.0, 420.0), n(630.0, 420.0), n(805.0, 420.0));
    // Top-left lounge, through its door on the lobby's west wall; a stop in
    // front of the coffee table.
    let (tl_out, tl_in, tl_s) = (n(425.0, 330.0), n(318.0, 305.0), n(205.0, 315.0));
    let (tl_gap_s, tl_gap_n, tl_sw) = (n(257.0, 292.0), n(257.0, 210.0), n(150.0, 300.0));
    let (tl_sofa, tl_arm_l, tl_arm_r) = (n(203.0, 198.0), n(119.0, 266.0), n(290.0, 250.0));
    // Washroom, through its door on the lobby's west strip, to the sinks.
    let (wc_out, wc_in, wc_n, wc_w) = (n(310.0, 500.0), n(228.0, 500.0), n(228.0, 425.0), n(110.0, 425.0));
    // Meeting room, through its door on the lobby's east wall: an aisle
    // round the table, and seven of its chairs (the rest stand too close).
    let (mt_out, mt_in, mt_w) = (n(805.0, 390.0), n(915.0, 390.0), n(915.0, 268.0));
    let (mt_nw, mt_n1, mt_n2) = (n(915.0, 125.0), n(992.0, 125.0), n(1093.0, 125.0));
    let (mt_sw, mt_s, mt_se, mt_e2, mt_e1) = (n(915.0, 475.0), n(1023.0, 478.0), n(1182.0, 475.0), n(1182.0, 368.0), n(1182.0, 268.0));
    let chairs = vec![n(992.0, 172.0), n(1093.0, 172.0), n(955.0, 268.0), n(1132.0, 268.0), n(955.0, 368.0), n(1132.0, 368.0), n(1023.0, 458.0)];
    // The copy room has no door on the picture: nobody goes in.
    // Open plan: past either end of the planter shelf, then a grid of aisles.
    let (op_nw, op_ne) = (n(345.0, 470.0), n(835.0, 470.0));
    let (xs, ys) = ([340.0, 505.0, 650.0, 822.0], [565.0, 742.0, 905.0, 1100.0]);
    let grid: Vec<Vec<usize>> = xs.iter().map(|&x| ys.iter().map(|&y| n(x, y)).collect()).collect();
    // Desks: the chair of each, reached from the aisle above its row.
    let (cols, rows) = ([430.0, 575.0, 720.0], [642.0, 806.0, 977.0]);
    let mut desks = Vec::new();
    let mut desk_entries = Vec::new();
    for (r, &y) in rows.iter().enumerate() {
        for (c, &x) in cols.iter().enumerate() {
            desk_entries.push((n(x, ys[r]), c, r));
            desks.push(n(x, y));
        }
    }
    // The manager's office, through its door on the open plan's east wall.
    let (mgr_out, mgr_in, mgr_w, mgr_nw) = (n(845.0, 812.0), n(945.0, 800.0), n(962.0, 735.0), n(962.0, 628.0));
    let (lead_desk, mgr_sofa) = (n(1048.0, 640.0), n(990.0, 815.0));
    // Bottom-right lounge, through its door on the open plan's east wall.
    let (br_out, br_in, br_w, br_n, br_s) = (n(850.0, 1120.0), n(975.0, 1112.0), n(975.0, 1000.0), n(1060.0, 1000.0), n(1060.0, 1150.0));
    let (br_tr, br_bl, br_br) = (n(1105.0, 1042.0), n(1015.0, 1132.0), n(1108.0, 1134.0));
    // Kitchen and dining room, through the door on the open plan's west wall.
    let (kit_in, kit_e, kit_se, kit_mid) = (n(265.0, 745.0), n(265.0, 850.0), n(268.0, 978.0), n(180.0, 985.0));
    let (kit_nw, kit_sw, kit_s) = (n(70.0, 985.0), n(70.0, 1150.0), n(200.0, 1155.0));
    let (stool_l, stool_r) = (n(120.0, 945.0), n(217.0, 945.0));
    let (din_tl, din_tr, din_bl, din_br) = (n(155.0, 1037.0), n(245.0, 1037.0), n(155.0, 1137.0), n(245.0, 1137.0));

    let mut e = |a: usize, b: usize| {
        g.adj[a].push(b);
        g.adj[b].push(a);
    };
    for (a, b) in [(exit, lob_n), (lob_n, lob_nw), (lob_n, lob_ne), (lob_nw, lob_sw), (lob_ne, mt_out), (mt_out, lob_se), (lob_sw, lob_s), (lob_s, lob_se)] {
        e(a, b);
    }
    for (a, b) in [(lob_nw, tl_out), (lob_sw, tl_out), (tl_out, tl_in), (tl_in, tl_gap_s), (tl_gap_s, tl_gap_n), (tl_gap_n, tl_sofa)] {
        e(a, b);
    }
    for (a, b) in [(tl_in, tl_s), (tl_s, tl_sw), (tl_sw, tl_arm_l), (tl_in, tl_arm_r)] {
        e(a, b);
    }
    for (a, b) in [(op_nw, wc_out), (wc_out, wc_in), (wc_in, wc_n), (wc_n, wc_w)] {
        e(a, b);
    }
    for (a, b) in [(mt_out, mt_in), (mt_in, mt_w), (mt_w, mt_nw), (mt_nw, mt_n1), (mt_n1, mt_n2), (mt_in, mt_sw), (mt_sw, mt_s), (mt_s, mt_se), (mt_se, mt_e2), (mt_e2, mt_e1)] {
        e(a, b);
    }
    for (a, b) in [mt_n1, mt_n2, mt_w, mt_e1, mt_in, mt_e2, mt_s].into_iter().zip(chairs.iter().copied()) {
        e(a, b);
    }
    for (a, b) in [(lob_sw, op_nw), (lob_se, op_ne), (op_nw, grid[0][0]), (op_ne, grid[3][0])] {
        e(a, b);
    }
    for c in 0..xs.len() {
        for r in 0..ys.len() {
            if c + 1 < xs.len() {
                e(grid[c][r], grid[c + 1][r]);
            }
            if r + 1 < ys.len() {
                e(grid[c][r], grid[c][r + 1]);
            }
        }
    }
    for (i, &(entry, c, r)) in desk_entries.iter().enumerate() {
        e(entry, grid[c][r]);
        e(entry, grid[c + 1][r]);
        e(entry, desks[i]);
    }
    for (a, b) in [(grid[3][1], mgr_out), (grid[3][2], mgr_out), (mgr_out, mgr_in), (mgr_in, mgr_w), (mgr_w, mgr_nw), (mgr_nw, lead_desk), (mgr_in, mgr_sofa)] {
        e(a, b);
    }
    for (a, b) in [(grid[3][3], br_out), (br_out, br_in), (br_in, br_w), (br_w, br_n), (br_n, br_tr), (br_in, br_bl), (br_in, br_s), (br_s, br_br)] {
        e(a, b);
    }
    for (a, b) in [(grid[0][1], kit_in), (kit_in, kit_e), (kit_e, kit_se), (kit_se, kit_mid), (kit_mid, kit_nw), (kit_nw, kit_sw), (kit_sw, kit_s)] {
        e(a, b);
    }
    for (a, b) in [(kit_mid, stool_l), (kit_mid, stool_r), (kit_mid, din_tl), (kit_mid, din_tr), (kit_s, din_bl), (kit_s, din_br)] {
        e(a, b);
    }
    g.exit = exit;
    g.lead_desk = lead_desk;
    g.desks = desks;
    g.chairs = chairs;
    // A seat in each rest room in turn, so idle agents spread over them.
    let rest = [vec![tl_sofa, tl_arm_l, tl_arm_r], vec![stool_l, din_tr, stool_r, din_bl, din_tl, din_br], vec![br_tr, br_bl, br_br], vec![mgr_sofa]];
    g.seats = (0..6).flat_map(|i| rest.iter().filter_map(move |r| r.get(i).copied())).collect();
    // Round the reception desk and to the lounge's and meeting room's doors:
    // the shortest way between any two of them stays in the lobby.
    g.pet_stops = vec![lob_n, lob_nw, lob_ne, lob_sw, lob_s, lob_se, tl_out, mt_out];
    let places: Vec<usize> = g.places().collect();
    g.stops = (0..g.nodes.len()).filter(|&n| n != exit && places.iter().all(|&p| p != n && apart(g.nodes[n], g.nodes[p]))).collect();
    g
});

/// Whether characters standing at p and q are clear of each other: side by
/// side CHARACTER_HEIGHT·MAX_ASPECT apart, or above each other a character
/// height apart.
fn apart(p: Point, q: Point) -> bool {
    (p.x - q.x).abs() >= CHARACTER_HEIGHT * scene::MAX_ASPECT || (p.y - q.y).abs() >= CHARACTER_HEIGHT
}

impl Graph {
    /// Every place an agent can take: the manager's desk, the desks, the chairs, the seats.
    fn places(&self) -> impl Iterator<Item = usize> + '_ {
        [self.lead_desk].into_iter().chain(self.desks.iter().copied()).chain(self.chairs.iter().copied()).chain(self.seats.iter().copied())
    }

    fn dist(&self, a: usize, b: usize) -> f64 {
        let (p, q) = (self.nodes[a], self.nodes[b]);
        (p.x - q.x).hypot(p.y - q.y)
    }

    /// The shortest path from a to b along corridors, both included, and its length.
    fn path(&self, a: usize, b: usize) -> (Vec<usize>, f64) {
        let mut best = vec![f64::INFINITY; self.nodes.len()];
        let mut prev = vec![usize::MAX; self.nodes.len()];
        let mut done = vec![false; self.nodes.len()];
        best[a] = 0.0;
        while let Some(u) = (0..self.nodes.len()).filter(|&i| !done[i] && best[i].is_finite()).min_by(|&i, &j| best[i].total_cmp(&best[j])) {
            done[u] = true;
            if u == b {
                break;
            }
            for &v in &self.adj[u] {
                let d = best[u] + self.dist(u, v);
                if d < best[v] {
                    best[v] = d;
                    prev[v] = u;
                }
            }
        }
        let mut path = vec![b];
        let mut cur = b;
        while cur != a {
            cur = prev[cur];
            if cur == usize::MAX {
                return (vec![a], f64::INFINITY);
            }
            path.push(cur);
        }
        path.reverse();
        (path, best[b])
    }
}

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

/// Whether the session root is active at now: one of its agents is running
/// or active, or is not done and was active within ACTIVE_WINDOW.
pub fn session_active(snap: &Snapshot, root: &str, now: i64) -> bool {
    let mut on = false;
    snap.walk(root, 0, &mut |a, _| on |= at_desk(a.status) || (a.status != Status::Done && now - a.last_activity <= ACTIVE_WINDOW));
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

/// An actor on its way: where it is, the corridor nodes ahead of it (the
/// last is where it heads), and the node it came from.
#[derive(Clone, Debug)]
struct Mover {
    pos: Point,
    from: usize,
    path: VecDeque<usize>,
    left: bool,
    actor: Actor, // as last drawn
    gone: bool,   // left the scene, walking out
}

impl Mover {
    fn dest(&self) -> usize {
        self.path.back().copied().unwrap_or(self.from)
    }

    /// Heads for dest from where it is: on to the next node or back to the
    /// last, whichever is shorter, then along the corridors.
    fn head_for(&mut self, dest: usize) {
        let g = &*GRAPH;
        let d = |n: usize| (g.nodes[n].x - self.pos.x).hypot(g.nodes[n].y - self.pos.y);
        let via = |n: usize| {
            let (p, len) = g.path(n, dest);
            (p, len + d(n))
        };
        let (back, ahead) = (via(self.from), self.path.front().map(|&n| via(n)));
        let (p, _) = match ahead {
            Some(a) if a.1 < back.1 => a,
            _ => back,
        };
        self.path = p.into();
        if self.path.front() == Some(&self.from) && d(self.from) == 0.0 {
            self.path.pop_front();
        }
    }

    fn advance(&mut self, mut step: f64) {
        let g = &*GRAPH;
        while step > 0.0 {
            let Some(&n) = self.path.front() else { return };
            let q = g.nodes[n];
            let (dx, dy) = (q.x - self.pos.x, q.y - self.pos.y);
            if dx != 0.0 {
                self.left = dx < 0.0;
            }
            let len = dx.hypot(dy);
            if len <= step {
                (self.pos, self.from) = (q, n);
                self.path.pop_front();
                step -= len;
            } else {
                self.pos = Point { x: self.pos.x + dx / len * step, y: self.pos.y + dy / len * step };
                step = 0.0;
            }
        }
    }
}

/// A visitor on its round: where it is, and the stops ahead with the seconds
/// it waits at each; the last stop is the entrance, where it leaves.
#[derive(Clone, Debug)]
struct Visit {
    m: Mover,
    stops: VecDeque<(usize, f64)>,
    waited: f64,
    speed: f64,
}

impl Visit {
    /// In at the entrance, round 2 to 4 stops, and back out.
    fn new(actor: Actor, rng: &mut Rng) -> Visit {
        let g = &*GRAPH;
        let mut stops = VecDeque::new();
        let mut at = g.exit;
        for _ in 0..2 + rng.below(3) {
            let next: Vec<usize> = g.stops.iter().copied().filter(|&n| n != at).collect();
            at = next[rng.below(next.len())];
            stops.push_back((at, if rng.below(2) == 0 { rng.range(VISIT_WAIT) } else { 0.0 }));
        }
        stops.push_back((g.exit, 0.0));
        let mut m = Mover { pos: g.nodes[g.exit], from: g.exit, path: VecDeque::new(), left: false, actor, gone: false };
        m.head_for(stops[0].0);
        Visit { m, stops, waited: 0.0, speed: VISITOR_SPEED }
    }

    /// Whether it stands at a stop, waiting.
    fn waiting(&self) -> bool {
        self.m.path.is_empty() && self.stops.front().is_some_and(|&(_, wait)| wait > 0.0)
    }

    /// Walks or waits dt seconds of the round; false once out of the door.
    fn walk(&mut self, dt: f64) -> bool {
        loop {
            if !self.m.path.is_empty() {
                self.m.advance(self.speed * dt);
                return true;
            }
            let Some(&(_, wait)) = self.stops.front() else { return false };
            if self.waited < wait {
                self.waited += dt;
                return true;
            }
            self.stops.pop_front();
            self.waited = 0.0;
            let Some(&(next, _)) = self.stops.front() else { return false };
            self.m.head_for(next);
        }
    }
}

/// xorshift64: the visitors' dice, seeded so a run can be replayed.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng((seed ^ 0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// One of 0..n.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn range(&mut self, (lo, hi): (f64, f64)) -> f64 {
        lo + (hi - lo) * (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The office pet: a visitor that never leaves the lobby. It walks from one
/// of GRAPH.pet_stops to another, waiting PET_WAIT at each, for good: its
/// round is topped up whenever it runs short.
#[derive(Clone, Debug)]
struct Pet {
    v: Visit,
    rng: Rng,
}

impl Pet {
    /// Waiting at one of its stops, picked by seed.
    fn new(seed: u64) -> Pet {
        let g = &*GRAPH;
        let mut rng = Rng::new(seed);
        let at = g.pet_stops[rng.below(g.pet_stops.len())];
        let pos = g.nodes[at];
        let actor = Actor { id: PET.into(), kind: "pet".into(), status: Status::Idle, node: at, pos, anim: Anim::Idle, left: false, selected: false, lead: false, sprite: 0, visitor: false };
        let m = Mover { pos, from: at, path: VecDeque::new(), left: false, actor, gone: false };
        let stops = VecDeque::from([(at, rng.range(PET_WAIT))]);
        Pet { v: Visit { m, stops, waited: 0.0, speed: PET_SPEED }, rng }
    }

    /// Walks or waits dt seconds; where it stands then. Waiting where one of
    /// `others` stands, it walks on instead.
    fn walk(&mut self, dt: f64, mut others: impl Iterator<Item = Point>) -> Pose {
        let g = &*GRAPH;
        if let (true, Some(&(_, wait))) = (self.v.m.path.is_empty(), self.v.stops.front())
            && others.any(|p| !apart(p, self.v.m.pos))
        {
            self.v.waited = self.v.waited.max(wait);
        }
        while self.v.stops.len() < 2 {
            let last = self.v.stops.back().map_or(self.v.m.from, |&(n, _)| n);
            let next: Vec<usize> = g.pet_stops.iter().copied().filter(|&n| n != last).collect();
            let stop = (next[self.rng.below(next.len())], self.rng.range(PET_WAIT));
            self.v.stops.push_back(stop);
        }
        self.v.walk(dt);
        Pose { p: self.v.m.pos, left: self.v.m.left, moving: !self.v.m.path.is_empty() }
    }
}

/// Carries actors between the places successive layouts give them: an actor
/// whose place changed walks there along the corridors; a newcomer walks in
/// from the entrance; one that vanished walks out to it. Every VISIT_EVERY
/// seconds, while fewer than MAX_VISITORS are in, a guest of the scene comes
/// in for a round: the one with the fewest visits so far (residents with no
/// place before the rest), first in line among equals. The pet strolls on
/// all the while.
pub struct Walker {
    movers: HashMap<String, Mover>,
    visits: Vec<Visit>,
    seen: HashMap<String, u32>, // visits so far, by agent
    next: Option<f64>,          // when the next visitor may come in
    rng: Rng,
    pet: Pet, // its own dice, so the visitors' are as they were
    last: Option<f64>,
    primed: bool,
}

impl Walker {
    pub fn new(seed: u64) -> Walker {
        let pet = Pet::new(seed.rotate_left(32));
        Walker { movers: HashMap::new(), visits: Vec::new(), seen: HashMap::new(), next: None, rng: Rng::new(seed), pet, last: None, primed: false }
    }

    /// Forgets every resident, so the next step places them without walking.
    pub fn reset(&mut self) {
        (self.movers, self.last, self.primed) = (HashMap::new(), None, false);
    }

    /// Advances the walk to sec (seconds on any steady clock) and rewrites
    /// sc: actors on their way are drawn where they are, walking, the
    /// visitors join the actors and the roster, and the pet is where it is.
    pub fn step(&mut self, sc: &mut Scene, sec: f64) {
        let g = &*GRAPH;
        let dt = self.last.map_or(0.0, |l| (sec - l).clamp(0.0, 0.25));
        self.last = Some(sec);
        let step = WALK_SPEED * dt;
        let mut here = HashSet::new();
        for a in &mut sc.actors {
            here.insert(a.id.clone());
            // A visitor who became a resident walks on from where it is.
            if let Some(i) = self.visits.iter().position(|v| v.m.actor.id == a.id) {
                self.movers.insert(a.id.clone(), self.visits.remove(i).m);
            }
            let primed = self.primed;
            let m = self.movers.entry(a.id.clone()).or_insert_with(|| {
                let start = if primed { g.exit } else { a.node };
                Mover { pos: g.nodes[start], from: start, path: VecDeque::new(), left: false, actor: a.clone(), gone: false }
            });
            if m.gone || m.dest() != a.node {
                m.gone = false;
                m.head_for(a.node);
            }
            m.advance(step);
            if !m.path.is_empty() {
                (a.pos, a.anim, a.left) = (m.pos, Anim::Walk, m.left);
            }
            m.actor = a.clone();
        }
        let mut ghosts = Vec::new();
        self.movers.retain(|id, m| {
            if here.contains(id) {
                return true;
            }
            if !m.gone {
                m.gone = true;
                m.head_for(g.exit);
            }
            m.advance(step);
            if m.path.is_empty() {
                return false; // out of the door
            }
            ghosts.push(Actor { pos: m.pos, anim: Anim::Walk, left: m.left, selected: false, ..m.actor.clone() });
            true
        });
        ghosts.sort_by(|a, b| a.id.cmp(&b.id));
        sc.actors.extend(ghosts);

        self.visits.retain_mut(|v| v.walk(dt));
        let due = *self.next.get_or_insert(sec + self.rng.range(VISIT_EVERY));
        if sec >= due {
            self.next = Some(sec + self.rng.range(VISIT_EVERY));
            let busy = |id: &str| self.movers.contains_key(id) || self.visits.iter().any(|v| v.m.actor.id == id);
            let free = sc.guests.iter().enumerate().filter(|(_, a)| !busy(&a.id));
            let pick = free.min_by_key(|&(i, a)| (i >= sc.overflow, self.seen.get(&a.id).copied().unwrap_or(0), i));
            if let (true, Some((_, a))) = (self.visits.len() < MAX_VISITORS, pick) {
                *self.seen.entry(a.id.clone()).or_default() += 1;
                self.visits.push(Visit::new(a.clone(), &mut self.rng));
            }
        }
        for v in &self.visits {
            let anim = if v.waiting() { Anim::Idle } else { Anim::Walk };
            sc.roster.push(v.m.actor.id.clone());
            sc.actors.push(Actor { pos: v.m.pos, left: v.m.left, anim, ..v.m.actor.clone() });
        }
        sc.pet = Some(self.pet.walk(dt, sc.actors.iter().map(|a| a.pos)));
        self.primed = true;
    }
}

/// The colors of the edge kinds: yellow spawn, cyan message, green handback.
pub fn link_rgb(k: EdgeKind) -> [u8; 3] {
    match k {
        EdgeKind::Spawn => [0xf5, 0xd4, 0x42],
        EdgeKind::Message => [0x4f, 0xdc, 0xec],
        EdgeKind::Handback => [0x5f, 0xe0, 0x7a],
    }
}

const CHECK: [&str; 6] = ["......#", ".....##", "#...##.", "##.##..", ".###...", "..#...."];
const QUESTION: [&str; 7] = [".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#.."];
const ZED: [&str; 5] = ["#####", "...#.", "..#..", ".#...", "#####"];
const ARROW: [&str; 4] = ["#######", ".#####.", "..###..", "...#..."];
const BUBBLE: [&str; 6] = [".#########.", "#.........#", "#.1..2..3.#", "#.........#", ".####.####.", ".....##...."];
const LETTER: [&str; 6] = ["#########", "##.....##", "#.#...#.#", "#..#.#..#", "#...#...#", "#########"];

/// Something drawn on the picture, by what it looks like this frame, and the
/// canvas pixels it covers. Two frames' items with equal keys look the same.
pub struct Item {
    pub key: u64,
    pub rects: Vec<[u32; 4]>,
}

/// The pet's item key: no agent id or link looks like it.
pub const PET: &str = "pet:";

/// Draws frame i of the pet (pet: its sprite at its size on l) at ps, sec
/// into its walk. Its key changes only with the frame and where it is drawn.
pub fn paint_pet(canvas: &mut RgbaImage, l: &Layout, office: (u32, u32), pet: &Scaled, i: usize, ps: Pose, sec: f64) -> Item {
    let i = i.min(pet.frames.len() - 1);
    let rect = scene::draw_character(canvas, l, office, &pet.frames[i], ps, sec);
    let mut h = DefaultHasher::new();
    (rect, i).hash(&mut h);
    Item { key: h.finish(), rects: vec![rect] }
}

/// Draws the actors and links of sc onto canvas (the office base) at t,
/// returning one item per actor (keyed by its id) and two per link: its
/// dotted line, which stays still while its ends do, and the letter
/// travelling along it. `scaled` holds each sprite at this layout's size.
pub fn paint(canvas: &mut RgbaImage, l: &Layout, office: (u32, u32), sc: &Scene, scaled: &[Scaled], t: Duration, frames_of: impl Fn(usize, Duration) -> usize) -> HashMap<String, Item> {
    let mut items = HashMap::new();
    let ms = t.as_millis() as u64;
    let sec = t.as_secs_f64();
    let unit = l.unit();
    let ch = f64::from(l.character_height());
    // Characters a few pixels tall (half blocks) get one-pixel marks.
    let tiny = ch < 16.0;
    let mut order: Vec<&Actor> = sc.actors.iter().collect();
    order.sort_by(|a, b| a.pos.y.total_cmp(&b.pos.y));
    let mut heads = HashMap::new();
    let mut marks = Vec::new();
    for a in order {
        let sprite = &scaled[a.sprite.min(scaled.len() - 1)];
        let phase = Duration::from_millis(u64::from(fnv32a(&a.id) % 997));
        let (frame, lift) = match a.anim {
            Anim::Walk => (frames_of(a.sprite, t + phase), 0.0),
            Anim::Typing => (frames_of(a.sprite, t + phase), if (ms + phase.as_millis() as u64) / 180 % 2 == 1 { 0.04 } else { 0.0 }),
            Anim::Idle if a.status != Status::Done => (frames_of(a.sprite, t + phase), 0.0),
            Anim::Idle | Anim::Sleep => (0, 0.0),
        };
        let dim = a.status == Status::Done && a.anim != Anim::Walk && !a.visitor;
        let img = if dim { &sprite.dimmed } else { &sprite.frames[frame.min(sprite.frames.len() - 1)] };
        let mut p = a.pos;
        p.y -= lift * CHARACTER_HEIGHT;
        let ps = Pose { p, left: a.left, moving: a.anim == Anim::Walk };
        let rect = scene::draw_character(canvas, l, office, img, ps, sec);
        let bob = if ps.moving { (scene::walk_bob(sec) * f64::from(img.height())).round() as i64 } else { 0 };
        let (fx, fy) = l.px(p);
        let top = fy - f64::from(img.height()) - bob as f64;
        // Links end above the head where it stands, not where it bobs.
        let (hx, hy) = l.px(a.pos);
        heads.insert(a.id.as_str(), (hx, hy - ch));
        let mut h = DefaultHasher::new();
        (rect, frame, a.left, dim, bob, a.selected).hash(&mut h);
        marks.push((a, fx, top, h, rect));
    }
    for k in &sc.links {
        let (Some(&a), Some(&b)) = (heads.get(k.from.as_str()), heads.get(k.to.as_str())) else { continue };
        let rgb = link_rgb(k.kind);
        let id = format!("{}>{}>{}", k.from, k.to, k.kind.name());
        let rects = scene::dotted_line(canvas, a, b, unit.div_ceil(2), rgb, (20 * unit).max(16));
        let mut h = DefaultHasher::new();
        ((a.0 as i64, a.1 as i64, b.0 as i64, b.1 as i64), k.kind).hash(&mut h);
        items.insert(id.clone(), Item { key: h.finish(), rects });
        // A letter travels from sender to receiver every 1.6 s: its own
        // item, so only its cells are painted again as it moves; they hold
        // the line under it, drawn before it.
        let f = (ms % 1600) as f64 / 1600.0;
        let f = (f * 16.0).floor() / 16.0;
        let (x, y) = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
        let u = i64::from(unit);
        let at = ((x - 4.5 * u as f64) as i64, (y - 3.0 * u as f64) as i64);
        let letter: &[&str] = if tiny { &["#"] } else { &LETTER };
        let rect = scene::draw_art(canvas, letter, at.0, at.1, unit, |c| match c {
            '#' => Some(rgb),
            _ => Some([0xf4, 0xf1, 0xe8]),
        });
        let mut h = DefaultHasher::new();
        (rect, k.kind).hash(&mut h);
        items.insert(id + ">letter", Item { key: h.finish(), rects: vec![rect] });
    }
    for (a, fx, top, mut h, rect) in marks {
        let u = i64::from(unit);
        let mut rects = vec![rect];
        // A mark above the head, dx right of it and dy below its usual
        // place; a single `dot` pixel when characters are tiny.
        let mut mark = |art: &[&str], dx: i64, dy: i64, dot: [u8; 3], color: &dyn Fn(char) -> Option<[u8; 3]>| {
            rects.push(if tiny {
                let y = top as i64 - 1 - i64::from(dy < -2 * u);
                scene::draw_art(canvas, &["#"], fx as i64 + dx.signum(), y, 1, |_| Some(dot))
            } else {
                let w = art[0].len() as i64 * u;
                scene::draw_art(canvas, art, fx as i64 - w / 2 + dx, top as i64 - art.len() as i64 * u + dy, unit, color)
            });
        };
        let (dark, white) = ([0x2a, 0x2a, 0x33], [0xff, 0xff, 0xff]);
        let (blue, green, yellow, orange) = ([0xa8, 0xd8, 0xff], [0x5f, 0xe0, 0x7a], [0xff, 0xd8, 0x3a], [0xff, 0x8c, 0x1a]);
        match a.anim {
            Anim::Typing => {
                let dots = (ms / 300 % 4) as u32;
                dots.hash(&mut h);
                mark(&BUBBLE, (ch * 0.35) as i64, -u, white, &|c| match c {
                    '#' => Some(dark),
                    '1'..='3' if c.to_digit(10).unwrap_or(9) <= dots => Some(dark),
                    _ => Some(white),
                });
            }
            Anim::Sleep => {
                let rise = (ms / 400 % 6) as i64;
                rise.hash(&mut h);
                mark(&ZED, (ch * 0.3) as i64, -rise * u, blue, &|c| (c == '#').then_some(blue));
            }
            // Waiting for the user, walking to its desk or there.
            _ if a.status == Status::Waiting => mark(&QUESTION, 0, -u, orange, &|c| (c == '#').then_some(orange)),
            _ if a.status == Status::Done && a.anim != Anim::Walk && !a.visitor => mark(&CHECK, 0, -u, green, &|c| (c == '#').then_some(green)),
            _ => {}
        }
        if a.selected {
            let y = if a.anim == Anim::Typing || matches!(a.status, Status::Done | Status::Waiting) { -8 * u } else { -u };
            mark(&ARROW, 0, y, yellow, &|c| (c == '#').then_some(yellow));
        }
        items.insert(a.id.clone(), Item { key: h.finish(), rects });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Edge, Usage};

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
            turns: 0,
            usage: Usage::default(),
            model_usage: Default::default(),
            events: Vec::new(),
            children: Vec::new(),
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
    #[test]
    #[ignore]
    fn graph_picture() {
        let out = std::env::var("OFFICE_GRAPH_PNG").expect("OFFICE_GRAPH_PNG names the output");
        let mut img = image::open(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/office.png")).unwrap().to_rgba8();
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
