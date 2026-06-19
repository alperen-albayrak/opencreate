//! Chunk section meshing (ARCHITECTURE.md §4): greedy quad merging.
//!
//! Per face direction and slice, visible faces with identical texture and
//! light merge into maximal rectangles. The merged extent rides in the
//! vertex so the shader tiles the texture (REPEAT sampler).

use glam::IVec3;
use oc_core::SECTION_SIZE;
use oc_world::{BlockId, blocks};

/// One packed vertex, 12 bytes (decoded in `chunk.wgsl`):
///   word 0: x:5 | y:5 | z:5 | face:3 | corner:2 | (su-1):4 | (sv-1):4 | ao:2
///     (corner positions 0..=16; su/sv = quad extent along the UV axes;
///      ao = 0 darkest .. 3 open, per vertex)
///   word 1: texture layer:16 | (reserved):8 | underwater:1 | surface_top:1 |
///           underwater_surface:1
///   word 2: light:16 (sky:4 << 12 | r:4 << 8 | g:4 << 4 | b:4) | heat:16
///     (per-vertex baked sky visibility + RGB block light; high 16 = the tier-2
///      source-heat delta °C above the base, 0..HEAT_DELTA_MAX → 0..65535, which
///      the geometry shader adds to the static depth temperature for the glow)
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PackedVertex(pub u32, pub u32, pub u32);

impl Default for ChunkMesh {
    fn default() -> Self {
        Self { vertices: Vec::new(), indices: Vec::new() }
    }
}

impl ChunkMesh {
    pub fn is_empty_mesh(&self) -> bool {
        self.indices.is_empty()
    }
}

pub struct ChunkMesh {
    pub vertices: Vec<PackedVertex>,
    pub indices: Vec<u32>,
}

/// One section's geometry, split by pipeline: solid faces draw opaque,
/// water draws later in the blended water pass (stage B).
#[derive(Default)]
pub struct SectionMeshes {
    pub solid: ChunkMesh,
    pub water: ChunkMesh,
}

impl SectionMeshes {
    pub fn is_empty(&self) -> bool {
        self.solid.indices.is_empty() && self.water.indices.is_empty()
    }
}

/// Face order matches `FACE_SHADE` in the shader: +Y, -Y, +Z, -Z, +X, -X.
const FACE_NORMALS: [IVec3; 6] = [
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
];

/// Quad corner offsets per face, in corner order 0..4 (UVs: 0=(0,1) 1=(1,1)
/// 2=(0,0) 3=(1,0), i.e. corners 2,3 are the texture's top edge).
const FACE_CORNERS: [[IVec3; 4]; 6] = [
    // +Y (top, seen from above): top edge of texture faces -Z
    [
        IVec3::new(0, 1, 1),
        IVec3::new(1, 1, 1),
        IVec3::new(0, 1, 0),
        IVec3::new(1, 1, 0),
    ],
    // -Y (bottom)
    [
        IVec3::new(0, 0, 0),
        IVec3::new(1, 0, 0),
        IVec3::new(0, 0, 1),
        IVec3::new(1, 0, 1),
    ],
    // +Z (south)
    [
        IVec3::new(0, 0, 1),
        IVec3::new(1, 0, 1),
        IVec3::new(0, 1, 1),
        IVec3::new(1, 1, 1),
    ],
    // -Z (north)
    [
        IVec3::new(1, 0, 0),
        IVec3::new(0, 0, 0),
        IVec3::new(1, 1, 0),
        IVec3::new(0, 1, 0),
    ],
    // +X (east)
    [
        IVec3::new(1, 0, 1),
        IVec3::new(1, 0, 0),
        IVec3::new(1, 1, 1),
        IVec3::new(1, 1, 0),
    ],
    // -X (west)
    [
        IVec3::new(0, 0, 0),
        IVec3::new(0, 0, 1),
        IVec3::new(0, 1, 0),
        IVec3::new(0, 1, 1),
    ],
];

/// Texture array layers (must match the order in `texture::build_block_textures`
/// and the `textures:` indices in `data/blocks.ron`). Now that layers are data,
/// these constants document the array order and back the meshing test + the
/// unknown-block fallback.
#[allow(dead_code)]
mod layers {
    pub const GRASS_TOP: u32 = 0;
    pub const DIRT: u32 = 1;
    pub const STONE: u32 = 2;
    pub const GRASS_SIDE: u32 = 3;
    pub const SAND: u32 = 4;
    pub const WATER: u32 = 5;
    pub const LOG_SIDE: u32 = 6;
    pub const LOG_TOP: u32 = 7;
    pub const LEAVES: u32 = 8;
    pub const LAMP: u32 = 9;
    pub const SNOW: u32 = 10;
    pub const PLANKS: u32 = 11;
}

fn face_texture(block: BlockId, face: usize) -> u32 {
    // Per-face texture layers are data now (`BlockDef.textures`); the layer
    // indices in `blocks.ron` match the `layers` constants below. Unknown
    // blocks fall back to stone.
    oc_world::registry::def(block).map_or(layers::STONE, |d| d.textures.layer(face))
}

/// Per face: (uv.x axis, uv.y axis) in block space — the axes whose corner
/// offsets vary with `corner_uv.x` / `corner_uv.y` in `FACE_CORNERS`.
/// Merged quads scale corner offsets and UVs along these axes together, so
/// texture orientation matches the unmerged case exactly.
const UV_AXES: [(usize, usize); 6] = [(0, 2), (0, 2), (0, 1), (0, 1), (2, 1), (2, 1)];

/// Mask cell: faces merge only when all of this matches.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FaceKey {
    layer: u32,
    light: u16,
    /// Tier-2 source-heat delta (°C above the geothermal base) at the voxel the
    /// face looks into, quantized `0..HEAT_DELTA_MAX → 0..=65535`. Part of the
    /// merge key so quads split at heat gradients (like `light`); 0 = no source.
    heat: u16,
    /// Non-opaque (water) faces are emitted double-sided.
    opaque: bool,
    /// Solid face submerged in water: the chunk shader plays caustics
    /// (sun dapples) over it.
    underwater: bool,
    /// Water face whose top edge is the open surface (no water above):
    /// the water shader drops those vertices to 14/16 block height.
    surface_top: bool,
    /// Underwater side face whose adjacent water is the open surface:
    /// caustics stop at the 14/16 waterline instead of covering the
    /// sliver of face that pokes above the water.
    underwater_surface: bool,
    /// Per-corner ambient occlusion (0 darkest .. 3 open), indexed by
    /// `(offv << 1) | offu` in the face's UV plane. Cells merge only on
    /// identical AO, and only along an axis the AO is constant over, so
    /// merged interpolation matches the per-cell result exactly.
    ao: [u8; 4],
}

/// Ambient occlusion for one corner of a face: counts opaque blocks
/// around the corner in the layer the face looks into (the classic
/// side1/side2/corner rule — a fully walled corner is darkest even if
/// the diagonal is open).
fn corner_ao(
    sample: &impl Fn(IVec3) -> BlockId,
    front: IVec3,
    du: IVec3,
    dv: IVec3,
) -> u8 {
    let side1 = sample(front + du).is_opaque();
    let side2 = sample(front + dv).is_opaque();
    if side1 && side2 {
        return 0;
    }
    let corner = sample(front + du + dv).is_opaque();
    3 - (side1 as u8 + side2 as u8 + corner as u8)
}

/// True when a face of `block` against `neighbor` is visible.
fn face_visible(block: BlockId, neighbor: BlockId) -> bool {
    // A neighbour hides the face only if it's an opaque *solid* — fluids
    // (water, lava) render but don't occlude, so a solid block keeps its face
    // at a lava/water boundary (no holes). A fluid/transparent block still
    // hides its own kind (no internal faces inside a volume of it).
    !(occludes(neighbor) || (!occludes(block) && neighbor == block))
}

/// A block occludes its neighbours' faces: opaque and not a fluid.
fn occludes(block: BlockId) -> bool {
    block.is_opaque() && !block.is_fluid()
}

/// Full-scale of the quantized tier-2 source-heat delta baked into vertex word 2
/// (°C above the geothermal base). Past this the deep is already glowing
/// brilliant-white, so finer range buys nothing. **Must match `HEAT_DELTA_MAX`
/// in `chunk_gbuffer.wgsl`.**
pub const HEAT_DELTA_MAX: f32 = 1500.0;

/// Quantize a source-heat delta (°C) into the u16 baked into word 2's high half.
pub fn quantize_heat(delta: f32) -> u16 {
    (delta / HEAT_DELTA_MAX * 65535.0).clamp(0.0, 65535.0) as u16
}

/// Meshes one section with greedy quad merging. `sample` takes
/// section-local coordinates and is also called one block outside the
/// section (components -1 or 16), so callers provide neighbor-section
/// blocks for cross-section face culling; ungenerated neighbors should
/// sample as air.
///
/// `light` returns the packed light (`sky << 12 | r << 8 | g << 4 | b`, each
/// nibble 0..=15) of the transparent voxel a face is emitted into; same
/// coordinate convention. `heat` returns the quantized tier-2 source-heat delta
/// ([`quantize_heat`]) of that same voxel, baked alongside the light for the
/// blackbody glow.
pub fn mesh_section(
    sample: impl Fn(IVec3) -> BlockId,
    light: impl Fn(IVec3) -> u16,
    heat: impl Fn(IVec3) -> u16,
) -> SectionMeshes {
    let mut meshes = SectionMeshes::default();
    let n = SECTION_SIZE as usize;

    for (face, normal) in FACE_NORMALS.iter().enumerate() {
        let axis = (0..3).find(|&a| normal[a] != 0).unwrap();
        let (uax, vax) = UV_AXES[face];

        for d in 0..SECTION_SIZE {
            // Mask of visible faces in this slice, indexed [v][u].
            let mut mask = [[None::<FaceKey>; 16]; 16];
            for v in 0..SECTION_SIZE {
                for u in 0..SECTION_SIZE {
                    let mut pos = IVec3::ZERO;
                    pos[axis] = d;
                    pos[uax] = u;
                    pos[vax] = v;
                    let block = sample(pos);
                    if block.is_air() || !face_visible(block, sample(pos + *normal)) {
                        continue;
                    }
                    let block_ref = &block;
                    let neighbor = sample(pos + *normal);
                    let is_water = !block.is_opaque();
                    let underwater = block.is_opaque() && neighbor == blocks::WATER;
                    // AO only shades solid faces; water stays uniform.
                    let ao = if is_water {
                        [3; 4]
                    } else {
                        let front = pos + *normal;
                        let dir = |ax: usize, off: i32| {
                            let mut d = IVec3::ZERO;
                            d[ax] = if off == 0 { -1 } else { 1 };
                            d
                        };
                        let (un, up) = (dir(uax, 0), dir(uax, 1));
                        let (vn, vp) = (dir(vax, 0), dir(vax, 1));
                        [
                            corner_ao(&sample, front, un, vn),
                            corner_ao(&sample, front, up, vn),
                            corner_ao(&sample, front, un, vp),
                            corner_ao(&sample, front, up, vp),
                        ]
                    };
                    mask[v as usize][u as usize] = Some(FaceKey {
                        layer: face_texture(block, face),
                        // A face is *lit* by the voxel it faces into (light
                        // arrives from outside), but *glows* by its own block's
                        // temperature — incandescence comes from within, so a hot
                        // stone shell glows on its outward faces too.
                        light: light(pos + *normal),
                        heat: heat(pos),
                        opaque: block.is_opaque(),
                        underwater,
                        // Top faces are always the open surface (no
                        // internal water faces exist); side faces only
                        // when nothing watery sits above this block.
                        surface_top: is_water
                            && (face == 0
                                || (face >= 2 && sample(pos + IVec3::Y) != *block_ref)),
                        // Side faces against surface water (nothing watery
                        // above the neighbor): the wet part ends at 14/16.
                        underwater_surface: underwater
                            && face >= 2
                            && sample(pos + *normal + IVec3::Y) != blocks::WATER,
                        ao,
                    });
                }
            }

            // Greedy sweep: grow each unvisited cell right (u), then up (v).
            // A quad may only grow along an axis its AO is constant over;
            // otherwise the stretched corner interpolation would diverge
            // from the per-cell gradient.
            for v0 in 0..n {
                for u0 in 0..n {
                    let Some(key) = mask[v0][u0] else { continue };
                    let ao_const_u = key.ao[0] == key.ao[1] && key.ao[2] == key.ao[3];
                    let ao_const_v = key.ao[0] == key.ao[2] && key.ao[1] == key.ao[3];
                    let mut su = 1;
                    while ao_const_u && u0 + su < n && mask[v0][u0 + su] == Some(key) {
                        su += 1;
                    }
                    let mut sv = 1;
                    'grow: while ao_const_v && v0 + sv < n {
                        for u in u0..u0 + su {
                            if mask[v0 + sv][u] != Some(key) {
                                break 'grow;
                            }
                        }
                        sv += 1;
                    }
                    for row in &mut mask[v0..v0 + sv] {
                        for cell in &mut row[u0..u0 + su] {
                            *cell = None;
                        }
                    }

                    let target =
                        if key.opaque { &mut meshes.solid } else { &mut meshes.water };
                    emit_quad(
                        &mut target.vertices,
                        &mut target.indices,
                        face,
                        Quad {
                            axis,
                            uax,
                            vax,
                            d,
                            u0: u0 as i32,
                            v0: v0 as i32,
                            su: su as i32,
                            sv: sv as i32,
                        },
                        key,
                    );
                }
            }
        }
    }

    meshes
}

struct Quad {
    axis: usize,
    uax: usize,
    vax: usize,
    d: i32,
    u0: i32,
    v0: i32,
    su: i32,
    sv: i32,
}

fn emit_quad(
    vertices: &mut Vec<PackedVertex>,
    indices: &mut Vec<u32>,
    face: usize,
    q: Quad,
    key: FaceKey,
) {
    let base = vertices.len() as u32;
    // Per-corner AO, in corner-index order (the key stores it by UV).
    let mut ao = [3u8; 4];
    for (corner, offset) in FACE_CORNERS[face].iter().enumerate() {
        let (offu, offv) = (offset[q.uax] as usize, offset[q.vax] as usize);
        ao[corner] = key.ao[(offv << 1) | offu];
        // Scale the unit-cube corner offsets to the merged extent; the
        // face-axis component (0 or 1) is unchanged.
        let mut p = IVec3::ZERO;
        p[q.axis] = q.d + offset[q.axis];
        p[q.uax] = q.u0 + offset[q.uax] * q.su;
        p[q.vax] = q.v0 + offset[q.vax] * q.sv;
        let w0 = (p.x as u32)
            | (p.y as u32) << 5
            | (p.z as u32) << 10
            | (face as u32) << 15
            | (corner as u32) << 18
            | (q.su as u32 - 1) << 20
            | (q.sv as u32 - 1) << 24
            | (ao[corner] as u32) << 28;
        let w1 = key.layer
            | (key.underwater as u32) << 24
            | (key.surface_top as u32) << 25
            | (key.underwater_surface as u32) << 26;
        let w2 = key.light as u32 | (key.heat as u32) << 16;
        vertices.push(PackedVertex(w0, w1, w2));
    }
    // Corners 0/3 and 1/2 are the quad's diagonals (corner1 flips U,
    // corner2 flips V relative to corner0). Split along the brighter
    // diagonal so AO rounds smoothly instead of streaking (the classic
    // anisotropy fix).
    if ao[0] + ao[3] > ao[1] + ao[2] {
        indices.extend_from_slice(&[base, base + 1, base + 3, base, base + 3, base + 2]);
    } else {
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    }
    if !key.opaque {
        // Water surfaces are visible from both sides (e.g. looking up at
        // the surface from underwater).
        indices.extend_from_slice(&[base, base + 2, base + 1, base + 2, base + 3, base + 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Reconstructs per-cell faces from the greedy quads: maps
    /// (face, block cell) -> (layer, light). Every 4 vertices = one quad.
    fn coverage(meshes: &SectionMeshes) -> HashMap<(usize, IVec3), (u32, u16)> {
        let mut map = HashMap::new();
        for quad in meshes
            .solid
            .vertices
            .chunks(4)
            .chain(meshes.water.vertices.chunks(4))
        {
            let w0 = quad[0].0;
            let face = ((w0 >> 15) & 7) as usize;
            let su = ((w0 >> 20) & 15) as i32 + 1;
            let sv = ((w0 >> 24) & 15) as i32 + 1;
            let layer = quad[0].1 & 0xFFFF;
            let light = (quad[0].2 & 0xFFFF) as u16;

            // Min corner of the quad across all four vertices.
            let mut min = IVec3::splat(i32::MAX);
            for v in quad {
                let p = IVec3::new((v.0 & 31) as i32, ((v.0 >> 5) & 31) as i32, ((v.0 >> 10) & 31) as i32);
                min = min.min(p);
            }
            let axis = (0..3).find(|&a| FACE_NORMALS[face][a] != 0).unwrap();
            let (uax, vax) = UV_AXES[face];
            // Positive faces sit on the +1 plane of their block layer.
            let d = if FACE_NORMALS[face][axis] > 0 { min[axis] - 1 } else { min[axis] };
            for dv in 0..sv {
                for du in 0..su {
                    let mut cell = IVec3::ZERO;
                    cell[axis] = d;
                    cell[uax] = min[uax] + du;
                    cell[vax] = min[vax] + dv;
                    let prev = map.insert((face, cell), (layer, light));
                    assert!(prev.is_none(), "quads overlap at {face} {cell}");
                }
            }
        }
        map
    }

    /// Reference: per-cell visibility straight from the samplers.
    fn reference(
        sample: impl Fn(IVec3) -> BlockId,
        light: impl Fn(IVec3) -> u16,
    ) -> HashMap<(usize, IVec3), (u32, u16)> {
        let mut map = HashMap::new();
        for y in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let pos = IVec3::new(x, y, z);
                    let block = sample(pos);
                    if block.is_air() {
                        continue;
                    }
                    for (face, normal) in FACE_NORMALS.iter().enumerate() {
                        if face_visible(block, sample(pos + *normal)) {
                            map.insert(
                                (face, pos),
                                (face_texture(block, face), light(pos + *normal)),
                            );
                        }
                    }
                }
            }
        }
        map
    }

    fn hash(pos: IVec3, salt: u32) -> u32 {
        let mut h = (pos.x as u32).wrapping_mul(374761393)
            ^ (pos.y as u32).wrapping_mul(668265263)
            ^ (pos.z as u32).wrapping_mul(2246822519)
            ^ salt.wrapping_mul(40503);
        h = (h ^ (h >> 13)).wrapping_mul(1274126177);
        h ^ (h >> 16)
    }

    #[test]
    fn single_block_emits_six_quads() {
        let sample = |pos: IVec3| {
            if pos == IVec3::new(8, 8, 8) { blocks::STONE } else { BlockId::AIR }
        };
        let mesh = mesh_section(sample, |_| 0xF0, |_| 0);
        assert_eq!(mesh.solid.vertices.len(), 6 * 4);
        assert_eq!(mesh.solid.indices.len(), 6 * 6);
        assert!(mesh.water.is_empty_mesh(), "stone is not water");
    }

    #[test]
    fn uniform_cube_merges_each_side_into_one_quad() {
        let solid = |pos: IVec3| {
            let inside = pos.cmpge(IVec3::splat(4)).all() && pos.cmplt(IVec3::splat(12)).all();
            if inside { blocks::STONE } else { BlockId::AIR }
        };
        let mesh = mesh_section(solid, |_| 0xF0, |_| 0);
        // 8x8x8 cube with uniform light: exactly one quad per side.
        assert_eq!(mesh.solid.vertices.len() / 4, 6, "cube sides should merge fully");
        // Coverage still matches the per-cell reference.
        assert_eq!(coverage(&mesh), reference(solid, |_| 0xF0));
    }

    #[test]
    fn greedy_mesh_covers_exactly_the_reference_faces() {
        // Pseudo-random terrain with air, three solids and water, plus a
        // varying light field: merging must never change what is covered.
        for salt in 0..4 {
            let sample = move |pos: IVec3| {
                let inside =
                    pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(SECTION_SIZE)).all();
                if !inside {
                    return BlockId::AIR; // isolated section
                }
                match hash(pos, salt) % 10 {
                    0 | 1 => blocks::STONE,
                    2 => blocks::DIRT,
                    3 => blocks::GRASS,
                    4 => blocks::WATER,
                    _ => BlockId::AIR,
                }
            };
            let light = move |pos: IVec3| (hash(pos, salt ^ 99) % 256) as u16;
            let mesh = mesh_section(sample, light, |_| 0);
            assert_eq!(
                coverage(&mesh),
                reference(sample, light),
                "coverage mismatch for salt {salt}"
            );
        }
    }

    #[test]
    fn flat_floor_top_is_one_quad() {
        let floor = |pos: IVec3| {
            let inside =
                pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(SECTION_SIZE)).all();
            if inside && pos.y == 0 { blocks::STONE } else { BlockId::AIR }
        };
        let mesh = mesh_section(floor, |_| 0xF0, |_| 0);
        let tops = mesh
            .solid
            .vertices
            .chunks(4)
            .filter(|q| (q[0].0 >> 15) & 7 == 0)
            .count();
        assert_eq!(tops, 1, "uniform 16x16 floor top should be a single quad");
    }

    #[test]
    fn face_layers_match_blocks() {
        let layer_of = |block: BlockId| -> u32 {
            let sample =
                move |pos: IVec3| if pos == IVec3::new(8, 8, 8) { block } else { BlockId::AIR };
            let mesh = mesh_section(sample, |_| 0xF0, |_| 0);
            coverage(&mesh)[&(0, IVec3::new(8, 8, 8))].0
        };
        assert_eq!(layer_of(blocks::GRASS), layers::GRASS_TOP);
        assert_eq!(layer_of(blocks::DIRT), layers::DIRT);
        assert_eq!(layer_of(blocks::STONE), layers::STONE);
        assert_eq!(layer_of(blocks::SAND), layers::SAND);
        assert_eq!(layer_of(blocks::WATER), layers::WATER);
        assert_eq!(layer_of(blocks::LOG), layers::LOG_TOP);
        assert_eq!(layer_of(blocks::LEAVES), layers::LEAVES);
        assert_eq!(layer_of(blocks::LAMP), layers::LAMP);
        assert_eq!(layer_of(blocks::SNOW), layers::SNOW);
        assert_eq!(layer_of(blocks::PLANKS), layers::PLANKS);
    }

    /// Decodes every top-face (face 0) vertex as (position, ao).
    fn top_face_ao(mesh: &SectionMeshes) -> Vec<(IVec3, u32)> {
        mesh.solid
            .vertices
            .iter()
            .filter(|v| (v.0 >> 15) & 7 == 0)
            .map(|v| {
                let p = IVec3::new(
                    (v.0 & 31) as i32,
                    ((v.0 >> 5) & 31) as i32,
                    ((v.0 >> 10) & 31) as i32,
                );
                (p, (v.0 >> 28) & 3)
            })
            .collect()
    }

    #[test]
    fn open_floor_has_fully_open_ao() {
        let floor = |pos: IVec3| {
            let inside =
                pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(SECTION_SIZE)).all();
            if inside && pos.y == 0 { blocks::STONE } else { BlockId::AIR }
        };
        let mesh = mesh_section(floor, |_| 0xF0, |_| 0);
        assert!(
            top_face_ao(&mesh).iter().all(|&(_, ao)| ao == 3),
            "nothing occludes an open floor"
        );
    }

    #[test]
    fn block_on_floor_darkens_adjacent_corners() {
        let world = |pos: IVec3| {
            let inside =
                pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(SECTION_SIZE)).all();
            if inside && (pos.y == 0 || pos == IVec3::new(8, 1, 8)) {
                blocks::STONE
            } else {
                BlockId::AIR
            }
        };
        let mesh = mesh_section(world, |_| 0xF0, |_| 0);
        let tops = top_face_ao(&mesh);
        // Floor vertices at the block's feet (y=1 plane, touching x/z 8..9)
        // are occluded; floor far away stays open.
        let near = |p: IVec3| p.y == 1 && (8..=9).contains(&p.x) && (8..=9).contains(&p.z);
        assert!(
            tops.iter().any(|&(p, ao)| near(p) && ao < 3),
            "floor corners against the block must darken: {tops:?}"
        );
        assert!(
            tops.iter().all(|&(p, ao)| near(p) || p.y != 1 || ao == 3),
            "open floor must stay undarkened"
        );
        // Coverage is unchanged by AO-driven merge splits.
        assert_eq!(coverage(&mesh), reference(world, |_| 0xF0));
    }

    #[test]
    fn caustics_stop_at_the_surface_waterline() {
        // A stone column at x=8 with water beside it: one surface water
        // block at y=8, deep water below it at y=7.
        let world = |pos: IVec3| {
            if pos.x == 8 && (0..=8).contains(&pos.y) && pos.z == 8 {
                blocks::STONE
            } else if pos.x == 9 && (7..=8).contains(&pos.y) && pos.z == 8 {
                blocks::WATER
            } else {
                BlockId::AIR
            }
        };
        let mesh = mesh_section(world, |_| 0xF0, |_| 0);
        // +X faces of the stone column at y=8 (against surface water) and
        // y=7 (water above it): only the surface one is waterline-cut.
        let flags_at = |y: i32| -> u32 {
            mesh.solid
                .vertices
                .chunks(4)
                .find(|q| {
                    let w0 = q[0].0;
                    let face = (w0 >> 15) & 7;
                    let min_y = q.iter().map(|v| (v.0 >> 5) & 31).min().unwrap();
                    face == 4 && min_y == y as u32
                })
                .map(|q| q[0].1 >> 24)
                .expect("side face exists")
        };
        let surface = flags_at(8);
        let deep = flags_at(7);
        assert_eq!(surface & 1, 1, "surface-level face is underwater");
        assert_eq!(surface >> 2 & 1, 1, "surface-level face is waterline-cut");
        assert_eq!(deep & 1, 1, "deep face is underwater");
        assert_eq!(deep >> 2 & 1, 0, "deep face must not cut");
    }

    /// Floor at y=0 with a roof slab at y=8: the floor's +Y faces under the
    /// roof must bake dimmer sky light than the open floor — and quads must
    /// not merge across the light boundary.
    #[test]
    fn roof_shadow_reaches_the_baked_vertices() {
        use oc_core::ChunkPos;
        use oc_world::light::compute_light;

        let blocks_at = |pos: IVec3| -> BlockId {
            if pos.y <= 0 {
                blocks::STONE
            } else if pos.y == 8 && (4..=11).contains(&pos.x) && (4..=11).contains(&pos.z) {
                blocks::STONE
            } else {
                BlockId::AIR
            }
        };
        let field = compute_light(blocks_at, ChunkPos::new(0, 0), -16, 32, true);
        let mesh = mesh_section(blocks_at, |local| field.get(local), |_| 0);
        let cov = coverage(&mesh);

        let sky_of = |x: i32, z: i32| cov[&(0, IVec3::new(x, 0, z))].1 >> 12;
        assert_eq!(sky_of(1, 1), 15, "open floor is fully sky-lit");
        let shaded = sky_of(7, 7);
        assert!(shaded < 13, "floor under the roof should be shaded, got {shaded}");
    }
}
