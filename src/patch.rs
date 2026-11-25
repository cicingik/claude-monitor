//! Incremental painting with iTerm2 images or sixels: the cells that changed,
//! as non-overlapping patches, sent as one frame.

use std::{
    collections::HashMap,
    io::{self, Write},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image::{
    ExtendedColorType, ImageEncoder, RgbaImage,
    codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    buffer::{Buffer, CellDiffOption},
    layout::Rect,
};

use ratatui_image::picker::ProtocolType;

use crate::{actors::Item, scene::Layout, terminal::Display};

/// The cells to repaint: those of every actor and link that appeared,
/// vanished or looks different, where it was and where it is, merged.
pub(crate) fn changed(was: &HashMap<String, Item>, now: &HashMap<String, Item>, font: (u16, u16)) -> Vec<CellRect> {
    let mut rects: Vec<&[u32; 4]> = Vec::new();
    for (id, it) in now {
        match was.get(id) {
            Some(old) if old.key == it.key => {}
            old => rects.extend(it.rects.iter().chain(old.into_iter().flat_map(|o| &o.rects))),
        }
    }
    for (id, old) in was {
        if !now.contains_key(id) {
            rects.extend(&old.rects);
        }
    }
    merge(rects.into_iter().map(|r| CellRect::covering(*r, font)).filter(|r| !r.is_empty()).collect())
}

/// Makes the patches of one frame, which never overlap: each patch erases
/// its cells before it is drawn, so a later one would blank part of an
/// earlier one until its own image lands, a flicker. Two rectangles whose
/// union is no bigger than the two apart are joined (an actor where it was
/// and where it is becomes one patch); of two others that overlap, the
/// second loses the shared cells. So the pieces of a long diagonal link,
/// which share a corner cell, stay small instead of growing into the box
/// around the whole link.
fn merge(mut rects: Vec<CellRect>) -> Vec<CellRect> {
    let mut i = 0;
    while i < rects.len() {
        let a = rects[i];
        match (i + 1..rects.len()).find(|&j| a.overlaps(rects[j]) || a.union(rects[j]).area() <= a.area() + rects[j].area()) {
            Some(j) => {
                let b = rects.swap_remove(j);
                if a.union(b).area() <= a.area() + b.area() {
                    rects[i] = a.union(b);
                } else {
                    rects.extend(b.minus(a));
                }
                i = 0;
            }
            None => i += 1,
        }
    }
    rects
}

/// An iTerm2 frame after a full repaint, with the card that was on the
/// screen (`old`, screen cells) and the card to show (`new`): the picture
/// patches (relative to area, never overlapping) and the screen cells to
/// blank. The cells old leaves are painted again (blanked off the picture),
/// and no patch touches new's cells, which hold its text.
pub(crate) fn plan(mut patches: Vec<CellRect>, area: Rect, old: Option<Rect>, new: Option<Rect>) -> (Vec<CellRect>, Vec<CellRect>) {
    let pic = CellRect::of(area);
    let new = new.map(CellRect::of);
    let relative = |r: CellRect| CellRect { x0: r.x0 - pic.x0, y0: r.y0 - pic.y0, x1: r.x1 - pic.x0, y1: r.y1 - pic.y0 };
    let mut blanks = Vec::new();
    if let Some(old) = old.map(CellRect::of).filter(|&o| Some(o) != new) {
        for r in new.map_or_else(|| vec![old], |n| old.minus(n)) {
            patches.extend(Some(r.intersect(pic)).filter(|i| !i.is_empty()).map(relative));
            blanks.extend(r.minus(pic));
        }
        patches = merge(patches);
    }
    if let Some(n) = new.map(|n| n.intersect(pic)).filter(|n| !n.is_empty()).map(relative) {
        patches = patches.into_iter().flat_map(|p| p.minus(n)).collect();
    }
    (patches, blanks)
}

/// Blanks the screen cells r, which hold no picture.
fn blank(out: &mut impl Write, r: CellRect) -> io::Result<()> {
    let w = r.x1 - r.x0;
    let seq: String = (r.y0..r.y1).map(|y| format!("\x1b[{};{}H\x1b[{w}X", y + 1, r.x0 + 1)).collect();
    out.write_all(seq.as_bytes())
}

/// Writes every cell of the card where it stands on the screen, then
/// resets the colors.
pub(crate) fn write_card<W: Write>(out: &mut CrosstermBackend<W>, card: &Buffer) -> io::Result<()> {
    let mut every = card.clone();
    for c in &mut every.content {
        c.set_diff_option(CellDiffOption::AlwaysUpdate);
    }
    out.draw(card.diff(&every).into_iter())?;
    Backend::flush(out)
}

/// A rectangle of cells, relative to the layout's area unless said: columns x0..x1, rows y0..y1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CellRect {
    pub(crate) x0: u16,
    pub(crate) y0: u16,
    pub(crate) x1: u16,
    pub(crate) y1: u16,
}

impl CellRect {
    pub(crate) fn all(area: Rect) -> CellRect {
        CellRect { x0: 0, y0: 0, x1: area.width, y1: area.height }
    }

    /// The cells of r, on the screen.
    fn of(r: Rect) -> CellRect {
        CellRect { x0: r.x, y0: r.y, x1: r.right(), y1: r.bottom() }
    }

    fn intersect(self, o: CellRect) -> CellRect {
        CellRect { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) }
    }

    /// The cells holding canvas pixels [x0, y0, x1, y1], for cells of font pixels.
    pub(crate) fn covering(px: [u32; 4], font: (u16, u16)) -> CellRect {
        let (fw, fh) = (u32::from(font.0), u32::from(font.1));
        let cell = |v: u32, f: u32| u16::try_from(v / f).unwrap_or(u16::MAX);
        let cell_up = |v: u32, f: u32| u16::try_from(v.div_ceil(f)).unwrap_or(u16::MAX);
        CellRect { x0: cell(px[0], fw), y0: cell(px[1], fh), x1: cell_up(px[2], fw), y1: cell_up(px[3], fh) }
    }

    fn is_empty(self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    fn area(self) -> u32 {
        if self.is_empty() { 0 } else { u32::from(self.x1 - self.x0) * u32::from(self.y1 - self.y0) }
    }

    fn overlaps(self, o: CellRect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    /// The cells of self outside o, as up to four rectangles: the rows
    /// above and below o, and left and right of it in the rows between.
    fn minus(self, o: CellRect) -> Vec<CellRect> {
        if !self.overlaps(o) {
            return vec![self];
        }
        let (y0, y1) = (self.y0.max(o.y0), self.y1.min(o.y1));
        let parts = [CellRect { y1: o.y0, ..self }, CellRect { y0: o.y1, ..self }, CellRect { x1: o.x0, y0, y1, ..self }, CellRect { x0: o.x1, y0, y1, ..self }];
        parts.into_iter().filter(|r| !r.is_empty()).collect()
    }

    fn union(self, o: CellRect) -> CellRect {
        match (self.is_empty(), o.is_empty()) {
            (true, _) => o,
            (_, true) => self,
            _ => CellRect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) },
        }
    }
}

/// Past this many iTerm2 images with a piece still on the screen, the next
/// frame paints the whole picture again, one image. Every patch is an image
/// of its own, and the terminal keeps each whole while a piece of it shows:
/// a busy office left alone becomes a mosaic of some 750 images, 9 Mpx in
/// all against 1.6 for the picture. WezTerm keeps 256 images decoded per
/// window and decodes any other one again, one at a time, whenever it draws
/// it: when its tab comes back, and each time its texture atlas fills up.
pub(crate) const MAX_PIECES: usize = 256;
/// At most one such repaint in this time: while many walk at once, the
/// pieces pass MAX_PIECES again within a second or two, and the whole
/// picture is some 4 MB against half a megabyte for a frame of patches.
pub(crate) const WHOLE_EVERY: Duration = Duration::from_secs(10);

/// Which iTerm2 image, numbered as sent, each cell of the picture shows.
#[derive(Default)]
pub(crate) struct Pieces {
    width: u16,
    cells: Vec<u32>,
    sent: u32,
}

impl Pieces {
    /// The picture's cells hold nothing yet.
    pub(crate) fn new(area: Rect) -> Pieces {
        Pieces { width: area.width, cells: vec![0; usize::from(area.width) * usize::from(area.height)], sent: 0 }
    }

    /// The cells p (relative to the picture) now show one new image.
    pub(crate) fn paint(&mut self, p: CellRect) {
        self.sent = self.sent.wrapping_add(1).max(1);
        let w = usize::from(self.width);
        for y in usize::from(p.y0)..usize::from(p.y1) {
            let row = self.cells.get_mut(y * w..(y + 1) * w).unwrap_or_default();
            let (x0, x1) = (usize::from(p.x0).min(row.len()), usize::from(p.x1).min(row.len()));
            row[x0..x1].fill(self.sent);
        }
    }

    /// How many images have a piece on the screen.
    pub(crate) fn live(&self) -> usize {
        let mut ids: Vec<u32> = self.cells.iter().copied().filter(|&i| i > 0).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }
}

/// Draws the cells `patch` of canvas img with the iTerm2 protocol: the cells
/// are erased (dropping the image pieces they held), then one PNG of exactly
/// those cells is placed over them, sized in cells. With `clear` the screen is
/// cleared first, in the same write, as on a full repaint.
fn paint_iterm2(out: &mut impl Write, img: &RgbaImage, l: &Layout, patch: CellRect, d: &Display, clear: bool) -> io::Result<()> {
    let p = CellRect { x1: patch.x1.min(l.area.width), y1: patch.y1.min(l.area.height), ..patch };
    if p.is_empty() {
        return Ok(());
    }
    let (fw, fh) = (u32::from(d.font.0), u32::from(d.font.1));
    let (cols, rows) = (p.x1 - p.x0, p.y1 - p.y0);
    let crop = image::imageops::crop_imm(img, u32::from(p.x0) * fw, u32::from(p.y0) * fh, u32::from(cols) * fw, u32::from(rows) * fh);
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, PngFilter::Adaptive)
        .write_image(&crop.to_image(), u32::from(cols) * fw, u32::from(rows) * fh, ExtendedColorType::Rgba8)
        .map_err(io::Error::other)?;
    let (left, top) = (l.area.x + p.x0 + 1, l.area.y + p.y0 + 1); // 1-based
    let mut seq = String::new();
    if clear {
        seq.push_str("\x1b[2J");
    }
    for r in 0..rows {
        seq.push_str(&format!("\x1b[{};{left}H\x1b[{cols}X", top + r));
    }
    seq.push_str(&format!(
        "\x1b[{top};{left}H\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=0;doNotMoveCursor=1:{}\x07",
        png.len(),
        BASE64.encode(&png)
    ));
    out.write_all(d.wrap(seq).as_bytes())
}

/// Draws the cells `patch` of canvas img as one sixel at their top left. A
/// sixel is opaque, so it covers what the cells showed with no erase first:
/// an erase would show between it and the image. With `clear` the screen is
/// cleared first, in the same write, as on a full repaint.
fn paint_sixel(out: &mut impl Write, img: &RgbaImage, l: &Layout, patch: CellRect, d: &Display, clear: bool) -> io::Result<()> {
    let p = CellRect { x1: patch.x1.min(l.area.width), y1: patch.y1.min(l.area.height), ..patch };
    if p.is_empty() {
        return Ok(());
    }
    let (fw, fh) = (u32::from(d.font.0), u32::from(d.font.1));
    let (w, h) = (u32::from(p.x1 - p.x0) * fw, u32::from(p.y1 - p.y0) * fh);
    let crop = image::imageops::crop_imm(img, u32::from(p.x0) * fw, u32::from(p.y0) * fh, w, h).to_image();
    let sixel = icy_sixel::sixel_encode(crop.as_raw(), w as usize, h as usize, &icy_sixel::EncodeOptions::default()).map_err(|e| io::Error::other(e.to_string()))?;
    let clear = if clear { "\x1b[2J" } else { "" };
    let seq = format!("{clear}\x1b[{};{}H{sixel}", l.area.y + p.y0 + 1, l.area.x + p.x0 + 1);
    out.write_all(d.wrap(seq).as_bytes())
}

/// Sends one frame of the office, iTerm2 images or sixels, in a single write: the blanks, the
/// patches of img (with `clear`, a full repaint led by clearing the screen),
/// then the card if it changed, between the begin and end of a synchronized
/// update (DECSET 2026). One write is not enough on its own: the terminal
/// reads a frame of hundreds of kilobytes in pieces and may show the screen
/// between two, with the cells erased for a patch before its image lands, a
/// flicker that grows with every actor on the move.
#[allow(clippy::too_many_arguments)]
pub(crate) fn frame_patches(out: &mut impl Write, img: &RgbaImage, l: &Layout, patches: &[CellRect], blanks: &[CellRect], card: Option<&Buffer>, d: &Display, clear: bool) -> io::Result<()> {
    const BEGIN: &[u8] = b"\x1b[?2026h";
    let mut frame = BEGIN.to_vec();
    for &b in blanks {
        blank(&mut frame, b)?;
    }
    for &p in patches {
        match d.protocol {
            ProtocolType::Sixel => paint_sixel(&mut frame, img, l, p, d, clear)?,
            _ => paint_iterm2(&mut frame, img, l, p, d, clear)?,
        }
    }
    if let Some(c) = card {
        write_card(&mut CrosstermBackend::new(&mut frame), c)?;
    }
    if frame.len() == BEGIN.len() {
        return Ok(());
    }
    frame.extend_from_slice(b"\x1b[?2026l");
    out.write_all(&frame)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use ratatui_image::picker::ProtocolType;

    use crate::{
        Stage, TICK,
        cast::Cast,
        scan::{self, Snapshot},
        scene,
        tests::{NOW, agent, busy, cells, noisy_office},
        ui::{self, App},
    };

    fn cr(x0: u16, y0: u16, x1: u16, y1: u16) -> CellRect {
        CellRect { x0, y0, x1, y1 }
    }

    /// Asserts that no two patches share a cell, that together they cover
    /// every cell asked for, and no more cells than the rectangles asked
    /// for hold between them.
    fn assert_disjoint_cover(patches: &[CellRect], asked: &[CellRect]) {
        for (i, a) in patches.iter().enumerate() {
            for b in &patches[i + 1..] {
                assert!(!a.overlaps(*b), "{a:?} and {b:?} overlap in {patches:?}");
            }
        }
        let got = cells(patches);
        assert!(cells(asked).iter().all(|c| got.binary_search(c).is_ok()), "{patches:?} misses cells of {asked:?}");
        assert!(got.len() <= cells(asked).len(), "{patches:?} paints more than {asked:?}");
    }

    #[test]
    fn merge_joins_when_it_paints_less_and_never_overlaps() {
        // Mostly overlapping: one patch. Diagonal neighbours sharing a
        // corner cell: two patches that no longer share it, not their box.
        let asked = [cr(0, 0, 4, 4), cr(1, 0, 5, 4), cr(10, 10, 12, 12), cr(11, 11, 13, 13)];
        let m = merge(asked.to_vec());
        assert!(m.contains(&cr(0, 0, 5, 4)), "{m:?}");
        assert_disjoint_cover(&m, &asked);
        assert_eq!(cells(&m).len(), 20 + 7, "{m:?}");
    }

    /// An actor whose mark overlaps its body, as seen in a real frame, and
    /// a diagonal link whose pieces share corner cells, both changed: the
    /// patches never overlap, cover just their cells, and the link does not
    /// grow into the box around it.
    #[test]
    fn changed_patches_never_overlap() {
        let font = (10, 20);
        let px = |r: CellRect| [u32::from(r.x0) * 10, u32::from(r.y0) * 20, u32::from(r.x1) * 10, u32::from(r.y1) * 20];
        let actor = |at: u16| vec![cr(at, 16, at + 4, 20), cr(at + 1, 15, at + 5, 17)];
        let link: Vec<CellRect> = (0..8).map(|k| cr(10 + 3 * k, 2 + 2 * k, 14 + 3 * k, 5 + 2 * k)).collect();
        let item = |key, rects: &[CellRect]| Item { key, rects: rects.iter().map(|&r| px(r)).collect() };
        let was: HashMap<String, Item> = [("a".into(), item(1, &actor(91))), ("l".into(), item(1, &link))].into();
        let now: HashMap<String, Item> = [("a".into(), item(2, &actor(92))), ("l".into(), item(2, &link))].into();
        let c = changed(&was, &now, font);
        let asked: Vec<CellRect> = actor(91).into_iter().chain(actor(92)).chain(link.iter().copied()).collect();
        assert_disjoint_cover(&c, &asked);
        let link_box = link.iter().fold(link[0], |a, &b| a.union(b));
        assert!(cells(&c).len() < 2 * cells(&link).len() && cells(&link).len() * 3 < link_box.area() as usize, "{c:?}");
    }

    #[test]
    fn only_changed_items_are_repainted() {
        let item = |key, r: [u32; 4]| Item { key, rects: vec![r] };
        let was: HashMap<String, Item> = [("a".into(), item(1, [0, 0, 10, 20])), ("b".into(), item(1, [100, 100, 110, 120])), ("gone".into(), item(1, [200, 0, 210, 20]))].into();
        let now: HashMap<String, Item> = [("a".into(), item(1, [0, 0, 10, 20])), ("b".into(), item(2, [110, 100, 120, 120]))].into();
        let mut c = changed(&was, &now, (10, 20));
        c.sort_by_key(|r| r.x0);
        assert_eq!(c, [cr(10, 5, 12, 6), cr(20, 0, 21, 1)], "b where it was and is, and where gone was; not a");
    }

    /// A lead (`lead`) and one active subagent, which just handed back to it.
    fn handback(lead: scan::Status) -> (App, Cast) {
        use scan::{Edge, EdgeKind, SEC, Status};
        let mut snap = Snapshot { roots: vec!["s".into()], scanned_at: NOW, ..Snapshot::default() };
        let mut s = agent("s", lead);
        s.children.push("a0".into());
        snap.by_id.insert("s".into(), s);
        snap.by_id.insert("a0".into(), agent("a0", Status::Active));
        snap.edges = vec![Edge { from: "a0".into(), to: "s".into(), kind: EdgeKind::Handback, at: NOW - 5 * SEC, label: String::new() }];
        let mut app = App::new(0, 1);
        app.set_snapshot(snap);
        (app, Cast::load(Path::new("/nonexistent"), false, None))
    }

    /// A link between two agents that stand still, over 48 frames: only the
    /// cells of its letter, where it was and where it is, are painted
    /// again, never a cell of the line alone. A typing end, which bobs, does
    /// not move the line either.
    #[test]
    fn a_link_paints_again_only_the_letter_travelling_along_it() {
        use std::collections::HashSet;
        let font = (10, 20);
        let l = scene::fit((1254, 1254), 140, 40, font, false);
        let office = noisy_office();
        let (line, letter) = ("a0>s>handback", "a0>s>handback>letter");
        let cells_of = |it: &Item| -> HashSet<(u16, u16)> { cells(&it.rects.iter().map(|&r| CellRect::covering(r, font)).collect::<Vec<_>>()).into_iter().collect() };
        for lead in [scan::Status::Active, scan::Status::Running] {
            let (mut app, mut cast) = handback(lead);
            app.refresh_scene(NOW, 0.0, &mut cast);
            let stage = Stage::new(&office, l, &cast);
            let (_, mut was) = stage.compose((1254, 1254), &app, &cast, Duration::ZERO);
            let (mut moved, mut most, mut line_cells) = (0, 0, 0);
            for i in 1..=48u64 {
                let t = Duration::from_millis(50 * i);
                app.refresh_scene(NOW, t.as_secs_f64(), &mut cast);
                let (_, items) = stage.compose((1254, 1254), &app, &cast, t);
                assert_eq!(items[line].key, was[line].key, "{lead:?}, frame {i}: the line changed");
                moved += usize::from(items[letter].key != was[letter].key);
                if lead == scan::Status::Active {
                    let patched = cells(&changed(&was, &items, font));
                    let (old, new) = (cells_of(&was[letter]), cells_of(&items[letter]));
                    let around: HashSet<_> = old.union(&new).copied().collect();
                    let line_only: HashSet<_> = cells_of(&items[line]).difference(&around).copied().collect();
                    assert!(patched.iter().all(|c| around.contains(c)), "frame {i}: patched {patched:?}, the letter is at {around:?}");
                    assert!(patched.len() <= old.len() + new.len() && !patched.iter().any(|c| line_only.contains(c)), "frame {i}");
                    (most, line_cells) = (most.max(patched.len()), line_only.len());
                }
                was = items;
            }
            assert!(moved >= 20, "{lead:?}: the letter moved in {moved} of 48 frames");
            if lead == scan::Status::Active {
                eprintln!("a still link: {line_cells} cells of line, at most {most} cells painted a frame");
                assert!(line_cells > 20 && most > 0, "{line_cells} {most}");
            }
        }
    }

    /// A busy office over two seconds of iTerm2 frames: only the cells of
    /// actors and links that changed are sent, never the whole picture again.
    #[test]
    fn iterm2_frames_send_only_what_moves() {
        let (mut app, mut cast, office) = busy();
        let now = NOW;
        let d = Display { protocol: ProtocolType::Iterm2, font: (10, 20), tmux: false, kitty_id: 1 };
        let l = scene::fit((1254, 1254), 140, 40, d.font, false);
        app.refresh_scene(now, 0.0, &mut cast);
        let stage = Stage::new(&office, l, &cast);
        let (img, mut was) = stage.compose((1254, 1254), &app, &cast, Duration::ZERO);
        let mut full = Vec::new();
        paint_iterm2(&mut full, &img, &l, CellRect::all(l.area), &d, true).unwrap();
        assert!(full.starts_with(b"\x1b[2J\x1b[1;31H\x1b[80X"), "a full repaint clears the screen in the same write");
        let (mut bytes, mut most, frames) = (0, 0, 40);
        for i in 1..=frames {
            let t = TICK * i;
            app.refresh_scene(now, t.as_secs_f64(), &mut cast);
            let (img, items) = stage.compose((1254, 1254), &app, &cast, t);
            let patches = changed(&was, &items, d.font);
            most = most.max(patches.iter().map(|p| u32::from(p.x1 - p.x0) * u32::from(p.y1 - p.y0)).sum::<u32>());
            let mut out = Vec::new();
            for p in patches {
                paint_iterm2(&mut out, &img, &l, p, &d, false).unwrap();
            }
            bytes += out.len();
            was = items;
        }
        let all = u32::from(l.area.width) * u32::from(l.area.height);
        let mean = bytes / frames as usize;
        eprintln!("iTerm2, 8 agents and 2 links on {all} cells: at most {most} cells a frame, {} KB a frame on average; the whole picture is {} KB", mean / 1024, full.len() / 1024);
        assert!(most * 4 < all, "a frame repainted {most} of {all} cells");
        assert!(mean * 5 < full.len(), "{mean} bytes a frame, the whole picture is {}", full.len());
    }

    /// A writer that counts its writes and flushes.
    #[derive(Default)]
    struct Calls {
        bytes: Vec<u8>,
        writes: usize,
        flushes: usize,
    }

    impl Write for Calls {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    /// A frame where several things changed, a card opened and cells were
    /// blanked reaches the terminal as one write and one flush, so it never
    /// shows the cells erased for one patch before the next lands.
    #[test]
    fn an_iterm2_frame_is_one_write() {
        let (mut app, mut cast, office) = busy();
        let d = Display { protocol: ProtocolType::Iterm2, font: (10, 20), tmux: false, kitty_id: 1 };
        let l = scene::fit((1254, 1254), 140, 40, d.font, false);
        app.refresh_scene(NOW, 0.0, &mut cast);
        let stage = Stage::new(&office, l, &cast);
        let (_, was) = stage.compose((1254, 1254), &app, &cast, Duration::ZERO);
        let t = TICK * 3;
        app.refresh_scene(NOW, t.as_secs_f64(), &mut cast);
        let (img, items) = stage.compose((1254, 1254), &app, &cast, t);
        let moved = items.iter().filter(|(id, it)| was.get(*id).is_none_or(|o| o.key != it.key)).count();
        assert!(moved >= 3, "{moved} items changed");
        let card = ui::card(vec![ratatui::text::Line::raw("hi")], Rect::new(l.area.x + 2, 2, 2, 2), Rect::new(0, 0, 140, 40));
        let (patches, blanks) = plan(changed(&was, &items, d.font), l.area, Some(Rect::new(0, 0, 3, 3)), Some(card.area));
        assert!(patches.len() >= 3 && !blanks.is_empty(), "{} patches, {} blanks", patches.len(), blanks.len());
        let mut out = Calls::default();
        frame_patches(&mut out, &img, &l, &patches, &blanks, Some(&card), &d, false).unwrap();
        assert_eq!((out.writes, out.flushes), (1, 1));
        let s = String::from_utf8_lossy(&out.bytes);
        assert_eq!(s.matches("\x1b]1337;File=").count(), patches.len());
        assert!(!s.contains("\x1b[6n") && !s.contains("\x1b[2J"), "a patch frame neither asks nor clears");
        // The terminal shows none of it until all of it is in.
        assert!(s.starts_with("\x1b[?2026h") && s.ends_with("\x1b[?2026l"), "a frame is one synchronized update");
        assert_eq!((s.matches("\x1b[?2026h").count(), s.matches("\x1b[?2026l").count()), (1, 1));

        // A full repaint: the clear leads the same single write.
        let mut out = Calls::default();
        frame_patches(&mut out, &img, &l, &[CellRect::all(l.area)], &[], None, &d, true).unwrap();
        assert_eq!((out.writes, out.flushes), (1, 1));
        assert!(out.bytes.starts_with(b"\x1b[?2026h\x1b[2J") && out.bytes.ends_with(b"\x1b[?2026l"));

        // Nothing changed: nothing sent.
        let mut out = Calls::default();
        frame_patches(&mut out, &img, &l, &[], &[], None, &d, false).unwrap();
        assert_eq!((out.writes, out.flushes), (0, 0));
    }

    /// With sixels, a frame is the changed cells as one sixel each at their
    /// top left, no cell erased first (the erase would show), in one
    /// synchronized write, and far smaller than the whole picture.
    #[test]
    fn a_sixel_frame_paints_only_the_changed_cells_over_the_old() {
        let (mut app, mut cast, office) = busy();
        let d = Display { protocol: ProtocolType::Sixel, font: (10, 20), tmux: false, kitty_id: 1 };
        let l = scene::fit((1254, 1254), 140, 40, d.font, false);
        app.refresh_scene(NOW, 0.0, &mut cast);
        let stage = Stage::new(&office, l, &cast);
        let (_, was) = stage.compose((1254, 1254), &app, &cast, Duration::ZERO);
        let t = TICK * 3;
        app.refresh_scene(NOW, t.as_secs_f64(), &mut cast);
        let (img, items) = stage.compose((1254, 1254), &app, &cast, t);
        let (patches, _) = plan(changed(&was, &items, d.font), l.area, None, None);
        assert!(!patches.is_empty());
        let mut out = Calls::default();
        frame_patches(&mut out, &img, &l, &patches, &[], None, &d, false).unwrap();
        assert_eq!((out.writes, out.flushes), (1, 1));
        let s = String::from_utf8_lossy(&out.bytes);
        assert!(s.starts_with("\x1b[?2026h") && s.ends_with("\x1b[?2026l"), "a frame is one synchronized update");
        assert_eq!(s.matches("\x1bP").count(), patches.len(), "one sixel a patch");
        assert!(!s.contains("X\x1b") && !s.contains("\x1b[2J") && !s.contains("1337"), "no erase, no clear, no iTerm2 image");
        let mut whole = Calls::default();
        frame_patches(&mut whole, &img, &l, &[CellRect::all(l.area)], &[], None, &d, true).unwrap();
        assert!(whole.bytes.starts_with(b"\x1b[?2026h\x1b[2J"), "a full repaint clears first, in the same update");
        assert!(out.bytes.len() * 4 < whole.bytes.len(), "a patch frame {} bytes, the whole picture {}", out.bytes.len(), whole.bytes.len());
    }

    /// Pieces counts the images with a piece on the screen: patches add
    /// theirs, cover others whole or in part, and the whole picture painted
    /// again leaves one.
    #[test]
    fn pieces_count_the_images_left_on_the_screen() {
        let area = Rect::new(5, 2, 40, 20);
        let mut p = Pieces::new(area);
        assert_eq!(p.live(), 0);
        p.paint(CellRect::all(area));
        assert_eq!(p.live(), 1);
        for y in 0..10 {
            for x in 0..30 {
                p.paint(cr(x, y, x + 1, y + 1));
            }
        }
        assert_eq!(p.live(), 301, "300 one-cell patches on the picture");
        assert!(p.live() > MAX_PIECES);
        p.paint(cr(0, 0, 30, 5));
        assert_eq!(p.live(), 1 + 150 + 1, "a patch over half of them");
        p.paint(cr(35, 15, 50, 30));
        assert_eq!(p.live(), 153, "a patch past the picture is cut to it");
        p.paint(CellRect::all(area));
        assert_eq!(p.live(), 1);
    }

    /// Asserts patches never overlap each other nor `card` (screen cells),
    /// and lie in area.
    fn assert_clear_of(patches: &[CellRect], area: Rect, card: Option<Rect>) {
        for (i, a) in patches.iter().enumerate() {
            assert!(!a.is_empty() && a.x1 <= area.width && a.y1 <= area.height, "{a:?} outside {area:?}");
            for b in &patches[i + 1..] {
                assert!(!a.overlaps(*b), "{a:?} and {b:?} overlap");
            }
            if let Some(c) = card {
                let screen = CellRect { x0: a.x0 + area.x, y0: a.y0 + area.y, x1: a.x1 + area.x, y1: a.y1 + area.y };
                assert!(!screen.overlaps(CellRect::of(c)), "patch {screen:?} under the card {c:?}");
            }
        }
    }

    /// iTerm2 frames of a busy office with a card open: no patch touches the
    /// card. Closed, its cells on the picture are painted again and those off
    /// it blanked; moved, the old cells it left are painted, the new ones not.
    #[test]
    fn patches_never_touch_an_open_card() {
        let (mut app, mut cast, office) = busy();
        let l = scene::fit((1254, 1254), 140, 40, (10, 20), false);
        assert_eq!(l.area, Rect::new(30, 0, 80, 40), "80x40@(1,31)");
        let ax = l.area.x;
        app.refresh_scene(NOW, 0.0, &mut cast);
        let stage = Stage::new(&office, l, &cast);
        let (_, mut was) = stage.compose((1254, 1254), &app, &cast, Duration::ZERO);
        // Over the picture's left edge, and over the lead at its desk.
        let cards = [Rect::new(20, 8, 40, 20), Rect::new(70, 14, 40, 20)];
        let mut touched = 0;
        for i in 1..=40u64 {
            let t = Duration::from_millis(50 * i);
            app.refresh_scene(NOW, t.as_secs_f64(), &mut cast);
            let (_, items) = stage.compose((1254, 1254), &app, &cast, t);
            let asked = changed(&was, &items, (10, 20));
            let card = cards[usize::from(i > 20)];
            touched += asked.iter().filter(|p| CellRect { x0: p.x0 + ax, x1: p.x1 + ax, ..**p }.overlaps(CellRect::of(card))).count();
            let old = if i == 21 { cards[0] } else { card };
            let (patches, blanks) = plan(asked.clone(), l.area, Some(old), Some(card));
            assert_clear_of(&patches, l.area, Some(card));
            assert!(blanks.iter().all(|b| !b.overlaps(CellRect::of(card)) && !b.overlaps(CellRect::of(l.area))), "{blanks:?}");
            if i == 21 {
                // The cells the card left, on the picture, are painted again.
                let left = CellRect::of(cards[0]).minus(CellRect::of(card));
                let painted = cells(&patches.iter().map(|p| CellRect { x0: p.x0 + ax, x1: p.x1 + ax, ..*p }).collect::<Vec<_>>());
                let want: Vec<_> = cells(&left).into_iter().filter(|&(x, _)| x >= ax).collect();
                assert!(want.iter().all(|c| painted.binary_search(c).is_ok()), "moved: {patches:?}");
                assert_eq!(cells(&blanks), cells(&[CellRect { x0: 20, y0: 8, x1: ax, y1: 28 }]), "off the picture");
            }
            was = items;
        }
        assert!(touched > 0, "the cards lie where something moves");
        // Closed: every cell of the card is painted or blanked, once.
        let (patches, blanks) = plan(Vec::new(), l.area, Some(cards[0]), None);
        assert_clear_of(&patches, l.area, None);
        let mut got = cells(&patches.iter().map(|p| CellRect { x0: p.x0 + ax, x1: p.x1 + ax, ..*p }).collect::<Vec<_>>());
        got.extend(cells(&blanks));
        got.sort();
        assert_eq!(got, cells(&[CellRect::of(cards[0])]));
        // Unchanged: nothing extra to paint, nothing to blank.
        assert_eq!(plan(Vec::new(), l.area, Some(cards[1]), Some(cards[1])), (Vec::new(), Vec::new()));
    }

    /// The card goes out as text alone: cursor moves, colors and its
    /// characters, every cell of it, ending with the colors reset.
    #[test]
    fn card_is_written_cell_by_cell() {
        let lines = vec![ratatui::text::Line::raw("hello"), ratatui::text::Line::raw("wide 字 ok")];
        let card = ui::card(lines, Rect::new(10, 5, 4, 4), Rect::new(0, 0, 140, 40));
        let mut bytes = Vec::new();
        write_card(&mut CrosstermBackend::new(&mut bytes), &card).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("\x1b[6;16H") && s.ends_with("\x1b[0m"), "right of the agent: {s:?}");
        assert!(!s.contains("\x1b[6n") && !s.contains("\x1b[2J") && !s.contains("\x1b]1337") && !s.contains("\x1b[?25"), "{s:?}");
        // Replayed on a grid: cursor moves place, colors are dropped, 字 takes two cells.
        let mut grid = vec![vec![' '; 60]; 12];
        let (mut x, mut y) = (0, 0);
        for (k, part) in s.split('\x1b').enumerate() {
            let text = if k == 0 {
                part
            } else {
                let end = part.find(|c: char| c.is_ascii_alphabetic()).unwrap();
                if part.ends_with('H') || part[..=end].ends_with('H') {
                    let (r, c) = part[1..end].split_once(';').unwrap();
                    (y, x) = (r.parse::<usize>().unwrap() - 1, c.parse::<usize>().unwrap() - 1);
                }
                &part[end + 1..]
            };
            for ch in text.chars() {
                grid[y][x] = ch;
                x += if ch == '字' { 2 } else { 1 };
            }
        }
        let rows: Vec<String> = grid[5..9].iter().map(|r| r[15..39].iter().collect()).collect();
        assert_eq!(rows[0], format!("┌{}┐", "─".repeat(22)));
        assert_eq!(rows[1], format!("│ hello{}│", " ".repeat(16)));
        // 字's second cell stays as the grid had it: a space.
        assert_eq!(rows[2], "│ wide 字  ok           │");
        assert_eq!(rows[3], format!("└{}┘", "─".repeat(22)));
    }
}
