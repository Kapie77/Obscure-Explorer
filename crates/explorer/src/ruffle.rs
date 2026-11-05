//! Ruffle (a free, open-source Flash emulator) plays `.swf` files in its own window.
//! It is not bundled: it can be downloaded once (Windows) or located by the user.

use eframe::egui;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

#[derive(Clone)]
pub struct Ruffle {
    path: PathBuf,
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "ruffle.exe"
    } else {
        "ruffle"
    }
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn config_paths() -> Vec<PathBuf> {
    vec![
        crate::media::tools_dir().join("ruffle_path.txt"),
        exe_dir().join("ruffle_path.txt"),
        PathBuf::from("ruffle_path.txt"),
    ]
}

/// The automatic download exists for Windows and Linux builds of Ruffle.
pub fn download_supported() -> bool {
    cfg!(windows) || cfg!(target_os = "linux")
}

/// The release asset of Ruffle for this system: a .zip on Windows, a .tar.gz on Linux.
fn asset_matches(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if cfg!(windows) {
        n.ends_with(".zip") && n.contains("windows") && n.contains("x86_64")
    } else if cfg!(target_os = "linux") {
        let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
        n.ends_with(".tar.gz") && n.contains("linux") && n.contains(arch)
    } else {
        false
    }
}

fn parse_octal(field: &[u8]) -> u64 {
    field
        .iter()
        .take_while(|b| **b != 0 && **b != b' ')
        .fold(0u64, |acc, b| acc * 8 + (b.wrapping_sub(b'0') as u64 & 7))
}

/// Unpacks a .tar.gz into `out_dir` (files only, flat: just the file names are used).
fn extract_tar_gz(archive: &Path, out_dir: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("cannot reopen the archive: {e}"))?;
    let mut gz = flate2::read::GzDecoder::new(file);
    let mut block = [0u8; 512];
    let mut long_name: Option<String> = None;
    loop {
        match gz.read_exact(&mut block) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("cannot read the archive: {e}")),
        }
        if block.iter().all(|b| *b == 0) {
            break;
        }
        let size = parse_octal(&block[124..136]);
        let typeflag = block[156];
        let padded = size.div_ceil(512) * 512;
        let name_end = block[0..100].iter().position(|b| *b == 0).unwrap_or(100);
        let mut name = String::from_utf8_lossy(&block[0..name_end]).into_owned();
        if &block[257..262] == b"ustar" {
            let pre_end = block[345..500].iter().position(|b| *b == 0).unwrap_or(155);
            if pre_end > 0 {
                name = format!("{}/{}", String::from_utf8_lossy(&block[345..345 + pre_end]), name);
            }
        }
        match typeflag {
            b'L' => {
                let mut data = Vec::new();
                (&mut gz).take(size).read_to_end(&mut data).map_err(|e| format!("archive error: {e}"))?;
                let skip = padded - size;
                std::io::copy(&mut (&mut gz).take(skip), &mut std::io::sink()).map_err(|e| format!("archive error: {e}"))?;
                long_name = Some(String::from_utf8_lossy(&data).trim_end_matches('\0').to_string());
            }
            b'0' | 0 => {
                let full = long_name.take().unwrap_or(name);
                let target = Path::new(&full).file_name().map(|n| out_dir.join(n));
                match target {
                    Some(path) if size > 0 || !full.ends_with('/') => {
                        let mut out = std::fs::File::create(&path)
                            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                        std::io::copy(&mut (&mut gz).take(size), &mut out)
                            .map_err(|e| format!("cannot unpack {}: {e}", path.display()))?;
                        drop(out);
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let mode = if path.file_name().map(|n| n == "ruffle").unwrap_or(false) { 0o755 } else { 0o644 };
                            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode));
                        }
                        std::io::copy(&mut (&mut gz).take(padded - size), &mut std::io::sink())
                            .map_err(|e| format!("archive error: {e}"))?;
                    }
                    _ => {
                        std::io::copy(&mut (&mut gz).take(padded), &mut std::io::sink())
                            .map_err(|e| format!("archive error: {e}"))?;
                    }
                }
            }
            _ => {
                long_name = None;
                std::io::copy(&mut (&mut gz).take(padded), &mut std::io::sink())
                    .map_err(|e| format!("archive error: {e}"))?;
            }
        }
    }
    Ok(())
}

/// Accepts a user-chosen file if it looks like Ruffle.
pub fn ruffle_from_path(path: PathBuf) -> Option<Ruffle> {
    let named_ruffle = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase().contains("ruffle"))
        .unwrap_or(false);
    if path.is_file() && named_ruffle {
        Some(Ruffle { path })
    } else {
        None
    }
}

/// Remembers the location (a small text file next to the program).
pub fn save_ruffle_path(r: &Ruffle) {
    if let Some(p) = config_paths().into_iter().next() {
        let _ = std::fs::write(p, r.path.to_string_lossy().as_bytes());
    }
}

/// Saved path, next to the program (also in a `ruffle` folder), current directory, then PATH.
pub fn find_ruffle() -> Option<Ruffle> {
    for c in config_paths() {
        if let Ok(t) = std::fs::read_to_string(c) {
            if let Some(r) = ruffle_from_path(PathBuf::from(t.trim())) {
                return Some(r);
            }
        }
    }
    let tools = crate::media::tools_dir();
    let dirs = [
        exe_dir(),
        exe_dir().join("ruffle"),
        tools.clone(),
        tools.join("ruffle"),
        PathBuf::from("."),
        PathBuf::from("ruffle"),
    ];
    for d in dirs {
        if let Some(r) = ruffle_from_path(d.join(exe_name())) {
            return Some(r);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&paths) {
            if let Some(r) = ruffle_from_path(d.join(exe_name())) {
                return Some(r);
            }
        }
    }
    None
}

impl Ruffle {
    /// Opens the file in a Ruffle window (it keeps running on its own).
    pub fn open(&self, swf: &Path) -> std::io::Result<()> {
        Command::new(&self.path)
            .arg(swf)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    }
}

/// Downloads the latest Windows build of Ruffle from its GitHub releases in a background thread.
pub struct RuffleDownload {
    rx: Receiver<Result<PathBuf, String>>,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

fn download_ruffle(dest_dir: &Path, done: &AtomicU64, total: &AtomicU64) -> Result<PathBuf, String> {
    const API: &str = "https://api.github.com/repos/ruffle-rs/ruffle/releases/latest";
    let body = ureq::get(API)
        .set("User-Agent", "obscure-explorer")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not ask GitHub for the latest Ruffle: {e}"))?
        .into_string()
        .map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let json = obscure_xmc::json::parse(&body)
        .ok_or_else(|| "unexpected answer from GitHub".to_string())?;
    let url = json
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|a| {
                let name = a.get("name")?.as_str()?.to_ascii_lowercase();
                if asset_matches(&name) {
                    a.get("browser_download_url")?.as_str().map(str::to_string)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| "no build of Ruffle for this system was found in the latest release".to_string())?;

    let resp = ureq::get(&url)
        .set("User-Agent", "obscure-explorer")
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    if let Some(len) = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()) {
        total.store(len, Ordering::Relaxed);
    }
    let out_dir = dest_dir.join("ruffle");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let is_tar = url.ends_with(".tar.gz");
    let zip_path = out_dir.join(if is_tar { "ruffle_download.tar.gz" } else { "ruffle_download.zip" });
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&zip_path)
        .map_err(|e| format!("cannot write {}: {e}", zip_path.display()))?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("cannot write the archive: {e}"))?;
        done.fetch_add(n as u64, Ordering::Relaxed);
    }
    drop(file);

    if is_tar {
        extract_tar_gz(&zip_path, &out_dir)?;
    } else {
        let archive_file =
            std::fs::File::open(&zip_path).map_err(|e| format!("cannot reopen the archive: {e}"))?;
        let mut archive = zip::ZipArchive::new(archive_file)
            .map_err(|e| format!("the download is not a valid zip: {e}"))?;
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).map_err(|e| format!("zip error: {e}"))?;
            if f.is_dir() {
                continue;
            }
            // only the file name is used, so nothing can be written outside the folder
            let Some(name) = Path::new(f.name()).file_name().map(|n| n.to_owned()) else { continue };
            let out_path = out_dir.join(name);
            let mut out = std::fs::File::create(&out_path)
                .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
            std::io::copy(&mut f, &mut out).map_err(|e| format!("cannot unpack Ruffle: {e}"))?;
        }
    }
    let _ = std::fs::remove_file(&zip_path);
    let exe = out_dir.join(exe_name());
    if exe.is_file() {
        Ok(exe)
    } else {
        Err(format!("{} was not found inside the downloaded archive", exe_name()))
    }
}

impl RuffleDownload {
    pub fn start(dest_dir: PathBuf, ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let done = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let (d, t) = (done.clone(), total.clone());
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let res = download_ruffle(&dest_dir, &d, &t);
            let _ = tx.send(res);
            ctx2.request_repaint();
        });
        Self { rx, done, total }
    }

    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed);
        if t == 0 {
            0.0
        } else {
            (self.done.load(Ordering::Relaxed) as f64 / t as f64).min(1.0) as f32
        }
    }

    pub fn done_mb(&self) -> f64 {
        self.done.load(Ordering::Relaxed) as f64 / 1_048_576.0
    }

    /// Some(result) once the download has finished.
    pub fn poll(&self) -> Option<Result<PathBuf, String>> {
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_sizes() {
        assert_eq!(parse_octal(b"0000644\0\0\0\0\0"), 0o644);
        assert_eq!(parse_octal(b"00000000012 "), 10);
    }

    /// One 512-byte tar header + data, as tar writes it.
    fn tar_file(name: &str, data: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        let size = format!("{:011o}", data.len());
        h[124..135].copy_from_slice(size.as_bytes());
        h[156] = b'0';
        let mut out = h.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
        out
    }

    #[test]
    fn extracts_files_flat_and_marks_ruffle_executable() {
        use std::io::Write;
        let mut tar = tar_file("dir/ruffle", b"binary");
        tar.extend(tar_file("dir/README.md", b"hello"));
        tar.extend([0u8; 1024]);
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&tar).unwrap();
        let gz = enc.finish().unwrap();
        let base = std::env::temp_dir().join(format!("obscure_tar_test_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let archive = base.join("a.tar.gz");
        std::fs::write(&archive, gz).unwrap();
        extract_tar_gz(&archive, &base).unwrap();
        assert_eq!(std::fs::read(base.join("ruffle")).unwrap(), b"binary");
        assert_eq!(std::fs::read(base.join("README.md")).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(base.join("ruffle")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
