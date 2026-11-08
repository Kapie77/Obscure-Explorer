//! Texture file formats. PC .dic/.dip live in dic.rs, Final Exam .hvt in finalexam.rs, Xbox .xbr in xbox.rs;
//! the pixel decoders are in pixel.rs and bc.rs. Ported from ObsCure-Texture-Editor (MIT) and
//! obscure-texture-tool (Kapie77).

use crate::{DecodeError, Texture, TextureFormat};
use obscure_core::{Context, Game, Platform};
use std::path::Path;

fn ext(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Obscure 2, PC: dicionário de texturas (cabeçalho big-endian; pixels B8G8R8A8, R5G6B5, A1R5G5B5 little-endian).
pub struct PcDic;
/// Obscure 1, PC: dicionário de texturas (B8G8R8A8, B5G6R5, B5G5R5A1).
pub struct PcDip;
/// Final Exam: BGRA/BGRX/ARGB/DXT1/DXT3/DXT5 on PC, PS3 and Xbox 360.
pub struct FinalExamHvt;
/// Obscure 1, Xbox: `.xbr` texture files (A8R8G8B8, R5G6B5, A1R5G5B5, swizzled).
pub struct XboxXbr;
/// Obscure 2, Wii: `.dic` packs in GX formats (I8, IA8, RGB5A3, RGBA8, C4, C8, CMPR).
pub struct WiiDic;
/// Obscure, PSP: `.dic` packs (PAL4 / PAL8 / RGBA8888, swizzled in 16 x 8 byte blocks).
pub struct PspDic;
/// Obscure, PS2: `.dic` packs (RenderWare TXD with PAL4 / PAL8 / RGBA5551 / RGBA8888 pixels).
pub struct Ps2Dic;
/// PS2 / PSP: `.hvi` single textures (8-bit palette, linear).
pub struct HviTexture;
/// Obscure 2, Wii: single-texture `.hvt` (" IVH" header).
pub struct WiiHvt;

impl TextureFormat for PcDic {
    fn name(&self) -> &'static str {
        "Obscure .dic (PC)"
    }
    fn matches(&self, path: &str, _data: &[u8], ctx: &Context) -> bool {
        ext(path) == "dic" && ctx.platform.is_pc()
    }
    fn decode(&self, _path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        crate::dic::parse_pc_dic(data)
    }
}

impl TextureFormat for PcDip {
    fn name(&self) -> &'static str {
        "Obscure .dip (PC)"
    }
    fn matches(&self, path: &str, _data: &[u8], ctx: &Context) -> bool {
        ext(path) == "dip" && ctx.game == Game::Obscure1 && ctx.platform.is_pc()
    }
    fn decode(&self, _path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        crate::dic::parse_dip(data)
    }
}

impl TextureFormat for FinalExamHvt {
    fn name(&self) -> &'static str {
        "Final Exam .hvt"
    }
    fn matches(&self, path: &str, _data: &[u8], ctx: &Context) -> bool {
        // the platform (PC / PS3 / X360) is read from the file header itself
        ext(path) == "hvt" && ctx.game == Game::FinalExam
    }
    fn decode(&self, path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        crate::finalexam::parse(path, data)
    }
}

impl TextureFormat for WiiDic {
    fn name(&self) -> &'static str {
        "Obscure .dic (Wii)"
    }
    fn matches(&self, path: &str, data: &[u8], ctx: &Context) -> bool {
        // the content is checked too, so a PC pack is never read as Wii data
        ext(path) == "dic" && (ctx.platform == Platform::Wii || crate::wii::looks_like_wii_dic(data))
    }
    fn decode(&self, _path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        crate::wii::parse_wii_dic(data)
    }
}

impl TextureFormat for PspDic {
    fn name(&self) -> &'static str {
        "Obscure .dic (PSP)"
    }
    fn matches(&self, path: &str, data: &[u8], ctx: &Context) -> bool {
        ext(path) == "dic" && (ctx.platform == Platform::Psp || crate::psp::looks_like_psp_dic(data))
    }
    fn decode(&self, _path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        crate::psp::parse_psp_dic(data)
    }
}

impl TextureFormat for Ps2Dic {
    fn name(&self) -> &'static str {
        "Obscure .dic (PS2)"
    }
    fn matches(&self, path: &str, data: &[u8], ctx: &Context) -> bool {
        ext(path) == "dic" && (ctx.platform == Platform::Ps2 || crate::ps2::looks_like_ps2_dic(data))
    }
    fn decode(&self, _path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        crate::ps2::parse_ps2_dic(data)
    }
}

impl TextureFormat for HviTexture {
    fn name(&self) -> &'static str {
        "Obscure .hvi (PS2 / PSP)"
    }
    fn matches(&self, path: &str, data: &[u8], _ctx: &Context) -> bool {
        ext(path) == "hvi" && crate::hvi::looks_like_hvi(data)
    }
    fn decode(&self, path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        let file = path.rsplit('/').next().unwrap_or(path);
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        crate::hvi::parse_hvi(stem, data)
    }
}

impl TextureFormat for WiiHvt {
    fn name(&self) -> &'static str {
        "Obscure .hvt (Wii)"
    }
    fn matches(&self, path: &str, data: &[u8], ctx: &Context) -> bool {
        ext(path) == "hvt" && ctx.game != Game::FinalExam && crate::wii::looks_like_wii_hvt(data)
    }
    fn decode(&self, path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        let file = path.rsplit('/').next().unwrap_or(path);
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        crate::wii::parse_wii_hvt(stem, data)
    }
}

impl TextureFormat for XboxXbr {
    fn name(&self) -> &'static str {
        "Obscure .xbr (Xbox)"
    }
    fn matches(&self, path: &str, _data: &[u8], _ctx: &Context) -> bool {
        // .xbr only exists on the Xbox version
        ext(path) == "xbr"
    }
    fn decode(&self, _path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        crate::xbox::parse_xbr(data)
    }
}

/// Plain BMP / TGA / PNG files (any game, any platform).
pub struct StdImage;

impl TextureFormat for StdImage {
    fn name(&self) -> &'static str {
        "BMP/TGA/PNG/JPEG image"
    }
    fn matches(&self, path: &str, _data: &[u8], _ctx: &Context) -> bool {
        matches!(ext(path).as_str(), "bmp" | "tga" | "png" | "jpg" | "jpeg")
    }
    fn decode(&self, path: &str, data: &[u8], _ctx: &Context) -> Result<Vec<Texture>, DecodeError> {
        let e = ext(path);
        let fmt = match e.as_str() {
            "bmp" => image::ImageFormat::Bmp,
            "tga" => image::ImageFormat::Tga,
            "png" => image::ImageFormat::Png,
            "jpg" | "jpeg" => image::ImageFormat::Jpeg,
            other => return Err(DecodeError::Unsupported(format!("image extension '{other}'"))),
        };
        let img = image::load_from_memory_with_format(data, fmt)
            .map_err(|err| DecodeError::Invalid(err.to_string()))?;
        let rgba = img.into_rgba8();
        let (width, height) = rgba.dimensions();
        let name = Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("image")
            .to_string();
        Ok(vec![Texture {
            name,
            width,
            height,
            rgba: rgba.into_raw(),
            format: format!("{} image", e.to_uppercase()),
        }])
    }
}

pub static ALL: &[&dyn TextureFormat] =
    &[&WiiDic, &PspDic, &Ps2Dic, &PcDic, &PcDip, &HviTexture, &WiiHvt, &FinalExamHvt, &XboxXbr, &StdImage];
