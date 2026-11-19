//! Custom character sprites from a directory of .gif and .png files, picked
//! per agent: the file named after the agent
//! type (general-purpose.gif, session.png, ...), else one of the files not
//! named after a type seen so far, by a hash of the agent id, else the
//! fallback character (the built-in assets/character.gif).

use std::{collections::HashSet, path::Path, time::Duration};

use image::RgbaImage;

use crate::scene::Sprite;

pub struct Cast {
    /// What to say about the sprites and the pet when the run ends, "" when
    /// there is nothing to say.
    pub note: String,
    /// The office pet, strolling round the lobby: not an agent, never picked.
    pub pet: Option<Sprite>,
    files: Vec<(String, Sprite)>, // file stem as written, sorted ignoring case
    fallback: Sprite,
    seen: HashSet<String>, // lower-case agent types seen so far
}

impl Cast {
    /// Loads every sprite in dir. It never fails: unreadable or broken files
    /// are skipped and summed up in the note. `explicit` says the user named
    /// dir, so a missing or empty one deserves a note.
    pub fn load(dir: &Path, explicit: bool, fallback: Option<Sprite>) -> Cast {
        let mut c = Cast { note: String::new(), pet: None, files: Vec::new(), fallback: fallback.unwrap_or_else(builtin), seen: HashSet::new() };
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                if explicit {
                    c.note = format!("sprites: cannot read {}, using the default character", dir.display());
                }
                return c;
            }
        };
        let mut bad = Vec::new();
        for e in entries.flatten() {
            let path = e.path();
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if e.file_type().is_ok_and(|t| t.is_dir()) || (ext != "gif" && ext != "png") {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            match Sprite::load(&path) {
                Ok(s) => c.files.push((path.file_stem().unwrap_or_default().to_string_lossy().into_owned(), s)),
                Err(_) => bad.push(name),
            }
        }
        c.files.sort_by_key(|f| f.0.to_lowercase());
        bad.sort();
        c.note = match (bad.len(), c.files.len()) {
            (1.., n) => format!("sprites: {n} loaded, {} skipped ({})", bad.len(), bad.join(", ")),
            (0, 1..) => format!("sprites: {} from {}", c.files.len(), dir.display()),
            _ if explicit => format!("sprites: no .gif or .png in {}", dir.display()),
            _ => String::new(),
        };
        c
    }

    /// Every sprite, by the index `pick` returns.
    pub fn sprite(&self, i: usize) -> &Sprite {
        self.files.get(i).map_or(&self.fallback, |(_, s)| s)
    }

    /// The name an agent drawn with sprite i goes by: its file name without
    /// the extension, "character" for the default one.
    pub fn name(&self, i: usize) -> &str {
        self.files.get(i).map_or("character", |(stem, _)| stem)
    }

    pub fn len(&self) -> usize {
        self.files.len() + 1
    }

    /// The sprite index of each (agent id, agent type) of one scene. Types
    /// are remembered, so a file named after a type never joins the pool.
    pub fn pick<'a>(&mut self, actors: impl Iterator<Item = (&'a str, &'a str)> + Clone) -> Vec<usize> {
        for (_, t) in actors.clone() {
            if !t.is_empty() {
                self.seen.insert(t.to_lowercase());
            }
        }
        let pool: Vec<usize> = (0..self.files.len()).filter(|&i| !self.seen.contains(&self.files[i].0.to_lowercase())).collect();
        actors
            .map(|(id, t)| {
                let t = t.to_lowercase();
                match self.files.iter().position(|(stem, _)| !t.is_empty() && stem.to_lowercase() == t) {
                    Some(i) => i,
                    None if pool.is_empty() => self.files.len(),
                    None => pool[fnv32a(id) as usize % pool.len()],
                }
            })
            .collect()
    }
}

/// FNV-1a, 32 bits: Go's hash/fnv New32a, so agents pick what they pick in Go.
pub fn fnv32a(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5, |h: u32, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// A plain 12×16 figure for when there is no character file at all.
fn builtin() -> Sprite {
    const ART: [&str; 16] = [
        "....hhhh....", "...hhhhhh...", "...hssssh...", "...ssssss...", "...sksskss..", "...ssssss...", "....ssss....", "..bbbbbbbb..",
        ".bbbbbbbbbb.", ".sbbbbbbbbs.", ".sbbbbbbbbs.", "..bbbbbbbb..", "...pppppp...", "...pp..pp...", "...pp..pp...", "..kkk..kkk..",
    ];
    let img = RgbaImage::from_fn(12, 16, |x, y| {
        let rgb = match ART[y as usize].as_bytes()[x as usize] {
            b'h' => [0x3b, 0x2a, 0x20],
            b's' => [0xf1, 0xc2, 0x9a],
            b'k' => [0x22, 0x22, 0x2a],
            b'b' => [0x4f, 0x7c, 0xd6],
            b'p' => [0x3a, 0x3f, 0x55],
            _ => return [0, 0, 0, 0].into(),
        };
        [rgb[0], rgb[1], rgb[2], 255].into()
    });
    Sprite::from_frames(vec![(img, Duration::from_millis(100))]).expect("the built-in figure has pixels")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_matches_go() {
        // hash/fnv New32a of "" and "a".
        assert_eq!(fnv32a(""), 0x811c_9dc5);
        assert_eq!(fnv32a("a"), 0xe40c_292c);
    }

    #[test]
    fn picks_type_file_then_pool_then_fallback() {
        let dir = std::env::temp_dir().join(format!("office-cast-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let px = RgbaImage::from_pixel(2, 3, [1, 2, 3, 255].into());
        for n in ["Explore.png", "zed.png", "amy.png"] {
            px.save(dir.join(n)).unwrap();
        }
        std::fs::write(dir.join("broken.gif"), b"no").unwrap();
        let mut c = Cast::load(&dir, true, None);
        assert_eq!(c.note, "sprites: 3 loaded, 1 skipped (broken.gif)");
        let picks = c.pick([("a1", "Explore"), ("a2", "general-purpose"), ("s", "session")].into_iter());
        assert_eq!(picks[0], 1, "explore.png, by its type");
        assert!(picks[1] == 0 || picks[1] == 2, "from the pool: {picks:?}");
        assert_eq!(picks[1], [0, 2][fnv32a("a2") as usize % 2]);

        let mut empty = Cast::load(&dir.join("missing"), false, None);
        assert_eq!(empty.note, "");
        assert_eq!(empty.pick([("a", "x")].into_iter()), [0], "only the fallback");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
