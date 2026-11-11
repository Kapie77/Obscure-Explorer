//! Audio playback (rodio) and video support through an external ffmpeg binary.
//! Bink (.bik) has no Rust decoder, so thumbnails and playback shell out to ffmpeg.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use obscure_core::Archive;
use rodio::buffer::SamplesBuffer;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

/// `xwma` (Final Exam) and `at3` (PSP, ATRAC3) have no Rust decoder: they are decoded through ffmpeg.
/// `wsi` (Wii) is DSP-ADPCM in a small container, decoded by the program itself (see wsi.rs).
pub const AUDIO_EXTS: &[&str] = &["wav", "ogg", "mp3", "flac", "xwma", "at3", "wsi"];
/// `pmf` is the PSP movie format (an MPEG program stream behind a PSMF header).
pub const VIDEO_EXTS: &[&str] = &["bik", "ogv", "pmf"];

/// Audio formats that always go through ffmpeg.
pub fn audio_needs_ffmpeg(ext: &str) -> bool {
    matches!(ext, "xwma" | "at3")
}

/// What ffmpeg should be given for a video file: the extension to use for the temporary file and the
/// bytes. PSP movies start with a PSMF header in front of the MPEG program stream, which ffmpeg does
/// not know: it is skipped by looking for the first pack start code (00 00 01 BA).
pub fn video_payload<'a>(ext: &str, data: &'a [u8]) -> (String, &'a [u8]) {
    if ext != "pmf" {
        return (ext.to_string(), data);
    }
    if data.starts_with(b"PSMF") {
        let scan = &data[..data.len().min(1024 * 1024)];
        if let Some(pos) = scan.windows(4).position(|w| w == [0x00, 0x00, 0x01, 0xBA]) {
            return ("mpg".to_string(), &data[pos..]);
        }
    }
    ("mpg".to_string(), data)
}

pub fn is_audio_ext(ext: &str) -> bool {
    AUDIO_EXTS.contains(&ext)
}

pub fn is_video_ext(ext: &str) -> bool {
    VIDEO_EXTS.contains(&ext)
}

pub fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

// ---------------------------------------------------------------- temp dir

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let p = std::env::temp_dir().join(format!("obscure_explorer_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&p);
        Self(p)
    }

    pub fn path_for(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------- thumbnails

pub fn thumb_from_rgba(
    ctx: &egui::Context,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    name: String,
) -> Option<egui::TextureHandle> {
    let img = image::RgbaImage::from_raw(w, h, rgba)?;
    let max = 128.0_f32;
    let scale = (max / w.max(h) as f32).min(1.0);
    let img = if scale < 1.0 {
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let (w, h) = img.dimensions();
    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
    Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR))
}

// ---------------------------------------------------------------- ffmpeg

#[derive(Clone)]
pub struct Ffmpeg {
    path: PathBuf,
}

#[derive(Clone, Copy)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    /// Seconds, when the container tells.
    pub duration: Option<f32>,
}

fn command(path: &Path) -> Command {
    let mut c = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Folder for the tools the program downloads (ffmpeg, Ruffle) and for the small files that remember
/// where they are: next to the program, or - when that folder is read-only, like /usr/bin on
/// Linux - the user's data folder (~/.local/share/obscure-explorer).
pub fn tools_dir() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(d) = &exe_dir {
        let probe = d.join(".obscure_write_test");
        if std::fs::write(&probe, b"x").is_ok() {
            let _ = std::fs::remove_file(&probe);
            return d.clone();
        }
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")));
    if let Some(b) = base {
        let d = b.join("obscure-explorer");
        if std::fs::create_dir_all(&d).is_ok() {
            return d;
        }
    }
    exe_dir.unwrap_or_else(|| PathBuf::from("."))
}

/// Automatic download of ffmpeg exists for Windows (gyan.dev build) and Linux (static build).
pub fn ffmpeg_download_supported() -> bool {
    cfg!(windows) || cfg!(target_os = "linux")
}

/// Package-manager command that installs ffmpeg, from the `ID` / `ID_LIKE` of /etc/os-release.
pub fn install_command_for(id: &str, id_like: &str) -> Option<&'static str> {
    let ids: Vec<String> = std::iter::once(id)
        .chain(id_like.split_whitespace())
        .map(|s| s.trim_matches('"').to_ascii_lowercase())
        .collect();
    for i in &ids {
        match i.as_str() {
            "debian" | "ubuntu" | "linuxmint" | "pop" | "raspbian" | "kali" | "zorin" | "elementary" => {
                return Some("sudo apt install ffmpeg")
            }
            "fedora" | "rhel" | "centos" | "rocky" | "almalinux" => return Some("sudo dnf install ffmpeg-free"),
            "arch" | "manjaro" | "endeavouros" | "steamos" | "cachyos" => return Some("sudo pacman -S ffmpeg"),
            "opensuse" | "opensuse-tumbleweed" | "opensuse-leap" | "suse" | "sles" => {
                return Some("sudo zypper install ffmpeg")
            }
            "alpine" => return Some("sudo apk add ffmpeg"),
            "void" => return Some("sudo xbps-install ffmpeg"),
            "gentoo" => return Some("sudo emerge media-video/ffmpeg"),
            "nixos" => return Some("nix-shell -p ffmpeg"),
            _ => {}
        }
    }
    None
}

/// (distribution name, install command) for the running Linux system.
pub fn ffmpeg_install_command() -> Option<(String, &'static str)> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    let (mut id, mut like, mut pretty) = (String::new(), String::new(), String::new());
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("ID=") {
            id = v.trim_matches('"').to_string();
        } else if let Some(v) = line.strip_prefix("ID_LIKE=") {
            like = v.trim_matches('"').to_string();
        } else if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
            pretty = v.trim_matches('"').to_string();
        }
    }
    install_command_for(&id, &like).map(|c| (if pretty.is_empty() { id } else { pretty }, c))
}

fn config_paths() -> Vec<PathBuf> {
    let mut v = vec![tools_dir().join("ffmpeg_path.txt")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("ffmpeg_path.txt"));
        }
    }
    v.push(PathBuf::from("ffmpeg_path.txt"));
    v
}

fn works(path: &Path) -> bool {
    command(path)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Accepts a user-chosen path if it really is a working ffmpeg.
pub fn ffmpeg_from_path(path: PathBuf) -> Option<Ffmpeg> {
    if works(&path) {
        Some(Ffmpeg { path })
    } else {
        None
    }
}

/// Remembers the ffmpeg location (a small text file next to the program).
pub fn save_ffmpeg_path(ff: &Ffmpeg) {
    if let Some(p) = config_paths().first() {
        let _ = std::fs::write(p, ff.path.to_string_lossy().as_bytes());
    }
}

/// Places where ffmpeg usually ends up on Windows even when PATH was not refreshed yet.
fn extra_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        let base = PathBuf::from(la).join("Microsoft").join("WinGet");
        v.push(base.join("Links").join("ffmpeg.exe"));
        if let Ok(rd) = std::fs::read_dir(base.join("Packages")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.contains("ffmpeg") {
                    if let Ok(rd2) = std::fs::read_dir(e.path()) {
                        for e2 in rd2.flatten() {
                            v.push(e2.path().join("bin").join("ffmpeg.exe"));
                        }
                    }
                }
            }
        }
    }
    if let Ok(up) = std::env::var("USERPROFILE") {
        v.push(PathBuf::from(up).join("scoop").join("shims").join("ffmpeg.exe"));
    }
    if cfg!(unix) {
        for p in ["/usr/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/snap/bin/ffmpeg", "/opt/homebrew/bin/ffmpeg"] {
            v.push(PathBuf::from(p));
        }
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join(".local").join("bin").join("ffmpeg"));
        }
    }
    v.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin\ffmpeg.exe"));
    v.push(PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"));
    v.push(PathBuf::from(r"C:\Program Files\ffmpeg\bin\ffmpeg.exe"));
    v
}

/// Looks for ffmpeg: saved path, next to the program, current directory, then PATH.
pub fn find_ffmpeg() -> Option<Ffmpeg> {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    for cfg in config_paths() {
        if let Ok(text) = std::fs::read_to_string(&cfg) {
            let t = text.trim();
            if !t.is_empty() {
                candidates.push(PathBuf::from(t));
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(name));
        }
    }
    candidates.push(tools_dir().join(name));
    candidates.push(PathBuf::from(".").join(name));
    candidates.push(PathBuf::from(name));
    candidates.extend(extra_candidates());
    for c in candidates {
        if works(&c) {
            return Some(Ffmpeg { path: c });
        }
    }
    None
}

/// "Duration: 00:01:23.45, start: ..." -> seconds
fn parse_duration(text: &str) -> Option<f32> {
    let pos = text.find("Duration:")?;
    let rest = text[pos + 9..].trim_start();
    let tok = rest.split(',').next()?.trim();
    let mut parts = tok.split(':');
    let h: f32 = parts.next()?.parse().ok()?;
    let m: f32 = parts.next()?.parse().ok()?;
    let sec: f32 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn parse_video_info(text: &str) -> Option<VideoInfo> {
    for line in text.lines() {
        let Some(pos) = line.find("Video:") else { continue };
        let rest = &line[pos + 6..];
        let mut size: Option<(u32, u32)> = None;
        let mut fps: Option<f32> = None;
        for tok in rest.split(',') {
            let t = tok.trim();
            if let Some(first) = t.split_whitespace().next() {
                if let Some((a, b)) = first.split_once('x') {
                    if let (Ok(w), Ok(h)) = (a.parse::<u32>(), b.parse::<u32>()) {
                        if size.is_none() {
                            size = Some((w, h));
                        }
                    }
                }
                if t.ends_with("fps") {
                    if let Ok(f) = first.parse::<f32>() {
                        fps = Some(f);
                    }
                }
            }
        }
        if let Some((width, height)) = size {
            if width > 0 && height > 0 {
                return Some(VideoInfo {
                    width,
                    height,
                    fps: fps.unwrap_or(25.0).clamp(1.0, 120.0),
                    duration: parse_duration(text),
                });
            }
        }
    }
    None
}

impl Ffmpeg {
    pub fn probe(&self, file: &Path) -> Option<VideoInfo> {
        let out = command(&self.path)
            .args(["-hide_banner", "-i"])
            .arg(file)
            .stdin(Stdio::null())
            .output()
            .ok()?;
        parse_video_info(&String::from_utf8_lossy(&out.stderr))
    }

    /// First frame as PNG bytes, scaled to `width` pixels wide.
    pub fn still_png(&self, file: &Path, width: u32) -> Option<Vec<u8>> {
        let out = command(&self.path)
            .args(["-v", "error", "-i"])
            .arg(file)
            .args(["-frames:v", "1", "-vf"])
            .arg(format!("scale={width}:-2"))
            .args(["-f", "image2pipe", "-vcodec", "png", "-"])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if out.stdout.is_empty() {
            None
        } else {
            Some(out.stdout)
        }
    }

    /// Audio track decoded to 44.1 kHz stereo 16-bit samples.
    pub fn audio_pcm(&self, file: &Path) -> Option<Vec<i16>> {
        let out = command(&self.path)
            .args(["-v", "error", "-i"])
            .arg(file)
            .args(["-vn", "-f", "s16le", "-acodec", "pcm_s16le", "-ar", "44100", "-ac", "2", "-"])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if out.stdout.len() < 4 {
            return None;
        }
        Some(out.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
    }
}

type ThumbMsg = (usize, Option<(u32, u32, Vec<u8>)>);

/// Video thumbnails produced in background threads (ffmpeg on the first frame).
pub struct VideoThumbs {
    tx: Sender<ThumbMsg>,
    rx: Receiver<ThumbMsg>,
    pending: HashSet<usize>,
    ready: HashMap<usize, Option<egui::TextureHandle>>,
}

impl VideoThumbs {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx, pending: HashSet::new(), ready: HashMap::new() }
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.ready.clear();
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((index, img)) = self.rx.try_recv() {
            if !self.pending.remove(&index) {
                continue; // stale result (cleared meanwhile)
            }
            let tex = img.and_then(|(w, h, rgba)| thumb_from_rgba(ctx, w, h, rgba, format!("vthumb_{index}")));
            self.ready.insert(index, tex);
        }
    }

    pub fn get(
        &mut self,
        ctx: &egui::Context,
        ff: &Ffmpeg,
        temp: &TempDir,
        archive: &dyn Archive,
        index: usize,
        budget: &mut usize,
    ) -> Option<egui::TextureHandle> {
        if let Some(t) = self.ready.get(&index) {
            return t.clone();
        }
        if self.pending.contains(&index) {
            return None;
        }
        if *budget == 0 {
            ctx.request_repaint();
            return None;
        }
        *budget -= 1;
        let data = match archive.read(index) {
            Ok(d) => d,
            Err(_) => {
                self.ready.insert(index, None);
                return None;
            }
        };
        let ext = Path::new(&archive.entries()[index].path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_ascii_lowercase();
        let (ext, payload) = video_payload(&ext, &data);
        // the first frame only needs the beginning of the file
        let head = &payload[..payload.len().min(8 * 1024 * 1024)];
        let file = temp.path_for(&format!("thumb_{index}.{ext}"));
        if std::fs::write(&file, head).is_err() {
            self.ready.insert(index, None);
            return None;
        }
        self.pending.insert(index);
        let tx = self.tx.clone();
        let ff = ff.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let result = ff.still_png(&file, 192).and_then(|png| {
                let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?;
                let rgba = img.into_rgba8();
                let (w, h) = rgba.dimensions();
                Some((w, h, rgba.into_raw()))
            });
            let _ = std::fs::remove_file(&file);
            let _ = tx.send((index, result));
            ctx2.request_repaint();
        });
        None
    }
}

// ---------------------------------------------------------------- audio

enum AudioSrc {
    File(Arc<[u8]>),
    Pcm { samples: Arc<Vec<i16>>, channels: u16, rate: u32 },
}

/// Builds a paused sink that starts playing at `start`.
fn build_sink(
    handle: &OutputStreamHandle,
    src: &AudioSrc,
    volume: f32,
    start: Duration,
) -> Result<(Sink, Option<Duration>), String> {
    let sink = Sink::try_new(handle).map_err(|e| e.to_string())?;
    sink.pause();
    sink.set_volume(volume);
    let dur = match src {
        AudioSrc::File(data) => {
            let dec = Decoder::new(Cursor::new(data.clone())).map_err(|e| e.to_string())?;
            let d = dec.total_duration();
            sink.append(dec.skip_duration(start));
            d
        }
        AudioSrc::Pcm { samples, channels, rate } => {
            let buf = SamplesBuffer::new(*channels, *rate, samples.as_ref().clone());
            let d = buf.total_duration();
            sink.append(buf.skip_duration(start));
            d
        }
    };
    Ok((sink, dur))
}

/// Some decoders (mp3, ogg) do not know the length: count the samples in a background thread.
fn measure_duration(data: Arc<[u8]>) -> Receiver<Duration> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        if let Ok(dec) = Decoder::new(Cursor::new(data)) {
            let ch = dec.channels().max(1) as f64;
            let rate = dec.sample_rate().max(1) as f64;
            let n = dec.count() as f64;
            let _ = tx.send(Duration::from_secs_f64(n / ch / rate));
        }
    });
    rx
}

fn decode_vorbis(data: &[u8]) -> Result<(Vec<i16>, u16, u32), String> {
    use lewton::inside_ogg::OggStreamReader;
    let mut rdr = OggStreamReader::new(Cursor::new(data)).map_err(|e| e.to_string())?;
    let channels = rdr.ident_hdr.audio_channels as u16;
    let rate = rdr.ident_hdr.audio_sample_rate as u32;
    let mut samples: Vec<i16> = Vec::new();
    while let Some(pck) = rdr.read_dec_packet_itl().map_err(|e| e.to_string())? {
        samples.extend_from_slice(&pck);
    }
    if samples.is_empty() {
        return Err("the file contains no audio data".to_string());
    }
    Ok((samples, channels, rate))
}

pub struct AudioPlayer {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Sink,
    src: AudioSrc,
    duration: Option<Duration>,
    dur_rx: Option<Receiver<Duration>>,
    volume: f32,
    since: Option<Instant>,
    elapsed: Duration,
    ended: bool,
}

impl AudioPlayer {
    fn new(src: AudioSrc) -> Result<Self, String> {
        let (stream, handle) = OutputStream::try_default().map_err(|e| e.to_string())?;
        let (sink, duration) = build_sink(&handle, &src, 1.0, Duration::ZERO)?;
        let dur_rx = match (&duration, &src) {
            (None, AudioSrc::File(data)) => Some(measure_duration(data.clone())),
            _ => None,
        };
        Ok(Self {
            _stream: stream,
            handle,
            sink,
            src,
            duration,
            dur_rx,
            volume: 1.0,
            since: None,
            elapsed: Duration::ZERO,
            ended: false,
        })
    }

    /// Any format rodio can probe (WAV, OGG, MP3, FLAC, ...), detected from the content.
    /// Ogg files that rodio rejects get a second chance with a stand-alone Vorbis decoder,
    /// which also reports clearer errors (e.g. "no audio data").
    pub fn from_file(bytes: Vec<u8>) -> Result<Self, String> {
        let data: Arc<[u8]> = Arc::from(bytes);
        match Self::new(AudioSrc::File(data.clone())) {
            Ok(p) => Ok(p),
            Err(first) => {
                if data.starts_with(b"OggS") {
                    match decode_vorbis(&data) {
                        Ok((samples, channels, rate)) => Self::from_pcm(samples, channels, rate),
                        Err(second) => Err(format!("{first}; Vorbis fallback: {second}")),
                    }
                } else {
                    Err(first)
                }
            }
        }
    }

    pub fn from_pcm(samples: Vec<i16>, channels: u16, rate: u32) -> Result<Self, String> {
        Self::new(AudioSrc::Pcm { samples: Arc::new(samples), channels, rate })
    }

    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v;
        self.sink.set_volume(v);
    }

    pub fn is_playing(&self) -> bool {
        self.since.is_some()
    }

    pub fn position(&self) -> Duration {
        let mut p = self.elapsed + self.since.map(|t| t.elapsed()).unwrap_or(Duration::ZERO);
        if let Some(d) = self.duration {
            p = p.min(d);
        }
        p
    }

    fn rebuild_at(&mut self, pos: Duration, resume: bool) {
        if let Ok((sink, d)) = build_sink(&self.handle, &self.src, self.volume, pos) {
            self.sink = sink;
            if d.is_some() {
                self.duration = d;
            }
        }
        self.elapsed = pos;
        self.since = None;
        self.ended = false;
        if resume {
            self.sink.play();
            self.since = Some(Instant::now());
        }
    }

    pub fn play(&mut self) {
        if self.sink.empty() {
            self.rebuild_at(Duration::ZERO, true);
        } else {
            self.sink.play();
            if self.since.is_none() {
                self.since = Some(Instant::now());
            }
        }
    }

    pub fn pause(&mut self) {
        self.sink.pause();
        if let Some(t) = self.since.take() {
            self.elapsed += t.elapsed();
        }
    }

    pub fn toggle(&mut self) {
        if self.is_playing() {
            self.pause();
        } else {
            self.play();
        }
    }

    pub fn stop(&mut self) {
        self.rebuild_at(Duration::ZERO, false);
    }

    /// Jumps to `pos` (keeps playing if it was playing).
    pub fn seek(&mut self, pos: Duration) {
        let was_playing = self.is_playing();
        let pos = pos.min(self.duration.unwrap_or(pos));
        self.rebuild_at(pos, was_playing);
    }

    /// Call every frame: picks up the measured length and detects the end of the sound.
    pub fn tick(&mut self) {
        let got = self.dur_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(d) = got {
            self.duration = Some(d);
            self.dur_rx = None;
        }
        if self.since.is_some() && self.sink.empty() {
            let end = self.duration.unwrap_or_else(|| self.position());
            self.since = None;
            self.elapsed = end;
            self.ended = true;
        }
    }

    /// True once after the sound reached its end by itself.
    pub fn take_ended(&mut self) -> bool {
        std::mem::replace(&mut self.ended, false)
    }
}

// ---------------------------------------------------------------- video player

#[allow(clippy::too_many_arguments)]
fn video_loop(
    ff: Ffmpeg,
    file: PathBuf,
    info: VideoInfo,
    start: Duration,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    counter: Arc<AtomicU64>,
    frame_tx: SyncSender<Vec<u8>>,
    ctx: egui::Context,
) {
    let frame_len = (info.width as usize) * (info.height as usize) * 4;
    let mut cmd = command(&ff.path);
    cmd.args(["-v", "error"]);
    if start > Duration::ZERO {
        cmd.arg("-ss").arg(format!("{:.3}", start.as_secs_f32()));
    }
    cmd.arg("-i")
        .arg(&file)
        .args(["-an", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            finished.store(true, Ordering::Relaxed);
            return;
        }
    };
    let Some(mut out) = child.stdout.take() else {
        let _ = child.kill();
        finished.store(true, Ordering::Relaxed);
        return;
    };
    let interval = Duration::from_secs_f32(1.0 / info.fps.max(1.0));
    let mut next = Instant::now();
    let mut sent_first = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // while paused, still let one frame through so a seek shows the new position
        if paused.load(Ordering::Relaxed) && sent_first {
            std::thread::sleep(Duration::from_millis(30));
            next = Instant::now();
            continue;
        }
        let mut buf = vec![0u8; frame_len];
        if out.read_exact(&mut buf).is_err() {
            break;
        }
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        next += interval;
        if next + Duration::from_millis(200) < Instant::now() {
            next = Instant::now();
        }
        if frame_tx.send(buf).is_err() {
            break;
        }
        sent_first = true;
        counter.fetch_add(1, Ordering::Relaxed);
        ctx.request_repaint();
    }
    let _ = child.kill();
    let _ = child.wait();
    finished.store(true, Ordering::Relaxed);
    ctx.request_repaint();
}

pub struct VideoPlayer {
    pub info: VideoInfo,
    ff: Ffmpeg,
    file: PathBuf,
    ctx: egui::Context,
    start_offset: Duration,
    counter: Arc<AtomicU64>,
    frame_rx: Receiver<Vec<u8>>,
    audio_rx: Option<Receiver<Option<Vec<i16>>>>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    texture: Option<egui::TextureHandle>,
    audio: Option<AudioPlayer>,
    volume: f32,
    started: bool,
}

impl VideoPlayer {
    pub fn start(ff: &Ffmpeg, file: PathBuf, info: VideoInfo, ctx: &egui::Context) -> Self {
        let (frame_tx, frame_rx) = sync_channel::<Vec<u8>>(2);
        let (audio_tx, audio_rx) = channel::<Option<Vec<i16>>>();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let counter = Arc::new(AtomicU64::new(0));
        let (s, p, f, c) = (stop.clone(), paused.clone(), finished.clone(), counter.clone());
        let ff2 = ff.clone();
        let file2 = file.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            // audio first (decoded fully, kept in memory so seeking is instant)
            let pcm = ff2.audio_pcm(&file2);
            let _ = audio_tx.send(pcm);
            ctx2.request_repaint();
            video_loop(ff2, file2, info, Duration::ZERO, s, p, f, c, frame_tx, ctx2);
        });
        Self {
            info,
            ff: ff.clone(),
            file,
            ctx: ctx.clone(),
            start_offset: Duration::ZERO,
            counter,
            frame_rx,
            audio_rx: Some(audio_rx),
            stop,
            paused,
            finished,
            texture: None,
            audio: None,
            volume: 1.0,
            started: false,
        }
    }

    /// Call every frame while the player is shown.
    pub fn update(&mut self, ctx: &egui::Context) {
        let got = self.audio_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(pcm) = got {
            self.audio_rx = None;
            if let Some(samples) = pcm {
                if let Ok(mut a) = AudioPlayer::from_pcm(samples, 2, 44100) {
                    a.set_volume(self.volume);
                    a.seek(self.position());
                    if !self.paused.load(Ordering::Relaxed) {
                        a.play();
                    }
                    self.audio = Some(a);
                }
            }
        }
        let mut latest = None;
        while let Ok(f) = self.frame_rx.try_recv() {
            latest = Some(f);
        }
        if let Some(buf) = latest {
            let size = [self.info.width as usize, self.info.height as usize];
            let img = egui::ColorImage::from_rgba_unmultiplied(size, &buf);
            match &mut self.texture {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => {
                    self.texture =
                        Some(ctx.load_texture("video_frame", img, egui::TextureOptions::LINEAR));
                }
            }
            self.started = true;
        }
        if let Some(a) = &mut self.audio {
            a.tick();
        }
        if !self.is_finished() && !self.is_paused() {
            ctx.request_repaint();
        }
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.texture.as_ref()
    }

    pub fn duration(&self) -> Option<Duration> {
        self.info.duration.map(Duration::from_secs_f32)
    }

    pub fn position(&self) -> Duration {
        let frames = self.counter.load(Ordering::Relaxed) as f64;
        let mut p = self.start_offset
            + Duration::from_secs_f64(frames / self.info.fps.max(1.0) as f64);
        if let Some(d) = self.duration() {
            p = p.min(d);
        }
        p
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v;
        if let Some(a) = &mut self.audio {
            a.set_volume(v);
        }
    }

    #[allow(dead_code)]
    pub fn is_loading(&self) -> bool {
        !self.started && !self.is_finished()
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    /// Restarts decoding at `pos` (the audio is already in memory).
    pub fn seek(&mut self, pos: Duration) {
        self.stop.store(true, Ordering::Relaxed);
        let (frame_tx, frame_rx) = sync_channel::<Vec<u8>>(2);
        let stop = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let counter = Arc::new(AtomicU64::new(0));
        self.stop = stop.clone();
        self.finished = finished.clone();
        self.counter = counter.clone();
        self.frame_rx = frame_rx;
        self.start_offset = pos;
        let (ff, file, ctx, info, paused) =
            (self.ff.clone(), self.file.clone(), self.ctx.clone(), self.info, self.paused.clone());
        std::thread::spawn(move || {
            video_loop(ff, file, info, pos, stop, paused, finished, counter, frame_tx, ctx);
        });
        if let Some(a) = &mut self.audio {
            a.seek(pos);
        }
    }

    pub fn toggle_pause(&mut self) {
        if self.is_finished() {
            // replay from the start
            self.paused.store(false, Ordering::Relaxed);
            self.seek(Duration::ZERO);
            if let Some(a) = &mut self.audio {
                a.play();
            }
            return;
        }
        let now = !self.paused.load(Ordering::Relaxed);
        self.paused.store(now, Ordering::Relaxed);
        if let Some(a) = &mut self.audio {
            if now {
                a.pause();
            } else {
                a.play();
            }
        }
    }

    /// Back to the first frame, paused.
    pub fn stop(&mut self) {
        self.paused.store(true, Ordering::Relaxed);
        if let Some(a) = &mut self.audio {
            a.pause();
        }
        self.seek(Duration::ZERO);
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------- ffmpeg download (Windows)

/// Downloads the official Windows build of ffmpeg (the one linked from ffmpeg.org)
/// in a background thread and unpacks `ffmpeg.exe` into a folder.
pub struct FfDownload {
    rx: Receiver<Result<PathBuf, String>>,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

fn download_ffmpeg_zip(dest_dir: &Path, done: &AtomicU64, total: &AtomicU64) -> Result<PathBuf, String> {
    const URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
    let resp = ureq::get(URL).call().map_err(|e| format!("download failed: {e}"))?;
    if let Some(len) = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()) {
        total.store(len, Ordering::Relaxed);
    }
    let mut reader = resp.into_reader();
    let zip_path = dest_dir.join("ffmpeg_download.zip");
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

    let out_path = dest_dir.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
    let archive_file =
        std::fs::File::open(&zip_path).map_err(|e| format!("cannot reopen the archive: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(archive_file).map_err(|e| format!("the download is not a valid zip: {e}"))?;
    let mut found = false;
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| format!("zip error: {e}"))?;
        let name = f.name().to_string();
        if name.ends_with("bin/ffmpeg.exe") || name.ends_with("bin/ffmpeg") {
            let mut out = std::fs::File::create(&out_path)
                .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
            std::io::copy(&mut f, &mut out).map_err(|e| format!("cannot unpack ffmpeg: {e}"))?;
            found = true;
            break;
        }
    }
    let _ = std::fs::remove_file(&zip_path);
    if !found {
        return Err("ffmpeg.exe was not found inside the downloaded archive".to_string());
    }
    Ok(out_path)
}

/// Downloads a static Linux build of ffmpeg (single gzip-compressed binary from the ffmpeg-static
/// project) and unpacks it as `ffmpeg` into a folder. No installation is needed.
#[allow(dead_code)]
fn download_ffmpeg_linux(dest_dir: &Path, done: &AtomicU64, total: &AtomicU64) -> Result<PathBuf, String> {
    const API: &str = "https://api.github.com/repos/eugeneware/ffmpeg-static/releases/latest";
    let body = ureq::get(API)
        .set("User-Agent", "obscure-explorer")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not ask GitHub for the latest ffmpeg build: {e}"))?
        .into_string()
        .map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let json = obscure_xmc::json::parse(&body).ok_or_else(|| "unexpected answer from GitHub".to_string())?;
    let arch: &[&str] = if cfg!(target_arch = "aarch64") { &["arm64", "aarch64"] } else { &["x64", "x86_64", "amd64"] };
    let url = json
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|a| {
                let name = a.get("name")?.as_str()?.to_ascii_lowercase();
                let wanted = name.ends_with(".gz")
                    && !name.contains(".tar")
                    && name.contains("linux")
                    && !name.contains("ffprobe")
                    && arch.iter().any(|t| name.contains(t));
                if wanted {
                    a.get("browser_download_url")?.as_str().map(str::to_string)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| "no Linux build of ffmpeg was found in the latest ffmpeg-static release".to_string())?;

    std::fs::create_dir_all(dest_dir).map_err(|e| format!("cannot create {}: {e}", dest_dir.display()))?;
    let resp = ureq::get(&url)
        .set("User-Agent", "obscure-explorer")
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    if let Some(len) = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()) {
        total.store(len, Ordering::Relaxed);
    }
    let gz_path = dest_dir.join("ffmpeg_download.gz");
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&gz_path)
        .map_err(|e| format!("cannot write {}: {e}", gz_path.display()))?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("cannot write the download: {e}"))?;
        done.fetch_add(n as u64, Ordering::Relaxed);
    }
    drop(file);

    let out_path = dest_dir.join("ffmpeg");
    let input = std::fs::File::open(&gz_path).map_err(|e| format!("cannot reopen the download: {e}"))?;
    let mut decoder = flate2::read::GzDecoder::new(input);
    let mut out = std::fs::File::create(&out_path)
        .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
    std::io::copy(&mut decoder, &mut out).map_err(|e| format!("cannot unpack ffmpeg: {e}"))?;
    drop(out);
    let _ = std::fs::remove_file(&gz_path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755));
    }
    Ok(out_path)
}

impl FfDownload {
    pub fn start(dest_dir: PathBuf, ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let done = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let (d, t) = (done.clone(), total.clone());
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let res = if cfg!(windows) {
                download_ffmpeg_zip(&dest_dir, &d, &t)
            } else if cfg!(target_os = "linux") {
                download_ffmpeg_linux(&dest_dir, &d, &t)
            } else {
                Err("automatic download is not available on this system".to_string())
            };
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
mod install_hint_tests {
    use super::*;

    #[test]
    fn picks_the_package_manager_of_the_distribution() {
        assert_eq!(install_command_for("ubuntu", "debian"), Some("sudo apt install ffmpeg"));
        assert_eq!(install_command_for("linuxmint", "ubuntu debian"), Some("sudo apt install ffmpeg"));
        assert_eq!(install_command_for("fedora", ""), Some("sudo dnf install ffmpeg-free"));
        assert_eq!(install_command_for("manjaro", "arch"), Some("sudo pacman -S ffmpeg"));
        assert_eq!(install_command_for("nobara", "fedora"), Some("sudo dnf install ffmpeg-free"));
        assert_eq!(install_command_for("somethingnew", ""), None);
    }
}

#[cfg(test)]
mod psp_media_tests {
    use super::*;

    #[test]
    fn pmf_header_is_skipped() {
        let mut d = b"PSMF0012".to_vec();
        d.resize(0x800, 0);
        d.extend([0x00, 0x00, 0x01, 0xBA, 0x44]);
        let (ext, payload) = video_payload("pmf", &d);
        assert_eq!(ext, "mpg");
        assert_eq!(&payload[..4], &[0x00, 0x00, 0x01, 0xBA]);
        assert_eq!(payload.len(), 5);
        let (ext, payload) = video_payload("bik", &d);
        assert_eq!(ext, "bik");
        assert_eq!(payload.len(), d.len());
    }

    #[test]
    fn at3_and_xwma_use_ffmpeg() {
        assert!(audio_needs_ffmpeg("at3"));
        assert!(audio_needs_ffmpeg("xwma"));
        assert!(!audio_needs_ffmpeg("wav"));
        assert!(is_audio_ext("at3"));
        assert!(is_video_ext("pmf"));
    }
}
