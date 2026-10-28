//! Texture file formats. PC .dic/.dip live in dic.rs, Final Exam .hvt in finalexam.rs;
//! the pixel decoders are in pixel.rs and bc.rs. Ported from the reference tool
//! ObsCure-Texture-Editor (MIT).

use crate::{DecodeError, Texture, TextureFormat};
use obscure_core::{Context, Game};
use std::path::Path;

fn ext(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Obscure 1/2, PC: dicionário de texturas (R8G8B8A8, R5G6B5, R5G5B5A1).
pub struct PcDic;
/// Obscure 1, PC: dicionário de texturas (B8G8R8A8, B5G6R5, B5G5R5A1).
pub struct PcDip;
/// Final Exam: BGRA/BGRX/ARGB/DXT1/DXT3/DXT5 on PC, PS3 and Xbox 360.
pub struct FinalExamHvt;

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

pub static ALL: &[&dyn TextureFormat] = &[&PcDic, &PcDip, &FinalExamHvt, &StdImage];
