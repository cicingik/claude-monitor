//! The terminal: detecting its graphics protocol, encoding for it, counting bytes to it, restoring it.

use std::{
    cell::Cell,
    env,
    error::Error,
    fs::{File, OpenOptions},
    io::{self, Stdout, Write, stdout},
    process,
    rc::Rc,
};

use image::{DynamicImage, RgbaImage};
use ratatui::{
    crossterm::{
        cursor::Show,
        event::DisableMouseCapture,
        execute,
        terminal::{LeaveAlternateScreen, disable_raw_mode},
    },
    layout::Size,
};
use ratatui_image::{
    picker::{Picker, ProtocolType, cap_parser::QueryStdioOptions},
    protocol::{Protocol, halfblocks::Halfblocks, iterm2::Iterm2, kitty::Kitty, sixel::Sixel},
};

use crate::cli::Args;

/// Cell size in pixels assumed when the terminal does not report one.
const DEFAULT_FONT: (u16, u16) = (10, 20);

/// How frames reach the terminal.
pub(crate) struct Display {
    pub(crate) protocol: ProtocolType,
    pub(crate) font: (u16, u16),
    pub(crate) tmux: bool,
    pub(crate) kitty_id: u32,
}

impl Display {
    /// Asks the terminal for its graphics protocol and cell size, unless the
    /// flags already say. Call it in raw mode on the alternate screen, before
    /// reading events. A terminal that never answers leaves ratatui-image's
    /// reader thread on stdin, and it takes the first key press.
    pub(crate) fn detect(args: &Args) -> Result<Display, Box<dyn Error>> {
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
    pub(crate) fn encode(&self, img: RgbaImage, size: Size) -> Result<Protocol, ratatui_image::errors::Errors> {
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
    pub(crate) fn wrap(&self, seq: String) -> String {
        if self.tmux { format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b")) } else { seq }
    }
}

/// Stdout that counts what is written to it and, with OFFICE_VIEW_TRACE
/// set, appends it to that file too.
pub(crate) struct Counted {
    out: Stdout,
    pub(crate) bytes: Rc<Cell<u64>>,
    trace: Option<File>,
}

impl Counted {
    pub(crate) fn new() -> io::Result<Counted> {
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

pub(crate) fn restore(out: &mut impl Write) {
    let _ = disable_raw_mode();
    let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen, Show);
}
