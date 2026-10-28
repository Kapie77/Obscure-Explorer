//! obscure-core: tipos comuns, acesso aos arquivos (pasta extraída hoje, HVP depois)
//! e a árvore virtual usada pelo explorador.

pub mod archive;
pub mod game;
pub mod hvp;
pub mod tree;

pub use archive::{Archive, DirArchive, Entry};
pub use game::GameArchive;
pub use hvp::HvpArchive;
pub use tree::DirNode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Game {
    Obscure1,
    Obscure2,
    FinalExam,
    Unknown,
}

impl Game {
    pub const ALL: [Game; 4] = [Game::Obscure1, Game::Obscure2, Game::FinalExam, Game::Unknown];

    pub fn label(self) -> &'static str {
        match self {
            Game::Obscure1 => "Obscure 1",
            Game::Obscure2 => "Obscure 2",
            Game::FinalExam => "Final Exam",
            Game::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    PcSteam,
    PcGog,
    PcRetail,
    Ps2,
    Psp,
    Xbox,
    Wii,
    Ps3,
    X360,
    Unknown,
}

impl Platform {
    pub const ALL: [Platform; 10] = [
        Platform::PcSteam,
        Platform::PcGog,
        Platform::PcRetail,
        Platform::Ps2,
        Platform::Psp,
        Platform::Xbox,
        Platform::Wii,
        Platform::Ps3,
        Platform::X360,
        Platform::Unknown,
    ];

    /// The three PC editions share the same file formats.
    pub fn is_pc(self) -> bool {
        matches!(self, Platform::PcSteam | Platform::PcGog | Platform::PcRetail)
    }

    pub fn label(self) -> &'static str {
        match self {
            Platform::PcSteam => "PC (Steam)",
            Platform::PcGog => "PC (GOG)",
            Platform::PcRetail => "PC (Retail)",
            Platform::Ps2 => "PS2",
            Platform::Psp => "PSP",
            Platform::Xbox => "Xbox",
            Platform::Wii => "Wii",
            Platform::Ps3 => "PS3",
            Platform::X360 => "Xbox 360",
            Platform::Unknown => "Unknown",
        }
    }
}

/// Contexto passado aos parsers: ajuda a desambiguar formatos que
/// compartilham extensão (ex.: `.hvt` do Wii vs. Final Exam).
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub game: Game,
    pub platform: Platform,
}

impl Default for Context {
    fn default() -> Self {
        Self { game: Game::Unknown, platform: Platform::Unknown }
    }
}
