//! Adaptador sobre o crate `hvp-archive` (do obscure-hvp): abre .hvp direto, sem extrair.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use hvp_archive::archive::{Archive as HvpInner, Obscure2NameMap, Options};
use hvp_archive::provider::ArchiveProvider;
use self_cell::self_cell;

use crate::archive::{Archive, Entry};
use crate::Game;

// O `Archive` do hvp-archive empresta o `ArchiveProvider`; o self_cell guarda os dois juntos.
self_cell!(
    struct Loaded {
        owner: ArchiveProvider,
        #[not_covariant]
        dependent: HvpInner,
    }
);

pub struct HvpArchive {
    loaded: Loaded,
    entries: Vec<Entry>,
    game: Game,
    warning: Option<String>,
}

fn map_game(g: hvp_archive::Game) -> Game {
    match g {
        hvp_archive::Game::Obscure1 => Game::Obscure1,
        hvp_archive::Game::Obscure2 => Game::Obscure2,
        hvp_archive::Game::FinalExam => Game::FinalExam,
    }
}

/// Mesma lógica do CLI: junta as linhas de todos os `*.txt` da pasta `hashes`.
fn load_names(dir: &Path) -> io::Result<Option<Obscure2NameMap>> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut names = Vec::new();
    for entry in dir.read_dir()? {
        let path = entry?.path();
        if !path.is_file() || path.extension().unwrap_or_default() != "txt" {
            continue;
        }
        let reader = BufReader::new(File::open(path)?);
        for line in reader.lines() {
            names.push(line?);
        }
    }
    if names.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Obscure2NameMap::new(names)))
    }
}

impl HvpArchive {
    /// Abre o .hvp com autodetecção do jogo. `hashes_dir` só importa para o Obscure 2.
    pub fn open(path: &Path, hashes_dir: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("could not open file: {e}"))?;
        let provider =
            ArchiveProvider::new(file, None).map_err(|e| format!("{e} ({e:?})"))?;
        let raw_game = provider.game();

        let mut warning = None;
        let names = if raw_game == hvp_archive::Game::Obscure2 {
            match load_names(hashes_dir) {
                Ok(Some(n)) => n,
                Ok(None) => {
                    warning = Some(format!(
                        "names folder missing or empty ({}): Obscure 2 file names will be wrong",
                        hashes_dir.display()
                    ));
                    Obscure2NameMap::default()
                }
                Err(e) => {
                    warning = Some(format!("error reading names folder: {e}"));
                    Obscure2NameMap::default()
                }
            }
        } else {
            Obscure2NameMap::default()
        };

        let options = Options { obscure2_names: names, rebuild_skip_compression: false };
        let loaded = Loaded::new(provider, |p| HvpInner::new_with_options(p, options));

        let entries = loaded.with_dependent(|_owner, arc| {
            arc.files()
                .map(|f| Entry {
                    path: f
                        .path
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/"),
                    size: 0, // tamanho só é conhecido depois de ler a entrada
                })
                .collect::<Vec<Entry>>()
        });

        Ok(Self { loaded, entries, game: map_game(raw_game), warning })
    }

    pub fn game(&self) -> Game {
        self.game
    }

    /// Aviso não fatal (ex.: mapa de nomes do Obscure 2 não encontrado).
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }
}

impl Archive for HvpArchive {
    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// `index` é a posição na iteração de `files()`, a mesma ordem de `entries()`.
    fn read(&self, index: usize) -> io::Result<Vec<u8>> {
        self.loaded.with_dependent(|_owner, arc| -> io::Result<Vec<u8>> {
            let file = arc
                .files()
                .nth(index)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "index out of range"))?;
            let bytes = file
                .get_bytes()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            Ok(bytes.to_vec())
        })
    }
}
