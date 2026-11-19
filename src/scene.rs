//! The picture: where the office sits on the pane, the character sprites
//! scaled to it, and the drawing of characters, marks and dotted links at the
//! resolution the office is shown.

use std::{collections::VecDeque, error::Error, f64::consts::PI, fs::File, io::BufReader, path::Path, time::Duration};

use image::{
    AnimationDecoder, RgbaImage,
    codecs::gif::GifDecoder,
    imageops::{self, FilterType},
};
use ratatui::layout::Rect;

pub const CHARACTER_HEIGHT: f64 = 0.07; // of the office height
/// How tall the pet usually looks: its typical frame, not the box around
/// all of them (see Sprite::typical_height).
pub const PET_HEIGHT: f64 = 0.6 * CHARACTER_HEIGHT;
/// Widest a character may be, for its height: sprites wider than this are
/// narrowed, so places CHARACTER_HEIGHT·MAX_ASPECT apart never overlap.
pub const MAX_ASPECT: f64 = 0.9;
pub const WALK_SPEED: f64 = 0.12; // office sizes per second
const STEPS_PER_SECOND: f64 = 2.5; // walking bob
const STEP_BOB: f64 = 0.05; // of the character height
const DEFAULT_DELAY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Where a character is drawn: its feet as fractions of the office, the way
/// it faces, and whether it walks (and so bobs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub p: Point,
    pub left: bool, // facing left
    pub moving: bool,
}

/// Where the office is drawn. `area` is the cell rectangle it covers, centered
/// on the pane; `canvas` the pixel grid a protocol draws over `area`; `office`
/// the pixel rectangle (x, y, w, h) the scaled office fills in the canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Layout {
    pub area: Rect,
    pub canvas: (u32, u32),
    pub office: [u32; 4],
}

impl Layout {
    /// Canvas pixels of an office point.
    pub fn px(&self, p: Point) -> (f64, f64) {
        let [ox, oy, w, h] = self.office;
        (f64::from(ox) + p.x * f64::from(w), f64::from(oy) + p.y * f64::from(h))
    }

    /// Canvas pixels per pixel of a mark: 4 office pixels of a 1254 px office.
    pub fn unit(&self) -> u32 {
        ((f64::from(self.office[3]) / 320.0).round() as u32).max(1)
    }

    /// A character's height in canvas pixels.
    pub fn character_height(&self) -> u32 {
        self.height(CHARACTER_HEIGHT)
    }

    /// Canvas pixels of h, a fraction of the office height.
    fn height(&self, h: f64) -> u32 {
        ((h * f64::from(self.office[3])).round() as u32).max(1)
    }
}

/// Fits an image of `img` pixels on a pane of cols×rows cells of `font`
/// pixels: its original size when the pane has room, else shrunk with its
/// aspect ratio kept, never upscaled, centered. A pixel protocol draws it at
/// that size in the middle of the cells it touches. Half blocks show a cell as
/// two stacked pixels, so their canvas is the cell grid, filled by the office.
pub fn fit(img: (u32, u32), cols: u16, rows: u16, font: (u16, u16), halfblocks: bool) -> Layout {
    let (fw, fh) = (u32::from(font.0), u32::from(font.1));
    if cols == 0 || rows == 0 || fw == 0 || fh == 0 || img.0 == 0 || img.1 == 0 {
        return Layout::default();
    }
    let (pw, ph) = (u32::from(cols) * fw, u32::from(rows) * fh);
    let scale = (f64::from(pw) / f64::from(img.0)).min(f64::from(ph) / f64::from(img.1)).min(1.0);
    let w = ((f64::from(img.0) * scale).round() as u32).clamp(1, pw);
    let h = ((f64::from(img.1) * scale).round() as u32).clamp(1, ph);
    let (c, r) = (w.div_ceil(fw) as u16, h.div_ceil(fh) as u16);
    let area = Rect::new((cols - c) / 2, (rows - r) / 2, c, r);
    if halfblocks {
        let canvas = (u32::from(c), 2 * u32::from(r));
        return Layout { area, canvas, office: [0, 0, canvas.0, canvas.1] };
    }
    let canvas = (u32::from(c) * fw, u32::from(r) * fh);
    Layout { area, canvas, office: [(canvas.0 - w) / 2, (canvas.1 - h) / 2, w, h] }
}

/// The office scaled into its place on the canvas, transparent around it.
/// Done once per layout; every frame starts from a copy.
pub fn base(office: &RgbaImage, l: &Layout) -> RgbaImage {
    let [x, y, w, h] = l.office;
    let mut canvas = RgbaImage::new(l.canvas.0, l.canvas.1);
    imageops::replace(&mut canvas, &imageops::resize(office, w, h, FilterType::Lanczos3), x.into(), y.into());
    canvas
}

/// A character's animation, cropped to the pixels any frame uses. Each
/// frame shows for its delay.
pub struct Sprite {
    frames: Vec<(RgbaImage, Duration)>,
}

impl Sprite {
    /// Reads an animated GIF, or any still image as one frame. A background
    /// of one opaque color around the figure (as some GIFs have) is made
    /// transparent.
    pub fn load(path: &Path) -> Result<Sprite, Box<dyn Error>> {
        let mut frames = Vec::new();
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif")) {
            for f in GifDecoder::new(BufReader::new(File::open(path)?))?.into_frames() {
                let f = f?;
                let (n, d) = f.delay().numer_denom_ms();
                // GIF delays of 0 and 1 (×10 ms) mean "as fast as possible": use the default.
                let ms = f64::from(n) / f64::from(d.max(1));
                let delay = if ms > 10.0 { Duration::from_secs_f64(ms / 1000.0) } else { DEFAULT_DELAY };
                frames.push((f.into_buffer(), delay));
            }
        } else {
            frames.push((image::open(path)?.to_rgba8(), DEFAULT_DELAY));
        }
        Sprite::from_frames(frames)
    }

    pub fn from_frames(mut frames: Vec<(RgbaImage, Duration)>) -> Result<Sprite, Box<dyn Error>> {
        if frames.is_empty() {
            return Err("no frames".into());
        }
        for (f, _) in &mut frames {
            clear_background(f);
        }
        // Union of every frame's opaque pixels.
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (f, _) in &frames {
            for (x, y, p) in f.enumerate_pixels() {
                if p[3] > 0 {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                }
            }
        }
        if x1 == 0 {
            return Err("no opaque pixels".into());
        }
        for (f, _) in &mut frames {
            *f = imageops::crop_imm(f, x0, y0, x1 - x0, y1 - y0).to_image();
        }
        Ok(Sprite { frames })
    }

    /// The index of the frame showing t into the animation, looping forever.
    pub fn frame_index(&self, t: Duration) -> usize {
        let total: Duration = self.frames.iter().map(|(_, d)| *d).sum();
        let mut t = Duration::from_nanos((t.as_nanos() % total.as_nanos().max(1)) as u64);
        for (i, (_, d)) in self.frames.iter().enumerate() {
            if t < *d {
                return i;
            }
            t -= *d;
        }
        self.frames.len() - 1
    }

    /// How tall the figure usually looks, as a fraction of the sprite's
    /// height: the median of the frames' heights over their own opaque
    /// pixels. A sprite whose figure jumps or floats now and then is much
    /// taller than its figure at rest.
    pub fn typical_height(&self) -> f64 {
        let mut hs: Vec<u32> = self
            .frames
            .iter()
            .map(|(f, _)| {
                let rows: Vec<u32> = (0..f.height()).filter(|&y| (0..f.width()).any(|x| f.get_pixel(x, y)[3] > 0)).collect();
                rows.last().map_or(0, |&last| last + 1 - rows[0])
            })
            .collect();
        hs.sort_unstable();
        f64::from(hs[hs.len() / 2]) / f64::from(self.frames[0].0.height())
    }

    /// Every frame scaled to `height` (of the office) on layout l, and a
    /// dimmed copy of the first. Made once per layout.
    pub fn scaled(&self, l: &Layout, height: f64) -> Scaled {
        let (w, h) = size(l.height(height), self.frames[0].0.dimensions());
        let frames: Vec<RgbaImage> = self.frames.iter().map(|(f, _)| scale(f, w, h)).collect();
        let mut dimmed = frames[0].clone();
        for p in dimmed.pixels_mut() {
            for (i, c) in [0x6a, 0x78, 0x90].into_iter().enumerate() {
                p[i] = (f64::from(p[i]) * 0.65 + f64::from(c) * 0.35) as u8;
            }
        }
        Scaled { frames, dimmed }
    }
}

/// A sprite's frames at the size of one layout.
pub struct Scaled {
    pub frames: Vec<RgbaImage>,
    pub dimmed: RgbaImage,
}

/// Makes a uniform opaque background transparent: when the four corners
/// share one opaque color, every pixel of that color connected to the edge.
fn clear_background(f: &mut RgbaImage) {
    let (w, h) = f.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let bg = *f.get_pixel(0, 0);
    if bg[3] == 0 || [(w - 1, 0), (0, h - 1), (w - 1, h - 1)].iter().any(|&(x, y)| *f.get_pixel(x, y) != bg) {
        return;
    }
    let before = f.clone();
    let mut todo: VecDeque<(u32, u32)> = VecDeque::new();
    for x in 0..w {
        todo.extend([(x, 0), (x, h - 1)]);
    }
    for y in 0..h {
        todo.extend([(0, y), (w - 1, y)]);
    }
    while let Some((x, y)) = todo.pop_front() {
        if *f.get_pixel(x, y) != bg {
            continue;
        }
        f.put_pixel(x, y, [0, 0, 0, 0].into());
        if x > 0 {
            todo.push_back((x - 1, y));
        }
        if y > 0 {
            todo.push_back((x, y - 1));
        }
        if x + 1 < w {
            todo.push_back((x + 1, y));
        }
        if y + 1 < h {
            todo.push_back((x, y + 1));
        }
    }
    if f.pixels().all(|p| p[3] == 0) {
        *f = before; // all one color: that is the figure
    }
}

/// The canvas size of a sprite whose frames are `frame` pixels, h canvas
/// pixels tall: at most MAX_ASPECT as wide.
fn size(h: u32, frame: (u32, u32)) -> (u32, u32) {
    let aspect = (f64::from(frame.0) / f64::from(frame.1.max(1))).min(MAX_ASPECT);
    (((f64::from(h) * aspect).round() as u32).max(1), h)
}

/// frame scaled to w×h: each pixel is the mean of the frame pixels it
/// covers, premultiplied by alpha, so the character's pixels match the scaled
/// office's.
fn scale(frame: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (fw, fh) = frame.dimensions();
    RgbaImage::from_fn(w, h, |x, y| {
        let (v0, v1) = span(f64::from(y), 0.0, f64::from(h), fh);
        let (u0, u1) = span(f64::from(x), 0.0, f64::from(w), fw);
        let (mut sum, mut a, mut n) = ([0u32; 3], 0u32, 0u32);
        for v in v0..v1 {
            for u in u0..u1 {
                let p = frame.get_pixel(u, v);
                for i in 0..3 {
                    sum[i] += u32::from(p[i]) * u32::from(p[3]);
                }
                a += u32::from(p[3]);
                n += 1;
            }
        }
        if a == 0 {
            return [0, 0, 0, 0].into();
        }
        [(sum[0] / a) as u8, (sum[1] / a) as u8, (sum[2] / a) as u8, (a / n) as u8].into()
    })
}

/// How far a walking character's feet are lifted sec into its walk, as a
/// fraction of its height.
pub fn walk_bob(sec: f64) -> f64 {
    STEP_BOB * (sec * STEPS_PER_SECOND * PI).sin().abs()
}

/// Draws frame (at its size on l, from Sprite::scaled) into canvas with its
/// feet at the pose, over a soft shadow, bobbing as it walks. Returns the
/// canvas pixels it may have changed as [x0, y0, x1, y1].
pub fn draw_character(canvas: &mut RgbaImage, l: &Layout, _office: (u32, u32), frame: &RgbaImage, ps: Pose, sec: f64) -> [u32; 4] {
    let [ox, oy, w, h] = l.office;
    let (w, h) = (i64::from(w), i64::from(h));
    let (fw, fh) = frame.dimensions();
    let (fx, fy) = (ps.p.x * w as f64, ps.p.y * h as f64);

    // Shadow: an ellipse under the feet, darkest at its centre.
    let (rx, ry) = (0.42 * f64::from(fw), 0.42 * f64::from(fw) * 0.35);
    for y in (fy - ry) as i64..=(fy + ry) as i64 {
        for x in (fx - rx) as i64..=(fx + rx) as i64 {
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let (dx, dy) = ((x as f64 + 0.5 - fx) / rx, (y as f64 + 0.5 - fy) / ry);
            let d = dx * dx + dy * dy;
            if d < 1.0 {
                let k = 1.0 - 0.45 * (1.0 - d);
                let c = canvas.get_pixel_mut(ox + x as u32, oy + y as u32);
                for i in 0..3 {
                    c[i] = (f64::from(c[i]) * k) as u8;
                }
            }
        }
    }

    let lift = if ps.moving { walk_bob(sec) * f64::from(fh) } else { 0.0 };
    let x0 = (fx - f64::from(fw) / 2.0).round() as i64;
    let y0 = (fy - lift - f64::from(fh)).round() as i64;
    let clamp = |v: i64, hi: i64| v.clamp(0, hi) as u32;
    let touched = [
        ox + clamp(x0.min((fx - rx).floor() as i64), w),
        oy + clamp(y0, h),
        ox + clamp((x0 + i64::from(fw)).max((fx + rx + 1.0).ceil() as i64), w),
        oy + clamp((fy + ry + 1.0).ceil() as i64, h),
    ];
    for v in 0..fh {
        let y = y0 + i64::from(v);
        if y < 0 || y >= h {
            continue;
        }
        for u in 0..fw {
            let x = x0 + i64::from(u);
            if x < 0 || x >= w {
                continue;
            }
            let p = frame.get_pixel(if ps.left { fw - 1 - u } else { u }, v);
            blend(canvas.get_pixel_mut(ox + x as u32, oy + y as u32), [p[0], p[1], p[2]], p[3]);
        }
    }
    touched
}

fn blend(c: &mut image::Rgba<u8>, rgb: [u8; 3], a: u8) {
    let (a, inv) = (u32::from(a), 255 - u32::from(a));
    for i in 0..3 {
        c[i] = ((u32::from(rgb[i]) * a + u32::from(c[i]) * inv) / 255) as u8;
    }
}

/// Maps canvas pixel i, inside an item starting at o and size long, to the
/// range of the item's n source pixels it covers (at least one).
fn span(i: f64, o: f64, size: f64, n: u32) -> (u32, u32) {
    let n = i64::from(n);
    let lo = (((i - o) / size * n as f64).floor() as i64).clamp(0, n - 1);
    let hi = (((i + 1.0 - o) / size * n as f64).ceil() as i64).clamp(0, n);
    (lo as u32, hi.max(lo + 1) as u32)
}

/// Draws a small pixel-art picture: each character of `art` is a square of
/// unit canvas pixels in the color `color` gives it (None: transparent), its
/// top-left at (x, y). Returns the canvas pixels it covers.
pub fn draw_art(canvas: &mut RgbaImage, art: &[&str], x: i64, y: i64, unit: u32, color: impl Fn(char) -> Option<[u8; 3]>) -> [u32; 4] {
    let (cw, chh) = (i64::from(canvas.width()), i64::from(canvas.height()));
    let u = i64::from(unit);
    for (r, row) in art.iter().enumerate() {
        for (c, ch) in row.chars().enumerate() {
            let Some(rgb) = color(ch) else { continue };
            for dy in 0..u {
                for dx in 0..u {
                    let (px, py) = (x + c as i64 * u + dx, y + r as i64 * u + dy);
                    if (0..cw).contains(&px) && (0..chh).contains(&py) {
                        canvas.put_pixel(px as u32, py as u32, [rgb[0], rgb[1], rgb[2], 255].into());
                    }
                }
            }
        }
    }
    let wide = art.iter().map(|r| r.chars().count()).max().unwrap_or(0) as i64;
    let clamp = |v: i64, hi: i64| v.clamp(0, hi) as u32;
    [clamp(x, cw), clamp(y, chh), clamp(x + wide * u, cw), clamp(y + art.len() as i64 * u, chh)]
}

/// Draws a dotted line from a to b, unit pixels thick. Returns the pixels it
/// covers, in pieces of at most `piece` pixels along the line, so a long
/// diagonal is not one big box.
pub fn dotted_line(canvas: &mut RgbaImage, a: (f64, f64), b: (f64, f64), unit: u32, rgb: [u8; 3], piece: u32) -> Vec<[u32; 4]> {
    let (cw, chh) = (canvas.width() as i64, canvas.height() as i64);
    let n = (b.0 - a.0).abs().max((b.1 - a.1).abs()).round() as i64;
    let (u, dash) = (i64::from(unit), 2 * i64::from(unit));
    let mut rects = Vec::new();
    let mut cur: Option<[i64; 4]> = None;
    for i in 0..=n {
        let k = if n == 0 { 0.0 } else { i as f64 / n as f64 };
        let (x, y) = ((a.0 + (b.0 - a.0) * k).round() as i64, (a.1 + (b.1 - a.1) * k).round() as i64);
        let (x0, y0) = (x - u / 2, y - u / 2);
        let r = cur.get_or_insert([x0, y0, x0 + u, y0 + u]);
        *r = [r[0].min(x0), r[1].min(y0), r[2].max(x0 + u), r[3].max(y0 + u)];
        if i % (2 * dash) < dash {
            for py in y0.max(0)..(y0 + u).min(chh) {
                for px in x0.max(0)..(x0 + u).min(cw) {
                    canvas.put_pixel(px as u32, py as u32, [rgb[0], rgb[1], rgb[2], 255].into());
                }
            }
        }
        if i % i64::from(piece.max(1)) == 0 || i == n {
            rects.extend(cur.take());
        }
    }
    let clamp = |v: i64, hi: i64| v.clamp(0, hi) as u32;
    rects.into_iter().map(|r| [clamp(r[0], cw), clamp(r[1], chh), clamp(r[2], cw), clamp(r[3], chh)]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FONT: (u16, u16) = (10, 20);

    #[test]
    fn fit_shows_original_size_when_there_is_room() {
        // 300×200 px at 10×20 px cells is 30×10 cells; the pane is 100×50 cells.
        let l = fit((300, 200), 100, 50, FONT, false);
        assert_eq!(l.area, Rect::new(35, 20, 30, 10));
        assert_eq!(l.canvas, (300, 200));
        assert_eq!(l.office, [0, 0, 300, 200]);

        // Not a whole number of cells: drawn at its own size in the middle of the cells it touches.
        let l = fit((305, 195), 100, 50, FONT, false);
        assert_eq!(l.area, Rect::new(34, 20, 31, 10));
        assert_eq!(l.office, [2, 2, 305, 195]);
    }

    #[test]
    fn fit_never_upscales() {
        for (cols, rows) in [(126, 63), (200, 100), (1000, 1000)] {
            let l = fit((1254, 1254), cols, rows, FONT, false);
            assert_eq!(l.office[2..], [1254, 1254], "pane {cols}×{rows}");
        }
    }

    #[test]
    fn fit_shrinks_keeping_the_aspect_ratio() {
        // A square office on a 120×40 pane of 10×20 cells (1200×800 px): limited by height.
        let l = fit((1254, 1254), 120, 40, FONT, false);
        assert_eq!(l.office[2..], [800, 800]);
        assert_eq!(l.area, Rect::new(20, 0, 80, 40));
        // Wide image limited by width.
        let l = fit((2000, 1000), 50, 100, FONT, false);
        assert_eq!(l.office[2..], [500, 250]);
        let [_, _, w, h] = fit((1254, 837), 37, 13, FONT, false).office;
        let (want, got) = (1254.0 / 837.0, f64::from(w) / f64::from(h));
        assert!((got - want).abs() < 0.01, "aspect {got}, want {want}");
    }

    #[test]
    fn fit_centers_on_the_pane() {
        for (cols, rows) in [(120, 40), (60, 60), (200, 70), (81, 33)] {
            let a = fit((1254, 1254), cols, rows, FONT, false).area;
            let (left, right) = (a.x, cols - a.right());
            let (top, bottom) = (a.y, rows - a.bottom());
            assert!(left.abs_diff(right) <= 1 && top.abs_diff(bottom) <= 1, "pane {cols}×{rows}: {a:?}");
        }
    }

    #[test]
    fn fit_for_halfblocks_is_two_pixels_per_cell() {
        let l = fit((1254, 1254), 120, 40, FONT, true);
        assert_eq!(l.area, Rect::new(20, 0, 80, 40));
        assert_eq!(l.canvas, (80, 80));
        assert_eq!(l.office, [0, 0, 80, 80]);
    }

    #[test]
    fn character_stands_on_its_feet_and_faces_its_way() {
        // A 2×2 frame, red on the left and blue on the right, in a 100×100
        // office drawn 1:1, so it stands 7 pixels tall and 7 wide above its feet.
        let frame = RgbaImage::from_fn(2, 2, |x, _| if x == 0 { [255, 0, 0, 255].into() } else { [0, 0, 255, 255].into() });
        let l = Layout { area: Rect::default(), canvas: (100, 100), office: [0, 0, 100, 100] };
        let frame = Sprite::from_frames(vec![(frame, DEFAULT_DELAY)]).unwrap().scaled(&l, CHARACTER_HEIGHT).frames.remove(0);
        for left in [false, true] {
            let mut img = RgbaImage::new(100, 100);
            let ps = Pose { p: Point { x: 0.5, y: 0.5 }, left, moving: false };
            draw_character(&mut img, &l, (100, 100), &frame, ps, 0.0);
            let (l, r, above) = (img.get_pixel(47, 47).0, img.get_pixel(52, 47).0, img.get_pixel(50, 40).0);
            let (mut want_l, mut want_r) = ([255, 0, 0], [0, 0, 255]);
            if left {
                (want_l, want_r) = (want_r, want_l);
            }
            assert_eq!((&l[..3], &r[..3]), (&want_l[..], &want_r[..]), "left={left}");
            assert_eq!(above, [0, 0, 0, 0], "left={left}: drawn above its height");
        }
    }

    #[test]
    fn a_uniform_background_is_cleared_but_not_the_figure_inside() {
        // White around a black ring with white inside it.
        let mut f = RgbaImage::from_pixel(7, 7, [255, 255, 255, 255].into());
        for i in 1..6 {
            for (x, y) in [(i, 1), (i, 5), (1, i), (5, i)] {
                f.put_pixel(x, y, [0, 0, 0, 255].into());
            }
        }
        let s = Sprite::from_frames(vec![(f, DEFAULT_DELAY)]).unwrap();
        let f = &s.frames[0].0;
        assert_eq!(f.dimensions(), (5, 5), "cropped to the ring");
        assert_eq!(f.get_pixel(2, 2).0, [255, 255, 255, 255], "inside the ring stays");
    }

    #[test]
    fn scaled_frames_have_the_character_height_and_draw_unscaled() {
        let frame = RgbaImage::from_pixel(40, 80, [9, 9, 9, 255].into());
        let s = Sprite::from_frames(vec![(frame, DEFAULT_DELAY)]).unwrap();
        let l = Layout { area: Rect::default(), canvas: (1000, 1000), office: [0, 0, 1000, 1000] };
        let sc = s.scaled(&l, CHARACTER_HEIGHT);
        assert_eq!(sc.frames[0].dimensions(), (35, 70));
        let mut img = RgbaImage::new(1000, 1000);
        let t = draw_character(&mut img, &l, (1000, 1000), &sc.frames[0], Pose { p: Point { x: 0.5, y: 0.5 }, left: false, moving: false }, 0.0);
        assert_eq!((t[1], t[3] >= 500), (430, true));
        assert_eq!(img.get_pixel(500, 470).0[..3], [9, 9, 9]);
    }

    /// A figure 10 px tall that jumps to 40 px in one frame of five: it
    /// usually looks 10 px tall, a quarter of the sprite.
    #[test]
    fn typical_height_ignores_a_rare_jump() {
        let frame = |top: u32| RgbaImage::from_fn(8, 40, |_, y| if y >= top { [1, 1, 1, 255].into() } else { [0, 0, 0, 0].into() });
        let s = Sprite::from_frames([30, 30, 0, 30, 30].map(|t| (frame(t), DEFAULT_DELAY)).into()).unwrap();
        assert_eq!(s.typical_height(), 0.25);
        let l = Layout { area: Rect::default(), canvas: (1000, 1000), office: [0, 0, 1000, 1000] };
        assert_eq!(s.scaled(&l, 0.05 / s.typical_height()).frames[0].height(), 200, "its figure at rest is 0.05 of the office");
    }
}
