//! Claude Code's own terminal, inside the session panel: `claude attach <job>`
//! of a background session, run in a pseudo-terminal whose screen vt100
//! keeps and tui-term draws. Every key but esc goes to it; esc detaches,
//! dropping the Embed, which ends the attach. Ending it never ends the
//! session: a background session runs on without a viewer.

use std::{
    cell::Cell,
    io::{self, Read, Write},
    process,
    sync::{Arc, Mutex},
    thread,
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::Rect,
};
use tui_term::widget::PseudoTerminal;

pub struct Embed {
    pub session: String, // the session it shows, as the scan names it
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: Cell<(u16, u16)>, // rows, columns, as last drawn
}

impl Embed {
    /// Runs cmd (claude, set up as bridge::claude sets it up) in a new
    /// pseudo-terminal of rows × cols.
    pub fn spawn(session: &str, cmd: process::Command, rows: u16, cols: u16) -> io::Result<Embed> {
        let pty = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(other)?;
        let mut b = CommandBuilder::new(cmd.get_program());
        b.args(cmd.get_args());
        if let Some(cwd) = cmd.get_current_dir() {
            b.cwd(cwd);
        }
        for (k, v) in cmd.get_envs() {
            match v {
                Some(v) => b.env(k, v),
                None => b.env_remove(k),
            }
        }
        b.env("TERM", "xterm-256color");
        b.env("COLORTERM", "truecolor");
        let child = pty.slave.spawn_command(b).map_err(other)?;
        drop(pty.slave); // the child holds it: its exit then ends the reads
        let mut reader = pty.master.try_clone_reader().map_err(other)?;
        let writer = pty.master.take_writer().map_err(other)?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let feed = parser.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                feed.lock().expect("never poisoned: process only parses").process(&buf[..n]);
            }
        });
        Ok(Embed { session: session.into(), parser, writer, master: pty.master, child, size: Cell::new((rows, cols)) })
    }

    /// Whether claude still runs.
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// What the screen shows, trimmed: what claude said if it exited at once.
    pub fn said(&self) -> String {
        let text = self.parser.lock().expect("never poisoned").screen().contents();
        text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · ")
    }

    /// Draws the screen into area, sizing the terminal to it first.
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let size = (area.height.max(1), area.width.max(1));
        if size != self.size.get() {
            self.size.set(size);
            let _ = self.master.resize(PtySize { rows: size.0, cols: size.1, pixel_width: 0, pixel_height: 0 });
            self.parser.lock().expect("never poisoned").screen_mut().set_size(size.0, size.1);
        }
        let parser = self.parser.lock().expect("never poisoned");
        f.render_widget(PseudoTerminal::new(parser.screen()), area);
    }

    /// Sends a key press, as an xterm sends it.
    pub fn key(&mut self, k: KeyEvent) {
        let app_cursor = self.parser.lock().expect("never poisoned").screen().application_cursor();
        let bytes = encode(k, app_cursor);
        let _ = self.writer.write_all(&bytes).and_then(|_| self.writer.flush());
    }

    /// Sends a paste, bracketed when claude asked for that.
    pub fn paste(&mut self, text: &str) {
        let bracketed = self.parser.lock().expect("never poisoned").screen().bracketed_paste();
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let body = if bracketed { format!("\x1b[200~{text}\x1b[201~") } else { text };
        let _ = self.writer.write_all(body.as_bytes()).and_then(|_| self.writer.flush());
    }

    /// Sends a wheel turn, as SGR mouse reports when claude asked for them,
    /// else as arrow keys.
    pub fn wheel(&mut self, up: bool) {
        let mouse = self.parser.lock().expect("never poisoned").screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None;
        let (rows, cols) = self.size.get();
        let body = match (mouse, up) {
            (true, _) => format!("\x1b[<{};{};{}M", if up { 64 } else { 65 }, cols / 2 + 1, rows / 2 + 1),
            (false, true) => "\x1b[A".repeat(3),
            (false, false) => "\x1b[B".repeat(3),
        };
        let _ = self.writer.write_all(body.as_bytes()).and_then(|_| self.writer.flush());
    }
}

impl Drop for Embed {
    /// Ends the attach (the session itself runs on).
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// portable-pty's errors are anyhow's: only their text is kept.
fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// The bytes an xterm sends for k; app_cursor: the arrows in application mode.
pub fn encode(k: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let (ctrl, alt, shift) = (k.modifiers.contains(KeyModifiers::CONTROL), k.modifiers.contains(KeyModifiers::ALT), k.modifiers.contains(KeyModifiers::SHIFT));
    // xterm's modifier parameter: 1 + shift + 2 alt + 4 ctrl.
    let m = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let csi = |end: char| if m > 1 { format!("\x1b[1;{m}{end}") } else if app_cursor { format!("\x1bO{end}") } else { format!("\x1b[{end}") };
    let tilde = |n: u8| if m > 1 { format!("\x1b[{n};{m}~") } else { format!("\x1b[{n}~") };
    let s = match k.code {
        KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => ((c.to_ascii_lowercase() as u8 - b'a' + 1) as char).to_string(),
        KeyCode::Char(c) if ctrl => match c {
            ' ' | '@' | '2' => "\0".into(),
            '[' | '3' => "\x1b".into(),
            '\\' | '4' => "\x1c".into(),
            ']' | '5' => "\x1d".into(),
            '^' | '6' => "\x1e".into(),
            '_' | '-' | '7' => "\x1f".into(),
            c => c.to_string(),
        },
        KeyCode::Char(c) => c.to_string(),
        // Shift+enter: a new line in Claude Code's prompt, as alt+enter is.
        KeyCode::Enter if shift || alt => return b"\x1b\r".to_vec(),
        KeyCode::Enter => "\r".into(),
        KeyCode::Tab => "\t".into(),
        KeyCode::BackTab => "\x1b[Z".into(),
        KeyCode::Backspace => "\x7f".into(),
        KeyCode::Esc => "\x1b".into(),
        KeyCode::Up => return csi('A').into_bytes(),
        KeyCode::Down => return csi('B').into_bytes(),
        KeyCode::Right => return csi('C').into_bytes(),
        KeyCode::Left => return csi('D').into_bytes(),
        KeyCode::Home => return csi('H').into_bytes(),
        KeyCode::End => return csi('F').into_bytes(),
        KeyCode::Insert => return tilde(2).into_bytes(),
        KeyCode::Delete => return tilde(3).into_bytes(),
        KeyCode::PageUp => return tilde(5).into_bytes(),
        KeyCode::PageDown => return tilde(6).into_bytes(),
        KeyCode::F(n @ 1..=4) => return format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        KeyCode::F(n @ 5..=12) => return tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]).into_bytes(),
        _ => return Vec::new(),
    };
    if alt { format!("\x1b{s}").into_bytes() } else { s.into_bytes() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, m: KeyModifiers) -> Vec<u8> {
        encode(KeyEvent::new(code, m), false)
    }

    #[test]
    fn keys_go_as_an_xterm_sends_them() {
        let none = KeyModifiers::NONE;
        assert_eq!(key(KeyCode::Char('a'), none), b"a");
        assert_eq!(key(KeyCode::Char('c'), KeyModifiers::CONTROL), b"\x03");
        assert_eq!(key(KeyCode::Char('z'), KeyModifiers::CONTROL), b"\x1a");
        assert_eq!(key(KeyCode::Char('b'), KeyModifiers::ALT), b"\x1bb");
        assert_eq!(key(KeyCode::Enter, none), b"\r");
        assert_eq!(key(KeyCode::Enter, KeyModifiers::SHIFT), b"\x1b\r");
        assert_eq!(key(KeyCode::Esc, none), b"\x1b");
        assert_eq!(key(KeyCode::Up, none), b"\x1b[A");
        assert_eq!(encode(KeyEvent::new(KeyCode::Up, none), true), b"\x1bOA");
        assert_eq!(key(KeyCode::Left, KeyModifiers::CONTROL), b"\x1b[1;5D");
        assert_eq!(key(KeyCode::Delete, none), b"\x1b[3~");
        assert_eq!(key(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z");
    }
}
