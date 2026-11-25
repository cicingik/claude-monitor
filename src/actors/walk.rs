//! Movement over time: residents walking to their places, visitors on their rounds, and the pet.

use std::collections::{HashMap, HashSet, VecDeque};

use super::{Actor, Anim, graph, MAX_VISITORS, PET, PET_SPEED, PET_WAIT, Scene, VISIT_EVERY, VISIT_WAIT, VISITOR_SPEED};
use crate::{
    scan::Status,
    scene::{Point, Pose, WALK_SPEED},
};

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
        let g = graph();
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
        let g = graph();
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
        let g = graph();
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
/// of graph().pet_stops to another, waiting PET_WAIT at each, for good: its
/// round is topped up whenever it runs short.
#[derive(Clone, Debug)]
struct Pet {
    v: Visit,
    rng: Rng,
}

impl Pet {
    /// Waiting at one of its stops, picked by seed.
    fn new(seed: u64) -> Pet {
        let g = graph();
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
        let g = graph();
        if let (true, Some(&(_, wait))) = (self.v.m.path.is_empty(), self.v.stops.front())
            && others.any(|p| !graph().apart(p, self.v.m.pos))
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
        let g = graph();
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
