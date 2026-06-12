//! Chunk streaming: keeps the server-fed world mirror meshed around the
//! camera.
//!
//! Terrain arrives from `oc-server` via column subscriptions (§1/§8); this
//! side holds a read-mirror `World` for physics/raycasts, meshes it with
//! rayon jobs (§4), uploads under a per-frame budget, and unloads what
//! falls behind. Block-edit remeshing stays synchronous so local
//! prediction never lags a frame.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;
use glam::{DVec3, IVec3};
use oc_core::coords::{block_in_section, block_to_chunk, block_to_section};
use oc_core::{BlockPos, ChunkPos, SECTION_SIZE, SectionPos};
use oc_protocol::ClientMessage;
use oc_renderer::{ChunkMesh, Renderer, mesh_section};
use oc_world::light::{LightField, compute_light};
use oc_world::terrain::BOTTOM_SECTION_Y;
use oc_world::world::GeneratedColumn;
use oc_world::{BlockId, Section, World};

/// Columns of meshed terrain kept around the camera (view distance).
const VIEW_RADIUS: i32 = 12;
/// Extra generated ring so view-edge chunks cull faces against real
/// neighbors instead of assumed air.
const GEN_MARGIN: i32 = 1;
/// Extra retained ring before unloading, so pacing back and forth across a
/// column border doesn't thrash generate/unload.
const UNLOAD_MARGIN: i32 = 2;
/// Maximum meshing jobs in flight at once.
const MAX_INFLIGHT: usize = 24;
/// Section meshes uploaded to the GPU per frame (bounds frame-time spikes).
const UPLOAD_BUDGET: usize = 32;

struct MeshJobResult {
    chunk: ChunkPos,
    meshes: Vec<(SectionPos, ChunkMesh)>,
}

pub struct ChunkStreamer {
    world: World,
    /// Section meshes currently uploaded to the renderer, per column.
    meshed: HashMap<ChunkPos, Vec<SectionPos>>,
    /// Columns we asked the server for.
    subscribed: HashSet<ChunkPos>,
    mesh_inflight: HashSet<ChunkPos>,
    mesh_tx: Sender<MeshJobResult>,
    mesh_rx: Receiver<MeshJobResult>,
    /// Mesh results that arrived but exceeded the frame's upload budget.
    upload_queue: Vec<MeshJobResult>,
}

impl ChunkStreamer {
    pub fn new(seed: u64) -> Self {
        let (mesh_tx, mesh_rx) = channel();
        Self {
            world: World::new(seed),
            meshed: HashMap::new(),
            subscribed: HashSet::new(),
            mesh_inflight: HashSet::new(),
            mesh_tx,
            mesh_rx,
            upload_queue: Vec::new(),
        }
    }

    /// Terrain arrived from the server.
    pub fn insert_column(&mut self, column: GeneratedColumn) {
        self.world.insert_column(column);
    }

    /// A remote block change (or the echo of a local one). Applies and
    /// remeshes only if the mirror doesn't already have the value.
    pub fn apply_block_change(
        &mut self,
        renderer: &mut Renderer,
        pos: BlockPos,
        block: BlockId,
    ) -> Result<()> {
        if self.world.block(pos) != block && self.world.set_block(pos, block) {
            self.remesh_after_edit(renderer, pos)?;
        }
        Ok(())
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Runs one frame of streaming work around the camera. Subscription
    /// changes for the server are appended to `outbox`.
    pub fn update(
        &mut self,
        renderer: &mut Renderer,
        camera_pos: DVec3,
        outbox: &mut Vec<ClientMessage>,
    ) -> Result<()> {
        let center = block_to_chunk(camera_pos.floor().as_ivec3());
        self.upload_meshes(renderer, center)?;
        self.unload_far(renderer, center, outbox);
        self.subscribe_near(center, outbox);
        self.dispatch_mesh_jobs(center);
        Ok(())
    }

    /// Uploads finished meshes, spreading work across frames.
    fn upload_meshes(&mut self, renderer: &mut Renderer, center: ChunkPos) -> Result<()> {
        while let Ok(result) = self.mesh_rx.try_recv() {
            self.mesh_inflight.remove(&result.chunk);
            self.upload_queue.push(result);
        }

        let mut budget = UPLOAD_BUDGET;
        while budget > 0
            && let Some(result) = self.upload_queue.pop()
        {
            if chebyshev(result.chunk, center) > VIEW_RADIUS + UNLOAD_MARGIN
                || !self.world.is_generated(result.chunk)
            {
                continue; // stale: out of range or column unloaded meanwhile
            }
            let mut sections = Vec::with_capacity(result.meshes.len());
            for (pos, mesh) in &result.meshes {
                renderer.set_chunk(*pos, mesh)?;
                sections.push(*pos);
                budget = budget.saturating_sub(1);
            }
            self.meshed.insert(result.chunk, sections);
        }
        Ok(())
    }

    fn unload_far(
        &mut self,
        renderer: &mut Renderer,
        center: ChunkPos,
        outbox: &mut Vec<ClientMessage>,
    ) {
        let limit = VIEW_RADIUS + GEN_MARGIN + UNLOAD_MARGIN;
        let far: Vec<ChunkPos> = self
            .world
            .loaded_columns()
            .filter(|&c| chebyshev(c, center) > limit)
            .collect();
        for chunk in far {
            // Drop GPU meshes by the column's actual sections, not just the
            // `meshed` bookkeeping — a column queued for re-mesh (removed
            // from `meshed`) still has meshes uploaded.
            self.meshed.remove(&chunk);
            for pos in self.world.column_sections(chunk) {
                renderer.remove_chunk(pos);
            }
            // The server owns persistence; the mirror just forgets.
            self.world.unload_column(chunk);
            if self.subscribed.remove(&chunk) {
                outbox.push(ClientMessage::UnsubscribeColumn(chunk));
            }
        }
    }

    /// Asks the server for every in-range column we don't have yet.
    fn subscribe_near(&mut self, center: ChunkPos, outbox: &mut Vec<ClientMessage>) {
        let mut wanted: Vec<ChunkPos> = ring(center, VIEW_RADIUS + GEN_MARGIN)
            .filter(|c| !self.subscribed.contains(c))
            .collect();
        wanted.sort_by_key(|&c| dist2(c, center));
        for chunk in wanted {
            self.subscribed.insert(chunk);
            outbox.push(ClientMessage::SubscribeColumn(chunk));
        }
    }

    fn dispatch_mesh_jobs(&mut self, center: ChunkPos) {
        let slots = MAX_INFLIGHT.saturating_sub(self.mesh_inflight.len());
        if slots == 0 {
            return;
        }
        let mut ready: Vec<ChunkPos> = ring(center, VIEW_RADIUS)
            .filter(|c| !self.meshed.contains_key(c) && !self.mesh_inflight.contains(c))
            // Mesh only once every neighbor exists, so border faces cull
            // against real blocks and never need a remesh.
            .filter(|&c| ring(c, 1).all(|n| self.world.is_generated(n)))
            .collect();
        ready.sort_by_key(|&c| dist2(c, center));

        for chunk in ready.into_iter().take(slots) {
            self.mesh_inflight.insert(chunk);
            let job = MeshJob::snapshot(&self.world, chunk);
            let tx = self.mesh_tx.clone();
            rayon::spawn(move || {
                let _ = tx.send(job.run());
            });
        }
    }

    /// Re-meshes after a block edit. The edited column's affected sections
    /// remesh synchronously (the edit must be visible this frame, with
    /// fresh lighting); surrounding columns re-mesh asynchronously, since
    /// light from the edit reaches up to 15 blocks into them.
    pub fn remesh_after_edit(&mut self, renderer: &mut Renderer, block: BlockPos) -> Result<()> {
        let section = block_to_section(block);
        let column = block_to_chunk(block);
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

        let field = self.light_for(column);
        for pos in affected {
            if ChunkPos::new(pos.x, pos.z) != column || !self.meshed.contains_key(&column) {
                continue; // neighbor-column sections go through the async path
            }
            let base = pos * SECTION_SIZE;
            let section = self.world.section(pos).cloned();
            let world = &self.world;
            let mesh = mesh_section(
                |local: IVec3| {
                    let inside = local.cmpge(IVec3::ZERO).all()
                        && local.cmplt(IVec3::splat(SECTION_SIZE)).all();
                    if inside {
                        section.as_ref().map_or(BlockId::AIR, |s| s.get(local))
                    } else {
                        world.block(base + local)
                    }
                },
                |local: IVec3| field.get(base + local),
            );
            renderer.set_chunk(pos, &mesh)?;
            let sections = self.meshed.get_mut(&column).expect("checked above");
            if !sections.contains(&pos) {
                // Placing into a previously all-air section.
                sections.push(pos);
            }
        }

        // Queue the 8 surrounding columns for re-mesh: their old meshes stay
        // visible until the replacements upload, so there's no flicker.
        for neighbor in ring(column, 1) {
            if neighbor != column {
                self.meshed.remove(&neighbor);
            }
        }
        Ok(())
    }

    /// Lighting for one column, computed fresh from current world data.
    /// The Y ceiling covers the whole 3×3 neighborhood so tall neighbors
    /// cast correct shadows.
    fn light_for(&self, column: ChunkPos) -> LightField {
        let max_y = ring(column, 1)
            .flat_map(|c| self.world.column_sections(c))
            .map(|s| (s.y + 1) * SECTION_SIZE)
            .max()
            .unwrap_or(SECTION_SIZE);
        compute_light(
            |pos| self.world.block(pos),
            column,
            BOTTOM_SECTION_Y * SECTION_SIZE,
            max_y,
        )
    }
}

/// Everything a worker needs to mesh one column: `Arc` snapshots of the
/// column's sections and its neighborhood, for border-face culling and the
/// 15-block light range (which the 3×3 region covers exactly).
struct MeshJob {
    chunk: ChunkPos,
    targets: Vec<SectionPos>,
    snapshot: HashMap<SectionPos, Arc<Section>>,
    /// Open sky above the tallest section in the neighborhood.
    max_y: i32,
}

impl MeshJob {
    fn snapshot(world: &World, chunk: ChunkPos) -> Self {
        let targets = world.column_sections(chunk);
        let mut snapshot = HashMap::new();
        let mut max_y = SECTION_SIZE;
        for column in ring(chunk, 1) {
            for pos in world.column_sections(column) {
                if let Some(section) = world.section(pos) {
                    max_y = max_y.max((pos.y + 1) * SECTION_SIZE);
                    snapshot.insert(pos, Arc::clone(section));
                }
            }
        }
        Self { chunk, targets, snapshot, max_y }
    }

    fn run(self) -> MeshJobResult {
        let block_at = |pos: BlockPos| -> BlockId {
            match self.snapshot.get(&block_to_section(pos)) {
                Some(section) => section.get(block_in_section(pos)),
                None => BlockId::AIR,
            }
        };
        let light = compute_light(
            block_at,
            self.chunk,
            BOTTOM_SECTION_Y * SECTION_SIZE,
            self.max_y,
        );
        let meshes = self
            .targets
            .iter()
            .map(|&pos| {
                let base = pos * SECTION_SIZE;
                let section = &self.snapshot[&pos];
                let mesh = mesh_section(
                    |local: IVec3| {
                        let inside = local.cmpge(IVec3::ZERO).all()
                            && local.cmplt(IVec3::splat(SECTION_SIZE)).all();
                        if inside {
                            // Hot path: direct section read, no hash lookup.
                            section.get(local)
                        } else {
                            block_at(base + local)
                        }
                    },
                    |local: IVec3| light.get(base + local),
                );
                (pos, mesh)
            })
            .collect();
        MeshJobResult { chunk: self.chunk, meshes }
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
