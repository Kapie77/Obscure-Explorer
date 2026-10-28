#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(deprecated)]

mod media;
mod model3d;

use eframe::egui;
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};
use media::{
    ffmpeg_from_path, find_ffmpeg, fmt_dur, is_audio_ext, is_video_ext, save_ffmpeg_path,
    thumb_from_rgba, AudioPlayer, FfDownload, Ffmpeg, TempDir, VideoPlayer, VideoThumbs,
};
use obscure_core::{Archive, Context, DirNode, Entry, Game, GameArchive, HvpArchive, Platform};
use obscure_textures::{decode_any, is_image_ext, is_pack_ext, Texture};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() -> eframe::Result<()> {
    // there is no console in release builds: keep a crash log next to the program instead
    std::panic::set_hook(Box::new(|info| {
        let dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = std::fs::write(dir.join("obscure_explorer_crash.log"), format!("{info}"));
    }));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1360.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Obscure Explorer",
        options,
        Box::new(|cc| {
            apply_style(&cc.egui_ctx, true);
            Ok(Box::new(App::default()))
        }),
    )
}

fn apply_style(ctx: &egui::Context, dark: bool) {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    if dark {
        v.panel_fill = Color32::from_rgb(30, 33, 38);
        v.selection.bg_fill = Color32::from_rgb(0, 95, 184);
    } else {
        v.panel_fill = Color32::from_rgb(244, 245, 247);
        v.selection.bg_fill = Color32::from_rgb(0, 120, 215);
    }
    ctx.set_visuals(v);
}

// ---------------------------------------------------------------- theme palette

#[derive(Clone, Copy)]
struct Palette {
    tile: Color32,
    tile_hover: Color32,
    sel_muted: Color32,
    thumb_bg: Color32,
    text: Color32,
    weak: Color32,
    page: Color32,
    folder_tab: Color32,
    folder_body: Color32,
    pack_bg: Color32,
    pack_accent: Color32,
}

fn palette(dark: bool) -> Palette {
    let g = Color32::from_gray;
    let folder_tab = Color32::from_rgb(214, 160, 70);
    let folder_body = Color32::from_rgb(236, 186, 96);
    let pack_accent = Color32::from_rgb(120, 86, 200);
    if dark {
        Palette {
            tile: g(38),
            tile_hover: g(52),
            sel_muted: g(70),
            thumb_bg: g(22),
            text: g(225),
            weak: g(140),
            page: g(58),
            folder_tab,
            folder_body,
            pack_bg: Color32::from_rgb(47, 41, 68),
            pack_accent,
        }
    } else {
        Palette {
            tile: g(232),
            tile_hover: g(219),
            sel_muted: g(200),
            thumb_bg: g(205),
            text: g(30),
            weak: g(105),
            page: g(252),
            folder_tab,
            folder_body,
            pack_bg: Color32::from_rgb(226, 220, 244),
            pack_accent,
        }
    }
}

// ---------------------------------------------------------------- types

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Grid,
    List,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tree,
    Items,
}

enum Item<'a> {
    Folder { name: &'a str, subfolders: usize, files: usize },
    File { name: &'a str, index: usize },
    Tex {
        name: &'a str,
        i: usize,
        width: u32,
        height: u32,
        format: &'a str,
        thumb: Option<&'a egui::TextureHandle>,
    },
}

/// Owned description of an item, used by keyboard navigation.
#[derive(Clone)]
enum Target {
    Folder(String),
    File(usize),
    Tex(usize),
}

/// Something that can be exported.
#[derive(Clone)]
enum ExpTarget {
    /// All files under a folder (empty path = everything).
    Folder(Vec<String>),
    File(usize),
    /// (texture pack file index, texture inside it)
    Tex(usize, usize),
}

enum Action {
    /// Tree / breadcrumb navigation.
    Cd(Vec<String>),
    /// Navigation started from a folder tile or the keyboard.
    Enter(Vec<String>),
    Toggle(String),
    /// Opens a file according to its type (nothing happens for unsupported files).
    Open(usize),
    /// Opens an image of the texture pack that is currently open.
    OpenTex(usize),
    /// Click on the play / pause badge of an audio thumbnail.
    ToggleAudio(usize),
    PlayVideo(usize),
    Cursor(usize),
    TreeFocus(Option<String>),
    TogglePlay,
    PlayerPrev,
    PlayerNext,
    PlayerClose,
    DownloadFfmpeg,
    LocateFfmpeg,
    DismissFfmpeg,
    ToggleSelect(String, usize),
    RangeSelect(usize),
    SelectAll,
    ClearSelection,
    ExportSelection,
    Export(Vec<ExpTarget>),
    Up,
}

struct Keys {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    enter: bool,
    space: bool,
    back: bool,
    esc: bool,
    select_all: bool,
}

/// The audio player bar (keeps playing while browsing).
struct PlayerState {
    index: usize,
    title: String,
    audio: Option<AudioPlayer>,
    error: Option<String>,
    playlist: Vec<usize>,
    pos: usize,
    drag_value: Option<f32>,
}

/// Full-window image viewer with zoom and pan.
struct Viewer {
    name: String,
    handle: egui::TextureHandle,
    w: u32,
    h: u32,
    zoom: f32,
    offset: egui::Vec2,
    fit_pending: bool,
    opened_at: f64,
    /// Images of the view it was opened from: (file index, image inside a pack).
    list: Vec<(usize, Option<usize>)>,
    pos: usize,
}

struct TextView {
    name: String,
    text: String,
    open: bool,
}

/// The full-window video player.
struct VideoSession {
    name: String,
    file: PathBuf,
    player: VideoPlayer,
    drag_value: Option<f32>,
    opened_at: f64,
    last_seek: f64,
    /// Videos of the view it was opened from, and the position of this one.
    playlist: Vec<usize>,
    pos: usize,
}

/// A texture file (.dic/.dip) opened like a folder of images.
struct Container {
    index: usize,
    file_name: String,
    textures: Vec<CTex>,
}

struct CTex {
    name: String,
    width: u32,
    height: u32,
    format: String,
    thumb: Option<egui::TextureHandle>,
}

struct TreeRow {
    path: Vec<String>,
    name: String,
    depth: usize,
    has_children: bool,
    expanded: bool,
    /// Some(index) for file rows.
    file: Option<usize>,
}

impl TreeRow {
    fn key(&self) -> String {
        match self.file {
            Some(i) => format!("#file:{i}"),
            None => self.path.join("/"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ExportKind {
    Raw,
    Png,
}

#[derive(Clone, Copy)]
struct ExportItem {
    index: usize,
    only_tex: Option<usize>,
    kind: ExportKind,
}

struct ExportJob {
    items: Vec<ExportItem>,
    pos: usize,
    ok: usize,
    failed: usize,
    last_error: Option<String>,
    cancel: bool,
    dest: PathBuf,
}

struct ExportDialog {
    targets: Vec<ExpTarget>,
    n_files: usize,
    has_tex: bool,
    tex_only: bool,
    raw: bool,
    png: bool,
    dest: Option<PathBuf>,
    job: Option<ExportJob>,
    message: Option<String>,
}

#[derive(Clone)]
struct ThumbInfo {
    handle: egui::TextureHandle,
    /// Number of images in the file (1 for plain images).
    count: usize,
}

/// Image thumbnails generated on demand (a few per frame), only for visible items.
struct ThumbCache {
    map: HashMap<usize, Option<ThumbInfo>>,
    budget: usize,
}

impl ThumbCache {
    fn clear(&mut self) {
        self.map.clear();
    }

    fn get(
        &mut self,
        ctx: &egui::Context,
        archive: &dyn Archive,
        gctx: &Context,
        index: usize,
    ) -> Option<ThumbInfo> {
        if let Some(v) = self.map.get(&index) {
            return v.clone();
        }
        if self.budget == 0 {
            ctx.request_repaint();
            return None;
        }
        self.budget -= 1;
        let th = make_thumb(ctx, archive, gctx, index);
        self.map.insert(index, th.clone());
        th
    }
}

fn make_thumb(
    ctx: &egui::Context,
    archive: &dyn Archive,
    gctx: &Context,
    index: usize,
) -> Option<ThumbInfo> {
    let path = archive.entries().get(index)?.path.clone();
    let data = archive.read(index).ok()?;
    let textures = decode_any(&path, &data, gctx)?.ok()?;
    let count = textures.len();
    let t = textures.into_iter().next()?;
    let handle = thumb_from_rgba(ctx, t.width, t.height, t.rgba, format!("thumb_{index}"))?;
    Some(ThumbInfo { handle, count })
}

struct Thumbs {
    img: ThumbCache,
    vid: VideoThumbs,
    vid_budget: usize,
}

impl Thumbs {
    fn clear(&mut self) {
        self.img.clear();
        self.vid.clear();
    }
}

struct Env<'a> {
    archive: &'a dyn Archive,
    game_ctx: Context,
    egui_ctx: &'a egui::Context,
    container_index: Option<usize>,
    playing: Option<usize>,
    player_active: bool,
    cursor: Option<usize>,
    items_focused: bool,
    selection: &'a HashSet<String>,
    pal: Palette,
    ffmpeg: Option<&'a Ffmpeg>,
    temp: &'a TempDir,
    now: f64,
    last_enter: f64,
}

struct App {
    archive: Option<Box<dyn Archive>>,
    root_name: String,
    tree: DirNode,
    cwd: Vec<String>,
    container: Option<Container>,
    expanded: HashSet<String>,
    tree_rows: Vec<TreeRow>,
    tree_dirty: bool,
    tree_query: String,
    tree_applied: String,
    tree_cursor: Option<String>,
    query: String,
    applied_query: String,
    results: Vec<usize>,
    view: ViewMode,
    tile: f32,
    dark_mode: bool,
    game_ctx: Context,
    thumb_ctx: (Game, Platform),
    thumbs: Thumbs,
    status: String,
    ffmpeg: Option<Ffmpeg>,
    ffmpeg_prompt: bool,
    ffmpeg_declined: bool,
    temp: TempDir,
    player: Option<PlayerState>,
    auto_next: bool,
    viewer: Option<Viewer>,
    text_view: Option<TextView>,
    model_viewer: Option<model3d::ModelViewer>,
    video: Option<VideoSession>,
    ff_download: Option<FfDownload>,
    selection: HashSet<String>,
    sel_anchor: Option<usize>,
    export: Option<ExportDialog>,
    // keyboard navigation
    focus: Focus,
    cursor: Option<usize>,
    last_cols: usize,
    scroll_cursor: bool,
    pending_scroll_items: Option<f32>,
    tree_scroll: bool,
    pending_scroll_tree: Option<f32>,
    tree_rect: Rect,
    items_rect: Rect,
    last_enter: f64,
}

impl Default for App {
    fn default() -> Self {
        let game_ctx = Context { game: Game::Obscure1, platform: Platform::PcSteam };
        Self {
            archive: None,
            root_name: String::new(),
            tree: DirNode::default(),
            cwd: Vec::new(),
            container: None,
            expanded: HashSet::new(),
            tree_rows: Vec::new(),
            tree_dirty: false,
            tree_query: String::new(),
            tree_applied: String::new(),
            tree_cursor: None,
            query: String::new(),
            applied_query: String::new(),
            results: Vec::new(),
            view: ViewMode::Grid,
            tile: 112.0,
            dark_mode: true,
            thumb_ctx: (game_ctx.game, game_ctx.platform),
            game_ctx,
            thumbs: Thumbs {
                img: ThumbCache { map: HashMap::new(), budget: 0 },
                vid: VideoThumbs::new(),
                vid_budget: 0,
            },
            status: "Open the game folder (all its .hvp files) or a single .hvp archive.".into(),
            ffmpeg: find_ffmpeg(),
            ffmpeg_prompt: false,
            ffmpeg_declined: false,
            temp: TempDir::new(),
            player: None,
            auto_next: true,
            viewer: None,
            text_view: None,
            model_viewer: None,
            video: None,
            ff_download: None,
            selection: HashSet::new(),
            sel_anchor: None,
            export: None,
            focus: Focus::Items,
            cursor: None,
            last_cols: 1,
            scroll_cursor: false,
            pending_scroll_items: None,
            tree_scroll: false,
            pending_scroll_tree: None,
            tree_rect: Rect::NOTHING,
            items_rect: Rect::NOTHING,
            last_enter: -1.0,
        }
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".to_string())
}

/// Steam / GOG installs are recognised by their launcher files; retail is the fallback (set by hand).
fn detect_pc_edition(entries: &[Entry]) -> Option<Platform> {
    let (mut steam, mut gog) = (false, false);
    for e in entries {
        let name = base_name(&e.path).to_ascii_lowercase();
        if name == "steam_api.dll" || name == "steam_api64.dll" || name == "steam_appid.txt" {
            steam = true;
        }
        if name.starts_with("goggame-") || name.starts_with("galaxy") {
            gog = true;
        }
    }
    if steam {
        Some(Platform::PcSteam)
    } else if gog {
        Some(Platform::PcGog)
    } else {
        None
    }
}

fn has_tree_children(node: &DirNode) -> bool {
    !node.dirs.is_empty() || !node.files.is_empty()
}

fn collect_rows(
    node: &DirNode,
    path: &mut Vec<String>,
    depth: usize,
    expanded: &HashSet<String>,
    filter: &str,
    out: &mut Vec<TreeRow>,
) {
    let filtering = !filter.is_empty();
    for (name, child) in &node.dirs {
        path.push(name.clone());
        let key = path.join("/");
        let open = filtering || expanded.contains(&key);
        let mut sub = Vec::new();
        if open {
            collect_rows(child, path, depth + 1, expanded, filter, &mut sub);
        }
        let own_match = !filtering
            || name.to_lowercase().contains(filter)
            || child.files.keys().any(|f| f.to_lowercase().contains(filter));
        if own_match || !sub.is_empty() {
            out.push(TreeRow {
                path: path.clone(),
                name: name.clone(),
                depth,
                has_children: has_tree_children(child),
                expanded: !sub.is_empty(),
                file: None,
            });
            out.extend(sub);
        }
        path.pop();
    }
    for (name, &idx) in &node.files {
        if !filtering || name.to_lowercase().contains(filter) {
            out.push(TreeRow {
                path: path.clone(),
                name: name.clone(),
                depth,
                has_children: false,
                expanded: false,
                file: Some(idx),
            });
        }
    }
}

// selection keys
fn key_file(i: usize) -> String {
    format!("f:{i}")
}

fn key_dir(cwd: &[String], name: &str) -> String {
    if cwd.is_empty() {
        format!("d:{name}")
    } else {
        format!("d:{}/{}", cwd.join("/"), name)
    }
}

fn key_tex(i: usize) -> String {
    format!("t:{i}")
}

fn item_key(item: &Item, cwd: &[String]) -> String {
    match item {
        Item::Folder { name, .. } => key_dir(cwd, name),
        Item::File { index, .. } => key_file(*index),
        Item::Tex { i, .. } => key_tex(*i),
    }
}

fn targets_from_selection(sel: &HashSet<String>, container: Option<usize>) -> Vec<ExpTarget> {
    let mut keys: Vec<&String> = sel.iter().collect();
    keys.sort();
    let mut out = Vec::new();
    for k in keys {
        if let Some(rest) = k.strip_prefix("f:") {
            if let Ok(i) = rest.parse::<usize>() {
                out.push(ExpTarget::File(i));
            }
        } else if let Some(rest) = k.strip_prefix("d:") {
            out.push(ExpTarget::Folder(
                rest.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect(),
            ));
        } else if let Some(rest) = k.strip_prefix("t:") {
            if let (Ok(i), Some(c)) = (rest.parse::<usize>(), container) {
                out.push(ExpTarget::Tex(c, i));
            }
        }
    }
    out
}

fn export_targets_for(item: &Item, cwd: &[String], env: &Env) -> Vec<ExpTarget> {
    let key = item_key(item, cwd);
    if env.selection.len() > 1 && env.selection.contains(&key) {
        return targets_from_selection(env.selection, env.container_index);
    }
    match item {
        Item::Folder { name, .. } => {
            let mut p = cwd.to_vec();
            p.push(name.to_string());
            vec![ExpTarget::Folder(p)]
        }
        Item::File { index, .. } => vec![ExpTarget::File(*index)],
        Item::Tex { i, .. } => vec![ExpTarget::Tex(env.container_index.unwrap_or(0), *i)],
    }
}

impl App {
    fn install(&mut self, archive: Box<dyn Archive>, root_name: String, status: String) {
        self.tree = DirNode::build(
            archive.entries().iter().enumerate().map(|(i, e)| (i, e.path.as_str())),
        );
        self.archive = Some(archive);
        self.root_name = root_name;
        self.status = status;
        self.cwd.clear();
        self.container = None;
        self.expanded.clear();
        self.tree_query.clear();
        self.tree_applied.clear();
        self.tree_cursor = None;
        self.tree_dirty = true;
        self.query.clear();
        self.applied_query.clear();
        self.results.clear();
        self.cursor = None;
        self.selection.clear();
        self.sel_anchor = None;
        self.viewer = None;
        self.text_view = None;
        self.model_viewer = None;
        self.video = None;
        self.export = None;
        self.player = None;
        self.thumbs.clear();
    }

    /// Opens a game folder: every .hvp inside (as folders) plus all loose files.
    fn open(&mut self, dir: PathBuf) {
        match GameArchive::open(&dir, &hashes_dir()) {
            Ok(a) => {
                if let Some(g) = a.game() {
                    self.game_ctx.game = g;
                }
                if let Some(edition) = detect_pc_edition(a.entries()) {
                    self.game_ctx.platform = edition;
                }
                let mut status = format!(
                    "{} files ({} HVP archive(s) + loose files) in {}",
                    a.entries().len(),
                    a.hvp_count(),
                    dir.display()
                );
                if let Some(w) = a.warnings().first() {
                    status.push_str(" - ");
                    status.push_str(w);
                    if a.warnings().len() > 1 {
                        status.push_str(&format!(" (+{} more warning(s))", a.warnings().len() - 1));
                    }
                }
                self.install(Box::new(a), file_name_of(&dir), status);
            }
            Err(e) => self.status = format!("Could not open folder: {e}"),
        }
    }

    fn open_hvp(&mut self, path: PathBuf) {
        match HvpArchive::open(&path, &hashes_dir()) {
            Ok(a) => {
                self.game_ctx.game = a.game();
                let mut status = format!("{} files in {}", a.entries().len(), path.display());
                if let Some(w) = a.warning() {
                    status.push_str(" - ");
                    status.push_str(w);
                }
                self.install(Box::new(a), file_name_of(&path), status);
            }
            Err(e) => self.status = format!("Could not open HVP: {e}"),
        }
    }

    fn rebuild_tree_rows(&mut self) {
        self.tree_dirty = false;
        self.tree_rows.clear();
        if self.archive.is_none() {
            return;
        }
        self.tree_rows.push(TreeRow {
            path: Vec::new(),
            name: self.root_name.clone(),
            depth: 0,
            has_children: false,
            expanded: true,
            file: None,
        });
        let filter = self.tree_query.trim().to_lowercase();
        let mut path = Vec::new();
        collect_rows(&self.tree, &mut path, 1, &self.expanded, &filter, &mut self.tree_rows);
    }

    /// Expands the ancestors of `parts` (and `parts` itself if `include_self`).
    fn reveal(&mut self, parts: &[String], include_self: bool) {
        let end = if include_self { parts.len() + 1 } else { parts.len() };
        for k in 1..end {
            self.expanded.insert(parts[..k].join("/"));
        }
        self.tree_dirty = true;
    }

    fn cd(&mut self, path: Vec<String>) {
        self.reveal(&path, false);
        self.cwd = path;
        self.container = None;
        self.cursor = None;
        self.sel_anchor = None;
        self.selection.clear();
        self.query.clear();
        self.tree_scroll = true;
    }

    fn recompute_results(&mut self) {
        self.applied_query = self.query.clone();
        self.results.clear();
        self.cursor = None;
        let q = self.query.to_lowercase();
        if q.is_empty() {
            return;
        }
        if let Some(a) = &self.archive {
            self.results.extend(
                a.entries()
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.path.to_lowercase().contains(&q))
                    .map(|(i, _)| i),
            );
        }
    }

    fn ext_of_index(&self, i: usize) -> String {
        match self.archive.as_deref() {
            Some(a) => ext_of(base_name(&a.entries()[i].path)),
            None => String::new(),
        }
    }

    fn is_audio_index(&self, i: usize) -> bool {
        is_audio_ext(&self.ext_of_index(i))
    }

    /// Items of the current view in display order (must match the order used when drawing).
    fn view_targets(&self) -> Vec<Target> {
        if self.archive.is_none() {
            return Vec::new();
        }
        if let Some(c) = &self.container {
            (0..c.textures.len()).map(Target::Tex).collect()
        } else if !self.query.is_empty() {
            self.results.iter().map(|&i| Target::File(i)).collect()
        } else {
            let node = self.tree.get(&self.cwd).unwrap_or(&self.tree);
            node.dirs
                .keys()
                .map(|n| Target::Folder(n.clone()))
                .chain(node.files.values().map(|&i| Target::File(i)))
                .collect()
        }
    }

    fn target_key(&self, t: &Target) -> String {
        match t {
            Target::Folder(n) => key_dir(&self.cwd, n),
            Target::File(i) => key_file(*i),
            Target::Tex(i) => key_tex(*i),
        }
    }

    // ------------------------------------------------------------ opening things

    /// Click / Enter on a file. Returns true when it navigated into a texture pack.
    /// Unsupported files do nothing.
    fn open_file(&mut self, index: usize, ctx: &egui::Context, now: f64) -> bool {
        let ext = self.ext_of_index(index);
        if is_pack_ext(&ext) {
            self.open_container(index, ctx);
            return true;
        }
        if is_image_ext(&ext) {
            self.open_viewer(index, None, ctx, now);
        } else if is_audio_ext(&ext) {
            self.toggle_or_play_audio(index);
        } else if is_video_ext(&ext) {
            self.start_video(index, ctx, now);
        } else if is_text_ext(&ext) {
            self.open_text(index);
        } else if ext == "zwo" {
            self.open_model(index);
        }
        false
    }

    /// Decodes one image (a plain image file, or one image of a texture pack) into a texture.
    fn load_image(
        &mut self,
        index: usize,
        only: Option<usize>,
        ctx: &egui::Context,
    ) -> Option<(String, egui::TextureHandle, u32, u32)> {
        let archive = self.archive.as_ref()?;
        let path = archive.entries()[index].path.clone();
        let data = match archive.read(index) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Read error: {e}");
                return None;
            }
        };
        match decode_any(&path, &data, &self.game_ctx) {
            None => {
                self.status = format!(
                    "No image decoder for '{path}' with the current Game/Platform selection."
                );
                None
            }
            Some(Err(e)) => {
                self.status = format!("Cannot decode '{path}': {e}");
                None
            }
            Some(Ok(textures)) => {
                let Some(t) = textures.into_iter().nth(only.unwrap_or(0)) else {
                    self.status = format!("'{path}' has no such image.");
                    return None;
                };
                let opts = egui::TextureOptions {
                    magnification: egui::TextureFilter::Nearest,
                    minification: egui::TextureFilter::Linear,
                    ..egui::TextureOptions::NEAREST
                };
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [t.width as usize, t.height as usize],
                    &t.rgba,
                );
                let handle = ctx.load_texture(t.name.clone(), img, opts);
                Some((t.name, handle, t.width, t.height))
            }
        }
    }

    fn open_viewer(&mut self, index: usize, only: Option<usize>, ctx: &egui::Context, now: f64) {
        let Some((name, handle, w, h)) = self.load_image(index, only, ctx) else { return };
        // the other images of the current view, for previous / next
        let mut list: Vec<(usize, Option<usize>)> = Vec::new();
        for t in self.view_targets() {
            match t {
                Target::Tex(i) => {
                    if let Some(c) = &self.container {
                        list.push((c.index, Some(i)));
                    }
                }
                Target::File(i) => {
                    if is_image_ext(&self.ext_of_index(i)) {
                        list.push((i, None));
                    }
                }
                Target::Folder(_) => {}
            }
        }
        let pos = match list.iter().position(|&(a, b)| a == index && b == only) {
            Some(p) => p,
            None => {
                list.push((index, only));
                list.len() - 1
            }
        };
        self.viewer = Some(Viewer {
            name,
            handle,
            w,
            h,
            zoom: 1.0,
            offset: egui::Vec2::ZERO,
            fit_pending: true,
            opened_at: now,
            list,
            pos,
        });
    }

    fn step_viewer(&mut self, step: isize, ctx: &egui::Context) {
        let (list, pos) = match &self.viewer {
            Some(v) => (v.list.clone(), v.pos as isize),
            None => return,
        };
        let np = pos + step;
        if np < 0 || np as usize >= list.len() {
            return;
        }
        let (index, only) = list[np as usize];
        if let Some((name, handle, w, h)) = self.load_image(index, only, ctx) {
            if let Some(v) = &mut self.viewer {
                v.name = name;
                v.handle = handle;
                v.w = w;
                v.h = h;
                v.pos = np as usize;
                v.fit_pending = true;
            }
        }
    }

    /// Opens a .zwo model in the 3D viewer. Textures come from the .dic/.dip with the same
    /// name next to it (like the Blender add-on does) or from loose images named like the texture.
    fn open_model(&mut self, index: usize) {
        let Some(archive) = &self.archive else { return };
        let entries = archive.entries();
        let path = entries[index].path.clone();
        let data = match archive.read(index) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Read error: {e}");
                return;
            }
        };
        let model = match obscure_models::parse_zwo(&data) {
            Ok(m) => m,
            Err(e) => {
                self.status = format!("Cannot read model '{path}': {e}");
                return;
            }
        };
        if model.meshes.is_empty() {
            self.status = if let Some(w) = model.warnings.first() {
                format!("Model '{path}' has nothing to show ({w}).")
            } else if model.has_animations {
                format!("'{path}' only contains animation data (no geometry), so there is nothing to show yet.")
            } else {
                format!("Model '{path}' has no meshes.")
            };
            return;
        }

        let (dir, file) = match path.rsplit_once('/') {
            Some((d, f)) => (d.to_string(), f.to_string()),
            None => (String::new(), path.clone()),
        };
        let stem = file.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| file.clone());
        let mut by_name: HashMap<String, Arc<model3d::TexImage>> = HashMap::new();
        let mut add = |t: Texture| {
            by_name.entry(t.name.to_lowercase()).or_insert_with(|| {
                Arc::new(model3d::TexImage { w: t.width as usize, h: t.height as usize, rgba: t.rgba })
            });
        };

        // 1) texture packs: same name as the model, else every pack in the same folder
        let in_dir = |p: &str| match p.rsplit_once('/') {
            Some((d, _)) => d.eq_ignore_ascii_case(&dir),
            None => dir.is_empty(),
        };
        let mut packs: Vec<usize> = Vec::new();
        let mut same_dir_packs: Vec<usize> = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            if !in_dir(&e.path) {
                continue;
            }
            let name = base_name(&e.path);
            if is_pack_ext(&ext_of(name)) {
                same_dir_packs.push(i);
                let s = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
                if s.eq_ignore_ascii_case(&stem) {
                    packs.push(i);
                }
            }
        }
        if packs.is_empty() {
            packs = same_dir_packs.into_iter().take(8).collect();
        }
        for i in packs {
            if let Ok(d) = archive.read(i) {
                if let Some(Ok(textures)) = decode_any(&entries[i].path, &d, &self.game_ctx) {
                    for t in textures {
                        add(t);
                    }
                }
            }
        }

        // 2) loose images (.bmp/.tga/.png/.jpg) named like the texture
        let wanted: HashSet<String> = model
            .materials
            .iter()
            .map(|m| m.texture.to_lowercase())
            .filter(|n| !n.is_empty())
            .collect();
        let mut loose: HashMap<String, usize> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            let name = base_name(&e.path);
            let ext = ext_of(name);
            if !matches!(ext.as_str(), "bmp" | "tga" | "png" | "jpg" | "jpeg") {
                continue;
            }
            let s = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name).to_lowercase();
            if wanted.contains(&s) {
                // prefer the file next to the model
                if in_dir(&e.path) || !loose.contains_key(&s) {
                    loose.insert(s, i);
                }
            }
        }
        for (name, i) in loose {
            if let Some(d) = archive.read(i).ok() {
                if let Some(Ok(textures)) = decode_any(&entries[i].path, &d, &self.game_ctx) {
                    if let Some(t) = textures.into_iter().next() {
                        add(Texture { name: name.clone(), ..t });
                    }
                }
            }
        }

        let scene = model3d::build_scene(&model, &by_name);
        if scene.triangle_count() == 0 {
            let note = model.warnings.first().cloned().unwrap_or_default();
            self.status = format!(
                "Model '{path}': {} mesh(es) read but no triangles to draw (instances without geometry?) {note}",
                model.meshes.len()
            );
            return;
        }
        self.model_viewer = Some(model3d::ModelViewer::new(file, scene, model.warnings.clone()));
    }

    fn open_text(&mut self, index: usize) {
        let Some(archive) = &self.archive else { return };
        let name = base_name(&archive.entries()[index].path).to_string();
        let data = match archive.read(index) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Read error: {e}");
                return;
            }
        };
        if !looks_like_text(&data) {
            return; // not really text: nothing to show
        }
        let n = data.len().min(256 * 1024);
        let mut text = String::from_utf8_lossy(&data[..n]).into_owned();
        if data.len() > n {
            text.push_str("\n\n... (file truncated for display)");
        }
        self.text_view = Some(TextView { name, text, open: true });
    }

    /// Opens a texture pack as a virtual folder with all its images.
    fn open_container(&mut self, index: usize, egui_ctx: &egui::Context) {
        let Some(archive) = &self.archive else { return };
        let path = archive.entries()[index].path.clone();
        let data = match archive.read(index) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Read error: {e}");
                return;
            }
        };
        match decode_any(&path, &data, &self.game_ctx) {
            None => {
                self.status = format!(
                    "No texture parser for '{path}' with the current Game/Platform selection."
                );
            }
            Some(Err(e)) => {
                self.status = format!("Cannot decode '{path}': {e}");
            }
            Some(Ok(textures)) => {
                let mut list = Vec::new();
                for (i, t) in textures.into_iter().enumerate() {
                    let Texture { name, width, height, rgba, format } = t;
                    let thumb =
                        thumb_from_rgba(egui_ctx, width, height, rgba, format!("ctex_{index}_{i}"));
                    list.push(CTex { name, width, height, format, thumb });
                }
                let count = list.len();
                self.container = Some(Container {
                    index,
                    file_name: base_name(&path).to_string(),
                    textures: list,
                });
                self.cursor = None;
                self.sel_anchor = None;
                self.selection.clear();
                self.query.clear();
                let mut parts: Vec<String> =
                    path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                parts.pop();
                self.reveal(&parts, true);
                self.cwd = parts;
                self.tree_scroll = true;
                self.status = format!("{count} image(s) in {path}");
            }
        }
    }

    // ------------------------------------------------------------ audio player

    fn toggle_or_play_audio(&mut self, index: usize) {
        let loaded_ok = self
            .player
            .as_ref()
            .map(|p| p.index == index && p.audio.is_some())
            .unwrap_or(false);
        if loaded_ok {
            if let Some(ps) = &mut self.player {
                if let Some(a) = &mut ps.audio {
                    a.toggle();
                }
            }
        } else {
            self.play_audio_file(index);
        }
    }

    /// Plays an audio file; the playlist is the audio files of the current view.
    fn play_audio_file(&mut self, index: usize) {
        let mut list: Vec<usize> = self
            .view_targets()
            .into_iter()
            .filter_map(|t| match t {
                Target::File(i) if self.is_audio_index(i) => Some(i),
                _ => None,
            })
            .collect();
        let pos = match list.iter().position(|&i| i == index) {
            Some(p) => p,
            None => {
                list = vec![index];
                0
            }
        };
        self.play_track(index, list, pos);
    }

    fn play_track(&mut self, index: usize, playlist: Vec<usize>, pos: usize) {
        let Some(archive) = &self.archive else { return };
        let title = base_name(&archive.entries()[index].path).to_string();
        let ext = ext_of(&title);
        let data = match archive.read(index) {
            Ok(d) => d,
            Err(e) => {
                self.player = Some(PlayerState {
                    index,
                    title: title.clone(),
                    audio: None,
                    error: Some(format!("Could not read '{title}': {e}")),
                    playlist,
                    pos,
                    drag_value: None,
                });
                return;
            }
        };
        let head: String = data.iter().take(16).map(|b| format!("{b:02x} ")).collect();
        let mut result = AudioPlayer::from_file(data.clone());
        if result.is_err() {
            // last resort: let ffmpeg decode it
            if let Some(ff) = &self.ffmpeg {
                let file = self.temp.path_for(&format!("audio_{index}.{ext}"));
                if std::fs::write(&file, &data).is_ok() {
                    if let Some(samples) = ff.audio_pcm(&file) {
                        if let Ok(p) = AudioPlayer::from_pcm(samples, 2, 44100) {
                            result = Ok(p);
                        }
                    }
                    let _ = std::fs::remove_file(&file);
                }
            }
        }
        let (audio, error) = match result {
            Ok(mut a) => {
                a.play();
                (Some(a), None)
            }
            Err(e) => (
                None,
                Some(format!("Could not play '{title}': {e}\nFile starts with: {head}")),
            ),
        };
        self.player = Some(PlayerState { index, title, audio, error, playlist, pos, drag_value: None });
    }

    fn player_step(&mut self, dir: isize) {
        let (playlist, pos, elapsed) = match &self.player {
            Some(ps) => (
                ps.playlist.clone(),
                ps.pos as isize,
                ps.audio.as_ref().map(|a| a.position()).unwrap_or_default(),
            ),
            None => return,
        };
        if dir < 0 && elapsed.as_secs_f32() > 3.0 {
            // like most players: "previous" restarts the track if it already played a bit
            if let Some(ps) = &mut self.player {
                if let Some(a) = &mut ps.audio {
                    a.seek(Duration::ZERO);
                    a.play();
                }
            }
            return;
        }
        let np = pos + dir;
        if np < 0 || np as usize >= playlist.len() {
            return;
        }
        let idx = playlist[np as usize];
        self.play_track(idx, playlist, np as usize);
    }

    /// Space: play / pause the audio player (or start the file under the cursor).
    fn toggle_play(&mut self) {
        if self.video.is_some() {
            return; // the video player handles Space itself
        }
        if let Some(ps) = &mut self.player {
            if let Some(a) = &mut ps.audio {
                a.toggle();
                return;
            }
        }
        if let Some(c) = self.cursor {
            if let Some(Target::File(i)) = self.view_targets().get(c) {
                if self.is_audio_index(*i) {
                    let i = *i;
                    self.play_audio_file(i);
                }
            }
        }
    }

    // ------------------------------------------------------------ video

    /// Opens the full-window video player (asks to set up ffmpeg when it is missing).
    fn start_video(&mut self, index: usize, ctx: &egui::Context, now: f64) {
        if self.ffmpeg.is_none() {
            self.ffmpeg = find_ffmpeg();
        }
        let Some(ff) = self.ffmpeg.clone() else {
            self.ffmpeg_prompt = true;
            return;
        };
        // close the previous video (when stepping through the playlist)
        if let Some(old) = self.video.take() {
            let f = old.file.clone();
            drop(old);
            let _ = std::fs::remove_file(f);
        }
        let mut playlist: Vec<usize> = self
            .view_targets()
            .into_iter()
            .filter_map(|t| match t {
                Target::File(i) if is_video_ext(&self.ext_of_index(i)) => Some(i),
                _ => None,
            })
            .collect();
        let pos = match playlist.iter().position(|&i| i == index) {
            Some(p) => p,
            None => {
                playlist = vec![index];
                0
            }
        };
        let Some(archive) = &self.archive else { return };
        let name = base_name(&archive.entries()[index].path).to_string();
        let ext = ext_of(&name);
        let data = match archive.read(index) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Read error: {e}");
                return;
            }
        };
        let file = self.temp.path_for(&format!("play_{index}.{ext}"));
        if let Err(e) = std::fs::write(&file, &data) {
            self.status = format!("Temp file error: {e}");
            return;
        }
        // probe the whole file: duration estimates from a truncated file are wrong
        let Some(info) = ff.probe(&file) else {
            let _ = std::fs::remove_file(&file);
            self.status = format!("ffmpeg could not read '{name}'.");
            return;
        };
        // pause the music while a video plays
        if let Some(ps) = &mut self.player {
            if let Some(a) = &mut ps.audio {
                a.pause();
            }
        }
        let player = VideoPlayer::start(&ff, file.clone(), info, ctx);
        self.video = Some(VideoSession {
            name,
            file,
            player,
            drag_value: None,
            opened_at: now,
            last_seek: 0.0,
            playlist,
            pos,
        });
    }

    fn step_video(&mut self, step: isize, ctx: &egui::Context, now: f64) {
        let (playlist, pos) = match &self.video {
            Some(v) => (v.playlist.clone(), v.pos as isize),
            None => return,
        };
        let np = pos + step;
        if np < 0 || np as usize >= playlist.len() {
            return;
        }
        self.start_video(playlist[np as usize], ctx, now);
    }

    // ------------------------------------------------------------ selection / export

    fn clear_selection(&mut self) {
        self.selection.clear();
        self.sel_anchor = None;
    }

    fn range_select(&mut self, vi: usize) {
        let targets = self.view_targets();
        if targets.is_empty() {
            return;
        }
        let a = self.sel_anchor.unwrap_or(vi).min(targets.len() - 1);
        let (lo, hi) = (a.min(vi), a.max(vi).min(targets.len() - 1));
        for t in &targets[lo..=hi] {
            let k = self.target_key(t);
            self.selection.insert(k);
        }
        self.sel_anchor = Some(vi);
    }

    fn select_all(&mut self) {
        for t in self.view_targets() {
            let k = self.target_key(&t);
            self.selection.insert(k);
        }
    }

    fn begin_export(&mut self, targets: Vec<ExpTarget>) {
        let Some(archive) = self.archive.as_deref() else { return };
        if targets.is_empty() {
            return;
        }
        let entries = archive.entries();
        let (mut n_files, mut has_tex) = (0usize, false);
        let tex_only = targets.iter().all(|t| matches!(t, ExpTarget::Tex(..)));
        let is_tex = |i: usize| {
            let e = ext_of(base_name(&entries[i].path));
            is_pack_ext(&e) || is_image_ext(&e)
        };
        for t in &targets {
            match t {
                ExpTarget::File(i) => {
                    n_files += 1;
                    has_tex |= is_tex(*i);
                }
                ExpTarget::Tex(..) => has_tex = true,
                ExpTarget::Folder(p) => {
                    let prefix = if p.is_empty() { String::new() } else { format!("{}/", p.join("/")) };
                    for (i, e) in entries.iter().enumerate() {
                        if e.path.starts_with(&prefix) {
                            n_files += 1;
                            has_tex |= is_tex(i);
                        }
                    }
                }
            }
        }
        self.export = Some(ExportDialog {
            targets,
            n_files,
            has_tex,
            tex_only,
            raw: !tex_only,
            png: has_tex,
            dest: None,
            job: None,
            message: None,
        });
    }

    fn start_export(&mut self) {
        let Some(archive) = self.archive.as_deref() else { return };
        let Some(ex) = &mut self.export else { return };
        let Some(dest) = ex.dest.clone() else { return };
        let entries = archive.entries();
        let (raw, png) = (ex.raw && !ex.tex_only, ex.png);
        let mut items: Vec<ExportItem> = Vec::new();
        let mut seen: SeenSet = HashSet::new();
        for t in &ex.targets {
            match t {
                ExpTarget::Tex(pack, i) => {
                    add_export_item(&mut items, &mut seen, *pack, Some(*i), ExportKind::Png)
                }
                ExpTarget::File(i) => add_file_items(entries, &mut items, &mut seen, *i, raw, png),
                ExpTarget::Folder(p) => {
                    let prefix =
                        if p.is_empty() { String::new() } else { format!("{}/", p.join("/")) };
                    for (i, e) in entries.iter().enumerate() {
                        if e.path.starts_with(&prefix) {
                            add_file_items(entries, &mut items, &mut seen, i, raw, png);
                        }
                    }
                }
            }
        }
        ex.job = Some(ExportJob {
            items,
            pos: 0,
            ok: 0,
            failed: 0,
            last_error: None,
            cancel: false,
            dest,
        });
    }

    /// Runs the export for a few milliseconds per frame (the archive cannot leave this thread).
    fn step_export(&mut self) {
        let Some(ex) = &mut self.export else { return };
        let Some(job) = &mut ex.job else { return };
        let Some(archive) = self.archive.as_deref() else {
            ex.job = None;
            return;
        };
        let t0 = Instant::now();
        while job.pos < job.items.len() && !job.cancel && t0.elapsed() < Duration::from_millis(25) {
            let item = job.items[job.pos];
            job.pos += 1;
            match export_one(archive, &item, &job.dest, &self.game_ctx) {
                Ok(()) => job.ok += 1,
                Err(e) => {
                    job.failed += 1;
                    if job.last_error.is_none() {
                        job.last_error = Some(e);
                    }
                }
            }
        }
        if job.cancel || job.pos >= job.items.len() {
            let mut msg = format!(
                "{} file(s) exported to {}{}",
                job.ok,
                job.dest.display(),
                if job.cancel { " (cancelled)" } else { "" }
            );
            if job.failed > 0 {
                msg.push_str(&format!(
                    "\n{} failed. First error: {}",
                    job.failed,
                    job.last_error.clone().unwrap_or_default()
                ));
            }
            ex.message = Some(msg);
            ex.job = None;
        }
    }

    // ------------------------------------------------------------ keyboard

    fn overlay_open(&self) -> bool {
        self.viewer.is_some()
            || self.video.is_some()
            || self.text_view.is_some()
            || self.model_viewer.is_some()
            || self.export.is_some()
            || (self.ffmpeg_prompt && self.ffmpeg.is_none())
    }

    fn handle_keys(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.overlay_open() {
            return; // the overlays handle their own keys
        }
        // keyboard focus follows the pointer between the two panels
        if let Some(pos) = ctx.pointer_hover_pos() {
            if self.tree_rect.contains(pos) {
                self.focus = Focus::Tree;
            } else if self.items_rect.contains(pos) {
                self.focus = Focus::Items;
            }
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        let k = ctx.input(|i| {
            let free = !i.modifiers.ctrl && !i.modifiers.alt && !i.modifiers.command;
            let p = |key: egui::Key| free && i.key_pressed(key);
            Keys {
                left: p(egui::Key::ArrowLeft) || p(egui::Key::A),
                right: p(egui::Key::ArrowRight) || p(egui::Key::D),
                up: p(egui::Key::ArrowUp) || p(egui::Key::W),
                down: p(egui::Key::ArrowDown) || p(egui::Key::S),
                enter: p(egui::Key::Enter),
                space: p(egui::Key::Space),
                back: p(egui::Key::Backspace),
                esc: p(egui::Key::Escape),
                select_all: i.modifiers.command && i.key_pressed(egui::Key::A),
            }
        });
        if k.space {
            actions.push(Action::TogglePlay);
        }
        if k.back {
            actions.push(Action::Up);
        }
        if k.esc {
            actions.push(Action::ClearSelection);
        }
        if k.select_all {
            actions.push(Action::SelectAll);
        }
        if self.archive.is_none() {
            return;
        }
        match self.focus {
            Focus::Tree => self.tree_key(&k, actions),
            Focus::Items => self.items_key(&k, actions),
        }
    }

    fn items_key(&mut self, k: &Keys, actions: &mut Vec<Action>) {
        let targets = self.view_targets();
        let n = targets.len();
        if n == 0 {
            return;
        }
        if k.enter {
            if let Some(c) = self.cursor {
                match targets.get(c) {
                    Some(Target::Folder(name)) => {
                        let mut p = self.cwd.clone();
                        p.push(name.clone());
                        actions.push(Action::Enter(p));
                    }
                    Some(Target::File(i)) => actions.push(Action::Open(*i)),
                    Some(Target::Tex(i)) => actions.push(Action::OpenTex(*i)),
                    None => {}
                }
            }
            return;
        }
        if !(k.left || k.right || k.up || k.down) {
            return;
        }
        let cols: isize = if self.view == ViewMode::Grid { self.last_cols.max(1) as isize } else { 1 };
        let new = match self.cursor {
            None => 0isize,
            Some(c) => {
                let mut c = c as isize;
                if k.left {
                    c -= 1;
                }
                if k.right {
                    c += 1;
                }
                if k.up {
                    c -= cols;
                }
                if k.down {
                    c += cols;
                }
                c.clamp(0, n as isize - 1)
            }
        };
        self.cursor = Some(new as usize);
        self.scroll_cursor = true;
    }

    fn tree_key(&mut self, k: &Keys, actions: &mut Vec<Action>) {
        let len = self.tree_rows.len();
        if len == 0 {
            return;
        }
        let cur = self
            .tree_cursor
            .as_ref()
            .and_then(|key| self.tree_rows.iter().position(|r| &r.key() == key))
            .or_else(|| self.tree_rows.iter().position(|r| r.file.is_none() && r.path == self.cwd))
            .unwrap_or(0);
        let (has_children, expanded, depth, file) = {
            let r = &self.tree_rows[cur];
            (r.has_children, r.expanded, r.depth, r.file)
        };
        if k.enter {
            if let Some(i) = file {
                actions.push(Action::Open(i));
            }
            return;
        }
        let mut target: Option<usize> = None;
        if k.up && cur > 0 {
            target = Some(cur - 1);
        }
        if k.down && cur + 1 < len {
            target = Some(cur + 1);
        }
        if k.right {
            if let Some(i) = file {
                actions.push(Action::Open(i));
            } else if cur == 0 && len > 1 {
                target = Some(1);
            } else if has_children && !expanded {
                let key = self.tree_rows[cur].path.join("/");
                self.expanded.insert(key);
                self.tree_dirty = true;
            } else if has_children && cur + 1 < len && self.tree_rows[cur + 1].depth > depth {
                target = Some(cur + 1);
            }
        }
        if k.left {
            if file.is_none() && has_children && expanded {
                let key = self.tree_rows[cur].path.join("/");
                self.expanded.remove(&key);
                self.tree_dirty = true;
            } else if depth >= 1 {
                // go to the parent folder row
                let want = depth - 1;
                if let Some(p) = (0..cur).rev().find(|&i| self.tree_rows[i].depth == want) {
                    target = Some(p);
                }
            }
        }
        if let Some(t) = target {
            let key = self.tree_rows[t].key();
            if self.tree_rows[t].file.is_none() {
                let path = self.tree_rows[t].path.clone();
                self.cd(path);
            }
            // files are only highlighted; Enter / Right opens them
            self.tree_cursor = Some(key);
            self.tree_scroll = true;
        }
    }
}

type SeenSet = HashSet<(usize, Option<usize>, ExportKind)>;

fn add_export_item(
    items: &mut Vec<ExportItem>,
    seen: &mut SeenSet,
    index: usize,
    only: Option<usize>,
    kind: ExportKind,
) {
    if seen.insert((index, only, kind)) {
        items.push(ExportItem { index, only_tex: only, kind });
    }
}

fn add_file_items(
    entries: &[Entry],
    items: &mut Vec<ExportItem>,
    seen: &mut SeenSet,
    i: usize,
    raw: bool,
    png: bool,
) {
    if raw {
        add_export_item(items, seen, i, None, ExportKind::Raw);
    }
    if png {
        let e = ext_of(base_name(&entries[i].path));
        if is_pack_ext(&e) || is_image_ext(&e) {
            add_export_item(items, seen, i, None, ExportKind::Png);
        }
    }
}

// ---------------------------------------------------------------- export of one item

fn save_png(path: &Path, t: &Texture) -> Result<(), String> {
    image::save_buffer(path, &t.rgba, t.width, t.height, image::ColorType::Rgba8)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn export_one(
    archive: &dyn Archive,
    item: &ExportItem,
    dest: &Path,
    gctx: &Context,
) -> Result<(), String> {
    let path = archive
        .entries()
        .get(item.index)
        .map(|e| e.path.clone())
        .ok_or_else(|| "entry index out of range".to_string())?;
    let data = archive.read(item.index).map_err(|e| format!("{path}: {e}"))?;
    match item.kind {
        ExportKind::Raw => {
            let out = dest.join(&path);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{path}: {e}"))?;
            }
            std::fs::write(&out, data).map_err(|e| format!("{path}: {e}"))
        }
        ExportKind::Png => {
            let textures = match decode_any(&path, &data, gctx) {
                Some(Ok(t)) => t,
                Some(Err(e)) => return Err(format!("{path}: {e}")),
                None => return Err(format!("{path}: no decoder for the current Game/Platform")),
            };
            let ext = ext_of(base_name(&path));
            let base = dest.join(&path);
            if is_pack_ext(&ext) || textures.len() > 1 {
                let dir = base.with_extension("");
                std::fs::create_dir_all(&dir).map_err(|e| format!("{path}: {e}"))?;
                let mut used: HashSet<String> = HashSet::new();
                for (i, t) in textures.iter().enumerate() {
                    if let Some(k) = item.only_tex {
                        if i != k {
                            continue;
                        }
                    }
                    let mut name = sanitize(&t.name);
                    if name.is_empty() {
                        name = format!("texture_{i:03}");
                    }
                    if !used.insert(name.to_lowercase()) {
                        name = format!("{name}_{i}");
                        used.insert(name.to_lowercase());
                    }
                    save_png(&dir.join(format!("{name}.png")), t)?;
                }
                Ok(())
            } else {
                let Some(t) = textures.first() else {
                    return Err(format!("{path}: no image inside"));
                };
                if let Some(parent) = base.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("{path}: {e}"))?;
                }
                save_png(&base.with_extension("png"), t)
            }
        }
    }
}

// ---------------------------------------------------------------- utilities

fn hashes_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("hashes");
            if p.is_dir() {
                return p;
            }
        }
    }
    PathBuf::from("hashes")
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c })
        .collect()
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn ext_of(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

fn short_format(f: &str) -> &str {
    f.split(',').next().unwrap_or(f)
}

fn fmt_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    }
}

fn is_text_ext(ext: &str) -> bool {
    matches!(
        ext,
        "txt" | "log" | "ini" | "xml" | "cfg" | "json" | "csv" | "md" | "bat" | "inf" | "lng" | "cre" | "sen" | "sub"
    )
}

/// Animation files are `.zwo` files with "anim" in the name (anims_pc.zwo, crabanim.zwo, ...):
/// they only hold skeletal animations and cameras, no geometry.
fn is_anim_zwo(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".zwo") && lower.contains("anim")
}

const ANIM_COLOR: Color32 = Color32::from_rgb(214, 120, 40);

fn file_type(name: &str, ext: &str) -> (&'static str, Color32) {
    if is_anim_zwo(name) {
        ("Animation", ANIM_COLOR)
    } else {
        type_info(ext)
    }
}

fn type_info(ext: &str) -> (&'static str, Color32) {
    match ext {
        "dic" | "dip" => ("Texture pack", Color32::from_rgb(120, 86, 200)),
        "hvt" | "hvi" | "xbr" => ("Texture", Color32::from_rgb(120, 86, 200)),
        "bmp" | "tga" | "png" | "jpg" | "jpeg" => ("Image", Color32::from_rgb(150, 90, 190)),
        "wav" | "ogg" | "mp3" | "flac" => ("Audio", Color32::from_rgb(30, 150, 150)),
        "bik" | "ogv" => ("Video", Color32::from_rgb(200, 70, 70)),
        "lng" | "cre" | "sen" | "sub" | "txt" | "log" | "ini" | "xml" | "cfg" | "json" | "csv" => {
            ("Text", Color32::from_rgb(60, 150, 90))
        }
        "zwo" => ("Model", Color32::from_rgb(50, 120, 200)),
        "pso" | "vso" => ("Shader", Color32::from_rgb(200, 120, 40)),
        "xmc" => ("UI", Color32::from_rgb(190, 90, 140)),
        "hoe" | "map" | "tm" | "noe" | "dat" => ("Data", Color32::from_rgb(70, 130, 140)),
        _ => ("File", Color32::from_rgb(90, 96, 110)),
    }
}

fn looks_like_text(data: &[u8]) -> bool {
    !data.is_empty()
        && data.iter().take(4096).all(|&b| {
            b == b'\n' || b == b'\r' || b == b'\t' || (0x20..0x7F).contains(&b) || b >= 0x80
        })
}

/// New scroll offset needed to make `row` visible, if any.
fn want_scroll(off_y: f32, viewport: f32, row: usize, pitch: f32, h: f32) -> Option<f32> {
    let top = row as f32 * pitch;
    let bottom = top + h;
    if top < off_y {
        Some(top)
    } else if bottom > off_y + viewport {
        Some((bottom - viewport).max(0.0))
    } else {
        None
    }
}

// ---------------------------------------------------------------- icons

fn paint_folder(p: &egui::Painter, r: Rect, pal: &Palette) {
    let w = r.width();
    let h = r.height();
    let tab = Rect::from_min_size(r.min + vec2(w * 0.08, h * 0.18), vec2(w * 0.38, h * 0.16));
    let body = Rect::from_min_max(
        pos2(r.min.x + w * 0.08, r.min.y + h * 0.28),
        pos2(r.max.x - w * 0.08, r.max.y - h * 0.18),
    );
    p.rect_filled(tab, 4.0, pal.folder_tab);
    p.rect_filled(body, 5.0, pal.folder_body);
}

fn paint_file(p: &egui::Painter, r: Rect, ext: &str, accent: Color32, pal: &Palette) {
    let page = Rect::from_center_size(r.center(), vec2(r.width() * 0.62, r.height() * 0.8));
    p.rect_filled(page, 4.0, pal.page);
    let band = Rect::from_min_max(pos2(page.min.x, page.max.y - page.height() * 0.28), page.max);
    p.rect_filled(band, 4.0, accent);
    let label = if ext.is_empty() { "?".to_string() } else { ext.to_uppercase() };
    p.text(
        band.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional((r.height() * 0.16).clamp(8.0, 18.0)),
        Color32::WHITE,
    );
}

fn paint_thumb(p: &egui::Painter, r: Rect, h: &egui::TextureHandle, pal: &Palette) {
    p.rect_filled(r, 4.0, pal.thumb_bg);
    let sz = h.size_vec2();
    let scale = (r.width() / sz.x).min(r.height() / sz.y);
    let img = Rect::from_center_size(r.center(), sz * scale);
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    p.image(h.id(), img, uv, Color32::WHITE);
}

/// A "stack of images" look for texture packs (files that contain several images).
fn paint_pack(
    p: &egui::Painter,
    r: Rect,
    thumb: Option<&egui::TextureHandle>,
    ext: &str,
    pal: &Palette,
) {
    let base = r.shrink(r.width() * 0.08);
    let dx = base.width() * 0.035;
    let dy = base.height() * 0.035;
    let back2 = base.translate(vec2(dx * 2.0, -dy * 2.0));
    let back1 = base.translate(vec2(dx, -dy));
    let front = base.translate(vec2(-dx, dy));
    p.rect_filled(back2, 5.0, pal.pack_accent.gamma_multiply(0.45));
    p.rect_filled(back1, 5.0, pal.pack_accent.gamma_multiply(0.7));
    match thumb {
        Some(h) => paint_thumb(p, front, h, pal),
        None => paint_file(p, front, ext, pal.pack_accent, pal),
    }
}

/// Small stacked-images icon used in the folder tree for texture packs.
fn paint_pack_icon(p: &egui::Painter, r: Rect, pal: &Palette) {
    let a = Rect::from_min_size(r.min + vec2(3.0, 0.0), vec2(r.width() - 4.0, r.height() - 4.0));
    let b = a.translate(vec2(-3.0, 4.0));
    p.rect_filled(a, 2.0, pal.pack_accent.gamma_multiply(0.6));
    p.rect_filled(b, 2.0, pal.pack_accent);
}

fn paint_tag(p: &egui::Painter, top_left: Pos2, text: &str, bg: Color32) {
    let galley = p.layout_no_wrap(text.to_string(), FontId::proportional(10.0), Color32::WHITE);
    let size = galley.size() + vec2(10.0, 4.0);
    let rect = Rect::from_min_size(top_left, size);
    p.rect_filled(rect, 4.0, bg);
    p.galley(rect.min + vec2(5.0, 2.0), galley, Color32::WHITE);
}

#[derive(Clone, Copy)]
enum Badge {
    /// Not the loaded track / a video.
    Play,
    /// The loaded track, currently paused.
    Active,
    /// The loaded track, currently playing (shows a pause symbol).
    Pause,
}

fn badge_radius(r: Rect) -> f32 {
    (r.width().min(r.height()) * 0.18).clamp(8.0, 22.0)
}

fn badge_rect(r: Rect) -> Rect {
    let rad = badge_radius(r);
    Rect::from_center_size(r.center(), vec2(rad * 2.0, rad * 2.0))
}

fn paint_badge(p: &egui::Painter, r: Rect, kind: Badge) {
    let c = r.center();
    let rad = badge_radius(r);
    let bg = match kind {
        Badge::Play => Color32::from_black_alpha(150),
        _ => Color32::from_rgb(30, 160, 90),
    };
    p.circle_filled(c, rad, bg);
    match kind {
        Badge::Pause => {
            let bar = vec2(rad * 0.32, rad * 1.0);
            p.rect_filled(Rect::from_center_size(pos2(c.x - rad * 0.28, c.y), bar), 1.0, Color32::WHITE);
            p.rect_filled(Rect::from_center_size(pos2(c.x + rad * 0.28, c.y), bar), 1.0, Color32::WHITE);
        }
        _ => {
            let tri = vec![
                pos2(c.x - rad * 0.3, c.y - rad * 0.5),
                pos2(c.x - rad * 0.3, c.y + rad * 0.5),
                pos2(c.x + rad * 0.55, c.y),
            ];
            p.add(egui::Shape::convex_polygon(tri, Color32::WHITE, Stroke::NONE));
        }
    }
}

fn audio_badge_kind(env: &Env, index: usize) -> Badge {
    if env.playing == Some(index) {
        if env.player_active {
            Badge::Pause
        } else {
            Badge::Active
        }
    } else {
        Badge::Play
    }
}

/// Makes the badge clickable: audio toggles the player, video opens the video player.
fn badge_click(
    ui: &mut egui::Ui,
    item: &Item,
    cwd: &[String],
    badge: Option<(Rect, usize, bool)>,
    actions: &mut Vec<Action>,
) {
    if let Some((rect, index, is_audio)) = badge {
        let br = ui.interact(rect, item_id(item, cwd).with("badge"), Sense::click());
        if br.clicked() {
            actions.push(if is_audio { Action::ToggleAudio(index) } else { Action::PlayVideo(index) });
        }
    }
}

/// Selection checkbox in the top-right corner of a tile (shown on hover or while selecting).
fn selection_checkbox(
    ui: &mut egui::Ui,
    p: &egui::Painter,
    cb: Rect,
    id: egui::Id,
    selected: bool,
    pal: &Palette,
    key: String,
    vi: usize,
    actions: &mut Vec<Action>,
) {
    let fill = if selected { pal.pack_accent } else { Color32::from_black_alpha(130) };
    p.rect_filled(cb, 4.0, fill);
    if selected {
        let st = Stroke::new(2.0_f32, Color32::WHITE);
        let c = cb.center();
        p.line_segment([pos2(c.x - 4.0, c.y), pos2(c.x - 1.0, c.y + 3.5)], st);
        p.line_segment([pos2(c.x - 1.0, c.y + 3.5), pos2(c.x + 5.0, c.y - 4.0)], st);
    } else {
        p.rect_stroke(cb, 4.0, Stroke::new(1.0_f32, Color32::from_gray(210)));
    }
    let r = ui.interact(cb, id.with("select"), Sense::click());
    if r.clicked() {
        actions.push(Action::ToggleSelect(key, vi));
    }
}

/// Tiny file icon for the folder tree.
fn paint_small_file(p: &egui::Painter, r: Rect, accent: Color32, pal: &Palette) {
    let page = Rect::from_min_size(r.min + vec2(2.0, 0.0), vec2(r.width() - 4.0, r.height()));
    p.rect_filled(page, 2.0, pal.page);
    let band = Rect::from_min_max(pos2(page.min.x, page.max.y - 5.0), page.max);
    p.rect_filled(band, 2.0, accent);
}

/// Thumbnail for a file (images/textures synchronously, videos through ffmpeg threads).
fn item_thumb(env: &Env, thumbs: &mut Thumbs, ext: &str, index: usize) -> Option<ThumbInfo> {
    if is_pack_ext(ext) || is_image_ext(ext) {
        thumbs.img.get(env.egui_ctx, env.archive, &env.game_ctx, index)
    } else if is_video_ext(ext) {
        let ff = env.ffmpeg?;
        thumbs
            .vid
            .get(env.egui_ctx, ff, env.temp, env.archive, index, &mut thumbs.vid_budget)
            .map(|handle| ThumbInfo { handle, count: 1 })
    } else {
        None
    }
}

fn pack_tag_text(count: Option<usize>) -> String {
    match count {
        Some(n) => format!("PACK - {n} img"),
        None => "PACK".to_string(),
    }
}

// ---------------------------------------------------------------- grid / list

fn item_id(item: &Item, cwd: &[String]) -> egui::Id {
    match item {
        Item::Folder { name, .. } => egui::Id::new(("tile_folder", cwd.join("/"), *name)),
        Item::File { index, .. } => egui::Id::new(("tile_file", *index)),
        Item::Tex { i, .. } => egui::Id::new(("tile_tex", *i)),
    }
}

fn grid_tile(
    ui: &mut egui::Ui,
    env: &Env,
    thumbs: &mut Thumbs,
    item: &Item,
    vi: usize,
    size: f32,
    cwd: &[String],
    actions: &mut Vec<Action>,
) {
    let tile_w = size + 16.0;
    let tile_h = size + 62.0;
    let (rect, _) = ui.allocate_exact_size(vec2(tile_w, tile_h), Sense::hover());
    let id = item_id(item, cwd);
    // explicit id: stays the same when the grid is re-laid out
    let resp = ui.interact(rect, id, Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let pal = &env.pal;
    let key = item_key(item, cwd);
    let is_sel = env.selection.contains(&key);

    let (name, tooltip) = match item {
        Item::Folder { name, .. } => (*name, name.to_string()),
        Item::File { name, index } => (*name, env.archive.entries()[*index].path.clone()),
        Item::Tex { name, width, height, format, .. } => {
            (*name, format!("{name}\n{width}x{height}\n{format}"))
        }
    };
    let is_pack = match item {
        Item::File { name, .. } => is_pack_ext(&ext_of(name)),
        _ => false,
    };
    let resp = resp.on_hover_text(tooltip);

    let highlighted = is_sel || env.cursor == Some(vi);
    let on_sel = highlighted && env.items_focused;
    let bg = if on_sel {
        ui.visuals().selection.bg_fill
    } else if highlighted {
        pal.sel_muted
    } else if resp.hovered() {
        pal.tile_hover
    } else if is_pack {
        pal.pack_bg
    } else {
        pal.tile
    };
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, bg);
    let icon_rect = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(size, size));

    let mut badge: Option<(Rect, usize, bool)> = None;
    let sub = match item {
        Item::Folder { subfolders, files, .. } => {
            paint_folder(&p, icon_rect, pal);
            format!("Subfolders: {subfolders}\nFiles: {files}")
        }
        Item::File { name, index } => {
            let ext = ext_of(name);
            let info = item_thumb(env, thumbs, &ext, *index);
            let (label, accent) = file_type(name, &ext);
            if is_pack {
                paint_pack(&p, icon_rect, info.as_ref().map(|i| &i.handle), &ext, pal);
                let count = info.as_ref().map(|i| i.count);
                paint_tag(
                    &p,
                    rect.min + vec2(6.0, 6.0),
                    &pack_tag_text(count),
                    pal.pack_accent,
                );
                match count {
                    Some(n) => format!("{label}\n{n} images"),
                    None => format!("{label}\n(click to open)"),
                }
            } else {
                match &info {
                    Some(i) => paint_thumb(&p, icon_rect, &i.handle, pal),
                    None => paint_file(&p, icon_rect, &ext, accent, pal),
                }
                if is_anim_zwo(name) {
                    paint_tag(&p, rect.min + vec2(6.0, 6.0), "ANIM", ANIM_COLOR);
                }
                if is_video_ext(&ext) {
                    paint_badge(&p, icon_rect, Badge::Play);
                    badge = Some((badge_rect(icon_rect), *index, false));
                } else if is_audio_ext(&ext) {
                    paint_badge(&p, icon_rect, audio_badge_kind(env, *index));
                    badge = Some((badge_rect(icon_rect), *index, true));
                }
                label.to_string()
            }
        }
        Item::Tex { width, height, format, thumb, .. } => {
            match thumb {
                Some(h) => paint_thumb(&p, icon_rect, h, pal),
                None => paint_file(&p, icon_rect, "tex", pal.pack_accent, pal),
            }
            format!("{width}x{height}\n{}", short_format(format))
        }
    };

    let (text_color, weak_color) = if on_sel {
        (Color32::WHITE, Color32::from_gray(220))
    } else {
        (pal.text, pal.weak)
    };
    let mut job = egui::text::LayoutJob::simple(
        name.to_string(),
        FontId::proportional(12.5),
        text_color,
        tile_w - 12.0,
    );
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = true;
    let galley = p.layout_job(job);
    p.galley(pos2(rect.min.x + 6.0, icon_rect.max.y + 6.0), galley, text_color);

    let lines = sub.lines().count() as f32;
    p.text(
        pos2(rect.min.x + 6.0, rect.max.y - 6.0 - 12.0 * lines),
        Align2::LEFT_TOP,
        sub,
        FontId::proportional(10.5),
        weak_color,
    );

    handle_clicks(env, &resp, item, vi, cwd, actions);
    badge_click(ui, item, cwd, badge, actions);
    if is_sel || !env.selection.is_empty() || ui.rect_contains_pointer(rect) {
        let cb = Rect::from_min_size(pos2(rect.max.x - 26.0, rect.min.y + 6.0), vec2(18.0, 18.0));
        selection_checkbox(ui, &p, cb, id, is_sel, pal, key, vi, actions);
    }
    resp.context_menu(|ui| {
        if ui.button("Export...").clicked() {
            actions.push(Action::Export(export_targets_for(item, cwd, env)));
            ui.close_menu();
        }
    });
}

fn handle_clicks(
    env: &Env,
    resp: &egui::Response,
    item: &Item,
    vi: usize,
    cwd: &[String],
    actions: &mut Vec<Action>,
) {
    // ignore the second click of a double click that just entered a folder / pack
    if env.now - env.last_enter < 0.35 {
        return;
    }
    if !resp.clicked() {
        return;
    }
    let mods = resp.ctx.input(|i| i.modifiers);
    if mods.shift {
        actions.push(Action::RangeSelect(vi));
        return;
    }
    if mods.ctrl || mods.command {
        actions.push(Action::ToggleSelect(item_key(item, cwd), vi));
        return;
    }
    actions.push(Action::Cursor(vi));
    match item {
        Item::Folder { name, .. } => {
            let mut path = cwd.to_vec();
            path.push(name.to_string());
            actions.push(Action::Enter(path));
        }
        Item::File { index, .. } => actions.push(Action::Open(*index)),
        Item::Tex { i, .. } => actions.push(Action::OpenTex(*i)),
    }
}

fn list_row(
    ui: &mut egui::Ui,
    env: &Env,
    thumbs: &mut Thumbs,
    item: &Item,
    vi: usize,
    row_h: f32,
    cwd: &[String],
    actions: &mut Vec<Action>,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
    let id = item_id(item, cwd);
    let resp = ui.interact(rect, id, Sense::click());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let pal = &env.pal;
    let key = item_key(item, cwd);
    let is_sel = env.selection.contains(&key);
    let name = match item {
        Item::Folder { name, .. } => *name,
        Item::File { name, .. } => *name,
        Item::Tex { name, .. } => *name,
    };
    let is_pack = match item {
        Item::File { name, .. } => is_pack_ext(&ext_of(name)),
        _ => false,
    };
    let highlighted = is_sel || env.cursor == Some(vi);
    let on_sel = highlighted && env.items_focused;
    let p = ui.painter_at(rect);
    if on_sel {
        p.rect_filled(rect, 3.0, ui.visuals().selection.bg_fill);
    } else if highlighted {
        p.rect_filled(rect, 3.0, pal.sel_muted);
    } else if resp.hovered() {
        p.rect_filled(rect, 3.0, pal.tile_hover);
    } else if is_pack {
        p.rect_filled(rect, 3.0, pal.pack_bg);
    }
    // selection checkbox on the left
    let cb = Rect::from_min_size(rect.min + vec2(4.0, (row_h - 16.0) / 2.0), vec2(16.0, 16.0));
    if is_sel || !env.selection.is_empty() || ui.rect_contains_pointer(rect) {
        selection_checkbox(ui, &p, cb, id, is_sel, pal, key.clone(), vi, actions);
    }
    let icon = Rect::from_min_size(rect.min + vec2(26.0, 2.0), vec2(row_h - 4.0, row_h - 4.0));
    let cy = rect.center().y;
    let type_x = rect.min.x + rect.width() * 0.60;
    let (text_color, weak_color) = if on_sel {
        (Color32::WHITE, Color32::from_gray(220))
    } else {
        (pal.text, pal.weak)
    };
    let small = FontId::proportional(12.0);
    let mut badge: Option<(Rect, usize, bool)> = None;

    match item {
        Item::Folder { subfolders, files, .. } => {
            paint_folder(&p, icon, pal);
            p.text(pos2(type_x, cy), Align2::LEFT_CENTER, "Folder", small.clone(), weak_color);
            p.text(
                pos2(rect.max.x - 8.0, cy),
                Align2::RIGHT_CENTER,
                format!("{subfolders} subfolders, {files} files"),
                small,
                weak_color,
            );
        }
        Item::File { index, .. } => {
            let ext = ext_of(name);
            let info = item_thumb(env, thumbs, &ext, *index);
            let (label, accent) = file_type(name, &ext);
            if is_pack {
                paint_pack(&p, icon, info.as_ref().map(|i| &i.handle), &ext, pal);
                p.text(pos2(type_x, cy), Align2::LEFT_CENTER, label, small.clone(), weak_color);
                if let Some(i) = &info {
                    p.text(
                        pos2(rect.max.x - 8.0, cy),
                        Align2::RIGHT_CENTER,
                        format!("{} images", i.count),
                        small,
                        weak_color,
                    );
                }
            } else {
                match &info {
                    Some(i) => paint_thumb(&p, icon, &i.handle, pal),
                    None => paint_file(&p, icon, &ext, accent, pal),
                }
                if is_video_ext(&ext) {
                    paint_badge(&p, icon, Badge::Play);
                    badge = Some((badge_rect(icon), *index, false));
                } else if is_audio_ext(&ext) {
                    paint_badge(&p, icon, audio_badge_kind(env, *index));
                    badge = Some((badge_rect(icon), *index, true));
                }
                p.text(pos2(type_x, cy), Align2::LEFT_CENTER, label, small.clone(), weak_color);
                let size = env.archive.entries()[*index].size;
                if size > 0 {
                    p.text(
                        pos2(rect.max.x - 8.0, cy),
                        Align2::RIGHT_CENTER,
                        fmt_size(size),
                        small,
                        weak_color,
                    );
                }
            }
        }
        Item::Tex { width, height, format, thumb, .. } => {
            match thumb {
                Some(h) => paint_thumb(&p, icon, h, pal),
                None => paint_file(&p, icon, "tex", pal.pack_accent, pal),
            }
            p.text(
                pos2(type_x, cy),
                Align2::LEFT_CENTER,
                short_format(format),
                small.clone(),
                weak_color,
            );
            p.text(
                pos2(rect.max.x - 8.0, cy),
                Align2::RIGHT_CENTER,
                format!("{width}x{height}"),
                small,
                weak_color,
            );
        }
    }
    p.text(
        pos2(icon.max.x + 8.0, cy),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(13.0),
        text_color,
    );

    handle_clicks(env, &resp, item, vi, cwd, actions);
    badge_click(ui, item, cwd, badge, actions);
    resp.context_menu(|ui| {
        if ui.button("Export...").clicked() {
            actions.push(Action::Export(export_targets_for(item, cwd, env)));
            ui.close_menu();
        }
    });
}

// ---------------------------------------------------------------- folder tree

fn tree_ui(
    ui: &mut egui::Ui,
    rows: &[TreeRow],
    cwd: &[String],
    container_open: Option<usize>,
    cursor_key: Option<&str>,
    pal: &Palette,
    focused: bool,
    actions: &mut Vec<Action>,
    pending: &mut Option<f32>,
    want_visible: &mut bool,
) {
    let row_h = 24.0;
    let active = |row: &TreeRow| -> bool {
        match row.file {
            Some(f) => container_open == Some(f),
            None => container_open.is_none() && row.path.as_slice() == cwd,
        }
    };
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut sa = egui::ScrollArea::vertical().auto_shrink([false, false]);
    if let Some(off) = pending.take() {
        sa = sa.vertical_scroll_offset(off);
    }
    let out = sa.show_rows(ui, row_h, rows.len(), |ui, range| {
        for i in range {
            let row = &rows[i];
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
            let resp = ui.interact(rect, ui.id().with(("tree_row", i)), Sense::click());
            let key = row.key();
            let is_cursor = cursor_key == Some(key.as_str());
            let hi = active(row) || is_cursor;
            let on_sel = hi && focused;
            let p = ui.painter_at(rect);
            if on_sel {
                p.rect_filled(rect, 3.0, ui.visuals().selection.bg_fill);
            } else if hi {
                p.rect_filled(rect, 3.0, pal.sel_muted);
            } else if resp.hovered() {
                p.rect_filled(rect, 3.0, pal.tile_hover);
            }
            let text_color = if on_sel { Color32::WHITE } else { pal.text };
            let x0 = rect.min.x + 4.0 + row.depth as f32 * 14.0;
            let cy = rect.center().y;

            let mut arrow_clicked = false;
            if row.has_children {
                let arrow_rect =
                    Rect::from_min_size(pos2(x0 - 2.0, rect.min.y), vec2(16.0, row_h));
                let ar = ui.interact(arrow_rect, ui.id().with(("tree_arrow", i)), Sense::click());
                if ar.clicked() {
                    arrow_clicked = true;
                    actions.push(Action::TreeFocus(Some(key.clone())));
                    actions.push(Action::Toggle(row.path.join("/")));
                }
                let cx = x0 + 6.0;
                let st = Stroke::new(1.5_f32, text_color);
                if row.expanded {
                    p.line_segment([pos2(cx - 4.0, cy - 2.0), pos2(cx, cy + 2.0)], st);
                    p.line_segment([pos2(cx, cy + 2.0), pos2(cx + 4.0, cy - 2.0)], st);
                } else {
                    p.line_segment([pos2(cx - 2.0, cy - 4.0), pos2(cx + 2.0, cy)], st);
                    p.line_segment([pos2(cx + 2.0, cy), pos2(cx - 2.0, cy + 4.0)], st);
                }
            }
            let icon = Rect::from_min_size(pos2(x0 + 16.0, cy - 8.0), vec2(16.0, 16.0));
            let ext = if row.file.is_some() { ext_of(&row.name) } else { String::new() };
            match row.file {
                Some(_) if is_pack_ext(&ext) => paint_pack_icon(&p, icon, pal),
                Some(_) => {
                    let (_, accent) = type_info(&ext);
                    paint_small_file(&p, icon, accent, pal);
                }
                None => paint_folder(&p, icon, pal),
            }
            p.text(
                pos2(x0 + 38.0, cy),
                Align2::LEFT_CENTER,
                row.name.as_str(),
                FontId::proportional(13.0),
                text_color,
            );
            if !arrow_clicked && resp.clicked() {
                actions.push(Action::TreeFocus(Some(key)));
                match row.file {
                    None => actions.push(Action::Cd(row.path.clone())),
                    Some(f) => actions.push(Action::Open(f)),
                }
            }
            resp.context_menu(|ui| {
                let label = if row.file.is_none() && row.path.is_empty() {
                    "Export everything..."
                } else {
                    "Export..."
                };
                if ui.button(label).clicked() {
                    let t = match row.file {
                        Some(f) => ExpTarget::File(f),
                        None => ExpTarget::Folder(row.path.clone()),
                    };
                    actions.push(Action::Export(vec![t]));
                    ui.close_menu();
                }
            });
        }
    });
    if *want_visible {
        *want_visible = false;
        let target = rows.iter().position(|r| match cursor_key {
            Some(ck) => r.key() == ck,
            None => active(r),
        });
        if let Some(row) = target {
            let off = out.state.offset.y;
            let vh = out.inner_rect.height();
            if let Some(o) = want_scroll(off, vh, row, row_h, row_h) {
                *pending = Some(o);
                ui.ctx().request_repaint();
            }
        }
    }
}

// ---------------------------------------------------------------- audio player bar

#[derive(Clone, Copy)]
enum Icon {
    Prev,
    Play,
    Pause,
    Stop,
    Next,
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, size: f32, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = ui.painter_at(rect);
    let bg = if resp.hovered() { pal.tile_hover } else { pal.tile };
    p.rect_filled(rect, size / 2.0, bg);
    let c = rect.center();
    let s = size * 0.2;
    let col = pal.text;
    let tri = |a: Pos2, b: Pos2, d: Pos2| {
        p.add(egui::Shape::convex_polygon(vec![a, b, d], col, Stroke::NONE));
    };
    match icon {
        Icon::Play => tri(
            pos2(c.x - s * 0.7, c.y - s * 1.1),
            pos2(c.x - s * 0.7, c.y + s * 1.1),
            pos2(c.x + s * 1.1, c.y),
        ),
        Icon::Pause => {
            p.rect_filled(
                Rect::from_center_size(pos2(c.x - s * 0.55, c.y), vec2(s * 0.7, s * 2.2)),
                1.0,
                col,
            );
            p.rect_filled(
                Rect::from_center_size(pos2(c.x + s * 0.55, c.y), vec2(s * 0.7, s * 2.2)),
                1.0,
                col,
            );
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(s * 2.0, s * 2.0)), 1.0, col);
        }
        Icon::Prev => {
            p.rect_filled(
                Rect::from_center_size(pos2(c.x - s * 1.0, c.y), vec2(s * 0.5, s * 2.2)),
                1.0,
                col,
            );
            tri(
                pos2(c.x + s * 1.1, c.y - s * 1.1),
                pos2(c.x + s * 1.1, c.y + s * 1.1),
                pos2(c.x - s * 0.6, c.y),
            );
        }
        Icon::Next => {
            p.rect_filled(
                Rect::from_center_size(pos2(c.x + s * 1.0, c.y), vec2(s * 0.5, s * 2.2)),
                1.0,
                col,
            );
            tri(
                pos2(c.x - s * 1.1, c.y - s * 1.1),
                pos2(c.x - s * 1.1, c.y + s * 1.1),
                pos2(c.x + s * 0.6, c.y),
            );
        }
    }
    resp
}

fn player_ui(
    ui: &mut egui::Ui,
    ps: &mut PlayerState,
    auto_next: &mut bool,
    pal: &Palette,
    actions: &mut Vec<Action>,
) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong(ps.title.as_str());
        ui.label(
            egui::RichText::new(format!("track {} of {}", ps.pos + 1, ps.playlist.len())).weak(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                actions.push(Action::PlayerClose);
            }
            ui.checkbox(auto_next, "Auto-next");
        });
    });
    if let Some(e) = &ps.error {
        ui.colored_label(ui.visuals().error_fg_color, e.as_str());
    }
    let mut seek_to: Option<f32> = None;
    if let Some(a) = &mut ps.audio {
        let dur = a.duration();
        let pos = a.position();
        let total = dur.map(|d| d.as_secs_f32()).unwrap_or(0.0).max(0.001);
        ui.horizontal(|ui| {
            ui.label(fmt_dur(pos));
            ui.add_enabled_ui(dur.is_some(), |ui| {
                ui.spacing_mut().slider_width = (ui.available_width() - 70.0).max(80.0);
                let mut v = ps.drag_value.unwrap_or(pos.as_secs_f32()).clamp(0.0, total);
                let resp = ui.add(egui::Slider::new(&mut v, 0.0..=total).show_value(false));
                if resp.dragged() {
                    ps.drag_value = Some(v);
                } else {
                    ps.drag_value = None;
                }
                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                    seek_to = Some(v);
                }
            });
            ui.label(dur.map(fmt_dur).unwrap_or_else(|| "--:--".to_string()));
        });
        ui.horizontal(|ui| {
            let sz = 34.0;
            if icon_button(ui, Icon::Prev, sz, pal).clicked() {
                actions.push(Action::PlayerPrev);
            }
            let icon = if a.is_playing() { Icon::Pause } else { Icon::Play };
            if icon_button(ui, icon, sz + 6.0, pal).clicked() {
                a.toggle();
            }
            if icon_button(ui, Icon::Stop, sz, pal).clicked() {
                a.stop();
            }
            if icon_button(ui, Icon::Next, sz, pal).clicked() {
                actions.push(Action::PlayerNext);
            }
            ui.add_space(16.0);
            let mut vol = a.volume();
            if ui.add(egui::Slider::new(&mut vol, 0.0..=1.0).text("Volume")).changed() {
                a.set_volume(vol);
            }
        });
        if let Some(s) = seek_to {
            a.seek(Duration::from_secs_f32(s.max(0.0)));
        }
        if a.is_playing() {
            ui.ctx().request_repaint();
        }
    }
    ui.add_space(4.0);
}

// ---------------------------------------------------------------- image viewer

/// Full-window viewer: wheel zooms, drag pans, double click toggles fit / 100%. Returns false to close.
fn viewer_ui(ctx: &egui::Context, v: &mut Viewer) -> (bool, isize) {
    let mut open = true;
    let mut step = 0isize;
    let now = ctx.input(|i| i.time);
    let settled = now - v.opened_at > 0.4; // ignore the tail of the click that opened it
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("image_viewer"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 0.0, Color32::from_black_alpha(235));

            let fit = ((rect.width() * 0.92) / v.w as f32)
                .min((rect.height() * 0.84) / v.h as f32)
                .max(0.01);
            if v.fit_pending {
                v.zoom = fit;
                v.offset = egui::Vec2::ZERO;
                v.fit_pending = false;
            }
            if resp.dragged() {
                v.offset += resp.drag_delta();
            }
            if let Some(ptr) = ctx.pointer_hover_pos() {
                let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    let new_zoom = (v.zoom * (scroll * 0.0015).exp()).clamp(0.02, 64.0);
                    let center = rect.center() + v.offset;
                    let new_center = ptr + (center - ptr) * (new_zoom / v.zoom);
                    v.offset = new_center - rect.center();
                    v.zoom = new_zoom;
                }
            }
            if settled && resp.double_clicked() {
                if (v.zoom - 1.0).abs() < 0.01 {
                    v.fit_pending = true;
                } else {
                    v.zoom = 1.0;
                    v.offset = egui::Vec2::ZERO;
                }
            }

            let size = vec2(v.w as f32, v.h as f32) * v.zoom;
            let img_rect = Rect::from_center_size(rect.center() + v.offset, size);
            p.rect_filled(img_rect, 0.0, Color32::from_gray(28));
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            p.image(v.handle.id(), img_rect, uv, Color32::WHITE);

            if settled && resp.clicked() {
                if let Some(ptr) = ctx.pointer_hover_pos() {
                    if !img_rect.contains(ptr) && ptr.y > rect.min.y + 44.0 {
                        open = false;
                    }
                }
            }

            let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 40.0));
            p.rect_filled(bar, 0.0, Color32::from_black_alpha(160));
            ui.allocate_ui_at_rect(bar.shrink2(vec2(10.0, 6.0)), |ui| {
                ui.horizontal(|ui| {
                    if ui.add_enabled(v.pos > 0, egui::Button::new("Previous")).clicked() {
                        step = -1;
                    }
                    if ui.add_enabled(v.pos + 1 < v.list.len(), egui::Button::new("Next")).clicked() {
                        step = 1;
                    }
                    ui.colored_label(
                        Color32::WHITE,
                        format!(
                            "{}  -  {}x{}  -  {:.0}%  -  {} / {}",
                            v.name,
                            v.w,
                            v.h,
                            v.zoom * 100.0,
                            v.pos + 1,
                            v.list.len()
                        ),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close (Esc)").clicked() {
                            open = false;
                        }
                        if ui.button("100%").clicked() {
                            v.zoom = 1.0;
                            v.offset = egui::Vec2::ZERO;
                        }
                        if ui.button("Fit").clicked() {
                            v.fit_pending = true;
                        }
                    });
                });
            });
        });
    let (esc, left, right) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::A),
            i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::D),
        )
    });
    if esc {
        open = false;
    }
    if left && v.pos > 0 {
        step = -1;
    }
    if right && v.pos + 1 < v.list.len() {
        step = 1;
    }
    (open, step)
}

// ---------------------------------------------------------------- video player window

/// Full-window video player. Returns false to close it.
fn video_overlay_ui(ctx: &egui::Context, s: &mut VideoSession, pal: &Palette) -> (bool, isize) {
    let mut open = true;
    let mut step = 0isize;
    let now = ctx.input(|i| i.time);
    let settled = now - s.opened_at > 0.4;
    s.player.update(ctx);
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("video_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(Color32::from_gray(230));
            let (rect, bg) = ui.allocate_exact_size(screen.size(), Sense::click());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 0.0, Color32::from_rgb(8, 8, 10));
            let top_h = 44.0;
            let bar_h = 100.0;
            let area = Rect::from_min_max(
                pos2(rect.min.x + 16.0, rect.min.y + top_h),
                pos2(rect.max.x - 16.0, rect.max.y - bar_h),
            );
            let (vw, vh) = (s.player.info.width as f32, s.player.info.height as f32);
            let scale = (area.width() / vw).min(area.height() / vh).max(0.05);
            let vrect = Rect::from_center_size(area.center(), vec2(vw * scale, vh * scale));
            p.rect_filled(vrect, 0.0, Color32::BLACK);
            match s.player.texture() {
                Some(t) => {
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    p.image(t.id(), vrect, uv, Color32::WHITE);
                }
                None => {
                    p.text(
                        vrect.center(),
                        Align2::CENTER_CENTER,
                        "Loading...",
                        FontId::proportional(16.0),
                        Color32::from_gray(180),
                    );
                }
            }
            if settled && bg.clicked() {
                if let Some(ptr) = ctx.pointer_hover_pos() {
                    if vrect.contains(ptr) {
                        s.player.toggle_pause();
                    }
                }
            }

            // title bar
            let top = Rect::from_min_size(rect.min, vec2(rect.width(), top_h));
            ui.allocate_ui_at_rect(top.shrink2(vec2(16.0, 8.0)), |ui| {
                ui.horizontal(|ui| {
                    ui.strong(s.name.as_str());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close (Esc)").clicked() {
                            open = false;
                        }
                    });
                });
            });

            // controls
            let bar = Rect::from_min_max(
                pos2(rect.min.x + 16.0, rect.max.y - bar_h + 6.0),
                pos2(rect.max.x - 16.0, rect.max.y - 8.0),
            );
            ui.allocate_ui_at_rect(bar, |ui| {
                let dur = s.player.duration();
                let pos = s.player.position();
                let total = dur.map(|d| d.as_secs_f32()).unwrap_or(0.0).max(0.001);
                let mut seek_to: Option<f32> = None;
                ui.horizontal(|ui| {
                    ui.label(fmt_dur(pos));
                    ui.add_enabled_ui(dur.is_some(), |ui| {
                        ui.spacing_mut().slider_width = (ui.available_width() - 70.0).max(80.0);
                        let mut v = s.drag_value.unwrap_or(pos.as_secs_f32()).clamp(0.0, total);
                        let resp = ui.add(egui::Slider::new(&mut v, 0.0..=total).show_value(false));
                        if resp.dragged() {
                            s.drag_value = Some(v);
                        } else {
                            s.drag_value = None;
                        }
                        if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                            seek_to = Some(v);
                        }
                    });
                    ui.label(dur.map(fmt_dur).unwrap_or_else(|| "--:--".to_string()));
                });
                ui.horizontal(|ui| {
                    let playing = !s.player.is_paused() && !s.player.is_finished();
                    let icon = if playing { Icon::Pause } else { Icon::Play };
                    if icon_button(ui, Icon::Prev, 34.0, pal).clicked() && s.pos > 0 {
                        step = -1;
                    }
                    if icon_button(ui, icon, 40.0, pal).clicked() {
                        s.player.toggle_pause();
                    }
                    if icon_button(ui, Icon::Stop, 34.0, pal).clicked() {
                        s.player.stop();
                    }
                    if icon_button(ui, Icon::Next, 34.0, pal).clicked() && s.pos + 1 < s.playlist.len() {
                        step = 1;
                    }
                    ui.add_space(16.0);
                    let mut vol = s.player.volume();
                    if ui.add(egui::Slider::new(&mut vol, 0.0..=1.0).text("Volume")).changed() {
                        s.player.set_volume(vol);
                    }
                    ui.label(format!(
                        "{} / {}    Space: play / pause    Left / Right: -5 s / +5 s    N / P: next / previous    Esc: close",
                        s.pos + 1,
                        s.playlist.len()
                    ));
                });
                if let Some(t) = seek_to {
                    s.player.seek(Duration::from_secs_f32(t.max(0.0)));
                }
            });
        });

    let (esc, space, left, right, next, prev) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::Space),
            i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::A),
            i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::D),
            i.key_pressed(egui::Key::N) || i.key_pressed(egui::Key::PageDown),
            i.key_pressed(egui::Key::P) || i.key_pressed(egui::Key::PageUp),
        )
    });
    if next && s.pos + 1 < s.playlist.len() {
        step = 1;
    }
    if prev && s.pos > 0 {
        step = -1;
    }
    if esc {
        open = false;
    }
    if space {
        s.player.toggle_pause();
    }
    if (left || right) && now - s.last_seek > 0.15 {
        s.last_seek = now;
        let step = Duration::from_secs(5);
        let pos = s.player.position();
        let mut np = if left { pos.saturating_sub(step) } else { pos + step };
        if let Some(d) = s.player.duration() {
            np = np.min(d.saturating_sub(Duration::from_millis(200)));
        }
        s.player.seek(np);
    }
    (open, step)
}

// ---------------------------------------------------------------- app

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // keep our theme even if the system theme changes
        if ctx.style().visuals.dark_mode != self.dark_mode {
            apply_style(ctx, self.dark_mode);
        }
        let pal = palette(self.dark_mode);
        let now = ctx.input(|i| i.time);

        if self.query != self.applied_query {
            self.recompute_results();
        }
        if self.tree_query != self.tree_applied {
            self.tree_applied = self.tree_query.clone();
            self.tree_dirty = true;
        }
        if self.tree_dirty {
            self.rebuild_tree_rows();
        }
        if (self.game_ctx.game, self.game_ctx.platform) != self.thumb_ctx {
            self.thumb_ctx = (self.game_ctx.game, self.game_ctx.platform);
            self.thumbs.clear();
            let cidx = self.container.as_ref().map(|c| c.index);
            if let Some(idx) = cidx {
                self.container = None;
                self.open_container(idx, ctx);
            }
        }
        self.thumbs.img.budget = 4;
        self.thumbs.vid_budget = 1;
        self.thumbs.vid.poll(ctx);

        // ffmpeg download finished?
        let dl_result = self.ff_download.as_ref().and_then(|d| d.poll());
        if self.ff_download.is_some() && dl_result.is_none() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        if let Some(res) = dl_result {
            self.ff_download = None;
            match res {
                Ok(path) => match ffmpeg_from_path(path) {
                    Some(ff) => {
                        save_ffmpeg_path(&ff);
                        self.ffmpeg = Some(ff);
                        self.ffmpeg_prompt = false;
                        self.thumbs.vid.clear();
                        self.status = "ffmpeg downloaded and ready: videos can be played now.".to_string();
                    }
                    None => {
                        self.status =
                            "ffmpeg was downloaded but does not run on this system.".to_string();
                    }
                },
                Err(e) => self.status = format!("ffmpeg download failed: {e}"),
            }
        }

        // export runs a little every frame
        if self.export.as_ref().map(|e| e.job.is_some()).unwrap_or(false) {
            self.step_export();
            ctx.request_repaint();
        }

        let mut actions: Vec<Action> = Vec::new();

        // audio player: end-of-track detection and auto-next
        let mut ended = false;
        if let Some(ps) = &mut self.player {
            if let Some(a) = &mut ps.audio {
                a.tick();
                if a.take_ended() {
                    ended = true;
                }
                if a.is_playing() {
                    ctx.request_repaint();
                }
            }
        }
        if ended && self.auto_next {
            actions.push(Action::PlayerNext);
        }

        self.handle_keys(ctx, &mut actions);

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open folder...").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.open(dir);
                    }
                }
                if ui.button("Open HVP...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("HVP", &["hvp"]).pick_file() {
                        self.open_hvp(p);
                    }
                }
                ui.separator();
                ui.label("Game:");
                egui::ComboBox::from_id_source("game_combo")
                    .selected_text(self.game_ctx.game.label())
                    .show_ui(ui, |ui| {
                        for g in Game::ALL {
                            ui.selectable_value(&mut self.game_ctx.game, g, g.label());
                        }
                    });
                ui.label("Platform:");
                egui::ComboBox::from_id_source("platform_combo")
                    .selected_text(self.game_ctx.platform.label())
                    .show_ui(ui, |ui| {
                        for p in Platform::ALL {
                            ui.selectable_value(&mut self.game_ctx.platform, p, p.label());
                        }
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.selectable_value(&mut self.dark_mode, false, "Light");
                    ui.selectable_value(&mut self.dark_mode, true, "Dark");
                    ui.label("Theme:");
                    if self.ffmpeg.is_none() {
                        ui.separator();
                        if ui.button("Set up video support...").clicked() {
                            self.ffmpeg_prompt = true;
                            self.ffmpeg_declined = false;
                        }
                    }
                });
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(self.status.as_str());
            });
        });

        let tree_panel =
            egui::SidePanel::left("folders").resizable(true).default_width(280.0).show(ctx, |ui| {
                ui.strong("Folders");
                ui.add(
                    egui::TextEdit::singleline(&mut self.tree_query)
                        .hint_text("Filter folders...")
                        .desired_width(f32::INFINITY),
                );
                ui.separator();
                if self.archive.is_some() {
                    tree_ui(
                        ui,
                        &self.tree_rows,
                        &self.cwd,
                        self.container.as_ref().map(|c| c.index),
                        self.tree_cursor.as_deref(),
                        &pal,
                        self.focus == Focus::Tree,
                        &mut actions,
                        &mut self.pending_scroll_tree,
                        &mut self.tree_scroll,
                    );
                } else {
                    ui.label("Nothing open.");
                }
            });
        self.tree_rect = tree_panel.response.rect;

        // audio player bar: bottom of the file view
        if self.player.is_some() {
            egui::TopBottomPanel::bottom("player").show(ctx, |ui| {
                if let Some(ps) = &mut self.player {
                    player_ui(ui, ps, &mut self.auto_next, &pal, &mut actions);
                }
            });
        }

        let central = egui::CentralPanel::default().show(ctx, |ui| {
            let Some(archive) = self.archive.as_deref() else {
                ui.centered_and_justified(|ui| {
                    ui.label("Open the game folder (all its .hvp files) or a single .hvp archive.");
                });
                return;
            };

            // breadcrumb
            ui.horizontal_wrapped(|ui| {
                let can_up = self.container.is_some() || !self.cwd.is_empty();
                if ui.add_enabled(can_up, egui::Button::new("Up")).clicked() {
                    actions.push(Action::Up);
                }
                ui.separator();
                if ui.add(egui::Button::new(self.root_name.as_str()).frame(false)).clicked() {
                    actions.push(Action::Cd(Vec::new()));
                }
                for i in 0..self.cwd.len() {
                    ui.label(">");
                    if ui.add(egui::Button::new(self.cwd[i].as_str()).frame(false)).clicked() {
                        actions.push(Action::Cd(self.cwd[..=i].to_vec()));
                    }
                }
                if let Some(c) = &self.container {
                    ui.label(">");
                    ui.label(egui::RichText::new(c.file_name.as_str()).strong());
                }
            });

            // toolbar: search, view mode, size, selection, export
            let n_sel = self.selection.len();
            let in_container = self.container.is_some();
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search all files...")
                        .desired_width(220.0),
                );
                if !self.query.is_empty() && ui.button("Clear").clicked() {
                    self.query.clear();
                }
                ui.separator();
                ui.selectable_value(&mut self.view, ViewMode::Grid, "Grid");
                ui.selectable_value(&mut self.view, ViewMode::List, "List");
                if self.view == ViewMode::Grid {
                    ui.add(egui::Slider::new(&mut self.tile, 64.0..=192.0).text("Size"));
                }
                ui.separator();
                if ui.button("Select all").on_hover_text("Ctrl+A").clicked() {
                    actions.push(Action::SelectAll);
                }
                if n_sel > 0 && ui.button("Clear selection").on_hover_text("Esc").clicked() {
                    actions.push(Action::ClearSelection);
                }
                let label = if n_sel > 0 {
                    format!("Export ({n_sel})...")
                } else if in_container {
                    "Export all images...".to_string()
                } else {
                    "Export...".to_string()
                };
                let enabled = n_sel > 0 || in_container;
                if ui
                    .add_enabled(enabled, egui::Button::new(label))
                    .on_hover_text(
                        "Tick the checkbox of items (or Ctrl+click, Shift+click) and export them. You can also right-click any item.",
                    )
                    .clicked()
                {
                    actions.push(Action::ExportSelection);
                }
            });
            ui.separator();

            // items: texture pack, search results (flat) or current folder
            let searching = !self.query.is_empty() && self.container.is_none();
            let entries = archive.entries();
            let items: Vec<Item> = if let Some(c) = &self.container {
                c.textures
                    .iter()
                    .enumerate()
                    .map(|(i, t)| Item::Tex {
                        name: t.name.as_str(),
                        i,
                        width: t.width,
                        height: t.height,
                        format: t.format.as_str(),
                        thumb: t.thumb.as_ref(),
                    })
                    .collect()
            } else if searching {
                self.results
                    .iter()
                    .map(|&i| Item::File { name: base_name(&entries[i].path), index: i })
                    .collect()
            } else {
                let node = self.tree.get(&self.cwd).unwrap_or(&self.tree);
                let mut v = Vec::with_capacity(node.dirs.len() + node.files.len());
                for (name, d) in &node.dirs {
                    v.push(Item::Folder {
                        name: name.as_str(),
                        subfolders: d.dirs.len(),
                        files: d.files.len(),
                    });
                }
                for (name, &idx) in &node.files {
                    v.push(Item::File { name: name.as_str(), index: idx });
                }
                v
            };

            // a video is visible but ffmpeg is missing: offer to set it up (once)
            if self.ffmpeg.is_none()
                && !self.ffmpeg_declined
                && !self.ffmpeg_prompt
                && self.ff_download.is_none()
                && items.iter().any(|it| match it {
                    Item::File { name, .. } => is_video_ext(&ext_of(name)),
                    _ => false,
                })
            {
                self.ffmpeg_prompt = true;
            }

            let label = if self.container.is_some() {
                format!("{} image(s)", items.len())
            } else if searching {
                format!("{} result(s)", items.len())
            } else {
                format!("{} item(s)", items.len())
            };
            ui.label(egui::RichText::new(label).weak());

            if items.is_empty() {
                ui.label(if searching { "Nothing found." } else { "Empty." });
                return;
            }

            let env = Env {
                archive,
                game_ctx: self.game_ctx,
                egui_ctx: ctx,
                container_index: self.container.as_ref().map(|c| c.index),
                playing: self.player.as_ref().map(|p| p.index),
                player_active: self
                    .player
                    .as_ref()
                    .and_then(|p| p.audio.as_ref())
                    .map(|a| a.is_playing())
                    .unwrap_or(false),
                cursor: self.cursor,
                items_focused: self.focus == Focus::Items,
                selection: &self.selection,
                pal,
                ffmpeg: self.ffmpeg.as_ref(),
                temp: &self.temp,
                now,
                last_enter: self.last_enter,
            };

            let mut sa = egui::ScrollArea::vertical().auto_shrink([false, false]);
            if let Some(off) = self.pending_scroll_items.take() {
                sa = sa.vertical_scroll_offset(off);
            }

            match self.view {
                ViewMode::Grid => {
                    let gap = 8.0;
                    let size = self.tile;
                    let tile_w = size + 16.0;
                    let tile_h = size + 62.0;
                    ui.spacing_mut().item_spacing = vec2(gap, gap);
                    let cols =
                        (((ui.available_width() + gap) / (tile_w + gap)).floor() as usize).max(1);
                    self.last_cols = cols;
                    let rows = items.len().div_ceil(cols);
                    let out = sa.show_rows(ui, tile_h, rows, |ui, range| {
                        for row in range {
                            ui.horizontal(|ui| {
                                for col in 0..cols {
                                    let i = row * cols + col;
                                    if i >= items.len() {
                                        break;
                                    }
                                    grid_tile(
                                        ui,
                                        &env,
                                        &mut self.thumbs,
                                        &items[i],
                                        i,
                                        size,
                                        &self.cwd,
                                        &mut actions,
                                    );
                                }
                            });
                        }
                    });
                    if self.scroll_cursor {
                        self.scroll_cursor = false;
                        if let Some(c) = self.cursor {
                            let off = out.state.offset.y;
                            let vh = out.inner_rect.height();
                            if let Some(o) = want_scroll(off, vh, c / cols, tile_h + gap, tile_h) {
                                self.pending_scroll_items = Some(o);
                                ctx.request_repaint();
                            }
                        }
                    }
                }
                ViewMode::List => {
                    self.last_cols = 1;
                    let row_h = 26.0;
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let (hr, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
                    let hp = ui.painter_at(hr);
                    let hf = FontId::proportional(12.0);
                    let hg = pal.weak;
                    hp.text(
                        pos2(hr.min.x + 26.0 + row_h + 4.0, hr.center().y),
                        Align2::LEFT_CENTER,
                        "Name",
                        hf.clone(),
                        hg,
                    );
                    hp.text(
                        pos2(hr.min.x + hr.width() * 0.60, hr.center().y),
                        Align2::LEFT_CENTER,
                        "Type",
                        hf.clone(),
                        hg,
                    );
                    hp.text(
                        pos2(hr.max.x - 8.0, hr.center().y),
                        Align2::RIGHT_CENTER,
                        "Details",
                        hf,
                        hg,
                    );
                    let out = sa.show_rows(ui, row_h, items.len(), |ui, range| {
                        for i in range {
                            list_row(
                                ui,
                                &env,
                                &mut self.thumbs,
                                &items[i],
                                i,
                                row_h,
                                &self.cwd,
                                &mut actions,
                            );
                        }
                    });
                    if self.scroll_cursor {
                        self.scroll_cursor = false;
                        if let Some(c) = self.cursor {
                            let off = out.state.offset.y;
                            let vh = out.inner_rect.height();
                            if let Some(o) = want_scroll(off, vh, c, row_h + 2.0, row_h) {
                                self.pending_scroll_items = Some(o);
                                ctx.request_repaint();
                            }
                        }
                    }
                }
            }
        });
        self.items_rect = central.response.rect;

        // ---- overlays ----
        let mut viewer_step = 0isize;
        let mut close_viewer = false;
        if let Some(v) = &mut self.viewer {
            let (keep, step) = viewer_ui(ctx, v);
            close_viewer = !keep;
            viewer_step = step;
        }
        if close_viewer {
            self.viewer = None;
        } else if viewer_step != 0 {
            self.step_viewer(viewer_step, ctx);
        }
        if let Some(t) = &mut self.text_view {
            let mut open = t.open;
            egui::Window::new(t.name.as_str())
                .open(&mut open)
                .default_size([720.0, 520.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(t.text.as_str()).monospace()));
                    });
                });
            if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                t.open = false;
            }
        }
        if self.text_view.as_ref().map(|t| !t.open).unwrap_or(false) {
            self.text_view = None;
        }
        let mut close_model = false;
        if let Some(mv) = &mut self.model_viewer {
            if !mv.ui(ctx) {
                close_model = true;
            }
        }
        if close_model {
            self.model_viewer = None;
        }
        let mut close_video = false;
        let mut video_step = 0isize;
        if let Some(sess) = &mut self.video {
            let (keep, step) = video_overlay_ui(ctx, sess, &pal);
            close_video = !keep;
            video_step = step;
        }
        if close_video {
            if let Some(sess) = self.video.take() {
                let f = sess.file.clone();
                drop(sess);
                let _ = std::fs::remove_file(f);
            }
        } else if video_step != 0 {
            self.step_video(video_step, ctx, now);
        }
        self.ffmpeg_popup(ctx, &mut actions);
        self.export_window(ctx);

        for a in actions {
            match a {
                Action::Cd(p) => self.cd(p),
                Action::Enter(p) => {
                    self.cd(p);
                    self.last_enter = now;
                    self.focus = Focus::Items;
                    self.tree_cursor = None;
                }
                Action::Toggle(key) => {
                    if !self.expanded.remove(&key) {
                        self.expanded.insert(key);
                    }
                    self.tree_dirty = true;
                }
                Action::Open(i) => {
                    if self.open_file(i, ctx, now) {
                        self.last_enter = now;
                    }
                }
                Action::OpenTex(i) => {
                    if let Some(ci) = self.container.as_ref().map(|c| c.index) {
                        self.open_viewer(ci, Some(i), ctx, now);
                    }
                }
                Action::ToggleAudio(i) => self.toggle_or_play_audio(i),
                Action::PlayVideo(i) => self.start_video(i, ctx, now),
                Action::Cursor(i) => {
                    self.cursor = Some(i);
                    self.focus = Focus::Items;
                    self.tree_cursor = None;
                }
                Action::TreeFocus(key) => {
                    self.focus = Focus::Tree;
                    if key.is_some() {
                        self.tree_cursor = key;
                    }
                }
                Action::TogglePlay => self.toggle_play(),
                Action::PlayerPrev => self.player_step(-1),
                Action::PlayerNext => self.player_step(1),
                Action::PlayerClose => self.player = None,
                Action::DownloadFfmpeg => {
                    if self.ff_download.is_none() {
                        let dir = std::env::current_exe()
                            .ok()
                            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                            .unwrap_or_else(|| PathBuf::from("."));
                        self.ff_download = Some(FfDownload::start(dir, ctx));
                        self.status = "Downloading ffmpeg...".to_string();
                    }
                }
                Action::LocateFfmpeg => {
                    if let Some(p) = rfd::FileDialog::new().set_title("Locate ffmpeg").pick_file() {
                        match ffmpeg_from_path(p) {
                            Some(ff) => {
                                save_ffmpeg_path(&ff);
                                self.ffmpeg = Some(ff);
                                self.ffmpeg_prompt = false;
                                self.thumbs.vid.clear();
                                self.status = "ffmpeg found and saved.".to_string();
                            }
                            None => {
                                self.status =
                                    "That file does not look like a working ffmpeg.".to_string();
                            }
                        }
                    }
                }
                Action::DismissFfmpeg => {
                    self.ffmpeg_prompt = false;
                    self.ffmpeg_declined = true;
                }
                Action::ToggleSelect(key, vi) => {
                    if !self.selection.remove(&key) {
                        self.selection.insert(key);
                    }
                    self.sel_anchor = Some(vi);
                }
                Action::RangeSelect(vi) => self.range_select(vi),
                Action::SelectAll => self.select_all(),
                Action::ClearSelection => self.clear_selection(),
                Action::ExportSelection => {
                    let ci = self.container.as_ref().map(|c| c.index);
                    let mut targets = targets_from_selection(&self.selection, ci);
                    if targets.is_empty() {
                        if let Some(c) = &self.container {
                            targets = (0..c.textures.len()).map(|i| ExpTarget::Tex(c.index, i)).collect();
                        }
                    }
                    self.begin_export(targets);
                }
                Action::Export(targets) => self.begin_export(targets),
                Action::Up => {
                    if self.container.is_some() {
                        self.container = None;
                        self.cursor = None;
                        self.sel_anchor = None;
                        self.selection.clear();
                        self.tree_scroll = true;
                    } else if !self.cwd.is_empty() {
                        let mut p = self.cwd.clone();
                        p.pop();
                        self.cd(p);
                    }
                }
            }
        }
    }
}

impl App {
    /// Centered pop-up asking to set up ffmpeg (needed for .bik / .ogv videos).
    fn ffmpeg_popup(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.ffmpeg_prompt || self.ffmpeg.is_some() {
            return;
        }
        egui::Area::new(egui::Id::new("ffmpeg_backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(pos2(0.0, 0.0))
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(ctx.screen_rect().size(), Sense::click());
                ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(150));
            });
        egui::Window::new("Video support needs ffmpeg")
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label(
                    "Videos (.bik and .ogv) are decoded with ffmpeg, a free and open-source tool \
                     that is not bundled with this program.",
                );
                ui.add_space(6.0);
                if cfg!(windows) {
                    ui.label(
                        "It can be downloaded automatically (about 90 MB, from gyan.dev - the \
                         Windows builds linked from ffmpeg.org). This only happens once.",
                    );
                } else {
                    ui.label("Install ffmpeg with your package manager, then point to it below.");
                }
                ui.add_space(10.0);
                if let Some(d) = &self.ff_download {
                    ui.add(
                        egui::ProgressBar::new(d.fraction())
                            .text(format!("Downloading... {:.0} MB", d.done_mb())),
                    );
                    ui.label("The pop-up closes by itself when it is done.");
                } else {
                    ui.horizontal(|ui| {
                        if cfg!(windows) && ui.button("Download ffmpeg").clicked() {
                            actions.push(Action::DownloadFfmpeg);
                        }
                        if ui.button("I already have it...").clicked() {
                            actions.push(Action::LocateFfmpeg);
                        }
                        if ui.button("Not now").clicked() {
                            actions.push(Action::DismissFfmpeg);
                        }
                    });
                }
            });
    }

    /// The export pop-up: options, destination, progress and result.
    fn export_window(&mut self, ctx: &egui::Context) {
        let Some(ex) = &mut self.export else { return };
        let mut close = false;
        let mut start = false;
        egui::Window::new("Export")
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                if let Some(job) = &mut ex.job {
                    let total = job.items.len().max(1);
                    ui.add(
                        egui::ProgressBar::new(job.pos as f32 / total as f32)
                            .text(format!("{} / {}", job.pos, job.items.len())),
                    );
                    if job.failed > 0 {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!("{} failed so far", job.failed),
                        );
                    }
                    if ui.button("Cancel").clicked() {
                        job.cancel = true;
                    }
                } else if let Some(msg) = &ex.message {
                    ui.label(msg.as_str());
                    ui.add_space(8.0);
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                } else {
                    ui.label(format!(
                        "{} item(s) selected, {} file(s) in total.",
                        ex.targets.len(),
                        ex.n_files.max(ex.targets.len())
                    ));
                    ui.add_space(6.0);
                    ui.add_enabled(
                        !ex.tex_only,
                        egui::Checkbox::new(&mut ex.raw, "Raw files (exactly as stored in the game)"),
                    );
                    ui.add_enabled(
                        ex.has_tex,
                        egui::Checkbox::new(
                            &mut ex.png,
                            "Textures and images as PNG (images inside packs become separate files)",
                        ),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Choose folder...").clicked() {
                            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                ex.dest = Some(d);
                            }
                        }
                        match &ex.dest {
                            Some(d) => ui.label(d.display().to_string()),
                            None => ui.label("no destination chosen"),
                        };
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ready = ex.dest.is_some() && ((ex.raw && !ex.tex_only) || ex.png);
                        if ui.add_enabled(ready, egui::Button::new("Export")).clicked() {
                            start = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && ex.job.is_none() {
            close = true;
        }
        if close {
            self.export = None;
        } else if start {
            self.start_export();
        }
    }
}
