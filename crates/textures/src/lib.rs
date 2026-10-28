//! obscure-textures: decodificação (somente leitura) das texturas dos jogos para RGBA8.

pub mod bc;
pub mod dic;
pub mod finalexam;
pub mod formats;
pub mod pixel;

use obscure_core::Context;
use std::fmt;

#[derive(Clone, Debug)]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// RGBA8, linha a linha, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Descrição do formato original (ex.: "DXT5", "PAL8 swizzled").
    pub format: String,
}

#[derive(Debug)]
pub enum DecodeError {
    Unsupported(String),
    Truncated { needed: usize, got: usize },
    Invalid(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Unsupported(s) => write!(f, "unsupported: {s}"),
            DecodeError::Truncated { needed, got } => {
                write!(f, "truncated data: need {needed} bytes, got {got}")
            }
            DecodeError::Invalid(s) => write!(f, "invalid data: {s}"),
        }
    }
}

impl std::error::Error for DecodeError {}

pub(crate) fn need(data: &[u8], needed: usize) -> Result<(), DecodeError> {
    if data.len() < needed {
        Err(DecodeError::Truncated { needed, got: data.len() })
    } else {
        Ok(())
    }
}

/// Um formato de arquivo de textura. Um arquivo pode conter várias texturas
/// (`.dic` e `.dip` são dicionários de texturas).
pub trait TextureFormat: Sync {
    fn name(&self) -> &'static str;
    fn matches(&self, path: &str, data: &[u8], ctx: &Context) -> bool;
    fn decode(&self, path: &str, data: &[u8], ctx: &Context) -> Result<Vec<Texture>, DecodeError>;
}

pub fn registry() -> &'static [&'static dyn TextureFormat] {
    formats::ALL
}

/// `None` se nenhum formato reconhece o arquivo.
pub fn decode_any(
    path: &str,
    data: &[u8],
    ctx: &Context,
) -> Option<Result<Vec<Texture>, DecodeError>> {
    registry()
        .iter()
        .find(|f| f.matches(path, data, ctx))
        .map(|f| f.decode(path, data, ctx))
}

/// Extensões que, em princípio, são texturas (usado para decidir se vale gerar miniatura).
pub fn is_texture_ext(ext: &str) -> bool {
    matches!(ext, "dic" | "dip" | "hvt" | "hvi" | "xbr")
}

/// Texture dictionaries holding several images (they behave like folders in the explorer).
pub fn is_pack_ext(ext: &str) -> bool {
    matches!(ext, "dic" | "dip")
}

/// Files holding a single image: standard formats and the single-texture game formats.
pub fn is_image_ext(ext: &str) -> bool {
    matches!(ext, "bmp" | "tga" | "png" | "jpg" | "jpeg" | "hvt" | "hvi" | "xbr")
}
