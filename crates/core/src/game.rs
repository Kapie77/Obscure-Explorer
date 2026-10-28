//! A whole game folder: every .hvp archive found (each shown as a top-level folder named
//! after the file) plus all the loose files, merged into a single `Archive`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::archive::{Archive, DirArchive, Entry};
use crate::hvp::HvpArchive;
use crate::Game;

pub struct GameArchive {
    sources: Vec<Box<dyn Archive>>,
    entries: Vec<Entry>,
    /// global entry index -> (source index, index inside that source)
    map: Vec<(usize, usize)>,
    game: Option<Game>,
    warnings: Vec<String>,
    hvp_count: usize,
}

fn find_hvps(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for item in fs::read_dir(dir)? {
        let item = item?;
        let path = item.path();
        let meta = item.metadata()?;
        if meta.is_dir() {
            find_hvps(&path, out)?;
        } else if meta.is_file() {
            let is_hvp = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("hvp"))
                .unwrap_or(false);
            if is_hvp {
                out.push(path);
            }
        }
    }
    Ok(())
}

impl GameArchive {
    pub fn open(root: &Path, hashes_dir: &Path) -> io::Result<Self> {
        let mut hvps = Vec::new();
        find_hvps(root, &mut hvps)?;
        hvps.sort();

        let mut me = Self {
            sources: Vec::new(),
            entries: Vec::new(),
            map: Vec::new(),
            game: None,
            warnings: Vec::new(),
            hvp_count: 0,
        };

        for p in hvps {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            match HvpArchive::open(&p, hashes_dir) {
                Ok(a) => {
                    if me.game.is_none() {
                        me.game = Some(a.game());
                    }
                    if let Some(w) = a.warning() {
                        if !me.warnings.iter().any(|x| x == w) {
                            me.warnings.push(w.to_string());
                        }
                    }
                    me.hvp_count += 1;
                    me.add_source(Box::new(a), Some(&rel));
                }
                Err(e) => me.warnings.push(format!("{rel}: {e}")),
            }
        }

        // loose files (the .hvp files themselves are shown as folders instead)
        let loose = DirArchive::open_skipping(root, &["hvp"])?;
        me.add_source(Box::new(loose), None);
        Ok(me)
    }

    fn add_source(&mut self, src: Box<dyn Archive>, prefix: Option<&str>) {
        let sid = self.sources.len();
        for (local, e) in src.entries().iter().enumerate() {
            let path = match prefix {
                Some(p) => format!("{p}/{}", e.path),
                None => e.path.clone(),
            };
            self.entries.push(Entry { path, size: e.size });
            self.map.push((sid, local));
        }
        self.sources.push(src);
    }

    /// Game of the first .hvp that could be opened.
    pub fn game(&self) -> Option<Game> {
        self.game
    }

    pub fn hvp_count(&self) -> usize {
        self.hvp_count
    }

    /// Non-fatal problems (archives that failed to open, missing Obscure 2 name map, ...).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl Archive for GameArchive {
    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn read(&self, index: usize) -> io::Result<Vec<u8>> {
        let (src, local) = *self
            .map
            .get(index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "index out of range"))?;
        self.sources[src].read(local)
    }
}
