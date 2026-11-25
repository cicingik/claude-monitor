//! The corridor graph: the places, the stops and the corridors between them, and the shortest way along them.

use std::sync::OnceLock;

use super::layout;
use crate::scene::{self, Point};

/// The places and corridors of the office picture, from its layout (see layout).
#[derive(Debug)]
pub struct Graph {
    pub nodes: Vec<Point>, // fractions of the picture
    pub(super) adj: Vec<Vec<usize>>,
    pub exit: usize,
    pub lead_desk: usize,
    pub desks: Vec<usize>,     // the work places, filled in order
    pub chairs: Vec<usize>,    // more work places, once the desks are taken
    pub seats: Vec<usize>,     // rest seats, in the order they fill: one room after another
    pub stops: Vec<usize>,     // where visitors may wait: no place, clear of every place
    pub pet_stops: Vec<usize>, // the nodes the pet strolls between
    pub character_height: f64, // of the picture's height
}

static GRAPH: OnceLock<Graph> = OnceLock::new();

/// The office's walk graph: the one use_graph set at startup, else the built-in office's.
pub fn graph() -> &'static Graph {
    GRAPH.get_or_init(|| Graph::from_layout(&layout::parse(layout::BUILT_IN).expect("office.json parses")).expect("office.json is a usable layout"))
}

/// Makes g the office's walk graph. Call it before anything asks for graph().
pub fn use_graph(g: Graph) {
    assert!(GRAPH.set(g).is_ok(), "the walk graph is set once, before it is used");
}

impl Graph {
    /// Whether characters standing at p and q are clear of each other: side
    /// by side a character height·MAX_ASPECT apart, or above each other a
    /// character height apart.
    pub fn apart(&self, p: Point, q: Point) -> bool {
        let h = self.character_height;
        (p.x - q.x).abs() >= h * scene::MAX_ASPECT || (p.y - q.y).abs() >= h
    }

    /// Every place an agent can take: the manager's desk, the desks, the chairs, the seats.
    pub(super) fn places(&self) -> impl Iterator<Item = usize> + '_ {
        [self.lead_desk].into_iter().chain(self.desks.iter().copied()).chain(self.chairs.iter().copied()).chain(self.seats.iter().copied())
    }

    /// How many corridors join the nodes.
    pub fn corridors(&self) -> usize {
        self.adj.iter().map(Vec::len).sum::<usize>() / 2
    }

    fn dist(&self, a: usize, b: usize) -> f64 {
        let (p, q) = (self.nodes[a], self.nodes[b]);
        (p.x - q.x).hypot(p.y - q.y)
    }

    /// The shortest path from a to b along corridors, both included, and its length.
    pub(super) fn path(&self, a: usize, b: usize) -> (Vec<usize>, f64) {
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
