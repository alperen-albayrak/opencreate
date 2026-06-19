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
use oc_renderer::{Renderer, SectionMeshes, mesh_section, quantize_heat};
use oc_world::heat::{HeatField, compute_heat};
use oc_world::light::{LightField, compute_light};
use oc_world::terrain::BOTTOM_SECTION_Y;
use oc_world::world::GeneratedColumn;
use oc_world::{BlockId, Section, World};

/// Default view radius (chunks); settings override per session.
const DEFAULT_VIEW_RADIUS: i32 = 12;
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
    meshes: Vec<(SectionPos, SectionMeshes)>,
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
    /// View radius in chunks (settings-driven).
    radius: i32,
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
            radius: DEFAULT_VIEW_RADIUS,
        }
    }

    /// Applies the settings' render distance; streaming adapts on the
    /// next update (new subscriptions or unloads as needed).
    pub fn set_radius(&mut self, radius: i32) {
        self.radius = radius.max(2);
    }

    /// Current view radius in chunks (the loaded square's half-extent).
    pub fn radius(&self) -> i32 {
        self.radius
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
            if chebyshev(result.chunk, center) > self.radius + UNLOAD_MARGIN
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
        let limit = self.radius + GEN_MARGIN + UNLOAD_MARGIN;
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
        let mut wanted: Vec<ChunkPos> = ring(center, self.radius + GEN_MARGIN)
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
        let mut ready: Vec<ChunkPos> = ring(center, self.radius)
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
        let center = block_to_chunk(block);
        let edit_sy = block.y.div_euclid(SECTION_SIZE);
        // One bounded light field for the 3×3 columns around the edit, plus a
        // matching tier-2 source-heat field (so placing/removing lava updates
        // the surrounding glow, attenuated by conductivity).
        let field = self.light_for(center, block.y);
        let heat = self.heat_for(center, block.y);
        // A block edit changes geometry/light only within the propagation
        // radius (~15 blocks < a section), so the affected set is exactly the
        // 3×3×3 section neighbourhood around the edit's section. Re-mesh those
        // synchronously from the one bounded field — instead of queuing 8 whole
        // neighbouring columns for an async full re-mesh, which in the deep
        // world meant re-lighting + re-meshing + re-uploading ~400 sections
        // (and a ~960-block light flood ×8) on every break.
        for col in ring(center, 1) {
            if !self.meshed.contains_key(&col) {
                continue; // neighbour column not loaded/meshed
            }
            for sy in (edit_sy - 1)..=(edit_sy + 1) {
                let pos = IVec3::new(col.x, sy, col.z);
                let section = self.world.section(pos).cloned();
                let known = self.meshed.get(&col).is_some_and(|s| s.contains(&pos));
                if section.is_none() && !known {
                    continue; // empty section that isn't drawn — nothing to do
                }
                let base = pos * SECTION_SIZE;
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
                    |local: IVec3| quantize_heat(heat.delta(base + local)),
                );
                renderer.set_chunk(pos, &mesh)?;
                let sections = self.meshed.get_mut(&col).expect("checked above");
                if !sections.contains(&pos) {
                    sections.push(pos); // placed into a previously all-air section
                }
            }
        }
        Ok(())
    }

    /// Lighting for the region around an edit at `edit_y`, computed fresh from
    /// current world data. The Y ceiling covers the whole 3×3 neighborhood so
    /// tall neighbors and the open sky light correctly; the floor is anchored
    /// just below the edit, because a single block edit can't change light more
    /// than the propagation radius (~15 blocks) below the sections being
    /// re-meshed. In the deep world this is the difference between relighting a
    /// ~64-block window and the whole ~960-block column on every break.
    fn light_for(&self, column: ChunkPos, edit_y: i32) -> LightField {
        // Affected sections span the edit's section ±1; cover one more section
        // below for the light that bleeds up into them.
        let edit_section = edit_y.div_euclid(SECTION_SIZE);
        let min_y = ((edit_section - 2) * SECTION_SIZE).max(BOTTOM_SECTION_Y * SECTION_SIZE);
        // If the edit sits well below the local surface, no skylight reaches it:
        // use a tight local window with no sky seeding, rather than spanning all
        // the way up to the surface (the deep relight that tanked FPS on every
        // break in the 128-section world). Near/above the surface, span to the
        // open sky so skylight stays correct.
        let surface = self.world.generator().surface_height(
            column.x * SECTION_SIZE + 8,
            column.z * SECTION_SIZE + 8,
        );
        if edit_y < surface - 64 {
            let max_y = (edit_section + 3) * SECTION_SIZE;
            compute_light(|pos| self.world.block(pos), column, min_y, max_y, false)
        } else {
            let max_y = ring(column, 1)
                .flat_map(|c| self.world.column_sections(c))
                .map(|s| (s.y + 1) * SECTION_SIZE)
                .max()
                .unwrap_or(SECTION_SIZE);
            compute_light(
                |pos| self.world.block(pos),
                column,
                min_y,
                max_y.max(min_y + SECTION_SIZE),
                true,
            )
        }
    }

    /// Tier-2 source-heat for the region around an edit. Heat spreads only
    /// ~12 blocks from a source, so a tight window (the edit's section ±2)
    /// covers the affected glow; sections outside it read delta 0.
    fn heat_for(&self, column: ChunkPos, edit_y: i32) -> HeatField {
        let edit_section = edit_y.div_euclid(SECTION_SIZE);
        let min_y = ((edit_section - 2) * SECTION_SIZE).max(BOTTOM_SECTION_Y * SECTION_SIZE);
        let max_y = (edit_section + 3) * SECTION_SIZE;
        compute_heat(
            |pos| self.world.block(pos),
            oc_world::env_registry::active(),
            column,
            min_y,
            max_y.max(min_y + SECTION_SIZE),
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
            true, // full column up to the open sky
        );
        // Tier-2 source heat over the same column, so natural lava glows the
        // surrounding rock as soon as a column streams in (deterministic from
        // the blocks — no server sync). Cheap where there are no sources.
        let heat = compute_heat(
            block_at,
            oc_world::env_registry::active(),
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
                    |local: IVec3| quantize_heat(heat.delta(base + local)),
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
