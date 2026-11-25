//! A layout file: the places and corridors of an office picture, by name, in
//! the picture's pixels. assets/office.json, built in, is the default
//! office's; `--office DIR` reads DIR/office.json beside DIR/office.png.
//!
//! ```text
//! {
//!   "size": [1254, 1254],          the pixels the coordinates are in
//!   "character_height": 0.07,      of the picture's height (optional)
//!   "nodes": [["exit", 630, 118], ["lobby", 630, 190], ...],
//!   "corridors": [["exit", "lobby"], ...],
//!   "exit": "exit",                where agents walk in and out
//!   "lead_desk": "boss",           the most recently active lead's desk
//!   "desks": ["desk_1", ...],      the other work places, filled in order
//!   "chairs": ["chair_1", ...],    more work places, once the desks are taken
//!   "rest": [["sofa", "arm"], ["stool"]],  rest rooms, each its seats in order
//!   "pet_stops": ["lobby", ...]    where the pet strolls
//! }
//! ```

use std::collections::{HashMap, VecDeque};

use image::RgbaImage;
use serde_json::Value;

use super::graph::Graph;
use crate::scene::{self, CHARACTER_HEIGHT, Point};

/// The built-in office's layout, for assets/office.png.
pub const BUILT_IN: &str = include_str!("../../assets/office.json");

/// A layout as its file holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub size: (f64, f64),
    pub character_height: f64,
    pub nodes: Vec<(String, f64, f64)>,
    pub corridors: Vec<(String, String)>,
    pub exit: String,
    pub lead_desk: String,
    pub desks: Vec<String>,
    pub chairs: Vec<String>,
    pub rest: Vec<Vec<String>>,
    pub pet_stops: Vec<String>,
}

/// Parses a layout file: what is wrong with it, by key, if anything.
pub fn parse(text: &str) -> Result<Layout, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let num = |v: &Value, what: &str| v.as_f64().ok_or(format!("{what} is not a number"));
    let name = |v: &Value, what: &str| v.as_str().map(str::to_string).ok_or(format!("{what} is not a name"));
    let list = |key: &str| v.get(key).and_then(Value::as_array).ok_or(format!("\"{key}\" is missing or not a list"));
    let names = |key: &str| list(key)?.iter().enumerate().map(|(i, n)| name(n, &format!("{key}[{i}]"))).collect::<Result<Vec<_>, _>>();
    let size = match list("size")?.as_slice() {
        [w, h] => (num(w, "size[0]")?, num(h, "size[1]")?),
        _ => return Err("\"size\" is not [width, height]".into()),
    };
    let nodes = list("nodes")?
        .iter()
        .enumerate()
        .map(|(i, n)| match n.as_array().map(Vec::as_slice) {
            Some([nm, x, y]) => Ok((name(nm, &format!("nodes[{i}][0]"))?, num(x, &format!("nodes[{i}][1]"))?, num(y, &format!("nodes[{i}][2]"))?)),
            _ => Err(format!("nodes[{i}] is not [name, x, y]")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let corridors = list("corridors")?
        .iter()
        .enumerate()
        .map(|(i, c)| match c.as_array().map(Vec::as_slice) {
            Some([a, b]) => Ok((name(a, &format!("corridors[{i}][0]"))?, name(b, &format!("corridors[{i}][1]"))?)),
            _ => Err(format!("corridors[{i}] is not [name, name]")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rest = list("rest")?
        .iter()
        .enumerate()
        .map(|(i, r)| r.as_array().ok_or(format!("rest[{i}] is not a list"))?.iter().enumerate().map(|(j, s)| name(s, &format!("rest[{i}][{j}]"))).collect())
        .collect::<Result<Vec<_>, _>>()?;
    let one = |key: &str| name(v.get(key).ok_or(format!("\"{key}\" is missing"))?, &format!("\"{key}\""));
    Ok(Layout {
        size,
        character_height: v.get("character_height").map_or(Ok(CHARACTER_HEIGHT), |h| num(h, "\"character_height\""))?,
        nodes,
        corridors,
        exit: one("exit")?,
        lead_desk: one("lead_desk")?,
        desks: names("desks")?,
        chairs: names("chairs")?,
        rest,
        pet_stops: names("pet_stops")?,
    })
}

impl Graph {
    /// The walk graph of a layout, or what makes it unusable: a node off
    /// the picture or named twice, a corridor or a role naming no node, no
    /// desk, or a place the entrance cannot reach.
    pub fn from_layout(l: &Layout) -> Result<Graph, String> {
        let (w, h) = l.size;
        if !(w > 0.0 && h > 0.0) {
            return Err(format!("\"size\" {w}×{h} is not positive"));
        }
        if !(l.character_height > 0.0 && l.character_height < 1.0) {
            return Err(format!("\"character_height\" {} is not between 0 and 1", l.character_height));
        }
        let mut at: HashMap<&str, usize> = HashMap::new();
        let mut nodes = Vec::new();
        for (i, (name, x, y)) in l.nodes.iter().enumerate() {
            if at.insert(name, i).is_some() {
                return Err(format!("node \"{name}\" is named twice"));
            }
            if !(0.0..=w).contains(x) || !(0.0..=h).contains(y) {
                return Err(format!("node \"{name}\" at {x},{y} is off the {w}×{h} picture"));
            }
            nodes.push(Point { x: x / w, y: y / h });
        }
        let id = |name: &str, role: &str| at.get(name).copied().ok_or(format!("{role} names \"{name}\", which is no node"));
        let mut adj = vec![Vec::new(); nodes.len()];
        for (a, b) in &l.corridors {
            let (a, b) = (id(a, "a corridor")?, id(b, "a corridor")?);
            adj[a].push(b);
            adj[b].push(a);
        }
        let ids = |names: &[String], role: &str| names.iter().map(|n| id(n, role)).collect::<Result<Vec<_>, _>>();
        let rest = l.rest.iter().map(|r| ids(r, "\"rest\"")).collect::<Result<Vec<_>, _>>()?;
        let longest = rest.iter().map(Vec::len).max().unwrap_or(0);
        let mut g = Graph {
            nodes,
            adj,
            exit: id(&l.exit, "\"exit\"")?,
            lead_desk: id(&l.lead_desk, "\"lead_desk\"")?,
            desks: ids(&l.desks, "\"desks\"")?,
            chairs: ids(&l.chairs, "\"chairs\"")?,
            // A seat in each rest room in turn, so idle agents spread over them.
            seats: (0..longest).flat_map(|i| rest.iter().filter_map(move |r| r.get(i).copied())).collect(),
            stops: Vec::new(),
            pet_stops: ids(&l.pet_stops, "\"pet_stops\"")?,
            character_height: l.character_height,
        };
        if g.desks.is_empty() {
            return Err("\"desks\" is empty: agents need somewhere to work".into());
        }
        let reached = reach(&g.adj, g.exit);
        let named = [g.lead_desk].into_iter().chain(g.desks.iter().chain(&g.chairs).chain(&g.seats).chain(&g.pet_stops).copied());
        if let Some(n) = named.into_iter().find(|&n| !reached[n]) {
            return Err(format!("no corridor leads from the exit to \"{}\"", l.nodes[n].0));
        }
        let places: Vec<usize> = g.places().collect();
        let stops = (0..g.nodes.len()).filter(|&n| n != g.exit && reached[n] && places.iter().all(|&p| p != n && g.apart(g.nodes[n], g.nodes[p]))).collect();
        g.stops = stops;
        Ok(g)
    }
}

/// Draws g over its picture img, for `office layout`: the corridors (red),
/// the places (desks blue, meeting chairs cyan, seats green, each seat
/// ticked white by the order it fills), the visitors' stops (magenta), the
/// entrance (yellow) and the pet's stops (orange rings).
pub fn preview(img: &mut RgbaImage, g: &Graph) {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    let px = |p: Point| (p.x * w, p.y * h);
    for (a, near) in g.adj.iter().enumerate() {
        for &b in near {
            scene::dotted_line(img, px(g.nodes[a]), px(g.nodes[b]), 3, [255, 40, 40], 64);
        }
    }
    let dot = |img: &mut RgbaImage, n: usize, rgb: [u8; 3]| {
        let (x, y) = px(g.nodes[n]);
        scene::draw_art(img, &["###", "###", "###"], x as i64 - 6, y as i64 - 6, 4, |_| Some(rgb));
    };
    for &d in g.desks.iter().chain([&g.lead_desk]) {
        dot(img, d, [60, 120, 255]);
    }
    for &c in &g.chairs {
        dot(img, c, [40, 230, 240]);
    }
    for (i, &s) in g.seats.iter().enumerate() {
        dot(img, s, [60, 220, 90]);
        let (x, y) = px(g.nodes[s]);
        scene::draw_art(img, &["#.".repeat(i + 1).as_str()], x as i64 - 6, y as i64 + 10, 3, |c| (c == '#').then_some([255, 255, 255]));
    }
    for &s in &g.stops {
        dot(img, s, [230, 60, 230]);
    }
    dot(img, g.exit, [255, 220, 0]);
    for &s in &g.pet_stops {
        let (x, y) = px(g.nodes[s]);
        scene::draw_art(img, &["#####", "#...#", "#...#", "#...#", "#####"], x as i64 - 10, y as i64 - 10, 4, |c| (c == '#').then_some([255, 140, 0]));
    }
}

/// Which nodes the corridors join to from.
fn reach(adj: &[Vec<usize>], from: usize) -> Vec<bool> {
    let mut seen = vec![false; adj.len()];
    let mut todo = VecDeque::from([from]);
    seen[from] = true;
    while let Some(n) = todo.pop_front() {
        for &m in &adj[n] {
            if !std::mem::replace(&mut seen[m], true) {
                todo.push_back(m);
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small office: a lobby at the door, two desks, a lounge with two
    /// seats and a kitchen with one; the boss's desk off the lobby.
    const SMALL: &str = r#"{
        "size": [400, 200],
        "nodes": [["door", 200, 10], ["lobby", 200, 60], ["boss", 380, 60], ["d1", 50, 150], ["d2", 350, 150],
                  ["sofa", 20, 60], ["arm", 60, 100], ["stool", 200, 190], ["hall", 200, 150]],
        "corridors": [["door", "lobby"], ["lobby", "boss"], ["lobby", "hall"], ["hall", "d1"], ["hall", "d2"],
                      ["lobby", "sofa"], ["sofa", "arm"], ["hall", "stool"]],
        "exit": "door", "lead_desk": "boss", "desks": ["d1", "d2"], "chairs": [],
        "rest": [["sofa", "arm"], ["stool"]], "pet_stops": ["lobby", "hall"]
    }"#;

    fn small() -> serde_json::Map<String, Value> {
        serde_json::from_str(SMALL).unwrap()
    }

    fn graph_of(v: &serde_json::Map<String, Value>) -> Result<Graph, String> {
        Graph::from_layout(&parse(&Value::Object(v.clone()).to_string())?)
    }

    #[test]
    fn a_layout_file_makes_its_walk_graph() {
        let g = graph_of(&small()).unwrap();
        assert_eq!(g.nodes[2], Point { x: 380.0 / 400.0, y: 60.0 / 200.0 }, "positions are fractions of the picture");
        assert_eq!((g.exit, g.lead_desk, g.desks.as_slice()), (0, 2, &[3, 4][..]));
        assert_eq!(g.seats, [5, 7, 6], "a seat of each rest room in turn");
        assert_eq!(g.character_height, CHARACTER_HEIGHT, "the default when the file names none");
        let (path, _) = g.path(0, 4);
        assert_eq!(path, [0, 1, 8, 4], "along the corridors");
        assert!(!g.stops.contains(&0) && g.stops.iter().all(|s| !g.places().any(|p| p == *s)), "visitors wait clear of the door and every place");
        assert!(graph_of(&parse_ok(BUILT_IN)).is_ok(), "the built-in office is a usable layout");
    }

    fn parse_ok(text: &str) -> serde_json::Map<String, Value> {
        serde_json::from_str(text).unwrap()
    }

    /// Each mistake in a layout says what is wrong, by name.
    #[test]
    fn a_broken_layout_says_what_is_wrong() {
        let broken = |change: &dyn Fn(&mut serde_json::Map<String, Value>)| {
            let mut v = small();
            change(&mut v);
            graph_of(&v).expect_err("a broken layout")
        };
        assert_eq!(broken(&|v| _ = v.remove("exit")), "\"exit\" is missing");
        assert_eq!(broken(&|v| v["corridors"].as_array_mut().unwrap().push(serde_json::json!(["hall", "attic"]))), "a corridor names \"attic\", which is no node");
        assert_eq!(broken(&|v| v["nodes"].as_array_mut().unwrap().push(serde_json::json!(["d1", 1, 1]))), "node \"d1\" is named twice");
        assert_eq!(broken(&|v| v["nodes"].as_array_mut().unwrap().push(serde_json::json!(["far", 500, 1]))), "node \"far\" at 500,1 is off the 400×200 picture");
        assert_eq!(broken(&|v| v["desks"] = serde_json::json!([])), "\"desks\" is empty: agents need somewhere to work");
        assert_eq!(broken(&|v| v["desks"] = serde_json::json!(["d1", "nowhere"])), "\"desks\" names \"nowhere\", which is no node");
        assert_eq!(broken(&|v| v["corridors"].as_array_mut().unwrap().retain(|c| c != &serde_json::json!(["hall", "d2"]))), "no corridor leads from the exit to \"d2\"");
        assert_eq!(broken(&|v| v["nodes"][0] = serde_json::json!(["door", 1])), "nodes[0] is not [name, x, y]");
        assert_eq!(broken(&|v| _ = v.insert("character_height".into(), serde_json::json!(2))),"\"character_height\" 2 is not between 0 and 1");
        assert!(parse("{").unwrap_err().starts_with("not JSON"));
    }
}
