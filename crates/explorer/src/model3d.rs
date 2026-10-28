//! 3D viewer for .zwo models. No GPU code: a small CPU rasterizer draws into an egui texture.

use eframe::egui;
use egui::{pos2, vec2, Color32, Rect, Sense};
use obscure_models::{mat_mul, transform_dir, transform_point, Mat4, Model};
use std::collections::HashMap;
use std::sync::Arc;

pub struct TexImage {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy)]
struct SV {
    p: [f32; 3],
    n: [f32; 3],
    uv: [f32; 2],
    c: [f32; 3],
    has_n: bool,
}

struct Tri {
    i: [u32; 3],
    tex: i32,
    mesh: u32,
}

pub struct MaterialStatus {
    pub name: String,
    pub texture: String,
    pub found: bool,
}

pub struct Scene {
    verts: Vec<SV>,
    tris: Vec<Tri>,
    textures: Vec<Arc<TexImage>>,
    pub mesh_names: Vec<String>,
    pub mesh_visible: Vec<bool>,
    pub mesh_tris: Vec<usize>,
    pub materials: Vec<MaterialStatus>,
    bmin: [f32; 3],
    bmax: [f32; 3],
}

impl Scene {
    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    fn center(&self) -> [f32; 3] {
        [
            (self.bmin[0] + self.bmax[0]) * 0.5,
            (self.bmin[1] + self.bmax[1]) * 0.5,
            (self.bmin[2] + self.bmax[2]) * 0.5,
        ]
    }

    fn radius(&self) -> f32 {
        let d = sub(self.bmax, self.bmin);
        (0.5 * dot(d, d).sqrt()).max(1e-3)
    }
}

// ---------------------------------------------------------------- small vector math

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l < 1e-12 {
        [0.0, 0.0, 1.0]
    } else {
        [a[0] / l, a[1] / l, a[2] / l]
    }
}

fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        [s[0], s[1], s[2], -dot(s, eye)],
        [u[0], u[1], u[2], -dot(u, eye)],
        [-f[0], -f[1], -f[2], dot(f, eye)],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fovy * 0.5).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, (far + near) / (near - far), 2.0 * far * near / (near - far)],
        [0.0, 0.0, -1.0, 0.0],
    ]
}

fn mul_point4(m: &Mat4, p: [f32; 3]) -> [f32; 4] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        m[3][0] * p[0] + m[3][1] * p[1] + m[3][2] * p[2] + m[3][3],
    ]
}

// ---------------------------------------------------------------- scene building

/// Flattens a model into world-space triangles. `tex_by_name` is keyed by lower-case texture name.
pub fn build_scene(model: &Model, tex_by_name: &HashMap<String, Arc<TexImage>>) -> Scene {
    let mat_tex: HashMap<String, String> = model
        .materials
        .iter()
        .map(|m| (m.name.to_lowercase(), m.texture.clone()))
        .collect();
    let materials: Vec<MaterialStatus> = model
        .materials
        .iter()
        .map(|m| MaterialStatus {
            name: m.name.clone(),
            texture: m.texture.clone(),
            found: tex_by_name.contains_key(&m.texture.to_lowercase()),
        })
        .collect();

    let mut verts: Vec<SV> = Vec::new();
    let mut tris: Vec<Tri> = Vec::new();
    let mut textures: Vec<Arc<TexImage>> = Vec::new();
    let mut slot_of: HashMap<String, i32> = HashMap::new();
    let mut mesh_tris = vec![0usize; model.meshes.len()];
    let mut bmin = [f32::INFINITY; 3];
    let mut bmax = [f32::NEG_INFINITY; 3];

    for item in model.draw_items() {
        let src = &model.meshes[item.source];
        let vs = &src.buffers[item.buffer];
        let m = &item.matrix;
        let base = verts.len() as u32;
        for (vi, p) in vs.positions.iter().enumerate() {
            let pw = transform_point(m, *p);
            let (n, has_n) = match vs.normals.get(vi) {
                Some(n) => (normalize(transform_dir(m, *n)), true),
                None => ([0.0, 0.0, 1.0], false),
            };
            let uv = vs.uv0.get(vi).copied().unwrap_or([0.0, 0.0]);
            let c = match vs.color0.get(vi) {
                Some(c) => [c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0],
                None => [1.0, 1.0, 1.0],
            };
            for k in 0..3 {
                bmin[k] = bmin[k].min(pw[k]);
                bmax[k] = bmax[k].max(pw[k]);
            }
            verts.push(SV { p: pw, n, uv, c, has_n });
        }

        // texture slot for every material this mesh uses
        let mut slots: Vec<i32> = Vec::with_capacity(src.material_names.len());
        for mn in &src.material_names {
            let mut slot = -1;
            if let Some(tex_name) = mat_tex.get(&mn.to_lowercase()) {
                let key = tex_name.to_lowercase();
                if let Some(img) = tex_by_name.get(&key) {
                    slot = match slot_of.get(&key) {
                        Some(s) => *s,
                        None => {
                            textures.push(img.clone());
                            let s = (textures.len() - 1) as i32;
                            slot_of.insert(key, s);
                            s
                        }
                    };
                }
            }
            slots.push(slot);
        }

        let n_src = vs.positions.len() as u32;
        for (ti, t) in src.triangles.iter().enumerate() {
            if t[0] >= n_src || t[1] >= n_src || t[2] >= n_src {
                continue;
            }
            let mat_idx = src.tri_material.get(ti).copied().unwrap_or(0) as usize;
            let slot = slots.get(mat_idx).copied().unwrap_or(-1);
            tris.push(Tri { i: [base + t[0], base + t[1], base + t[2]], tex: slot, mesh: item.mesh as u32 });
            mesh_tris[item.mesh] += 1;
        }
    }

    if verts.is_empty() {
        bmin = [0.0; 3];
        bmax = [0.0; 3];
    }
    Scene {
        verts,
        tris,
        textures,
        mesh_names: model.meshes.iter().map(|m| m.name.clone()).collect(),
        mesh_visible: vec![true; model.meshes.len()],
        mesh_tris,
        materials,
        bmin,
        bmax,
    }
}

// ---------------------------------------------------------------- rasterizer

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Textured,
    Solid,
    Wire,
}

struct RenderOpts {
    mode: Mode,
    cull: bool,
    vcolors: bool,
    z_up: bool,
}

#[derive(Clone)]
struct Camera {
    target: [f32; 3],
    dist: f32,
    yaw: f32,
    pitch: f32,
}

impl Camera {
    fn eye_up(&self, z_up: bool) -> ([f32; 3], [f32; 3]) {
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        let (cy, sy) = (self.yaw.cos(), self.yaw.sin());
        let t = self.target;
        if z_up {
            (
                [t[0] + self.dist * cp * cy, t[1] + self.dist * cp * sy, t[2] + self.dist * sp],
                [0.0, 0.0, 1.0],
            )
        } else {
            (
                [t[0] + self.dist * cp * cy, t[1] + self.dist * sp, t[2] + self.dist * cp * sy],
                [0.0, 1.0, 0.0],
            )
        }
    }
}

#[derive(Clone, Copy)]
struct CV {
    c: [f32; 4],
    uv: [f32; 2],
    col: [f32; 3],
    light: f32,
}

fn lerp_cv(a: &CV, b: &CV, t: f32) -> CV {
    let l = |x: f32, y: f32| x + (y - x) * t;
    CV {
        c: [l(a.c[0], b.c[0]), l(a.c[1], b.c[1]), l(a.c[2], b.c[2]), l(a.c[3], b.c[3])],
        uv: [l(a.uv[0], b.uv[0]), l(a.uv[1], b.uv[1])],
        col: [l(a.col[0], b.col[0]), l(a.col[1], b.col[1]), l(a.col[2], b.col[2])],
        light: l(a.light, b.light),
    }
}

/// Keeps the part of the polygon in front of the near plane (z + w >= 0).
fn clip_near(poly: &[CV]) -> Vec<CV> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    for i in 0..poly.len() {
        let a = &poly[i];
        let b = &poly[(i + 1) % poly.len()];
        let da = a.c[2] + a.c[3];
        let db = b.c[2] + b.c[3];
        if da >= 0.0 {
            out.push(*a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            out.push(lerp_cv(a, b, da / (da - db)));
        }
    }
    out
}

fn to_screen(c: &[f32; 4], w: f32, h: f32) -> (f32, f32, f32, f32) {
    let iw = 1.0 / c[3];
    ((c[0] * iw * 0.5 + 0.5) * w, (1.0 - (c[1] * iw * 0.5 + 0.5)) * h, c[2] * iw, iw)
}

fn sample(t: &TexImage, u: f32, v: f32) -> [f32; 4] {
    let fu = u - u.floor();
    let fv = v - v.floor();
    let x = ((fu * t.w as f32) as usize).min(t.w - 1);
    let y = ((fv * t.h as f32) as usize).min(t.h - 1);
    let i = (y * t.w + x) * 4;
    [
        t.rgba[i] as f32 / 255.0,
        t.rgba[i + 1] as f32 / 255.0,
        t.rgba[i + 2] as f32 / 255.0,
        t.rgba[i + 3] as f32 / 255.0,
    ]
}

#[allow(clippy::too_many_arguments)]
fn draw_tri(
    a: &CV,
    b: &CV,
    c: &CV,
    tex: Option<&TexImage>,
    o: &RenderOpts,
    w: usize,
    h: usize,
    color: &mut [u8],
    depth: &mut [f32],
) {
    let (wf, hf) = (w as f32, h as f32);
    let (ax, ay, az, aiw) = to_screen(&a.c, wf, hf);
    let (bx, by, bz, biw) = to_screen(&b.c, wf, hf);
    let (cx, cy, cz, ciw) = to_screen(&c.c, wf, hf);
    let area = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    if !area.is_finite() || area.abs() < 1e-8 {
        return;
    }
    if o.cull && area < 0.0 {
        return;
    }
    let min_x = ax.min(bx).min(cx);
    let max_x = ax.max(bx).max(cx);
    let min_y = ay.min(by).min(cy);
    let max_y = ay.max(by).max(cy);
    if max_x < 0.0 || max_y < 0.0 || min_x > wf || min_y > hf {
        return;
    }
    let x0 = min_x.floor().max(0.0) as usize;
    let x1 = (max_x.ceil().min(wf - 1.0)).max(0.0) as usize;
    let y0 = min_y.floor().max(0.0) as usize;
    let y1 = (max_y.ceil().min(hf - 1.0)).max(0.0) as usize;
    let inv_area = 1.0 / area;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = ((bx - px) * (cy - py) - (by - py) * (cx - px)) * inv_area;
            let w1 = ((cx - px) * (ay - py) - (cy - py) * (ax - px)) * inv_area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * az + w1 * bz + w2 * cz;
            let idx = y * w + x;
            if z >= depth[idx] {
                continue;
            }
            let iw = w0 * aiw + w1 * biw + w2 * ciw;
            let inv = 1.0 / iw;
            let (pa, pb, pc) = (w0 * aiw * inv, w1 * biw * inv, w2 * ciw * inv);
            let mut rgb = [0.78f32; 3];
            let mut alpha = 1.0;
            if let Some(t) = tex {
                let u = pa * a.uv[0] + pb * b.uv[0] + pc * c.uv[0];
                let v = pa * a.uv[1] + pb * b.uv[1] + pc * c.uv[1];
                let s = sample(t, u, v);
                rgb = [s[0], s[1], s[2]];
                alpha = s[3];
            }
            if alpha < 0.5 {
                continue;
            }
            let light = pa * a.light + pb * b.light + pc * c.light;
            let col = [
                pa * a.col[0] + pb * b.col[0] + pc * c.col[0],
                pa * a.col[1] + pb * b.col[1] + pc * c.col[1],
                pa * a.col[2] + pb * b.col[2] + pc * c.col[2],
            ];
            depth[idx] = z;
            let o4 = idx * 4;
            for k in 0..3 {
                let mut v = rgb[k] * light;
                if o.vcolors {
                    v *= col[k];
                }
                color[o4 + k] = (v * 255.0).clamp(0.0, 255.0) as u8;
            }
            color[o4 + 3] = 255;
        }
    }
}

fn draw_line(a: &CV, b: &CV, w: usize, h: usize, color: &mut [u8], depth: &mut [f32]) {
    let (wf, hf) = (w as f32, h as f32);
    let (ax, ay, az, _) = to_screen(&a.c, wf, hf);
    let (bx, by, bz, _) = to_screen(&b.c, wf, hf);
    if !(ax.is_finite() && ay.is_finite() && bx.is_finite() && by.is_finite()) {
        return;
    }
    if (ax < 0.0 && bx < 0.0) || (ax > wf && bx > wf) || (ay < 0.0 && by < 0.0) || (ay > hf && by > hf) {
        return;
    }
    let steps = ((bx - ax).abs().max((by - ay).abs()).ceil().max(1.0) as usize).min(4096);
    let light = 0.5 * (a.light + b.light);
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let x = ax + (bx - ax) * t;
        let y = ay + (by - ay) * t;
        if x < 0.0 || y < 0.0 || x >= wf || y >= hf {
            continue;
        }
        let z = az + (bz - az) * t;
        let idx = y as usize * w + x as usize;
        if z > depth[idx] + 1e-4 {
            continue;
        }
        depth[idx] = z.min(depth[idx]);
        let o4 = idx * 4;
        color[o4] = (110.0 * light).clamp(0.0, 255.0) as u8;
        color[o4 + 1] = (200.0 * light).clamp(0.0, 255.0) as u8;
        color[o4 + 2] = (255.0 * light).clamp(0.0, 255.0) as u8;
        color[o4 + 3] = 255;
    }
}

fn render(
    scene: &Scene,
    cam: &Camera,
    o: &RenderOpts,
    w: usize,
    h: usize,
    color: &mut Vec<u8>,
    depth: &mut Vec<f32>,
) {
    color.resize(w * h * 4, 0);
    depth.resize(w * h, f32::INFINITY);
    for y in 0..h {
        let t = y as f32 / h as f32;
        let r = (22.0 + 20.0 * t) as u8;
        let g = (24.0 + 22.0 * t) as u8;
        let b = (30.0 + 26.0 * t) as u8;
        for x in 0..w {
            let i = (y * w + x) * 4;
            color[i] = r;
            color[i + 1] = g;
            color[i + 2] = b;
            color[i + 3] = 255;
        }
    }
    for d in depth.iter_mut() {
        *d = f32::INFINITY;
    }

    let (eye, up) = cam.eye_up(o.z_up);
    let view = look_at(eye, cam.target, up);
    let near = (cam.dist * 0.02).max(1e-3);
    let far = cam.dist * 30.0 + scene.radius() * 4.0;
    let proj = perspective(FOV, w as f32 / h as f32, near, far);
    let vp = mat_mul(&proj, &view);

    let mut clips: Vec<[f32; 4]> = Vec::with_capacity(scene.verts.len());
    let mut lights: Vec<f32> = Vec::with_capacity(scene.verts.len());
    for v in &scene.verts {
        clips.push(mul_point4(&vp, v.p));
        let l = if v.has_n {
            let nz = view[2][0] * v.n[0] + view[2][1] * v.n[1] + view[2][2] * v.n[2];
            0.45 + 0.55 * nz.abs()
        } else {
            -1.0
        };
        lights.push(l);
    }

    for t in &scene.tris {
        if !scene.mesh_visible[t.mesh as usize] {
            continue;
        }
        let ids = [t.i[0] as usize, t.i[1] as usize, t.i[2] as usize];
        let mut flat = -1.0f32;
        if lights[ids[0]] < 0.0 || lights[ids[1]] < 0.0 || lights[ids[2]] < 0.0 {
            let p0 = scene.verts[ids[0]].p;
            let n = normalize(cross(sub(scene.verts[ids[1]].p, p0), sub(scene.verts[ids[2]].p, p0)));
            let nz = view[2][0] * n[0] + view[2][1] * n[1] + view[2][2] * n[2];
            flat = 0.45 + 0.55 * nz.abs();
        }
        let mk = |k: usize| -> CV {
            let v = &scene.verts[ids[k]];
            CV {
                c: clips[ids[k]],
                uv: v.uv,
                col: if o.vcolors { v.c } else { [1.0, 1.0, 1.0] },
                light: if lights[ids[k]] >= 0.0 { lights[ids[k]] } else { flat },
            }
        };
        let cv = [mk(0), mk(1), mk(2)];
        let tex = if o.mode == Mode::Textured && t.tex >= 0 {
            Some(&*scene.textures[t.tex as usize])
        } else {
            None
        };
        let inside = cv.iter().all(|v| v.c[2] + v.c[3] >= 0.0 && v.c[3] > 1e-6);
        let polys: Vec<CV> = if inside { cv.to_vec() } else { clip_near(&cv) };
        if polys.len() < 3 {
            continue;
        }
        if o.mode == Mode::Wire {
            for k in 0..polys.len() {
                draw_line(&polys[k], &polys[(k + 1) % polys.len()], w, h, color, depth);
            }
        } else {
            for k in 1..polys.len() - 1 {
                draw_tri(&polys[0], &polys[k], &polys[k + 1], tex, o, w, h, color, depth);
            }
        }
    }
}

const FOV: f32 = 0.9; // vertical field of view in radians

// ---------------------------------------------------------------- viewer window

pub struct ModelViewer {
    pub name: String,
    scene: Scene,
    cam: Camera,
    z_up: bool,
    mode: Mode,
    cull: bool,
    vcolors: bool,
    handle: Option<egui::TextureHandle>,
    dirty: bool,
    last_size: (usize, usize),
    color: Vec<u8>,
    depth: Vec<f32>,
    warnings: Vec<String>,
    interacting: bool,
}

impl ModelViewer {
    pub fn new(name: String, scene: Scene, warnings: Vec<String>) -> Self {
        let mut v = Self {
            name,
            scene,
            cam: Camera { target: [0.0; 3], dist: 1.0, yaw: 0.8, pitch: 0.35 },
            z_up: true,
            mode: Mode::Textured,
            cull: false,
            vcolors: true,
            handle: None,
            dirty: true,
            last_size: (0, 0),
            color: Vec::new(),
            depth: Vec::new(),
            warnings,
            interacting: false,
        };
        v.frame();
        v
    }

    fn frame(&mut self) {
        self.cam.target = self.scene.center();
        self.cam.dist = self.scene.radius() / (FOV * 0.5).tan() * 1.3;
        self.dirty = true;
    }

    /// Draws the viewer over the whole window; returns false when it should close.
    pub fn ui(&mut self, ctx: &egui::Context) -> bool {
        let mut open = true;
        let screen = ctx.screen_rect();
        let panel_w = 290.0;
        egui::Area::new(egui::Id::new("model_viewer"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.visuals_mut().override_text_color = Some(Color32::from_gray(225));
                let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 0.0, Color32::from_rgb(20, 22, 26));
                let top_h = 40.0;
                let view = Rect::from_min_max(
                    pos2(rect.min.x, rect.min.y + top_h),
                    pos2(rect.max.x - panel_w, rect.max.y),
                );
                let panel = Rect::from_min_max(pos2(rect.max.x - panel_w, rect.min.y + top_h), rect.max);

                // camera input
                let hovering = ctx.pointer_hover_pos().map(|q| view.contains(q)).unwrap_or(false);
                let dragging = resp.dragged() && hovering;
                let mut changed = false;
                if dragging {
                    let d = resp.drag_delta();
                    if resp.dragged_by(egui::PointerButton::Secondary)
                        || resp.dragged_by(egui::PointerButton::Middle)
                    {
                        let (eye, up) = self.cam.eye_up(self.z_up);
                        let f = normalize(sub(self.cam.target, eye));
                        let right = normalize(cross(f, up));
                        let upv = cross(right, f);
                        let k = self.cam.dist * 0.0016;
                        for i in 0..3 {
                            self.cam.target[i] += (-right[i] * d.x + upv[i] * d.y) * k;
                        }
                    } else {
                        self.cam.yaw -= d.x * 0.008;
                        self.cam.pitch = (self.cam.pitch + d.y * 0.008).clamp(-1.55, 1.55);
                    }
                    changed = true;
                }
                if hovering {
                    let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
                    if scroll != 0.0 {
                        self.cam.dist = (self.cam.dist * (-scroll * 0.0015).exp())
                            .clamp(self.scene.radius() * 0.01, self.scene.radius() * 60.0);
                        changed = true;
                    }
                }
                if dragging != self.interacting {
                    self.interacting = dragging;
                    changed = true;
                }
                if changed {
                    self.dirty = true;
                }

                // render (half resolution while dragging)
                let scale = if self.interacting { 0.5 } else { 1.0 };
                let k = (1400.0 / view.width()).min(900.0 / view.height()).min(1.0) * scale;
                let w = ((view.width() * k) as usize).max(16);
                let h = ((view.height() * k) as usize).max(16);
                if self.dirty || self.last_size != (w, h) || self.handle.is_none() {
                    let opts = RenderOpts {
                        mode: self.mode,
                        cull: self.cull,
                        vcolors: self.vcolors,
                        z_up: self.z_up,
                    };
                    render(&self.scene, &self.cam, &opts, w, h, &mut self.color, &mut self.depth);
                    let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &self.color);
                    match &mut self.handle {
                        Some(hd) => hd.set(img, egui::TextureOptions::LINEAR),
                        None => {
                            self.handle =
                                Some(ctx.load_texture("model_view", img, egui::TextureOptions::LINEAR));
                        }
                    }
                    self.last_size = (w, h);
                    self.dirty = false;
                }
                if let Some(hd) = &self.handle {
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    p.image(hd.id(), view, uv, Color32::WHITE);
                }

                // top bar
                let bar = Rect::from_min_size(rect.min, vec2(rect.width(), top_h));
                p.rect_filled(bar, 0.0, Color32::from_black_alpha(170));
                ui.allocate_ui_at_rect(bar.shrink2(vec2(12.0, 7.0)), |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(self.name.as_str());
                        let visible_tris: usize = self
                            .scene
                            .mesh_tris
                            .iter()
                            .zip(&self.scene.mesh_visible)
                            .filter(|(_, v)| **v)
                            .map(|(t, _)| *t)
                            .sum();
                        ui.label(format!(
                            "{} meshes - {} vertices - {} triangles",
                            self.scene.mesh_names.len(),
                            self.scene.verts.len(),
                            visible_tris
                        ));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Close (Esc)").clicked() {
                                open = false;
                            }
                            if ui.button("Frame (F)").clicked() {
                                self.frame();
                            }
                        });
                    });
                });

                // side panel
                p.rect_filled(panel, 0.0, Color32::from_rgb(34, 36, 42));
                ui.allocate_ui_at_rect(panel.shrink(10.0), |ui| {
                    ui.strong("Display");
                    ui.horizontal(|ui| {
                        for (m, label) in
                            [(Mode::Textured, "Textured"), (Mode::Solid, "Solid"), (Mode::Wire, "Wireframe")]
                        {
                            if ui.selectable_value(&mut self.mode, m, label).changed() {
                                self.dirty = true;
                            }
                        }
                    });
                    if ui.checkbox(&mut self.vcolors, "Vertex colors").changed() {
                        self.dirty = true;
                    }
                    if ui.checkbox(&mut self.cull, "Backface culling").changed() {
                        self.dirty = true;
                    }
                    if ui.checkbox(&mut self.z_up, "Z axis is up").changed() {
                        self.frame();
                    }
                    ui.label(
                        egui::RichText::new("Drag: orbit   Right-drag: pan   Wheel: zoom").weak().small(),
                    );
                    ui.separator();

                    ui.horizontal(|ui| {
                        ui.strong("Meshes");
                        if ui.small_button("All").clicked() {
                            self.scene.mesh_visible.iter_mut().for_each(|v| *v = true);
                            self.dirty = true;
                        }
                        if ui.small_button("None").clicked() {
                            self.scene.mesh_visible.iter_mut().for_each(|v| *v = false);
                            self.dirty = true;
                        }
                    });
                    let n = self.scene.mesh_names.len();
                    let row_h = ui.spacing().interact_size.y;
                    egui::ScrollArea::vertical().max_height(250.0).auto_shrink([false, true]).show_rows(
                        ui,
                        row_h,
                        n,
                        |ui, range| {
                            for i in range {
                                let mut vis = self.scene.mesh_visible[i];
                                let label =
                                    format!("{} ({})", self.scene.mesh_names[i], self.scene.mesh_tris[i]);
                                if ui.checkbox(&mut vis, label).changed() {
                                    self.scene.mesh_visible[i] = vis;
                                    self.dirty = true;
                                }
                            }
                        },
                    );
                    ui.separator();

                    let found = self.scene.materials.iter().filter(|m| m.found).count();
                    ui.strong(format!(
                        "Materials: {} textures found of {}",
                        found,
                        self.scene.materials.len()
                    ));
                    egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                        for m in &self.scene.materials {
                            let color = if m.found {
                                Color32::from_rgb(120, 200, 120)
                            } else {
                                Color32::from_rgb(230, 120, 110)
                            };
                            ui.colored_label(color, format!("{} -> {}", m.name, m.texture));
                        }
                    });
                    if !self.warnings.is_empty() {
                        ui.separator();
                        ui.colored_label(
                            Color32::from_rgb(230, 190, 90),
                            format!("{} warning(s) while reading the file:", self.warnings.len()),
                        );
                        for w in self.warnings.iter().take(3) {
                            ui.label(egui::RichText::new(w.as_str()).small());
                        }
                    }
                });
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F)) {
            self.frame();
        }
        open
    }
}
