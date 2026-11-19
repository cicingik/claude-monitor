//! office watches Claude Code agents on an office picture, like the Go
//! claude-monitor but drawn through a terminal graphics protocol (kitty,
//! iTerm2 or sixel) when the terminal has one, so it shows true pixels; else
//! with half blocks. It is read-only: it reads the transcripts under
//! ~/.claude/projects and every ~/.claude-*/projects (or --root) and rescans
//! every second, reading only appended lines.
//!
//! Every session shares one office. The agents of active sessions (one of
//! them running or active, or not done and active in the last 5 minutes)
//! live there: the most recently active lead at a desk sits at the
//! manager's desk, the other leads and then the subagents at the nine
//! open-plan desks. Running agents type at their desk, active ones sit
//! there, idle ones doze in a lounge and done ones sit dimmed with a ✓;
//! done for more than 10 minutes, they walk out of the entrance and count as
//! "left". Each keeps its place while it still suits it. Newcomers walk in
//! from the entrance, and everyone walks the corridors between places. The
//! agents of inactive sessions (leads first), and residents with no place
//! left, drop in as visitors: every 4 to 12 s, while fewer than 3 are in,
//! one walks in from the entrance, wanders two to four spots, pausing at
//! some, and walks out again; none comes twice before the others had a turn.
//! Edges younger than 90 s between residents are still dotted lines with a
//! letter travelling along: yellow spawn, cyan message, green handback. The
//! picture is office.png at its original size, centered
//! in the whole pane, shrunk with its proportions kept when the pane is too
//! small, and refitted on resize. The office view is the picture alone, with
//! no text; the selected agent is marked on it. Clicking an agent (or i)
//! opens its card beside it, the only text on the office: its name, role,
//! model, status and skills, what it did, and its spawns, messages and
//! handbacks with other agents. Clicking elsewhere (not on the card), or
//! esc, closes it.
//!
//! With iTerm2 images, office paints the office itself, not through
//! ratatui: a full repaint (at start, after a resize or a switch of view) is
//! one write of a screen clear and the whole picture; every later frame sends
//! only the cells of actors and links that changed, as patches that never
//! overlap; a link's line is sent again only when it appears, goes or an
//! end moves, while its letter's cells follow the letter. An open card is written as text over the picture when it changes,
//! and patches leave its cells alone; when it closes or moves, its old cells
//! are painted again. Every frame is one synchronized update. Once more than
//! MAX_PIECES patches show on the screen, the whole picture is painted
//! again, at most once every WHOLE_EVERY.
//! Other protocols draw the picture as a ratatui image, with the card on it.
//!
//! Keys, office: click an agent (a visitor too) to select it and open its
//! card; i opens or closes the card; ↑/↓, k/j, tab, the mouse wheel select
//! an agent, residents then visitors (the card follows); enter opens the
//! agent in the tree view; v the tree view; r drops all state and rescans;
//! esc closes the card; q, esc, ctrl+c quit. Tree: ↑/↓, k/j move; enter, space collapse; tab, 1 2 3
//! change tab; v back to the office.
//!
//! With OFFICE_VIEW_TRACE=FILE set, every byte office writes to the
//! terminal is also appended to FILE, except the queries that detect the
//! protocol and cell size when --protocol and --font do not both say.

mod actors;
mod cast;
mod format;
mod limits;
mod scan;
mod scene;
mod ui;

use std::{
    cell::Cell,
    collections::HashMap,
    env,
    error::Error,
    fs::{File, OpenOptions},
    io::{self, Stdout, Write, stdout},
    path::{Path, PathBuf},
    process,
    rc::Rc,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant, SystemTime},
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image::{
    DynamicImage, ExtendedColorType, ImageEncoder, RgbaImage,
    codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder},
};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
    buffer::{Buffer, CellDiffOption},
    crossterm::{
        cursor::Show,
        event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEventKind},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode},
    },
    layout::{Rect, Size},
};
use ratatui_image::{
    Image,
    picker::{Picker, ProtocolType, cap_parser::QueryStdioOptions},
    protocol::{Protocol, halfblocks::Halfblocks, iterm2::Iterm2, kitty::Kitty, sixel::Sixel},
};

use actors::{Item, Scene};
use cast::Cast;
use scan::{Scanner, Snapshot, unix_nanos};
use scene::{Layout, Scaled, Sprite};
use ui::{Action, App, View};

/// 10 frames a second: walkers and animations look smooth enough, and every
/// frame repaints each of them, so a faster tick costs the terminal more
/// than it shows.
const TICK: Duration = Duration::from_millis(100);
/// Cell size in pixels assumed when the terminal does not report one.
const DEFAULT_FONT: (u16, u16) = (10, 20);

/// The room, the character for agents the sprites give none, and the pet,
/// built into the binary so it runs from any directory.
const OFFICE_PNG: &[u8] = include_bytes!("../assets/office.png");
const CHARACTER_GIF: &[u8] = include_bytes!("../assets/character.gif");
const PET_GIF: &[u8] = include_bytes!("../assets/pet.gif");

const USAGE: &str = "usage: office [flags]

  --root DIR   a Claude Code projects directory; repeat for more (default
               ~/.claude/projects and every ~/.claude-*/projects)
  --since DUR  only sessions whose transcript changed within DUR (default 24h, 0 = all)
  --sprites DIR  character .gif/.png files named after agent types, or any name
               (default $XDG_CONFIG_HOME/claude-monitor/sprites, else ~/.config/claude-monitor/sprites)
  --dump       scan once, print the tree, edges and usage as text, exit
  --snapshot OUT.png  scan once, render the office at full size to OUT.png as it stands
               once a visitor has walked in (at most 30 s on), exit
  --offline    never fetch the accounts' 5 h and 7 day limits (tree, Usage tab)
  --protocol   how to draw (default auto: ask the terminal, else half blocks)
  --font       the terminal's cell size in pixels, for when the terminal does not answer;
               with --protocol, nothing is asked (halfblocks never asks, and assume 10x20)

office: one room for every session. Agents of active sessions (running or active,
      or not done and active in the last 5 min) work at desks, then in the meeting
      room, and rest in the lounges and the kitchen; agents of the others walk in
      from the entrance now and then, wander, and walk out. A pet strolls round the lobby.

keys: click an agent (visitors too): select it and open its card (name, what it
      did, whom it dealt with); click elsewhere or esc: close it · i open/close the card
      ↑/↓ k/j tab wheel select · enter open in tree · v tree view
      r rescan · q esc ctrl+c quit; in the tree: enter/space collapse, tab 1 2 3 tabs

env: OFFICE_VIEW_TRACE=FILE  also append every byte written to the terminal to FILE
      (not the queries that detect the protocol and cell size)";

struct Args {
    protocol: Option<ProtocolType>, // None: auto
    font: Option<(u16, u16)>,
    roots: Vec<PathBuf>,      // empty: ~/.claude/projects and every ~/.claude-*/projects
    since: i64,               // nanoseconds, 0 = all
    sprites: Option<PathBuf>, // None: the default directory
    dump: bool,
    snapshot: Option<PathBuf>,
    offline: bool, // no requests for the accounts' limits
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args {
        protocol: None,
        font: None,
        roots: Vec::new(),
        since: 24 * 3600 * scan::SEC,
        sprites: None,
        dump: false,
        snapshot: None,
        offline: false,
    };
    while let Some(a) = it.next() {
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = || inline.clone().or_else(|| it.next()).ok_or(format!("{name} needs a value"));
        match name.as_str() {
            "-h" | "--help" => return Err(USAGE.into()),
            "--protocol" => {
                args.protocol = match value()?.as_str() {
                    "auto" => None,
                    "kitty" => Some(ProtocolType::Kitty),
                    "iterm2" => Some(ProtocolType::Iterm2),
                    "sixel" => Some(ProtocolType::Sixel),
                    "halfblocks" => Some(ProtocolType::Halfblocks),
                    p => return Err(format!("unknown --protocol {p:?}\n\n{USAGE}")),
                }
            }
            "--font" => {
                let v = value()?;
                let font = v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                args.font = Some(font.filter(|&(w, h)| w > 0 && h > 0).ok_or(format!("--font {v:?} is not WxH"))?);
            }
            "--root" => args.roots.push(value()?.into()),
            "--since" => args.since = format::parse_duration(&value()?)?,
            "--sprites" => args.sprites = Some(value()?.into()),
            "--dump" => args.dump = true,
            "--snapshot" => args.snapshot = Some(value()?.into()),
            "--offline" => args.offline = true,
            _ if name.starts_with('-') && name != "-" => return Err(format!("unknown flag {name}\n\n{USAGE}")),
            _ => return Err(format!("unexpected argument {a:?}\n\n{USAGE}")),
        }
    }
    Ok(args)
}

/// How frames reach the terminal.
struct Display {
    protocol: ProtocolType,
    font: (u16, u16),
    tmux: bool,
    kitty_id: u32,
}

impl Display {
    /// Asks the terminal for its graphics protocol and cell size, unless the
    /// flags already say. Call it in raw mode on the alternate screen, before
    /// reading events. A terminal that never answers leaves ratatui-image's
    /// reader thread on stdin, and it takes the first key press.
    fn detect(args: &Args) -> Result<Display, Box<dyn Error>> {
        let kitty_id = process::id().max(1);
        match (args.protocol, args.font) {
            (Some(ProtocolType::Halfblocks), font) => {
                let font = font.unwrap_or(DEFAULT_FONT);
                return Ok(Display { protocol: ProtocolType::Halfblocks, font, tmux: false, kitty_id });
            }
            // Picker::halfblocks asks nothing; it only notices tmux and turns on its passthrough.
            (Some(protocol), Some(font)) => {
                return Ok(Display { protocol, font, tmux: Picker::halfblocks().tmux_detected(), kitty_id });
            }
            _ => {}
        }
        let mut options = QueryStdioOptions::default();
        // ratatui-image steers WezTerm to iTerm2 (its kitty lacks the placeholders
        // ratatui-image uses) only when WEZTERM_EXECUTABLE is set, which does not
        // reach WSL. TERM_PROGRAM does, through WSLENV.
        let wezterm = env::var("TERM_PROGRAM").is_ok_and(|p| p.contains("WezTerm"));
        if wezterm {
            options.blacklist_protocols = vec![ProtocolType::Kitty, ProtocolType::Sixel];
        }
        let picker = Picker::from_query_stdio_with_options(options)?;
        let f = picker.font_size();
        // WezTerm over WSL answers neither CSI 16t nor the pixel-size ioctl, and
        // without a cell size ratatui-image falls back to half blocks. iTerm2
        // images are sized in cells, so an assumed cell size only costs sharpness.
        let detected = match picker.protocol_type() {
            ProtocolType::Halfblocks if wezterm => ProtocolType::Iterm2,
            p => p,
        };
        Ok(Display {
            protocol: args.protocol.unwrap_or(detected),
            font: args.font.unwrap_or((f.width, f.height)),
            tmux: picker.tmux_detected(),
            kitty_id,
        })
    }

    /// Encodes img, the canvas of a layout, for the cells of size.
    fn encode(&self, img: RgbaImage, size: Size) -> Result<Protocol, ratatui_image::errors::Errors> {
        let img = DynamicImage::ImageRgba8(img);
        Ok(match self.protocol {
            ProtocolType::Halfblocks => Protocol::Halfblocks(Halfblocks::new(img, size)?),
            ProtocolType::Iterm2 => Protocol::ITerm2(Iterm2::new(img, size, self.tmux)?),
            ProtocolType::Sixel => Protocol::Sixel(Sixel::new(img, size, self.tmux)?),
            // One id for the run: each frame replaces the last image instead of adding one.
            ProtocolType::Kitty => Protocol::Kitty(Kitty::new(img, size, self.kitty_id, self.tmux, false)?),
        })
    }

    /// Wraps an escape sequence for tmux, which passes it through when it is
    /// wrapped and its escapes doubled.
    fn wrap(&self, seq: String) -> String {
        if self.tmux { format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b")) } else { seq }
    }
}

/// Per-frame cost, summed over the run.
#[derive(Default)]
struct Cost {
    frames: u32,
    compose: Duration,
    draw: Duration,
    bytes: u64,
}

/// Stdout that counts what is written to it and, with OFFICE_VIEW_TRACE
/// set, appends it to that file too.
struct Counted {
    out: Stdout,
    bytes: Rc<Cell<u64>>,
    trace: Option<File>,
}

impl Counted {
    fn new() -> io::Result<Counted> {
        let trace = match env::var_os("OFFICE_VIEW_TRACE") {
            Some(p) => Some(OpenOptions::new().create(true).append(true).open(p)?),
            None => None,
        };
        Ok(Counted { out: stdout(), bytes: Rc::default(), trace })
    }
}

impl Write for Counted {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.out.write(buf)?;
        if let Some(t) = &mut self.trace {
            t.write_all(&buf[..n])?;
        }
        self.bytes.set(self.bytes.get() + n as u64);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// The picture as it stands for one layout: the office scaled into the
/// canvas, every sprite at this layout's character size, and the pet at its own.
struct Stage {
    layout: Layout,
    base: RgbaImage,
    scaled: Vec<Scaled>,
    pet: Option<Scaled>,
}

impl Stage {
    fn new(office: &RgbaImage, layout: Layout, cast: &Cast) -> Stage {
        let scaled = (0..cast.len()).map(|i| cast.sprite(i).scaled(&layout, scene::CHARACTER_HEIGHT)).collect();
        let pet = cast.pet.as_ref().map(|p| p.scaled(&layout, scene::PET_HEIGHT / p.typical_height()));
        Stage { base: scene::base(office, &layout), layout, scaled, pet }
    }

    /// The office with the pet and the app's scene drawn in, t into the
    /// animation, and what was drawn, keyed by actor and link (and PET).
    fn compose(&self, office: (u32, u32), app: &App, cast: &Cast, t: Duration) -> (RgbaImage, HashMap<String, Item>) {
        let mut img = self.base.clone();
        // Under everyone: small, it only ever passes them.
        let pet = (self.pet.as_ref().zip(cast.pet.as_ref()).zip(app.scene.pet))
            .map(|((s, p), ps)| actors::paint_pet(&mut img, &self.layout, office, s, p.frame_index(t), ps, t.as_secs_f64()));
        let mut items = actors::paint(&mut img, &self.layout, office, &app.scene, &self.scaled, t, |i, t| cast.sprite(i).frame_index(t));
        items.extend(pet.map(|it| (actors::PET.to_string(), it)));
        (img, items)
    }
}

/// Shows the app on out until a quit key: redraws every TICK (or as fast as
/// frames can be made, if slower), takes each scan as it comes, and refits on
/// resize.
fn show(out: &mut Counted, office: &RgbaImage, cast: &mut Cast, d: &Display, app: &mut App, scans: &Receiver<Snapshot>, rescan: &Sender<()>) -> Result<(Cost, Size), Box<dyn Error>> {
    let bytes = out.bytes.clone();
    let mut term = Terminal::new(CrosstermBackend::new(out))?;
    let (start, mut next) = (Instant::now(), Instant::now());
    let (mut cost, mut pane, mut full) = (Cost::default(), Size::default(), true);
    let mut stage: Option<Stage> = None;
    // What each actor and link looked like when last painted, for iTerm2's
    // patches and for clicks; None until the whole picture is.
    let mut shown: Option<HashMap<String, Item>> = None;
    // Other protocols: the encoded picture and what it showed.
    let mut encoded: Option<(Protocol, u64)> = None;
    // The open card's agent and the cells it was beside when the card
    // opened, kept so the card does not chase it when it walks.
    let mut anchor: Option<(String, Rect)> = None;
    // The card as it stands on the screen.
    let mut on_screen: Option<Buffer> = None;
    // iTerm2: the image each cell of the picture shows, when the whole
    // picture was last painted, and whether the next frame paints it again
    // to make the many one.
    let (mut pieces, mut whole, mut collapse) = (Pieces::default(), Instant::now(), false);
    loop {
        while let Ok(s) = scans.try_recv() {
            app.set_snapshot(s);
        }
        let now = Instant::now();
        if full || now >= next {
            let collapsing = std::mem::take(&mut collapse);
            let size = term.size()?;
            if size != pane {
                pane = size;
                full = true;
            }
            let t = now - start;
            app.refresh_scene(unix_nanos(SystemTime::now()), t.as_secs_f64(), cast);
            let (drawing, written) = (Instant::now(), bytes.get());
            // The office in iTerm2 goes straight to the terminal, never
            // through ratatui, whose clear asks where the cursor is (CSI 6n)
            // and whose text the picture has none of.
            let iterm2 = app.view == View::Office && d.protocol == ProtocolType::Iterm2;
            if full {
                (shown, encoded, anchor, on_screen) = (None, None, None, None);
                if !iterm2 {
                    term.clear()?;
                }
            }
            let halfblocks = d.protocol == ProtocolType::Halfblocks;
            let l = scene::fit(office.dimensions(), pane.width, pane.height, d.font, halfblocks);
            if stage.as_ref().is_none_or(|s| s.layout != l) {
                stage = Some(Stage::new(office, l, cast));
            }
            let stage = stage.as_ref().expect("set above");
            if app.view == View::Tree {
                term.draw(|f| app.draw(f))?;
            } else if !l.area.is_empty() {
                let composing = Instant::now();
                let (img, items) = stage.compose(office.dimensions(), app, cast, t);
                cost.compose += composing.elapsed();
                let card = open_card(app, &items, &l, pane, &mut anchor);
                if iterm2 {
                    // The whole office, cleared around, after a full repaint;
                    // else only the cells of actors and links that changed:
                    // re-sending the whole picture every frame is megabytes a
                    // frame, more than the terminal keeps up with.
                    let cards = (on_screen.as_ref().map(|w| w.area), card.as_ref().map(|c| c.area));
                    let (patches, blanks) = match &shown {
                        Some(_) if collapsing => plan(vec![CellRect::all(l.area)], l.area, cards.0, cards.1),
                        Some(was) => plan(changed(was, &items, d.font), l.area, cards.0, cards.1),
                        None => (vec![CellRect::all(l.area)], Vec::new()),
                    };
                    let fresh_card = card.as_ref().filter(|&c| on_screen.as_ref() != Some(c));
                    frame_iterm2(term.backend_mut(), &img, &l, &patches, &blanks, fresh_card, d, shown.is_none())?;
                    if shown.is_none() {
                        pieces = Pieces::new(l.area);
                    }
                    if shown.is_none() || collapsing {
                        whole = now;
                    }
                    patches.iter().for_each(|&p| pieces.paint(p));
                    collapse = pieces.live() > MAX_PIECES && now - whole >= WHOLE_EVERY;
                } else {
                    let key = picture_key(&items);
                    let fresh = encoded.as_ref().is_none_or(|(_, k)| *k != key);
                    if fresh {
                        encoded = Some((d.encode(img, l.area.as_size())?, key));
                    }
                    let (p, _) = encoded.as_ref().expect("set above");
                    term.draw(|f| {
                        f.render_widget(Image::new(p), l.area);
                        if let Some(c) = &card {
                            f.buffer_mut().merge(c);
                        }
                    })?;
                    // A sixel just sent covers the card's cells, which ratatui
                    // sees unchanged: write them again over it.
                    if let (true, Some(c), ProtocolType::Sixel) = (fresh, &card, d.protocol) {
                        write_card(term.backend_mut(), c)?;
                    }
                }
                (shown, on_screen) = (Some(items), card);
            }
            cost.draw += drawing.elapsed();
            cost.bytes += bytes.get() - written;
            cost.frames += 1;
            next = now + TICK.max(now.elapsed());
            full = false;
        }
        if !event::poll(next.saturating_duration_since(Instant::now()))? {
            continue;
        }
        // Take every pending event before the next frame, so a drag-resize refits once.
        loop {
            let action = match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => Some(app.key(k)),
                // With clicks reported, the wheel no longer arrives as arrow keys.
                Event::Mouse(m) if m.kind == MouseEventKind::ScrollUp => Some(app.key(KeyEvent::from(KeyCode::Up))),
                Event::Mouse(m) if m.kind == MouseEventKind::ScrollDown => Some(app.key(KeyEvent::from(KeyCode::Down))),
                // A click on the card itself leaves it be.
                Event::Mouse(m) if on_screen.as_ref().is_some_and(|c| c.area.contains((m.column, m.row).into())) => None,
                Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) && app.view == View::Office => {
                    let on = shown.as_ref().zip(stage.as_ref()).and_then(|(items, s)| hit(&app.scene, items, &s.layout, m.column, m.row));
                    app.click(on);
                    Some(Action::None)
                }
                Event::Resize(..) => {
                    full = true;
                    None
                }
                _ => None,
            };
            if let Some(action) = action {
                match action {
                    Action::Quit => return Ok((cost, pane)),
                    Action::Rescan => _ = rescan.send(()),
                    Action::Switched => {
                        if d.protocol == ProtocolType::Kitty {
                            // Drop the office image itself, not only its cells.
                            let seq = d.wrap(format!("\x1b_Ga=d,d=I,i={}\x1b\\", d.kitty_id));
                            term.backend_mut().write_all(seq.as_bytes())?;
                        }
                        if app.view == View::Office && d.protocol == ProtocolType::Iterm2 {
                            // The tree's draws hid the cursor; the old office never did.
                            term.show_cursor()?;
                        }
                        full = true;
                    }
                    Action::None => next = Instant::now(),
                }
            }
            if !event::poll(Duration::ZERO)? {
                break;
            }
        }
    }
}

/// One number for everything drawn on the picture.
fn picture_key(items: &HashMap<String, Item>) -> u64 {
    let mut keys: Vec<(&String, u64)> = items.iter().map(|(id, it)| (id, it.key)).collect();
    keys.sort();
    let mut h = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(&keys, &mut h);
    std::hash::Hasher::finish(&h)
}

/// The cells to repaint: those of every actor and link that appeared,
/// vanished or looks different, where it was and where it is, merged.
fn changed(was: &HashMap<String, Item>, now: &HashMap<String, Item>, font: (u16, u16)) -> Vec<CellRect> {
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

/// The canvas pixels of one cell of l: the font's for a pixel protocol,
/// 1×2 for half blocks.
fn cell_px(l: &Layout) -> (u16, u16) {
    let per = |px: u32, cells: u16| u16::try_from(px / u32::from(cells.max(1))).unwrap_or(1).max(1);
    (per(l.canvas.0, l.area.width), per(l.canvas.1, l.area.height))
}

/// The agent of the roster drawn frontmost at screen cell (col, row), as
/// items (last painted from sc on layout l) cover it. Later-drawn actors
/// are in front: paint draws them by their feet, top to bottom.
fn hit(sc: &Scene, items: &HashMap<String, Item>, l: &Layout, col: u16, row: u16) -> Option<String> {
    let (x, y) = (col.checked_sub(l.area.x)?, row.checked_sub(l.area.y)?);
    let font = cell_px(l);
    let covers = |r: &[u32; 4]| {
        let c = CellRect::covering(*r, font);
        (c.x0..c.x1).contains(&x) && (c.y0..c.y1).contains(&y)
    };
    sc.actors
        .iter()
        .enumerate()
        .filter(|(_, a)| sc.roster.contains(&a.id) && items.get(&a.id).is_some_and(|it| it.rects.iter().any(covers)))
        .max_by(|(i, a), (j, b)| a.pos.y.total_cmp(&b.pos.y).then(i.cmp(j)))
        .map(|(_, a)| a.id.clone())
}

/// The card of app's selected agent while it is open, beside the screen
/// cells the agent's body covered (in items, on l) when the card opened or
/// moved to it, which anchor keeps.
fn open_card(app: &App, items: &HashMap<String, Item>, l: &Layout, pane: Size, anchor: &mut Option<(String, Rect)>) -> Option<Buffer> {
    if !app.card {
        *anchor = None;
        return None;
    }
    if anchor.as_ref().is_none_or(|(id, _)| *id != app.focus) {
        let body = items.get(&app.focus).and_then(|it| it.rects.first()).map(|&r| CellRect::covering(r, cell_px(l)));
        let at = match body {
            Some(c) => Rect::new(l.area.x + c.x0, l.area.y + c.y0, c.x1.saturating_sub(c.x0), c.y1.saturating_sub(c.y0)),
            None => Rect::new(l.area.x + l.area.width / 2, l.area.y + l.area.height / 2, 0, 0),
        };
        *anchor = Some((app.focus.clone(), at));
    }
    let pane = Rect::new(0, 0, pane.width, pane.height);
    let (_, at) = anchor.as_ref()?;
    Some(ui::card(app.card_lines(usize::from(pane.height.saturating_sub(2))), *at, pane))
}

/// An iTerm2 frame after a full repaint, with the card that was on the
/// screen (`old`, screen cells) and the card to show (`new`): the picture
/// patches (relative to area, never overlapping) and the screen cells to
/// blank. The cells old leaves are painted again (blanked off the picture),
/// and no patch touches new's cells, which hold its text.
fn plan(mut patches: Vec<CellRect>, area: Rect, old: Option<Rect>, new: Option<Rect>) -> (Vec<CellRect>, Vec<CellRect>) {
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
fn write_card<W: Write>(out: &mut CrosstermBackend<W>, card: &Buffer) -> io::Result<()> {
    let mut every = card.clone();
    for c in &mut every.content {
        c.set_diff_option(CellDiffOption::AlwaysUpdate);
    }
    out.draw(card.diff(&every).into_iter())?;
    Backend::flush(out)
}

/// A rectangle of cells, relative to the layout's area unless said: columns x0..x1, rows y0..y1.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CellRect {
    x0: u16,
    y0: u16,
    x1: u16,
    y1: u16,
}

impl CellRect {
    fn all(area: Rect) -> CellRect {
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
    fn covering(px: [u32; 4], font: (u16, u16)) -> CellRect {
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
const MAX_PIECES: usize = 256;
/// At most one such repaint in this time: while many walk at once, the
/// pieces pass MAX_PIECES again within a second or two, and the whole
/// picture is some 4 MB against half a megabyte for a frame of patches.
const WHOLE_EVERY: Duration = Duration::from_secs(10);

/// Which iTerm2 image, numbered as sent, each cell of the picture shows.
#[derive(Default)]
struct Pieces {
    width: u16,
    cells: Vec<u32>,
    sent: u32,
}

impl Pieces {
    /// The picture's cells hold nothing yet.
    fn new(area: Rect) -> Pieces {
        Pieces { width: area.width, cells: vec![0; usize::from(area.width) * usize::from(area.height)], sent: 0 }
    }

    /// The cells p (relative to the picture) now show one new image.
    fn paint(&mut self, p: CellRect) {
        self.sent = self.sent.wrapping_add(1).max(1);
        let w = usize::from(self.width);
        for y in usize::from(p.y0)..usize::from(p.y1) {
            let row = self.cells.get_mut(y * w..(y + 1) * w).unwrap_or_default();
            let (x0, x1) = (usize::from(p.x0).min(row.len()), usize::from(p.x1).min(row.len()));
            row[x0..x1].fill(self.sent);
        }
    }

    /// How many images have a piece on the screen.
    fn live(&self) -> usize {
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

/// Sends one iTerm2 frame of the office in a single write: the blanks, the
/// patches of img (with `clear`, a full repaint led by clearing the screen),
/// then the card if it changed, between the begin and end of a synchronized
/// update (DECSET 2026). One write is not enough on its own: the terminal
/// reads a frame of hundreds of kilobytes in pieces and may show the screen
/// between two, with the cells erased for a patch before its image lands, a
/// flicker that grows with every actor on the move.
#[allow(clippy::too_many_arguments)]
fn frame_iterm2(out: &mut impl Write, img: &RgbaImage, l: &Layout, patches: &[CellRect], blanks: &[CellRect], card: Option<&Buffer>, d: &Display, clear: bool) -> io::Result<()> {
    const BEGIN: &[u8] = b"\x1b[?2026h";
    let mut frame = BEGIN.to_vec();
    for &b in blanks {
        blank(&mut frame, b)?;
    }
    for &p in patches {
        paint_iterm2(&mut frame, img, l, p, d, clear)?;
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

/// The projects directories to scan: --root, else the default ones.
fn roots(args: &Args) -> Vec<scan::Root> {
    if !args.roots.is_empty() {
        return scan::dedup_roots(args.roots.clone());
    }
    scan::default_roots(&home())
}

/// $HOME, else %USERPROFILE% (Windows).
fn home() -> PathBuf {
    env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

/// The sprites, from --sprites or $XDG_CONFIG_HOME/claude-monitor/sprites
/// (else ~/.config/claude-monitor/sprites), with the built-in character for
/// the rest, and the built-in pet.
fn load_cast(args: &Args) -> Result<Cast, String> {
    let fallback = Some(Sprite::decode(CHARACTER_GIF).map_err(|e| format!("character: {e}"))?);
    let dir = args.sprites.clone().unwrap_or_else(|| {
        let base = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map_or_else(|| home().join(".config"), PathBuf::from);
        base.join("claude-monitor").join("sprites")
    });
    let mut cast = Cast::load(&dir, args.sprites.is_some(), fallback);
    cast.pet = Some(Sprite::decode(PET_GIF).map_err(|e| format!("pet: {e}"))?);
    Ok(cast)
}

fn now() -> i64 {
    unix_nanos(SystemTime::now())
}

/// --dump: a text report of one scan.
fn dump(args: &Args) -> Result<String, Box<dyn Error>> {
    let roots = roots(args);
    let names: Vec<String> = roots.iter().map(|r| r.dir.display().to_string()).collect();
    let snap = Scanner::new(roots, args.since).scan(now());
    match format::dump(&mut io::stdout().lock(), &snap, &names.join(","), args.since) {
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(String::new()),
    }
}

/// --snapshot: the office as the TUI would show it at the picture's full
/// size, laid out at the time of the scan and walked on (one TICK a step)
/// until a visitor has been in for 2 s, or for at most 30 s.
fn snapshot(args: &Args, out: &Path) -> Result<String, Box<dyn Error>> {
    let office = image::load_from_memory(OFFICE_PNG)?.to_rgba8();
    let mut cast = load_cast(args)?;
    let snap = Scanner::new(roots(args), args.since).scan(now());
    if snap.roots.is_empty() {
        return Err("no sessions in the time window".into());
    }
    let at = snap.scanned_at;
    let mut app = App::new(args.since, at as u64);
    app.set_snapshot(snap);
    let (mut t, mut since_visitor) = (Duration::ZERO, None);
    app.refresh_scene(at, 0.0, &mut cast);
    while t < Duration::from_secs(30) && since_visitor.is_none_or(|v| t - v < Duration::from_secs(2)) {
        t += TICK;
        app.refresh_scene(at, t.as_secs_f64(), &mut cast);
        if app.scene.actors.iter().any(|a| a.visitor) {
            since_visitor.get_or_insert(t);
        }
    }
    let (w, h) = office.dimensions();
    let l = Layout { area: Rect::default(), canvas: (w, h), office: [0, 0, w, h] };
    let stage = Stage::new(&office, l, &cast);
    let (img, _) = stage.compose((w, h), &app, &cast, t);
    img.save(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let sc = &app.scene;
    let visitors: Vec<&str> = sc.actors.iter().filter(|a| a.visitor).map(|a| a.id.as_str()).collect();
    let mut s = format!(
        "office: wrote {} ({w}x{h}) {:.1} s on: {} active sessions, {} agents drawn, visitors {visitors:?}, {} links, {} left, {}",
        out.display(),
        t.as_secs_f64(),
        sc.sessions,
        sc.actors.len(),
        sc.links.len(),
        sc.left,
        if cast.pet.is_some() { "a pet" } else { "no pet" }
    );
    if !cast.note.is_empty() {
        s += &format!("\n{}", cast.note);
    }
    Ok(s)
}

/// Scans in the background: once now, then every second, and at once with
/// fresh state on each message from rescan. Stops when either side hangs up.
fn scanner(roots: Vec<scan::Root>, since: i64) -> (Receiver<Snapshot>, Sender<()>) {
    let (snaps, scans) = mpsc::channel();
    let (rescan, requests) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut s = Scanner::new(roots, since);
        loop {
            if snaps.send(s.scan(now())).is_err() {
                return;
            }
            match requests.recv_timeout(Duration::from_secs(1)) {
                Ok(()) => s.reset(),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    (scans, rescan)
}

fn restore(out: &mut impl Write) {
    let _ = disable_raw_mode();
    let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen, Show);
}

fn run(args: Args) -> Result<String, Box<dyn Error>> {
    if args.dump {
        return dump(&args);
    }
    if let Some(out) = &args.snapshot {
        return snapshot(&args, out);
    }
    let office = image::load_from_memory(OFFICE_PNG)?.to_rgba8();
    let mut cast = load_cast(&args)?;
    let mut app = App::new(args.since, now() as u64);
    // Only here, never for --dump or --snapshot; nothing is fetched until the Usage tab shows.
    app.limits = Some(limits::Limits::new(limits::accounts(&home()), args.offline));
    let (scans, rescan) = scanner(roots(&args), args.since);

    let mut out = Counted::new().map_err(|e| format!("OFFICE_VIEW_TRACE: {e}"))?;
    enable_raw_mode()?;
    execute!(out, EnterAlternateScreen, SetTitle("office"))?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(&mut stdout());
        hook(info);
    }));
    // Clicks are reported only once detection has had its answers, so none is taken for one.
    let shown = Display::detect(&args).and_then(|d| {
        execute!(out, EnableMouseCapture)?;
        Ok((show(&mut out, &office, &mut cast, &d, &mut app, &scans, &rescan)?, d))
    });
    restore(&mut out);
    let ((c, pane), d) = shown?;

    let n = c.frames.max(1);
    let ms = |t: Duration| t.as_secs_f64() * 1000.0 / f64::from(n);
    // The office view has no text, so the sprites note comes after it.
    let note = if cast.note.is_empty() { String::new() } else { format!("\n{}", cast.note) };
    Ok(format!(
        "office: {:?} at {}x{} px cells on {}x{} cells; {} frames, per frame: compose {:.1} ms, encode and draw {:.1} ms, {:.0} KB",
        d.protocol,
        d.font.0,
        d.font.1,
        pane.width,
        pane.height,
        c.frames,
        ms(c.compose),
        ms(c.draw),
        c.bytes as f64 / 1024.0 / f64::from(n),
    ) + &note)
}

fn main() {
    let args = match parse_args(env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            process::exit(2);
        }
    };
    match run(args) {
        Ok(summary) if summary.is_empty() => {}
        Ok(summary) => eprintln!("{summary}"),
        Err(e) => {
            eprintln!("office: {e}");
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cr(x0: u16, y0: u16, x1: u16, y1: u16) -> CellRect {
        CellRect { x0, y0, x1, y1 }
    }

    /// Every cell of rects, once per rectangle holding it, sorted.
    fn cells(rects: &[CellRect]) -> Vec<(u16, u16)> {
        let mut out: Vec<_> = rects.iter().flat_map(|r| (r.y0..r.y1).flat_map(move |y| (r.x0..r.x1).map(move |x| (x, y)))).collect();
        out.sort();
        out
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

    const NOW: i64 = 1000 * scan::SEC;

    /// An agent of session "s", the lead when its id is "s".
    fn agent(id: &str, status: scan::Status) -> scan::Agent {
        use scan::{Agent, Kind, SEC, Usage};
        Agent {
            id: id.into(),
            parent_id: if id == "s" { String::new() } else { "s".into() },
            title: id.into(),
            agent_type: String::new(),
            model: String::new(),
            project: "p".into(),
            session_id: "s".into(),
            config: "claude".into(),
            kind: if id == "s" { Kind::Session } else { Kind::Subagent },
            status,
            last_activity: NOW - SEC,
            last_tool: "Bash: ls".into(),
            turns: 1,
            usage: Usage::default(),
            model_usage: Default::default(),
            events: Vec::new(),
            children: Vec::new(),
        }
    }

    /// The picture tests draw on: one that compresses as badly as a photo.
    fn noisy_office() -> RgbaImage {
        RgbaImage::from_fn(1254, 1254, |x, y| {
            let v = (x.wrapping_mul(7919) ^ y.wrapping_mul(104_729) ^ x.wrapping_mul(y)) as u8;
            [v, v.wrapping_mul(3), v.wrapping_mul(7), 255].into()
        })
    }

    /// A busy session (a lead, seven subagents, a spawn and a message) on a
    /// picture that compresses as badly as a photo.
    fn busy() -> (App, Cast, RgbaImage) {
        use scan::{Edge, EdgeKind, SEC, Status};
        let now = NOW;
        let mut snap = Snapshot { roots: vec!["s".into()], scanned_at: now, ..Snapshot::default() };
        let subs = [("a0", Status::Running), ("a1", Status::Running), ("a2", Status::Running), ("a3", Status::Active), ("a4", Status::Idle), ("a5", Status::Idle), ("a6", Status::Done)];
        let mut lead = agent("s", Status::Running);
        for (id, st) in subs {
            lead.children.push(id.into());
            snap.by_id.insert(id.into(), agent(id, st));
        }
        snap.by_id.insert("s".into(), lead);
        snap.edges = vec![
            Edge { from: "s".into(), to: "a0".into(), kind: EdgeKind::Spawn, at: now - 10 * SEC, label: String::new() },
            Edge { from: "a4".into(), to: "s".into(), kind: EdgeKind::Message, at: now - 5 * SEC, label: String::new() },
        ];
        let cast = Cast::load(Path::new("/nonexistent"), false, None);
        let mut app = App::new(0, 1);
        app.set_snapshot(snap);
        (app, cast, noisy_office())
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
        frame_iterm2(&mut out, &img, &l, &patches, &blanks, Some(&card), &d, false).unwrap();
        assert_eq!((out.writes, out.flushes), (1, 1));
        let s = String::from_utf8_lossy(&out.bytes);
        assert_eq!(s.matches("\x1b]1337;File=").count(), patches.len());
        assert!(!s.contains("\x1b[6n") && !s.contains("\x1b[2J"), "a patch frame neither asks nor clears");
        // The terminal shows none of it until all of it is in.
        assert!(s.starts_with("\x1b[?2026h") && s.ends_with("\x1b[?2026l"), "a frame is one synchronized update");
        assert_eq!((s.matches("\x1b[?2026h").count(), s.matches("\x1b[?2026l").count()), (1, 1));

        // A full repaint: the clear leads the same single write.
        let mut out = Calls::default();
        frame_iterm2(&mut out, &img, &l, &[CellRect::all(l.area)], &[], None, &d, true).unwrap();
        assert_eq!((out.writes, out.flushes), (1, 1));
        assert!(out.bytes.starts_with(b"\x1b[?2026h\x1b[2J") && out.bytes.ends_with(b"\x1b[?2026l"));

        // Nothing changed: nothing sent.
        let mut out = Calls::default();
        frame_iterm2(&mut out, &img, &l, &[], &[], None, &d, false).unwrap();
        assert_eq!((out.writes, out.flushes), (0, 0));
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

    /// A click lands on the agent whose cells hold it, the front one where
    /// two overlap (drawn later: feet lower), never one walking out.
    #[test]
    fn clicks_hit_the_frontmost_agent_under_them() {
        use actors::{Actor, Anim};
        let actor = |id: &str, y: f64| Actor {
            id: id.into(),
            kind: String::new(),
            status: scan::Status::Running,
            node: 0,
            pos: scene::Point { x: 0.5, y },
            anim: Anim::Idle,
            left: false,
            selected: false,
            lead: false,
            sprite: 0,
            visitor: false,
        };
        // b's feet are lower: drawn after a, in front of it. g walks out.
        let sc = Scene { actors: vec![actor("b", 0.6), actor("a", 0.5), actor("g", 0.9)], roster: vec!["a".into(), "b".into()], ..Scene::default() };
        let item = |rects: Vec<[u32; 4]>| Item { key: 0, rects };
        // Cells (10 by 20 px) of a: x 10..20, y 5..15 and its mark 12..14, 3..5; b: 15..25, 10..20; g: 40..50, 0..5.
        let items: HashMap<String, Item> =
            [("a".into(), item(vec![[100, 100, 200, 300], [120, 60, 140, 100]])), ("b".into(), item(vec![[150, 200, 250, 400]])), ("g".into(), item(vec![[400, 0, 500, 100]]))].into();
        let l = Layout { area: Rect::new(5, 2, 80, 40), canvas: (800, 800), office: [0, 0, 800, 800] };
        let at = |x: u16, y: u16| hit(&sc, &items, &l, 5 + x, 2 + y);
        assert_eq!(at(11, 6).as_deref(), Some("a"));
        assert_eq!(at(13, 3).as_deref(), Some("a"), "its mark is its own");
        assert_eq!(at(17, 12).as_deref(), Some("b"), "the front one where they overlap");
        assert_eq!(at(24, 19).as_deref(), Some("b"));
        assert_eq!(at(20, 5), None, "x1 is past the last cell");
        assert_eq!(at(45, 2), None, "not on the roster");
        assert_eq!(at(60, 30), None);
        assert_eq!(hit(&sc, &items, &l, 4, 8), None, "left of the picture");
        // Half blocks: a cell is 1×2 canvas pixels.
        let hb = Layout { area: Rect::new(5, 2, 80, 40), canvas: (80, 80), office: [0, 0, 80, 80] };
        let items: HashMap<String, Item> = [("a".into(), item(vec![[10, 10, 20, 30]]))].into();
        assert_eq!(hit(&sc, &items, &hb, 5 + 10, 2 + 5).as_deref(), Some("a"));
        assert_eq!(hit(&sc, &items, &hb, 5 + 10, 2 + 15), None);
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

    /// The pet: drawn in the lobby (the built-in assets/pet.gif), its own
    /// item, repainted only when its frame or where it stands changes, never
    /// clickable, no bigger on the screen than a typing agent.
    #[test]
    fn a_pet_strolls_the_lobby() {
        let font = (10, 20);
        let l = scene::fit((1254, 1254), 140, 40, font, false);
        let flags = |extra: &[&str]| parse_args(["--sprites", "/nonexistent"].iter().chain(extra).map(|s| s.to_string())).unwrap();
        let (mut app, _, office) = busy();
        let mut cast = load_cast(&flags(&[])).unwrap();
        assert!(cast.pet.is_some() && !cast.note.contains("pet"), "{}", cast.note);
        app.refresh_scene(NOW, 0.0, &mut cast);
        let stage = Stage::new(&office, l, &cast);
        let (_, items) = stage.compose((1254, 1254), &app, &cast, Duration::ZERO);
        let [[x0, y0, x1, y1]] = items[actors::PET].rects[..] else { panic!("one rect") };
        // Its feet in the lobby (x 285..870, y 120..445 of the 1254 px picture), its sprite over them.
        let ps = app.scene.pet.expect("walked once");
        assert!((285.0..870.0).contains(&(ps.p.x * 1254.0)) && (120.0..445.0).contains(&(ps.p.y * 1254.0)), "{ps:?}");
        let (fx, fy) = l.px(ps.p);
        assert!((f64::from(x0)..f64::from(x1)).contains(&fx) && (f64::from(y0)..f64::from(y1)).contains(&fy), "{:?} {ps:?}", items[actors::PET].rects);
        let cells = |it: &Item| cells(&it.rects.iter().map(|&r| CellRect::covering(r, font)).collect::<Vec<_>>()).len();
        let typing = app.scene.actors.iter().filter(|a| a.anim == actors::Anim::Typing).map(|a| cells(&items[&a.id])).min().unwrap();
        assert!(cells(&items[actors::PET]) <= typing, "the pet covers {} cells, a typing agent {typing}", cells(&items[actors::PET]));
        let mid = |a: u32, b: u32, o: u16, f: u16| o + u16::try_from((a + b) / 2).unwrap() / f;
        assert_eq!(hit(&app.scene, &items, &l, mid(x0, x1, l.area.x, font.0), mid(y0, y1, l.area.y, font.1)), None, "a click on the pet selects nobody");
        // Over ten seconds its key changes exactly when its frame or where it
        // stands does, and it walks some of them.
        let frame: Vec<usize> = (0..=100).map(|i| cast.pet.as_ref().unwrap().frame_index(TICK * i)).collect();
        let (mut was, mut pose, mut walked) = (items, ps, 0);
        for i in 1..=100 {
            let t = TICK * i;
            app.refresh_scene(NOW, t.as_secs_f64(), &mut cast);
            let (_, items) = stage.compose((1254, 1254), &app, &cast, t);
            let now = app.scene.pet.expect("always there");
            let turned = frame[i as usize] != frame[i as usize - 1];
            assert_eq!(items[actors::PET].key != was[actors::PET].key, turned || now != pose, "frame {i}");
            walked += usize::from(now.moving);
            (was, pose) = (items, now);
        }
        assert!(walked > 10, "walked {walked} frames of 100");
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

    #[test]
    fn flags_parse() {
        let a = parse_args(["--root", "/a", "--root=/b", "--since", "2h", "--dump"].map(String::from).into_iter()).unwrap();
        assert_eq!((a.roots.len(), a.since, a.dump), (2, 7200 * scan::SEC, true));
        // The pictures are built in: no picture argument, no --character or --pet.
        for gone in [&["pic.png"][..], &["--character", "c.gif"], &["--pet", "p.gif"]] {
            assert!(parse_args(gone.iter().map(|s| s.to_string())).is_err(), "{gone:?}");
        }
        assert!(parse_args(["--since", "2"].map(String::from).into_iter()).is_err());
        // One room: there is no session to pick.
        assert!(parse_args(["--session", "s"].map(String::from).into_iter()).is_err_and(|e| e.starts_with("unknown flag --session")));
    }
}
