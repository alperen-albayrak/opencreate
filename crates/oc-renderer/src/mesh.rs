//! Chunk section meshing (ARCHITECTURE.md §4).
//!
//! Milestone 1: simple culled mesher — one quad per visible voxel face.
//! Binary greedy meshing replaces this without changing the vertex format.

use glam::IVec3;
use oc_core::SECTION_SIZE;
use oc_world::{BlockId, blocks};

/// One packed vertex, 8 bytes (decoded in `chunk.wgsl`):
///   word 0: x:5 | y:5 | z:5 | face:3 | corner:2   (corner positions, 0..=16)
///   word 1: texture layer:16 | light:8
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PackedVertex(pub u32, pub u32);

pub struct ChunkMesh {
    pub vertices: Vec<PackedVertex>,
    pub indices: Vec<u32>,
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
        _ => layers::STONE,
    }
}

/// Meshes one section. `sample` takes section-local coordinates and is also
/// called one block outside the section (components -1 or 16), so callers
/// provide neighbor-section blocks for cross-section face culling. Ungenerated
/// neighbors should sample as air.
///
/// `light` returns the packed light (`sky << 4 | block`, 0..=15 each) of the
/// transparent voxel a face is emitted into; same coordinate convention.
pub fn mesh_section(
    sample: impl Fn(IVec3) -> BlockId,
    light: impl Fn(IVec3) -> u8,
) -> ChunkMesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for y in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let pos = IVec3::new(x, y, z);
                let block = sample(pos);
                if block.is_air() {
                    continue;
                }
                for (face, normal) in FACE_NORMALS.iter().enumerate() {
                    let neighbor = sample(pos + *normal);
                    // Opaque neighbors hide the face; water also hides its
                    // own kind (no internal faces inside a water volume).
                    if neighbor.is_opaque() || (!block.is_opaque() && neighbor == block) {
                        continue;
                    }

                    let layer = face_texture(block, face);
                    // Faces are lit by the transparent voxel they face into.
                    let face_light = light(pos + *normal) as u32;
                    let base = vertices.len() as u32;
                    for (corner, offset) in FACE_CORNERS[face].iter().enumerate() {
                        let p = pos + *offset;
                        let w0 = (p.x as u32)
                            | (p.y as u32) << 5
                            | (p.z as u32) << 10
                            | (face as u32) << 15
                            | (corner as u32) << 18;
                        let w1 = layer | face_light << 16;
                        vertices.push(PackedVertex(w0, w1));
                    }
                    indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
                    if !block.is_opaque() {
                        // Water surfaces are visible from both sides (e.g.
                        // looking up at the surface from underwater).
                        indices.extend_from_slice(&[
                            base, base + 2, base + 1, base + 2, base + 3, base + 1,
                        ]);
                    }
                }
            }
        }
    }

    ChunkMesh { vertices, indices }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_world::Section;

    /// Sampler over one section with all-air surroundings.
    fn isolated(section: &Section) -> impl Fn(IVec3) -> BlockId {
        move |pos| {
            if pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(SECTION_SIZE)).all() {
                section.get(pos)
            } else {
                BlockId::AIR
            }
        }
    }

    #[test]
    fn single_block_has_six_faces() {
        let mut section = Section::empty();
        section.set(IVec3::new(8, 8, 8), blocks::STONE);
        let mesh = mesh_section(isolated(&section), |_| 0xF0);
        assert_eq!(mesh.vertices.len(), 6 * 4);
        assert_eq!(mesh.indices.len(), 6 * 6);
    }

    #[test]
    fn buried_faces_are_culled() {
        let mut section = Section::empty();
        // 3³ solid cube: 27 blocks, only the outer shell's 54 faces visible.
        for x in 7..10 {
            for y in 7..10 {
                for z in 7..10 {
                    section.set(IVec3::new(x, y, z), blocks::STONE);
                }
            }
        }
        let mesh = mesh_section(isolated(&section), |_| 0xF0);
        assert_eq!(mesh.indices.len() / 6, 54);
    }

    #[test]
    fn faces_against_solid_neighbor_sections_are_culled() {
        // Section fully solid, surrounded by solid neighbors on all sides:
        // nothing is visible at all.
        let mesh = mesh_section(|_| blocks::STONE, |_| 0xF0);
        assert_eq!(mesh.indices.len(), 0);

        // Solid section with solid blocks below only (-Y neighbor): the
        // bottom face of the floor layer must be culled, 5 sides + nothing
        // below -> 16*16*5 visible faces.
        let mesh = mesh_section(
            |pos: IVec3| {
                let inside =
                    pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(SECTION_SIZE)).all();
                if inside || pos.y < 0 { blocks::STONE } else { BlockId::AIR }
            },
            |_| 0xF0,
        );
        assert_eq!(mesh.indices.len() / 6, 16 * 16 * 5);
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;
    use glam::IVec3;

    fn top_face_layer(block: BlockId) -> u32 {
        let mesh = mesh_section(
            |pos: IVec3| if pos == IVec3::new(8, 8, 8) { block } else { BlockId::AIR },
            |_| 0xF0,
        );
        // Find the +Y face (face bits 15..18 == 0) and return its layer.
        mesh.vertices
            .iter()
            .find(|v| (v.0 >> 15) & 7 == 0)
            .map(|v| v.1 & 0xFFFF)
            .expect("top face present")
    }

    #[test]
    fn face_layers_match_blocks() {
        assert_eq!(top_face_layer(blocks::GRASS), layers::GRASS_TOP);
        assert_eq!(top_face_layer(blocks::DIRT), layers::DIRT);
        assert_eq!(top_face_layer(blocks::STONE), layers::STONE);
        assert_eq!(top_face_layer(blocks::SAND), layers::SAND);
        assert_eq!(top_face_layer(blocks::WATER), layers::WATER);
        assert_eq!(top_face_layer(blocks::LOG), layers::LOG_TOP);
        assert_eq!(top_face_layer(blocks::LEAVES), layers::LEAVES);
        assert_eq!(top_face_layer(blocks::LAMP), layers::LAMP);
    }
}

#[cfg(test)]
mod light_bake_tests {
    use super::*;
    use glam::IVec3;
    use oc_core::ChunkPos;
    use oc_world::light::compute_light;

    /// Floor at y=0 with a roof slab at y=8 over x 4..=11, z 4..=11 of the
    /// center column. The floor's +Y faces under the roof must bake dimmer
    /// sky light than the open floor.
    #[test]
    fn roof_shadow_reaches_the_baked_vertices() {
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

        // Mesh the section containing the floor (world section y=0 == local).
        let mesh = mesh_section(blocks_at, |local| field.get(local));

        // Collect +Y faces of floor blocks (face bits == 0, vertex y == 1).
        let sky_of = |x: i32, z: i32| -> u8 {
            mesh.vertices
                .iter()
                .find(|v| {
                    let w0 = v.0;
                    (w0 >> 15) & 7 == 0
                        && (w0 & 31) == x as u32          // corner 0 has the block's min x
                        && ((w0 >> 5) & 31) == 1
                        && ((w0 >> 10) & 31) == (z + 1) as u32 // corner 0 of +Y is at z+1
                })
                .map(|v| ((v.1 >> 16) >> 4) as u8 & 15)
                .expect("floor face present")
        };

        assert_eq!(sky_of(1, 1), 15, "open floor is fully sky-lit");
        let shaded = sky_of(7, 7);
        assert!(shaded < 13, "floor under the roof should be shaded, got {shaded}");
    }
}
