//! Reader for `.zwo` models (Obscure 1 and 2).
//!
//! The file layout was worked out by the zwoBlender add-on (Al-Hydra); this is a port of its
//! reader. Everything is big-endian. A file is a list of entities, each starting with a u32
//! type id and (except for nodes) a u32 size that includes itself:
//!   0x02 camera, 0x03 material, 0x04 end, 0x05 mesh, 0x06 skeleton, 0x07 skeletal animation,
//!   0x09 camera animation, 0x0A node (no size), 0x0D light, 0x0E animation,
//!   0x0F ambient box, 0x10 omni light.
//! Only materials and meshes are read; skeletons and animations are skipped for now.

use std::fmt;

/// Row-major 4x4 matrix as stored in the file (column vectors, translation in column 3).
pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut o = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[i][k] * b[k][j];
            }
            o[i][j] = s;
        }
    }
    o
}

pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
    ]
}

/// Rotates / scales a direction (no translation).
pub fn transform_dir(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

#[derive(Debug)]
pub enum ModelError {
    Truncated(&'static str),
    Invalid(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::Truncated(what) => write!(f, "unexpected end of data while reading {what}"),
            ModelError::Invalid(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for ModelError {}

#[derive(Debug, Clone, Default)]
pub struct Material {
    pub name: String,
    pub texture: String,
    pub texture2: String,
}

/// One vertex buffer. A mesh can have several (one per transformer, e.g. the links of a chain);
/// they all share the mesh's index buffer.
#[derive(Debug, Clone, Default)]
pub struct VertexSet {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv0: Vec<[f32; 2]>,
    /// Vertex color 0 as stored: bytes A, R, G, B.
    pub color0: Vec<[u8; 4]>,
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub name: String,
    /// 2 = rigid, 6 = deformable (skinned).
    pub mesh_type: u32,
    /// Names of the materials the faces refer to.
    pub material_names: Vec<String>,
    pub buffers: Vec<VertexSet>,
    /// (local, world) matrices, one pair per transformer.
    pub transformers: Vec<(Mat4, Mat4)>,
    pub triangles: Vec<[u32; 3]>,
    pub tri_material: Vec<u32>,
    /// Name of the mesh this one instances, with its own world matrix.
    pub instance_of: Option<String>,
    pub instance_world: Option<Mat4>,
}

#[derive(Debug, Clone, Default)]
pub struct Model {
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub warnings: Vec<String>,
    /// The file has a skeleton entity (not read yet).
    pub has_skeleton: bool,
    /// The file has animation entities (not read yet).
    pub has_animations: bool,
}

/// A buffer of a mesh to draw: geometry of `source`, placed with `matrix`; `mesh` is the entry it belongs to.
#[derive(Debug, Clone)]
pub struct DrawItem {
    pub mesh: usize,
    pub source: usize,
    pub buffer: usize,
    pub matrix: Mat4,
}

impl Model {
    /// Resolves instances and transforms, like the Blender importer does:
    /// rigid: world * local, deformable (rest pose): world, instance: instance_world * local of the original.
    /// Every vertex buffer is drawn with the transformer of the same index.
    pub fn draw_items(&self) -> Vec<DrawItem> {
        let mut out = Vec::new();
        for (i, m) in self.meshes.iter().enumerate() {
            if let Some(target) = &m.instance_of {
                let src = self
                    .meshes
                    .iter()
                    .position(|o| o.instance_of.is_none() && &o.name == target);
                if let Some(s) = src {
                    let sm = &self.meshes[s];
                    for (b, vs) in sm.buffers.iter().enumerate() {
                        if vs.positions.is_empty() {
                            continue;
                        }
                        let local = if sm.mesh_type == 6 {
                            IDENTITY
                        } else {
                            sm.transformers.get(b).or(sm.transformers.first()).map_or(IDENTITY, |t| t.0)
                        };
                        let matrix = mat_mul(&m.instance_world.unwrap_or(IDENTITY), &local);
                        out.push(DrawItem { mesh: i, source: s, buffer: b, matrix });
                    }
                }
            } else {
                for (b, vs) in m.buffers.iter().enumerate() {
                    if vs.positions.is_empty() {
                        continue;
                    }
                    let (local, world) = m
                        .transformers
                        .get(b)
                        .or(m.transformers.first())
                        .copied()
                        .unwrap_or((IDENTITY, IDENTITY));
                    let matrix = if m.mesh_type == 6 { world } else { mat_mul(&world, &local) };
                    out.push(DrawItem { mesh: i, source: i, buffer: b, matrix });
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------- reader

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], ModelError> {
        let end = self.pos.checked_add(n).ok_or(ModelError::Truncated(what))?;
        if end > self.data.len() {
            return Err(ModelError::Truncated(what));
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn skip(&mut self, n: usize, what: &'static str) -> Result<(), ModelError> {
        self.take(n, what).map(|_| ())
    }

    fn u8(&mut self, what: &'static str) -> Result<u8, ModelError> {
        Ok(self.take(1, what)?[0])
    }

    fn u16(&mut self, what: &'static str) -> Result<u16, ModelError> {
        let b = self.take(2, what)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, ModelError> {
        let b = self.take(4, what)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f32(&mut self, what: &'static str) -> Result<f32, ModelError> {
        let b = self.take(4, what)?;
        Ok(f32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn peek_u32(&self, what: &'static str) -> Result<u32, ModelError> {
        if self.remaining() < 4 {
            return Err(ModelError::Truncated(what));
        }
        let b = &self.data[self.pos..self.pos + 4];
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn floats<const N: usize>(&mut self, what: &'static str) -> Result<[f32; N], ModelError> {
        let mut o = [0.0f32; N];
        for v in o.iter_mut() {
            *v = self.f32(what)?;
        }
        Ok(o)
    }

    /// u32 length + bytes (cp1252 / Latin-1).
    fn string(&mut self, what: &'static str) -> Result<String, ModelError> {
        let len = self.u32(what)? as usize;
        if len > 8192 {
            return Err(ModelError::Invalid(format!("implausible string length {len} in {what}")));
        }
        let b = self.take(len, what)?;
        let s: String = b.iter().map(|&c| c as char).collect();
        Ok(s.trim_end_matches('\0').to_string())
    }

    /// position(3) + scale(3) + rotation(4) + 4x4 matrix; only the matrix is kept.
    fn transformer(&mut self) -> Result<Mat4, ModelError> {
        self.skip(10 * 4, "transformer header")?;
        let mut m = [[0.0f32; 4]; 4];
        for row in m.iter_mut() {
            *row = self.floats::<4>("transformer matrix")?;
        }
        Ok(m)
    }
}

fn read_vertex_buffer(r: &mut Reader, keep: bool) -> Result<VertexSet, ModelError> {
    let count = r.u32("vertex count")? as usize;
    let flags = r.u32("vertex flags")?;
    let has_normal = flags & 0x1 != 0;
    let has_c0 = flags & 0x2 != 0;
    let has_c1 = flags & 0x4 != 0;
    let has_uv0 = flags & 0x8 != 0;
    let mut stride = 12usize;
    if has_normal {
        stride += 12;
    }
    if has_c0 {
        stride += 4;
    }
    if has_c1 {
        stride += 4;
    }
    for bit in [0x8u32, 0x10, 0x20, 0x40] {
        if flags & bit != 0 {
            stride += 8; // uv0..uv3
        }
    }
    for bit in [0x80u32, 0x100, 0x200, 0x400] {
        if flags & bit != 0 {
            stride += 8; // bone index (u32) + weight (f32)
        }
    }
    let total = count
        .checked_mul(stride)
        .ok_or_else(|| ModelError::Invalid("vertex buffer size overflow".into()))?;
    if total > r.remaining() {
        return Err(ModelError::Invalid(format!(
            "vertex buffer ({count} vertices x {stride} bytes) is larger than the entity"
        )));
    }
    let mut vb = VertexSet::default();
    if !keep {
        r.skip(total, "vertex data")?;
        return Ok(vb);
    }
    vb.positions.reserve(count);
    for _ in 0..count {
        vb.positions.push(r.floats::<3>("vertex position")?);
        if has_normal {
            vb.normals.push(r.floats::<3>("vertex normal")?);
        }
        if has_c0 {
            let b = r.take(4, "vertex color")?;
            vb.color0.push([b[0], b[1], b[2], b[3]]);
        }
        if has_c1 {
            r.skip(4, "vertex color 1")?;
        }
        if has_uv0 {
            vb.uv0.push(r.floats::<2>("vertex uv")?);
        }
        for bit in [0x10u32, 0x20, 0x40] {
            if flags & bit != 0 {
                r.skip(8, "extra uv")?;
            }
        }
        for bit in [0x80u32, 0x100, 0x200, 0x400] {
            if flags & bit != 0 {
                r.skip(8, "bone weight")?;
            }
        }
    }
    Ok(vb)
}

/// Layout details that are not known for certain. All combinations are tried and the one that
/// consumes the entity exactly wins. (Every real file checked so far uses the first one.)
#[derive(Clone, Copy)]
struct Variant {
    /// `flags2 & 0x10000` / `0x20000` are followed by one u32 each.
    extra_values: bool,
    /// The animation frame (flags1 & 0x20) comes right after the entity header, before the geometry.
    anim_before_geometry: bool,
}

fn variants() -> Vec<Variant> {
    vec![
        Variant { extra_values: false, anim_before_geometry: false },
        Variant { extra_values: true, anim_before_geometry: false },
        Variant { extra_values: false, anim_before_geometry: true },
        Variant { extra_values: true, anim_before_geometry: true },
    ]
}

/// Reads a mesh whose size field starts at `start`. Returns the mesh and the offset after it.
fn read_mesh(data: &[u8], start: usize) -> Result<(Mesh, usize), ModelError> {
    let size = Reader::new(data, start).peek_u32("mesh size")? as usize;
    let end = start
        .checked_add(size)
        .filter(|&e| size >= 8 && e <= data.len())
        .ok_or_else(|| ModelError::Invalid(format!("bad mesh size {size} at offset {start}")))?;

    let mut best: Option<(usize, Mesh)> = None;
    let mut first_err: Option<ModelError> = None;
    for v in variants() {
        match read_mesh_variant(&data[..end], start, v) {
            Ok((mesh, pos)) => {
                let leftover = end.saturating_sub(pos);
                if best.as_ref().map_or(true, |(l, _)| leftover < *l) {
                    best = Some((leftover, mesh));
                }
                if leftover == 0 {
                    break;
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    match best {
        Some((_, mesh)) => Ok((mesh, end)),
        None => Err(first_err.unwrap_or_else(|| ModelError::Invalid("unreadable mesh".into()))),
    }
}

fn read_mesh_variant(data: &[u8], start: usize, v: Variant) -> Result<(Mesh, usize), ModelError> {
    let mut r = Reader::new(data, start);

    // entity header
    r.u32("entity size")?;
    let header_type = r.u8("entity header type")?;
    let mut mesh = Mesh::default();
    if header_type == 5 {
        mesh.name = r.string("mesh name")?;
        r.u32("entity type")?;
    } else if header_type == 6 {
        mesh.name = r.string("mesh name")?;
        r.u32("entity type")?;
        r.skip(5 * 4, "entity header")?;
    }

    // Entity3D
    let material_count = r.u32("material count")? as usize;
    if material_count > 4096 {
        return Err(ModelError::Invalid(format!("implausible material count {material_count}")));
    }
    for _ in 0..material_count {
        mesh.material_names.push(r.string("material name")?);
    }
    let flags1 = r.u32("flags1")?;
    let flags2 = r.u32("flags2")?;
    r.u32("unknown")?;
    if v.extra_values {
        if flags2 & 0x10000 != 0 {
            r.u32("flags2 value 1")?;
        }
        if flags2 & 0x20000 != 0 {
            r.u32("flags2 value 2")?;
        }
    }
    let has_geometry = flags1 & 0x1 != 0;
    let has_instance = flags1 & 0x2 != 0;
    let has_parent_sibling = flags1 & 0x4 != 0;
    let has_anim_frame = flags1 & 0x20 != 0;
    let has_extra_int = flags1 & 0x40 != 0;
    if has_parent_sibling {
        r.string("parent name")?;
        r.string("sibling name")?;
    }
    // the entity type is always present (verified on real files: with or without parent / sibling)
    r.u32("entity type")?;
    if has_extra_int {
        r.u32("extra value")?;
    }
    if has_geometry {
        mesh.mesh_type = r.u32("mesh type")?;
    }
    if has_instance {
        mesh.instance_of = Some(r.string("instanced object name")?);
        mesh.instance_world = Some(r.transformer()?);
    }
    if has_anim_frame && v.anim_before_geometry {
        r.u32("animation frame")?;
    }

    // geometry (instances have none: the entity ends after the instance transformer)
    if has_geometry {
        let tcount = r.u32("transformer count")? as usize;
        if tcount > 4096 {
            return Err(ModelError::Invalid(format!("implausible transformer count {tcount}")));
        }
        r.skip(tcount * 4, "transformer table")?;
        for _ in 0..tcount {
            let local = r.transformer()?;
            let world = r.transformer()?;
            r.skip(12 * 4, "bounding box")?;
            mesh.transformers.push((local, world));
        }
        let vb_flag = r.u32("vertex buffer flag")?;
        r.u32("unknown")?;
        if mesh.mesh_type == 2 {
            r.skip(8, "rigid mesh values")?;
        }
        let vb_count = if (vb_flag >> 2) & 1 == 1 { 1 } else { tcount };

        // vertex buffers: one per transformer, sharing the index buffer
        for _ in 0..vb_count {
            mesh.buffers.push(read_vertex_buffer(&mut r, true)?);
        }

        // faces
        let face_count = r.u32("face count")? as usize;
        r.u32("triangles type")?;
        let index_type = r.u32("index type")?;
        let rec = if index_type == 1 { 16 } else { 8 };
        if face_count.checked_mul(rec).map_or(true, |n| n > r.remaining()) {
            return Err(ModelError::Invalid(format!(
                "face buffer ({face_count} faces) is larger than the entity"
            )));
        }
        mesh.triangles.reserve(face_count);
        mesh.tri_material.reserve(face_count);
        for _ in 0..face_count {
            if index_type == 1 {
                let a = r.u32("face index")?;
                let b = r.u32("face index")?;
                let c = r.u32("face index")?;
                let m = r.u32("face material")?;
                mesh.triangles.push([a, b, c]);
                mesh.tri_material.push(m);
            } else {
                let a = r.u16("face index")? as u32;
                let b = r.u16("face index")? as u32;
                let c = r.u16("face index")? as u32;
                let m = r.u16("face material")? as u32;
                mesh.triangles.push([a, b, c]);
                mesh.tri_material.push(m);
            }
        }
    }
    if has_anim_frame && !v.anim_before_geometry {
        r.u32("animation frame")?;
    }
    Ok((mesh, r.pos))
}

fn read_material(data: &[u8], start: usize) -> Result<(Material, usize), ModelError> {
    let size = Reader::new(data, start).peek_u32("material size")? as usize;
    let end = start
        .checked_add(size)
        .filter(|&e| size >= 5 && e <= data.len())
        .ok_or_else(|| ModelError::Invalid(format!("bad material size {size} at offset {start}")))?;
    let mut r = Reader::new(&data[..end], start);
    r.u32("material size")?;
    let flag = r.u8("material flag")?;
    let mut mat = Material::default();
    if flag <= 3 {
        let name_len = r.u32("material name length")? as usize;
        if name_len > 8192 {
            return Err(ModelError::Invalid("implausible material name length".into()));
        }
        let b = r.take(name_len, "material name")?;
        mat.name = b.iter().map(|&c| c as char).collect::<String>().trim_end_matches('\0').to_string();
        r.skip((9 + 5 + 3) * 4, "material colors")?; // 3 vectors, 5 floats, 1 vector
        r.skip(2 * 4, "material values")?;
        r.skip((2 + 2 + 3 + 2) * 4, "material uv transform")?;
        r.skip(4, "material value")?;
        mat.texture = r.string("texture name")?;
        mat.texture2 = r.string("second texture name")?;
    }
    Ok((mat, end))
}

/// Nodes have no size field: id, flags, 4 vectors, then a child and a sibling chain.
fn skip_node(r: &mut Reader) -> Result<(), ModelError> {
    loop {
        r.skip(2 * 4, "node header")?;
        r.skip(12 * 4, "node vectors")?;
        let child_count = r.u32("node child count")?;
        if child_count > 0 {
            let child_index = r.u32("node child index")?;
            if child_index > 0 {
                skip_node(r)?;
            }
        }
        let next = r.u32("node flag")?;
        if next == 0 {
            return Ok(());
        }
    }
}

fn sized_end(data: &[u8], pos: usize) -> Option<usize> {
    let r = Reader::new(data, pos);
    let size = r.peek_u32("size").ok()? as usize;
    let end = pos.checked_add(size)?;
    if size < 4 || end > data.len() {
        None
    } else {
        Some(end)
    }
}

pub fn parse_zwo(data: &[u8]) -> Result<Model, ModelError> {
    let mut model = Model::default();
    let mut pos = 0usize;
    while pos + 4 <= data.len() {
        let t = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        pos += 4;
        match t {
            0x04 => break,
            0x03 => match read_material(data, pos) {
                Ok((m, end)) => {
                    model.materials.push(m);
                    pos = end;
                }
                Err(e) => {
                    model.warnings.push(format!("material at offset {pos}: {e}"));
                    match sized_end(data, pos) {
                        Some(end) => pos = end,
                        None => break,
                    }
                }
            },
            0x05 => match read_mesh(data, pos) {
                Ok((m, end)) => {
                    model.meshes.push(m);
                    pos = end;
                }
                Err(e) => {
                    model.warnings.push(format!("mesh at offset {pos}: {e}"));
                    match sized_end(data, pos) {
                        Some(end) => pos = end,
                        None => break,
                    }
                }
            },
            0x0A => {
                let mut r = Reader::new(data, pos);
                match skip_node(&mut r) {
                    Ok(()) => pos = r.pos,
                    Err(e) => {
                        model.warnings.push(format!("node at offset {pos}: {e}; the rest of the file was not read"));
                        break;
                    }
                }
            }
            0x02 | 0x06 | 0x07 | 0x09 | 0x0D | 0x0E | 0x0F | 0x10 => {
                if t == 0x06 {
                    model.has_skeleton = true;
                }
                if matches!(t, 0x07 | 0x09 | 0x0E) {
                    model.has_animations = true;
                }
                match sized_end(data, pos) {
                Some(end) => pos = end,
                    None => {
                        model.warnings.push(format!("entity 0x{t:02X} at offset {}: bad size", pos - 4));
                        break;
                    }
                }
            }
            other => {
                model.warnings.push(format!(
                    "unknown entity type 0x{other:X} at offset {}; the rest of the file was not read",
                    pos - 4
                ));
                break;
            }
        }
    }
    let missing: Vec<String> = model
        .meshes
        .iter()
        .filter_map(|m| m.instance_of.as_ref())
        .filter(|t| !model.meshes.iter().any(|o| o.instance_of.is_none() && &o.name == *t))
        .cloned()
        .collect();
    if !missing.is_empty() {
        model.warnings.push(format!(
            "{} instance(s) refer to meshes that are not in this file (e.g. '{}')",
            missing.len(),
            missing[0]
        ));
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_translation_is_in_the_last_column() {
        let mut m = IDENTITY;
        m[0][3] = 5.0;
        m[1][3] = -2.0;
        assert_eq!(transform_point(&m, [1.0, 1.0, 1.0]), [6.0, -1.0, 1.0]);
        assert_eq!(transform_dir(&m, [1.0, 1.0, 1.0]), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn matrix_product_applies_right_operand_first() {
        let mut t = IDENTITY;
        t[0][3] = 10.0;
        let mut s = IDENTITY;
        s[0][0] = 2.0;
        // world * local: scale first, then translate
        let m = mat_mul(&t, &s);
        assert_eq!(transform_point(&m, [1.0, 0.0, 0.0]), [12.0, 0.0, 0.0]);
    }

    #[test]
    fn empty_and_end_marker_files_parse() {
        assert!(parse_zwo(&[]).unwrap().meshes.is_empty());
        assert!(parse_zwo(&[0, 0, 0, 4]).unwrap().meshes.is_empty());
    }


    // ---- synthetic file builders (layout verified against real .zwo files) ----
    fn put_u32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_be_bytes());
    }
    fn put_f32(b: &mut Vec<u8>, v: f32) {
        b.extend_from_slice(&v.to_be_bytes());
    }
    fn put_str(b: &mut Vec<u8>, s: &str) {
        put_u32(b, s.len() as u32);
        b.extend_from_slice(s.as_bytes());
    }
    fn put_transformer(b: &mut Vec<u8>, tx: f32) {
        for _ in 0..10 {
            put_f32(b, 0.0);
        }
        for (r, row) in IDENTITY.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                put_f32(b, if r == 0 && c == 3 { tx } else { *v });
            }
        }
    }

    /// A rigid mesh with `buffers` vertex buffers of one triangle each (they share the faces).
    fn mesh_bytes(flags2: u32, flags2_value: Option<u32>, buffers: usize) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, 0); // size, patched below
        b.push(5); // header type
        put_str(&mut b, "tri");
        put_u32(&mut b, 0);
        put_u32(&mut b, 1); // material count
        put_str(&mut b, "mat0");
        put_u32(&mut b, 1); // flags1: geometry
        put_u32(&mut b, flags2);
        put_u32(&mut b, 0);
        if let Some(v) = flags2_value {
            put_u32(&mut b, v);
        }
        put_u32(&mut b, 0); // entity type (always present)
        put_u32(&mut b, 2); // mesh type: rigid
        put_u32(&mut b, buffers as u32); // transformer count
        for _ in 0..buffers {
            put_u32(&mut b, 0x21);
        }
        for _ in 0..buffers {
            put_transformer(&mut b, 0.0);
            put_transformer(&mut b, 0.0);
            for _ in 0..12 {
                put_f32(&mut b, 0.0);
            }
        }
        put_u32(&mut b, 0); // vertex buffer flag
        put_u32(&mut b, 0);
        put_f32(&mut b, 0.0);
        put_f32(&mut b, 0.0);
        for k in 0..buffers {
            put_u32(&mut b, 3); // vertices
            put_u32(&mut b, 0x1 | 0x8); // normal + uv0
            for (i, p) in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].iter().enumerate() {
                put_f32(&mut b, p[0] + k as f32 * 10.0);
                put_f32(&mut b, p[1]);
                put_f32(&mut b, p[2]);
                for v in [0.0f32, 0.0, 1.0] {
                    put_f32(&mut b, v);
                }
                put_f32(&mut b, i as f32 * 0.5);
                put_f32(&mut b, 0.25);
            }
        }
        put_u32(&mut b, 1); // faces
        put_u32(&mut b, 3);
        put_u32(&mut b, 0); // 16-bit indices
        for v in [0u16, 1, 2, 0] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        let size = b.len() as u32;
        b[0..4].copy_from_slice(&size.to_be_bytes());
        b
    }

    /// An instance entity: no geometry, it ends right after the instance transformer.
    fn instance_bytes(target: &str, tx: f32) -> Vec<u8> {
        let mut b = Vec::new();
        put_u32(&mut b, 0);
        b.push(5);
        put_str(&mut b, "inst");
        put_u32(&mut b, 0);
        put_u32(&mut b, 0); // no materials
        put_u32(&mut b, 2); // flags1: instance
        put_u32(&mut b, 0);
        put_u32(&mut b, 0);
        put_u32(&mut b, 0); // entity type
        put_str(&mut b, target);
        put_transformer(&mut b, tx);
        let size = b.len() as u32;
        b[0..4].copy_from_slice(&size.to_be_bytes());
        b
    }

    fn file_with(meshes: Vec<Vec<u8>>) -> Vec<u8> {
        let mut f = Vec::new();
        for m in meshes {
            put_u32(&mut f, 5);
            f.extend(m);
        }
        put_u32(&mut f, 4);
        f
    }

    #[test]
    fn reads_a_rigid_triangle() {
        let m = parse_zwo(&file_with(vec![mesh_bytes(0, None, 1)])).unwrap();
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        assert_eq!(m.meshes.len(), 1);
        let mesh = &m.meshes[0];
        assert_eq!(mesh.name, "tri");
        assert_eq!(mesh.mesh_type, 2);
        assert_eq!(mesh.buffers.len(), 1);
        assert_eq!(mesh.buffers[0].positions.len(), 3);
        assert_eq!(mesh.buffers[0].normals.len(), 3);
        assert_eq!(mesh.buffers[0].uv0[1], [0.5, 0.25]);
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
        assert_eq!(mesh.material_names, vec!["mat0".to_string()]);
        assert_eq!(m.draw_items().len(), 1);
    }

    #[test]
    fn reads_many_buffers_sharing_the_faces() {
        // like chain1.zwo: one vertex buffer per transformer (the links of a chain)
        let m = parse_zwo(&file_with(vec![mesh_bytes(0, None, 3)])).unwrap();
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        let mesh = &m.meshes[0];
        assert_eq!(mesh.buffers.len(), 3);
        assert_eq!(mesh.transformers.len(), 3);
        assert_eq!(mesh.buffers[2].positions[0], [20.0, 0.0, 0.0]);
        assert_eq!(m.draw_items().len(), 3);
    }

    #[test]
    fn instances_have_no_geometry_and_reuse_the_original() {
        let m = parse_zwo(&file_with(vec![mesh_bytes(0, None, 1), instance_bytes("tri", 5.0)])).unwrap();
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        assert_eq!(m.meshes.len(), 2);
        let items = m.draw_items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].source, 0);
        assert_eq!(items[1].matrix[0][3], 5.0);
    }

    #[test]
    fn missing_instance_target_is_reported() {
        let m = parse_zwo(&file_with(vec![instance_bytes("nowhere", 0.0)])).unwrap();
        assert_eq!(m.meshes.len(), 1);
        assert!(!m.warnings.is_empty());
        assert!(m.draw_items().is_empty());
    }

    #[test]
    fn reads_the_optional_flags2_value() {
        // flags2 bit 0x10000 would be followed by an extra u32 in the header
        let m = parse_zwo(&file_with(vec![mesh_bytes(0x10000, Some(7), 1)])).unwrap();
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        assert_eq!(m.meshes.len(), 1);
        assert_eq!(m.meshes[0].buffers[0].positions.len(), 3);
        assert_eq!(m.meshes[0].triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn unknown_entity_is_reported_not_fatal() {
        let m = parse_zwo(&[0, 0, 0, 0x77, 0, 0, 0, 8, 0, 0, 0, 0]).unwrap();
        assert!(!m.warnings.is_empty());
    }
}
