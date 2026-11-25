//! Command-line flags: the usage text and parsing argv into Args.

use std::path::PathBuf;

use ratatui_image::picker::ProtocolType;

use crate::{format, scan};

const USAGE: &str = "usage: office [flags]

  --root DIR   a Claude Code projects directory; repeat for more (default
               ~/.claude/projects and every ~/.claude-*/projects)
  --since DUR  only sessions whose transcript changed within DUR (default 24h, 0 = all)
  --sprites DIR  character .gif/.png files named after agent types, or any name
               (default $XDG_CONFIG_HOME/claude-monitor/sprites, else ~/.config/claude-monitor/sprites)
  --office DIR  the office picture DIR/office.png, with its layout DIR/office.json
               (without one, the built-in layout); default claude-monitor/office in
               the same config directory when it holds an office.png, else the built-in office
  --dump      scan once, print the tree, edges and usage as text, exit
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
      ↑/↓ k/j tab wheel select · enter open in tree · p open its session in Claude Code · v tree view
      r rescan · q esc ctrl+c quit; in the tree: enter/space collapse, tab 1 2 3 tabs

session panel (under the tree): the selected session's conversation; p opens
      Claude Code's own terminal for it there (claude attach), esc detaches.
      Exact waiting status needs a Claude Code hook, once: office hook install
      (office hook uninstall removes it)

env: OFFICE_VIEW_TRACE=FILE  also append every byte written to the terminal to FILE
      (not the queries that detect the protocol and cell size)";

pub(crate) struct Args {
    pub(crate) protocol: Option<ProtocolType>, // None: auto
    pub(crate) font: Option<(u16, u16)>,
    pub(crate) roots: Vec<PathBuf>,      // empty: ~/.claude/projects and every ~/.claude-*/projects
    pub(crate) since: i64,               // nanoseconds, 0 = all
    pub(crate) sprites: Option<PathBuf>, // None: the default directory
    pub(crate) office: Option<PathBuf>,  // None: the default directory if it holds a picture, else built in
    pub(crate) dump: bool,
    pub(crate) snapshot: Option<PathBuf>,
    pub(crate) offline: bool, // no requests for the accounts' limits
}

pub(crate) fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args {
        protocol: None,
        font: None,
        roots: Vec::new(),
        since: 24 * 3600 * scan::SEC,
        sprites: None,
        office: None,
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
            "--office" => args.office = Some(value()?.into()),
            "--dump" => args.dump = true,
            "--snapshot" => args.snapshot = Some(value()?.into()),
            "--offline" => args.offline = true,
            _ if name.starts_with('-') && name != "-" => return Err(format!("unknown flag {name}\n\n{USAGE}")),
            _ => return Err(format!("unexpected argument {a:?}\n\n{USAGE}")),
        }
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

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
