//! Chunk section meshing (ARCHITECTURE.md §4): greedy quad merging.
//!
//! Per face direction and slice, visible faces with identical texture and
//! light merge into maximal rectangles. The merged extent rides in the
//! vertex so the shader tiles the texture (REPEAT sampler).

use glam::IVec3;
use oc_core::SECTION_SIZE;
use oc_world::{BlockId, blocks};

/// One packed vertex, 8 bytes (decoded in `chunk.wgsl`):
///   word 0: x:5 | y:5 | z:5 | face:3 | corner:2 | (su-1):4 | (sv-1):4
///     (corner positions 0..=16; su/sv = quad extent along the UV axes)
///   word 1: texture layer:16 | light:8
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PackedVertex(pub u32, pub u32);

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

/// Texture array layers (must match the order in `texture::build_block_textures`).
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
    match block {
        blocks::GRASS => match face {
            0 => layers::GRASS_TOP,
            1 => layers::DIRT,
            _ => layers::GRASS_SIDE,
        },
        blocks::DIRT => layers::DIRT,
        blocks::SAND => layers::SAND,
        blocks::WATER => layers::WATER,
        blocks::LOG => match face {
            0 | 1 => layers::LOG_TOP,
            _ => layers::LOG_SIDE,
        },
        blocks::LEAVES => layers::LEAVES,
        blocks::LAMP => layers::LAMP,
        blocks::SNOW => layers::SNOW,
        blocks::PLANKS => layers::PLANKS,
        _ => layers::STONE,
    }
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
    light: u8,
    /// Non-opaque (water) faces are emitted double-sided.
    opaque: bool,
}

/// True when a face of `block` against `neighbor` is visible.
fn face_visible(block: BlockId, neighbor: BlockId) -> bool {
    // Opaque neighbors hide the face; water also hides its own kind (no
    // internal faces inside a water volume).
    !(neighbor.is_opaque() || (!block.is_opaque() && neighbor == block))
}

/// Meshes one section with greedy quad merging. `sample` takes
/// section-local coordinates and is also called one block outside the
/// section (components -1 or 16), so callers provide neighbor-section
/// blocks for cross-section face culling; ungenerated neighbors should
/// sample as air.
///
/// `light` returns the packed light (`sky << 4 | block`, 0..=15 each) of the
/// transparent voxel a face is emitted into; same coordinate convention.
pub fn mesh_section(
    sample: impl Fn(IVec3) -> BlockId,
    light: impl Fn(IVec3) -> u8,
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
                    mask[v as usize][u as usize] = Some(FaceKey {
                        layer: face_texture(block, face),
                        // Faces are lit by the voxel they face into.
                        light: light(pos + *normal),
                        opaque: block.is_opaque(),
                    });
                }
            }

            // Greedy sweep: grow each unvisited cell right (u), then up (v).
            for v0 in 0..n {
                for u0 in 0..n {
                    let Some(key) = mask[v0][u0] else { continue };
                    let mut su = 1;
                    while u0 + su < n && mask[v0][u0 + su] == Some(key) {
                        su += 1;
                    }
                    let mut sv = 1;
                    'grow: while v0 + sv < n {
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
    for (corner, offset) in FACE_CORNERS[face].iter().enumerate() {
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
            | (q.sv as u32 - 1) << 24;
        let w1 = key.layer | (key.light as u32) << 16;
        vertices.push(PackedVertex(w0, w1));
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
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
    fn coverage(meshes: &SectionMeshes) -> HashMap<(usize, IVec3), (u32, u8)> {
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
            let light = (quad[0].1 >> 16) as u8;

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
        light: impl Fn(IVec3) -> u8,
    ) -> HashMap<(usize, IVec3), (u32, u8)> {
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
        let mesh = mesh_section(sample, |_| 0xF0);
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
        let mesh = mesh_section(solid, |_| 0xF0);
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
            let light = move |pos: IVec3| (hash(pos, salt ^ 99) % 256) as u8;
            let mesh = mesh_section(sample, light);
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
        let mesh = mesh_section(floor, |_| 0xF0);
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
            let mesh = mesh_section(sample, |_| 0xF0);
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
        let field = compute_light(blocks_at, ChunkPos::new(0, 0), -16, 32);
        let mesh = mesh_section(blocks_at, |local| field.get(local));
        let cov = coverage(&mesh);

        let sky_of = |x: i32, z: i32| cov[&(0, IVec3::new(x, 0, z))].1 >> 4;
        assert_eq!(sky_of(1, 1), 15, "open floor is fully sky-lit");
        let shaded = sky_of(7, 7);
        assert!(shaded < 13, "floor under the roof should be shaded, got {shaded}");
    }
}
