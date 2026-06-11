//! Chunk streaming: keeps terrain generated and meshed around the camera.
//!
//! Milestone-2 simplification: generation and meshing run on the main thread
//! under small per-frame budgets, nearest column first. The §4 async pipeline
//! (rayon stages + upload budget) replaces the budgets without changing the
//! renderer API.

use std::collections::HashMap;

use anyhow::Result;
use glam::{DVec3, IVec3};
use oc_core::coords::{block_in_section, block_to_chunk, block_to_section};
use oc_core::{BlockPos, ChunkPos, SECTION_SIZE, SectionPos};
use oc_renderer::{Renderer, mesh_section};
use oc_world::{BlockId, World};

/// Columns of meshed terrain kept around the camera (view distance).
const VIEW_RADIUS: i32 = 8;
/// Extra generated ring so view-edge chunks cull faces against real
/// neighbors instead of assumed air.
const GEN_MARGIN: i32 = 1;
/// Extra retained ring before unloading, so pacing back and forth across a
/// column border doesn't thrash generate/unload.
const UNLOAD_MARGIN: i32 = 2;
/// Per-frame work budgets, in columns.
const GEN_BUDGET: usize = 6;
const MESH_BUDGET: usize = 3;

pub struct ChunkStreamer {
    world: World,
    /// Section meshes currently uploaded to the renderer, per column.
    meshed: HashMap<ChunkPos, Vec<SectionPos>>,
}

impl ChunkStreamer {
    pub fn new(seed: u64) -> Self {
        Self {
            world: World::new(seed),
            meshed: HashMap::new(),
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Re-meshes the section containing an edited block, plus any neighbor
    /// sections the edit borders on (their face culling may have changed).
    pub fn remesh_after_edit(&mut self, renderer: &mut Renderer, block: BlockPos) -> Result<()> {
        let section = block_to_section(block);
        let local = block_in_section(block);
        let mut affected = vec![section];
        for axis in 0..3 {
            let mut neighbor = section;
            if local[axis] == 0 {
                neighbor[axis] -= 1;
            } else if local[axis] == SECTION_SIZE - 1 {
                neighbor[axis] += 1;
            } else {
                continue;
            }
            affected.push(neighbor);
        }

        for pos in affected {
            let column = ChunkPos::new(pos.x, pos.z);
            // Only columns that are already meshed need an update; everything
            // else gets meshed (or skipped) by the streaming pass.
            if !self.meshed.contains_key(&column) {
                continue;
            }
            let mesh = self.mesh_one(pos);
            renderer.set_chunk(pos, &mesh)?;
            let sections = self.meshed.get_mut(&column).expect("checked above");
            if !sections.contains(&pos) {
                // Placing into a previously all-air section.
                sections.push(pos);
            }
        }
        Ok(())
    }

    /// Runs one frame of streaming work around the camera.
    pub fn update(&mut self, renderer: &mut Renderer, camera_pos: DVec3) -> Result<()> {
        let center = block_to_chunk(camera_pos.floor().as_ivec3());
        self.unload_far(renderer, center);
        self.generate_near(center);
        self.mesh_near(renderer, center)
    }

    fn unload_far(&mut self, renderer: &mut Renderer, center: ChunkPos) {
        let limit = VIEW_RADIUS + GEN_MARGIN + UNLOAD_MARGIN;
        let far: Vec<ChunkPos> = self
            .world
            .loaded_columns()
            .filter(|&c| chebyshev(c, center) > limit)
            .collect();
        for chunk in far {
            if let Some(sections) = self.meshed.remove(&chunk) {
                for pos in sections {
                    renderer.remove_chunk(pos);
                }
            }
            self.world.unload_column(chunk);
        }
    }

    fn generate_near(&mut self, center: ChunkPos) {
        let mut wanted: Vec<ChunkPos> = ring(center, VIEW_RADIUS + GEN_MARGIN)
            .filter(|&c| !self.world.is_generated(c))
            .collect();
        wanted.sort_by_key(|&c| dist2(c, center));
        for chunk in wanted.into_iter().take(GEN_BUDGET) {
            self.world.generate_column(chunk);
        }
    }

    fn mesh_near(&mut self, renderer: &mut Renderer, center: ChunkPos) -> Result<()> {
        let mut ready: Vec<ChunkPos> = ring(center, VIEW_RADIUS)
            .filter(|c| !self.meshed.contains_key(c))
            // Mesh only once every neighbor exists, so border faces cull
            // against real blocks and never need a remesh.
            .filter(|&c| {
                ring(c, 1).all(|n| self.world.is_generated(n))
            })
            .collect();
        ready.sort_by_key(|&c| dist2(c, center));

        for chunk in ready.into_iter().take(MESH_BUDGET) {
            let sections = self.world.column_sections(chunk);
            for &pos in &sections {
                renderer.set_chunk(pos, &self.mesh_one(pos))?;
            }
            self.meshed.insert(chunk, sections);
        }
        Ok(())
    }

    fn mesh_one(&self, pos: SectionPos) -> oc_renderer::ChunkMesh {
        let base = pos * SECTION_SIZE;
        let section = self.world.section(pos);
        mesh_section(|local: IVec3| {
            let inside =
                local.cmpge(IVec3::ZERO).all() && local.cmplt(IVec3::splat(SECTION_SIZE)).all();
            if inside {
                // Hot path: direct section read, no world hash lookup.
                section.map_or(BlockId::AIR, |s| s.get(local))
            } else {
                self.world.block(base + local)
            }
        })
    }
}

/// All columns within Chebyshev distance `radius` of `center`.
fn ring(center: ChunkPos, radius: i32) -> impl Iterator<Item = ChunkPos> {
    (-radius..=radius).flat_map(move |dz| {
        (-radius..=radius).map(move |dx| ChunkPos::new(center.x + dx, center.z + dz))
    })
}

fn chebyshev(a: ChunkPos, b: ChunkPos) -> i32 {
    (a.x - b.x).abs().max((a.z - b.z).abs())
}

fn dist2(a: ChunkPos, b: ChunkPos) -> i64 {
    let (dx, dz) = ((a.x - b.x) as i64, (a.z - b.z) as i64);
    dx * dx + dz * dz
}
