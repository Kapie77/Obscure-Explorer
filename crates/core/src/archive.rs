use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Entry {
    /// Caminho relativo, separado por '/'.
    pub path: String,
    /// 0 quando desconhecido (HVP: só se sabe ao ler a entrada).
    pub size: u64,
}

/// Fonte de arquivos. Hoje: pasta extraída (`DirArchive`).
/// Depois: um adaptador sobre o crate `hvp-archive` que lê direto do .hvp.
pub trait Archive {
    fn entries(&self) -> &[Entry];
    fn read(&self, index: usize) -> io::Result<Vec<u8>>;
}

/// Lê uma pasta já extraída (ex.: com `obscure-hvp extract game.hvp pasta`).
pub struct DirArchive {
    root: PathBuf,
    entries: Vec<Entry>,
}

impl DirArchive {
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        Self::open_skipping(root, &[])
    }

    /// Like `open`, but ignores files whose extension is in `skip_ext` (case-insensitive).
    pub fn open_skipping(root: impl AsRef<Path>, skip_ext: &[&str]) -> io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        let mut entries = Vec::new();
        walk(&root, &root, skip_ext, &mut entries)?;
        entries.sort_by_key(|e| e.path.to_lowercase());
        Ok(Self { root, entries })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn walk(root: &Path, dir: &Path, skip: &[&str], out: &mut Vec<Entry>) -> io::Result<()> {
    for item in fs::read_dir(dir)? {
        let item = item?;
        let path = item.path();
        let meta = item.metadata()?;
        if meta.is_dir() {
            walk(root, &path, skip, out)?;
        } else if meta.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if skip.iter().any(|s| s.eq_ignore_ascii_case(ext)) {
                    continue;
                }
            }
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(Entry { path: rel, size: meta.len() });
        }
    }
    Ok(())
}

impl Archive for DirArchive {
    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn read(&self, index: usize) -> io::Result<Vec<u8>> {
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "index out of range"))?;
        fs::read(self.root.join(&entry.path))
    }
}
