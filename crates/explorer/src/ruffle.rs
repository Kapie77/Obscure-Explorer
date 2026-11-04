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
    vec![exe_dir().join("ruffle_path.txt"), PathBuf::from("ruffle_path.txt")]
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
    let dirs = [exe_dir(), exe_dir().join("ruffle"), PathBuf::from("."), PathBuf::from("ruffle")];
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
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let url = json["assets"]
        .as_array()
        .and_then(|assets| {
            assets.iter().find_map(|a| {
                let name = a["name"].as_str()?.to_ascii_lowercase();
                if name.ends_with(".zip") && name.contains("windows") && name.contains("x86_64") {
                    a["browser_download_url"].as_str().map(str::to_string)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| "no Windows build was found in the latest Ruffle release".to_string())?;

    let resp = ureq::get(&url)
        .set("User-Agent", "obscure-explorer")
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    if let Some(len) = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()) {
        total.store(len, Ordering::Relaxed);
    }
    let out_dir = dest_dir.join("ruffle");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let zip_path = out_dir.join("ruffle_download.zip");
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

    let archive_file =
        std::fs::File::open(&zip_path).map_err(|e| format!("cannot reopen the archive: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(archive_file).map_err(|e| format!("the download is not a valid zip: {e}"))?;
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
    let _ = std::fs::remove_file(&zip_path);
    let exe = out_dir.join(exe_name());
    if exe.is_file() {
        Ok(exe)
    } else {
        Err("ruffle.exe was not found inside the downloaded archive".to_string())
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
