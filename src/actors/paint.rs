//! Drawing the scene onto the canvas: the characters, their marks, the links and the pet.

use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    time::Duration,
};

use image::RgbaImage;

use super::{Actor, Anim, Scene};
use crate::{
    cast::fnv32a,
    scan::{EdgeKind, Status},
    scene::{self, CHARACTER_HEIGHT, Layout, Pose, Scaled},
};

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
