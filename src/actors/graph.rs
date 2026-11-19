//! The corridor graph: the places, the stops and the corridors between them, and the shortest way along them.

use std::sync::LazyLock;

use crate::scene::{self, CHARACTER_HEIGHT, Point};

/// The places and corridors, built once from office.png measurements.
pub struct Graph {
    pub nodes: Vec<Point>, // fractions of the office
    pub(super) adj: Vec<Vec<usize>>,
    pub exit: usize,
    pub lead_desk: usize,
    pub desks: Vec<usize>,  // the open plan's desks, row by row
    pub chairs: Vec<usize>, // the meeting room's: work places once the desks are taken
    pub seats: Vec<usize>,  // rest seats, in the order they fill: one room after another
    pub stops: Vec<usize>,  // where visitors may wait: no place, clear of every place
    pub pet_stops: Vec<usize>, // the lobby's nodes the pet strolls between: not the entrance
}

/// The picture the measurements were taken on is 1254×1254.
pub(super) const SIZE: f64 = 1254.0;

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
pub(super) fn apart(p: Point, q: Point) -> bool {
    (p.x - q.x).abs() >= CHARACTER_HEIGHT * scene::MAX_ASPECT || (p.y - q.y).abs() >= CHARACTER_HEIGHT
}

impl Graph {
    /// Every place an agent can take: the manager's desk, the desks, the chairs, the seats.
    pub(super) fn places(&self) -> impl Iterator<Item = usize> + '_ {
        [self.lead_desk].into_iter().chain(self.desks.iter().copied()).chain(self.chairs.iter().copied()).chain(self.seats.iter().copied())
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
