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
mod bridge;
mod cast;
mod cli;
mod commands;
mod edit;
mod format;
mod history;
mod limits;
mod markdown;
mod patch;
mod scan;
mod scene;
mod terminal;
mod ui;

use std::{
    collections::HashMap,
    env,
    error::Error,
    io::{self, Write, stdout},
    path::{Path, PathBuf},
    process,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant, SystemTime},
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

use image::RgbaImage;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    crossterm::{
        cursor::Show,
        event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton, MouseEventKind},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode},
    },
    layout::{Rect, Size},
};
use ratatui_image::{Image, picker::ProtocolType, protocol::Protocol};

use actors::{Item, Scene};
use cast::Cast;
use cli::{Args, parse_args};
use patch::{CellRect, MAX_PIECES, Pieces, WHOLE_EVERY, changed, frame_iterm2, plan, write_card};
use scan::{Scanner, Snapshot, unix_nanos};
use scene::{Layout, Scaled, Sprite};
use terminal::{Counted, Display, restore};
use ui::{Action, App, View};

/// 10 frames a second: walkers and animations look smooth enough, and every
/// frame repaints each of them, so a faster tick costs the terminal more
/// than it shows.
const TICK: Duration = Duration::from_millis(100);

/// The room, the character for agents the sprites give none, and the pet,
/// built into the binary so it runs from any directory.
const OFFICE_PNG: &[u8] = include_bytes!("../assets/office.png");
const CHARACTER_GIF: &[u8] = include_bytes!("../assets/character.gif");
const PET_GIF: &[u8] = include_bytes!("../assets/pet.gif");

/// Per-frame cost, summed over the run.
#[derive(Default)]
struct Cost {
    frames: u32,
    compose: Duration,
    draw: Duration,
    bytes: u64,
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
    let mut beat: Option<Instant> = None;
    loop {
        while let Ok(s) = scans.try_recv() {
            app.set_snapshot(s);
        }
        // The prompt panel's hooks hold permission requests only while office beats.
        if beat.is_none_or(|b| b.elapsed() >= Duration::from_secs(1)) {
            bridge::heartbeat(&app.bridge);
            beat = Some(Instant::now());
        }
        app.poll_bridge();
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
                if let Some(text) = app.copied.take() {
                    // OSC 52: the terminal puts it on the system clipboard.
                    term.backend_mut().write_all(d.wrap(format!("\x1b]52;c;{}\x07", BASE64.encode(text))).as_bytes())?;
                    term.backend_mut().flush()?;
                }
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
                Event::Mouse(m) if m.kind == MouseEventKind::ScrollUp => Some(app.wheel(true)),
                Event::Mouse(m) if m.kind == MouseEventKind::ScrollDown => Some(app.wheel(false)),
                // A click on the card itself leaves it be.
                Event::Mouse(m) if on_screen.as_ref().is_some_and(|c| c.area.contains((m.column, m.row).into())) => None,
                Event::Mouse(m) if app.view == View::Tree => {
                    app.mouse(m.kind, m.column, m.row);
                    Some(Action::None)
                }
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
                    Action::RunHere(h) => {
                        // Claude Code takes the terminal as office got it, and gives it back.
                        let back = term.backend_mut();
                        disable_raw_mode()?;
                        execute!(back, DisableMouseCapture, LeaveAlternateScreen, Show)?;
                        let r = bridge::resume_here(&app.claude, &h.config_dir, &h.cwd, &h.session);
                        enable_raw_mode()?;
                        execute!(back, EnterAlternateScreen, SetTitle("office"), EnableMouseCapture)?;
                        app.ran_here(&h, r);
                        full = true;
                    }
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

/// The projects directories to scan: --root, else the default ones.
/// `office hook permission|wait` (run by Claude Code) and `office hook
/// install|uninstall` (into every Claude Code config dir office watches).
fn hook_command(kind: &str) -> i32 {
    match kind {
        "permission" | "wait" => bridge::hook(kind, &bridge::dir()),
        "install" | "uninstall" => {
            let dirs: Vec<PathBuf> = scan::default_roots(&home()).iter().filter_map(|r| r.dir.parent().map(Path::to_path_buf)).collect();
            match bridge::manage(kind == "install", &dirs, &mut stdout()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("office: {e}");
                    1
                }
            }
        }
        _ => {
            eprintln!("usage: office hook install|uninstall   (permission and wait are run by Claude Code)");
            2
        }
    }
}

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
    let mut snap = Scanner::new(roots, args.since).scan(now());
    bridge::overlay(&mut snap, &bridge::dir());
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
    let mut snap = Scanner::new(roots(args), args.since).scan(now());
    bridge::overlay(&mut snap, &bridge::dir());
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
        let (mut s, bridge) = (Scanner::new(roots, since), bridge::dir());
        loop {
            // The hooks' requests make Waiting exact while office runs.
            let mut snap = s.scan(now());
            bridge::overlay(&mut snap, &bridge);
            if snaps.send(snap).is_err() {
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
    app.hooks = roots(&args).iter().filter_map(|r| r.dir.parent()).any(bridge::installed);
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
    bridge::leave(&app.bridge);
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
    // `office hook ...`: the Claude Code hooks of the prompt panel (see bridge).
    let argv: Vec<String> = env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("hook") {
        process::exit(hook_command(argv.get(1).map_or("", String::as_str)));
    }
    let args = match parse_args(argv.into_iter()) {
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

    /// Every cell of rects, once per rectangle holding it, sorted.
    pub(crate) fn cells(rects: &[CellRect]) -> Vec<(u16, u16)> {
        let mut out: Vec<_> = rects.iter().flat_map(|r| (r.y0..r.y1).flat_map(move |y| (r.x0..r.x1).map(move |x| (x, y)))).collect();
        out.sort();
        out
    }

    pub(crate) const NOW: i64 = 1000 * scan::SEC;

    /// An agent of session "s", the lead when its id is "s".
    pub(crate) fn agent(id: &str, status: scan::Status) -> scan::Agent {
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
            last_text: String::new(),
            config_dir: Default::default(),
            cwd: String::new(),
            asking: String::new(),
            turns: 1,
            usage: Usage::default(),
            model_usage: Default::default(),
            events: Vec::new(),
            children: Vec::new(),
            live: false,
            continued: false,
        }
    }

    /// The picture tests draw on: one that compresses as badly as a photo.
    pub(crate) fn noisy_office() -> RgbaImage {
        RgbaImage::from_fn(1254, 1254, |x, y| {
            let v = (x.wrapping_mul(7919) ^ y.wrapping_mul(104_729) ^ x.wrapping_mul(y)) as u8;
            [v, v.wrapping_mul(3), v.wrapping_mul(7), 255].into()
        })
    }

    /// A busy session (a lead, seven subagents, a spawn and a message) on a
    /// picture that compresses as badly as a photo.
    pub(crate) fn busy() -> (App, Cast, RgbaImage) {
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
}
