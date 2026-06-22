//! Section streaming: keeps the server-fed world mirror meshed in a **box**
//! around the camera.
//!
//! The streamed unit is a 16³ **section** (§1/§8). The client subscribes to the
//! box of sections within a horizontal radius `H` and a vertical radius `V` of
//! the camera, holds a read-mirror `World` for physics/raycasts, meshes each
//! section with rayon jobs (§4) — seeding sky light from the server's per-column
//! heightmap so a band lights correctly without the column above it — uploads
//! under a per-frame budget, and unmeshes/unsubscribes what leaves the box. The
//! GPU only ever holds the box (a few thousand sections), not the whole vertical
//! extent of every column. Block-edit remeshing stays synchronous so local
//! prediction never lags a frame.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;
use glam::{DVec3, IVec3};
use oc_core::coords::{block_in_section, block_to_chunk, block_to_section};
use oc_core::{BlockPos, ChunkPos, SECTION_SIZE, SectionPos};
use oc_protocol::ClientMessage;
use oc_renderer::{Renderer, SectionMeshes, mesh_section, quantize_heat};
use oc_world::env_registry::EnvDef;
use oc_world::heat::{HeatField, compute_heat_in};
use oc_world::light::{LightField, compute_light_banded};
use oc_world::terrain::{BOTTOM_SECTION_Y, WORLD_MAX_Y};
use oc_world::world::GeneratedSection;
use oc_world::{BlockId, Section, World};

/// Default horizontal view radius (chunks); settings override per session.
const DEFAULT_VIEW_RADIUS: i32 = 12;
/// Default vertical view radius (16³ sections); settings override per session.
const DEFAULT_VERTICAL_RADIUS: i32 = 6;
/// Extra subscribed ring (each axis) beyond the meshed box, so a meshed
/// section's neighbours exist and its border faces cull against real blocks.
const GEN_MARGIN: i32 = 1;
/// Extra retained ring (each axis) before unsubscribing, so pacing back and
/// forth across a box border doesn't thrash subscribe/unload.
const UNLOAD_MARGIN: i32 = 2;
/// Maximum meshing jobs in flight at once.
const MAX_INFLIGHT: usize = 24;
/// Section meshes uploaded to the GPU per frame (bounds frame-time spikes).
const UPLOAD_BUDGET: usize = 32;
/// Minimum frames between full box reconciles, so a fast traversal at a large
/// render distance spreads the O(box) scan out instead of running it per frame.
const RECONCILE_MIN_FRAMES: u32 = 4;
/// Highest section the world generates into (the build ceiling / 16).
const TOP_SECTION_Y: i32 = WORLD_MAX_Y / SECTION_SIZE;

struct MeshJobResult {
    pos: SectionPos,
    mesh: SectionMeshes,
}

pub struct ChunkStreamer {
    world: World,
    /// Sections with a mesh currently uploaded to the renderer.
    meshed: HashSet<SectionPos>,
    mesh_inflight: HashSet<SectionPos>,
    /// Sections we asked the server for.
    subscribed: HashSet<SectionPos>,
    /// Sections the server has answered (content inserted, or known-air) — the
    /// gate for meshing a neighbour against real blocks instead of assumed air.
    resolved: HashSet<SectionPos>,
    /// The mesh frontier: box sections that have content but no mesh yet, as a
    /// FIFO queue (nearest-first after a rebuild) processed under a per-frame
    /// budget so `dispatch` is O(budget), never O(frontier) — vital when a large
    /// render distance makes the box tens of thousands of sections. `pending_in`
    /// mirrors it for O(1) membership / dedup.
    pending: VecDeque<SectionPos>,
    pending_in: HashSet<SectionPos>,
    /// Camera section at the last box reconcile; the box only shifts when this
    /// changes, so the O(box) subscribe/unload/unmesh scans run on a boundary
    /// crossing, not every frame.
    last_center: Option<(ChunkPos, i32)>,
    /// Frames since the last reconcile, to cap how often the O(box) scan runs
    /// during a fast traversal at a large render distance.
    since_reconcile: u32,
    /// Per-column sky-light heightmaps from `ColumnSky` (256 entries each): the
    /// world Y of the highest sky-blocker per (x,z), used to seed band skylight.
    heights: HashMap<(i32, i32), Vec<i32>>,
    mesh_tx: Sender<MeshJobResult>,
    mesh_rx: Receiver<MeshJobResult>,
    /// Mesh results that arrived but exceeded the frame's upload budget.
    upload_queue: Vec<MeshJobResult>,
    /// Horizontal view radius in chunks (settings-driven).
    radius: i32,
    /// Vertical view radius in 16³ sections (settings-driven).
    vertical_radius: i32,
}

impl ChunkStreamer {
    pub fn new(seed: u64) -> Self {
        let (mesh_tx, mesh_rx) = channel();
        Self {
            world: World::new(seed),
            meshed: HashSet::new(),
            mesh_inflight: HashSet::new(),
            subscribed: HashSet::new(),
            resolved: HashSet::new(),
            pending: VecDeque::new(),
            pending_in: HashSet::new(),
            last_center: None,
            since_reconcile: 0,
            heights: HashMap::new(),
            mesh_tx,
            mesh_rx,
            upload_queue: Vec::new(),
            radius: DEFAULT_VIEW_RADIUS,
            vertical_radius: DEFAULT_VERTICAL_RADIUS,
        }
    }

    /// Applies the settings' horizontal render distance; streaming adapts on the
    /// next update.
    pub fn set_radius(&mut self, radius: i32) {
        self.radius = radius.max(2);
    }

    /// Applies the settings' vertical render distance (box half-height in 16³
    /// sections); streaming adapts on the next update.
    pub fn set_vertical_radius(&mut self, radius: i32) {
        self.vertical_radius = radius.max(1);
    }

    /// Current horizontal view radius in chunks.
    pub fn radius(&self) -> i32 {
        self.radius
    }

    /// Terrain for a subscribed section arrived from the server.
    pub fn insert_section(&mut self, section: GeneratedSection) {
        let pos = section.pos;
        self.world.insert_section(section);
        self.resolved.insert(pos);
        // A box section with content joins the mesh frontier (unless already
        // meshing). Bounding to the box keeps the frontier from ballooning to the
        // whole subscribed set when the render distance is large.
        if let Some((center, center_sy)) = self.last_center
            && self.in_mesh_box(pos, center, center_sy)
            && !self.meshed.contains(&pos)
            && !self.mesh_inflight.contains(&pos)
            && self.pending_in.insert(pos)
        {
            self.pending.push_back(pos);
        }
    }

    /// The server confirmed a subscribed section is all air — resolve it so its
    /// neighbours can mesh and it isn't re-requested.
    pub fn resolve_empty(&mut self, pos: SectionPos) {
        self.resolved.insert(pos);
    }

    /// A column's sky-light heightmap arrived (or changed): cache it and re-mesh
    /// any of its meshed sections, since the sky seed may have moved (e.g. a
    /// shaft dug elsewhere in the column opened it up).
    pub fn set_column_sky(
        &mut self,
        renderer: &mut Renderer,
        col: (i32, i32),
        heights: Vec<i32>,
    ) -> Result<()> {
        let changed = self.heights.get(&col) != Some(&heights);
        self.heights.insert(col, heights);
        if changed {
            let meshed_here: Vec<SectionPos> =
                self.meshed.iter().filter(|p| (p.x, p.z) == col).copied().collect();
            if let (Some(lo), Some(hi)) = (
                meshed_here.iter().map(|p| p.y).min(),
                meshed_here.iter().map(|p| p.y).max(),
            ) {
                self.remesh_sections(
                    renderer,
                    ChunkPos::new(col.0, col.1),
                    &meshed_here,
                    lo,
                    hi,
                )?;
            }
        }
        Ok(())
    }

    /// A remote block change (or the echo of a local one). Applies and remeshes
    /// only if the mirror doesn't already have the value.
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

    /// Whether the server has answered this section (so physics may rely on the
    /// blocks around the player being present).
    pub fn is_resolved(&self, pos: SectionPos) -> bool {
        self.resolved.contains(&pos)
    }

    /// Runs one frame of streaming work around the camera. Subscription changes
    /// for the server are appended to `outbox`.
    pub fn update(
        &mut self,
        renderer: &mut Renderer,
        camera_pos: DVec3,
        outbox: &mut Vec<ClientMessage>,
    ) -> Result<()> {
        let center = block_to_chunk(camera_pos.floor().as_ivec3());
        let center_sy = (camera_pos.y.floor() as i32).div_euclid(SECTION_SIZE);
        self.upload_meshes(renderer, center, center_sy)?;
        // Reconcile the box (unmesh / unsubscribe / subscribe / rebuild the
        // frontier) when the camera crosses a section boundary — but at most once
        // every few frames, so a fast traversal at a large render distance can't
        // run the O(box) scan every frame (it would dominate the frame). Between
        // reconciles, dispatch + upload keep meshing from the existing frontier.
        self.since_reconcile = self.since_reconcile.saturating_add(1);
        let moved = self.last_center != Some((center, center_sy));
        if moved && self.since_reconcile >= RECONCILE_MIN_FRAMES {
            self.unmesh_out_of_box(renderer, center, center_sy);
            self.unload_far(renderer, center, center_sy, outbox);
            self.subscribe_near(center, center_sy, outbox);
            self.rebuild_pending(center, center_sy);
            self.last_center = Some((center, center_sy));
            self.since_reconcile = 0;
        }
        self.dispatch_mesh_jobs(center, center_sy);
        Ok(())
    }

    /// Rebuilds the mesh frontier from the box (the one O(box) scan, on a
    /// boundary crossing): every box section with content that isn't meshed or
    /// in flight. Incremental `insert_section` keeps it fresh between crossings.
    fn rebuild_pending(&mut self, center: ChunkPos, center_sy: i32) {
        self.pending.clear();
        self.pending_in.clear();
        let r = self.radius;
        let (lo, hi) = section_y_bounds(center_sy, self.vertical_radius);
        let mut box_secs: Vec<SectionPos> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                for sy in lo..=hi {
                    let pos = IVec3::new(center.x + dx, sy, center.z + dz);
                    if self.world.is_section_loaded(pos)
                        && !self.meshed.contains(&pos)
                        && !self.mesh_inflight.contains(&pos)
                    {
                        box_secs.push(pos);
                    }
                }
            }
        }
        // Nearest first, so the frontier meshes outward from the camera.
        box_secs.sort_by_key(|&p| dist2(p, center, center_sy));
        for pos in box_secs {
            self.pending_in.insert(pos);
            self.pending.push_back(pos);
        }
    }

    /// Whether a section is inside the meshed box (the drawn region).
    fn in_mesh_box(&self, pos: SectionPos, center: ChunkPos, center_sy: i32) -> bool {
        chebyshev(pos, center) <= self.radius && (pos.y - center_sy).abs() <= self.vertical_radius
    }

    /// Whether a section is inside the retained box (subscribed + unload margin).
    fn in_unload_box(&self, pos: SectionPos, center: ChunkPos, center_sy: i32) -> bool {
        let r = self.radius + GEN_MARGIN + UNLOAD_MARGIN;
        let v = self.vertical_radius + GEN_MARGIN + UNLOAD_MARGIN;
        chebyshev(pos, center) <= r && (pos.y - center_sy).abs() <= v
    }

    /// Uploads finished meshes, spreading work across frames.
    fn upload_meshes(
        &mut self,
        renderer: &mut Renderer,
        center: ChunkPos,
        center_sy: i32,
    ) -> Result<()> {
        while let Ok(result) = self.mesh_rx.try_recv() {
            self.mesh_inflight.remove(&result.pos);
            self.upload_queue.push(result);
        }

        let mut budget = UPLOAD_BUDGET;
        while budget > 0
            && let Some(result) = self.upload_queue.pop()
        {
            if !self.in_mesh_box(result.pos, center, center_sy)
                || !self.world.is_section_loaded(result.pos)
            {
                continue; // stale: out of the box or unloaded meanwhile
            }
            renderer.set_chunk(result.pos, &result.mesh)?;
            self.meshed.insert(result.pos);
            budget -= 1;
        }
        Ok(())
    }

    /// Drops GPU meshes for sections that left the meshed box (e.g. flying up
    /// past the vertical band) — they stay subscribed/loaded within the margin.
    fn unmesh_out_of_box(&mut self, renderer: &mut Renderer, center: ChunkPos, center_sy: i32) {
        let gone: Vec<SectionPos> = self
            .meshed
            .iter()
            .filter(|&&p| !self.in_mesh_box(p, center, center_sy))
            .copied()
            .collect();
        for pos in gone {
            renderer.remove_chunk(pos);
            self.meshed.remove(&pos);
        }
    }

    /// Unsubscribes + forgets sections that left the retained box entirely.
    fn unload_far(
        &mut self,
        renderer: &mut Renderer,
        center: ChunkPos,
        center_sy: i32,
        outbox: &mut Vec<ClientMessage>,
    ) {
        let gone: Vec<SectionPos> = self
            .subscribed
            .iter()
            .filter(|&&p| !self.in_unload_box(p, center, center_sy))
            .copied()
            .collect();
        for pos in gone {
            if self.meshed.remove(&pos) {
                renderer.remove_chunk(pos);
            }
            self.mesh_inflight.remove(&pos);
            self.resolved.remove(&pos);
            self.subscribed.remove(&pos);
            // The server owns persistence; the mirror just forgets.
            self.world.unload_section(pos);
            outbox.push(ClientMessage::UnsubscribeSection(pos));
        }
        // Forget heightmaps for columns with no subscribed section left.
        let active_cols: HashSet<(i32, i32)> = self.subscribed.iter().map(|p| (p.x, p.z)).collect();
        self.heights.retain(|c, _| active_cols.contains(c));
    }

    /// Asks the server for every section in the box (+ margin) we don't have yet.
    fn subscribe_near(
        &mut self,
        center: ChunkPos,
        center_sy: i32,
        outbox: &mut Vec<ClientMessage>,
    ) {
        let r = self.radius + GEN_MARGIN;
        let (lo, hi) = section_y_bounds(center_sy, self.vertical_radius + GEN_MARGIN);
        let mut wanted: Vec<SectionPos> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                for sy in lo..=hi {
                    let pos = IVec3::new(center.x + dx, sy, center.z + dz);
                    if !self.subscribed.contains(&pos) {
                        wanted.push(pos);
                    }
                }
            }
        }
        wanted.sort_by_key(|&p| dist2(p, center, center_sy));
        for pos in wanted {
            self.subscribed.insert(pos);
            outbox.push(ClientMessage::SubscribeSection(pos));
        }
    }

    /// Spawns mesh jobs by draining the frontier queue under a per-frame budget:
    /// pop from the front (nearest first), mesh the ready ones (neighbours + the
    /// 3×3 heightmaps present), drop stale entries, and re-queue the not-ready at
    /// the back. O(budget), independent of how large the frontier is.
    fn dispatch_mesh_jobs(&mut self, center: ChunkPos, center_sy: i32) {
        let mut slots = MAX_INFLIGHT.saturating_sub(self.mesh_inflight.len());
        if slots == 0 || self.pending.is_empty() {
            return;
        }
        // Cap on entries inspected per frame, so a huge frontier (large render
        // distance) can't stall the main thread; the queue cycles over frames.
        const EXAMINE_BUDGET: usize = 2048;
        let mut examined = 0;
        let mut requeue: Vec<SectionPos> = Vec::new();
        while slots > 0 && examined < EXAMINE_BUDGET {
            let Some(pos) = self.pending.pop_front() else { break };
            examined += 1;
            if self.meshed.contains(&pos)
                || self.mesh_inflight.contains(&pos)
                || !self.in_mesh_box(pos, center, center_sy)
                || !self.world.is_section_loaded(pos)
            {
                self.pending_in.remove(&pos); // stale — drop from the frontier
                continue;
            }
            if !self.section_ready(pos) {
                requeue.push(pos); // neighbours/sky not ready yet — try again later
                continue;
            }
            if let Some(job) = MeshJob::snapshot(&self.world, &self.heights, pos) {
                self.pending_in.remove(&pos);
                self.mesh_inflight.insert(pos);
                let tx = self.mesh_tx.clone();
                rayon::spawn(move || {
                    let _ = tx.send(job.run());
                });
                slots -= 1;
            } else {
                requeue.push(pos);
            }
        }
        for pos in requeue {
            self.pending.push_back(pos);
        }
    }

    /// Whether a section can be meshed now: the 3×3 columns' heightmaps are
    /// cached (for sky) and the 3×3×3 neighbour sections are resolved (for
    /// border-face culling against real blocks).
    fn section_ready(&self, pos: SectionPos) -> bool {
        for dz in -1..=1 {
            for dx in -1..=1 {
                if !self.heights.contains_key(&(pos.x + dx, pos.z + dz)) {
                    return false;
                }
            }
        }
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if !self.resolved.contains(&(pos + IVec3::new(dx, dy, dz))) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Re-meshes the sections around a block edit. The edit changes geometry +
    /// light within the propagation radius (~15 blocks < a section), so the
    /// affected set is the 3×3×3 section neighbourhood — and **one** banded field
    /// centred on the edit's column covers all of it (the field's window is the
    /// 3×3 columns), so the whole cluster shares a single light/heat flood
    /// instead of one per section. Drawn sections re-mesh; a section that just
    /// gained its first block (placed into air) joins the drawn set; one emptied
    /// to all air leaves it.
    pub fn remesh_after_edit(&mut self, renderer: &mut Renderer, block: BlockPos) -> Result<()> {
        let center = block_to_section(block);
        let mut targets: Vec<SectionPos> = Vec::with_capacity(27);
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let pos = center + IVec3::new(dx, dy, dz);
                    if self.meshed.contains(&pos)
                        || (pos == center && self.world.is_section_loaded(pos))
                    {
                        targets.push(pos);
                    }
                }
            }
        }
        let col = ChunkPos::new(center.x, center.z);
        self.remesh_sections(renderer, col, &targets, center.y - 1, center.y + 1)
    }

    /// Folds a batch of tier-3 stored-temperature updates from the server into
    /// the world mirror and re-meshes the glow of the affected drawn sections,
    /// grouped by column so each column shares one flood. A value back at the
    /// local ambient is pruned (the base glow covers it).
    pub fn apply_block_temps(
        &mut self,
        renderer: &mut Renderer,
        updates: &[(BlockPos, f32)],
    ) -> Result<()> {
        let env = oc_world::env_registry::active();
        let mut dirty: HashMap<(i32, i32), Vec<SectionPos>> = HashMap::new();
        for &(pos, temp) in updates {
            if (temp - oc_world::temperature::base(pos, env)).abs() < oc_world::heat::EQUILIBRIUM_C {
                self.world.remove_temperature(pos);
            } else {
                self.world.set_temperature(pos, temp);
            }
            let sp = block_to_section(pos);
            let group = dirty.entry((sp.x, sp.z)).or_default();
            if !group.contains(&sp) {
                group.push(sp);
            }
        }
        for (col, secs) in dirty {
            let drawn: Vec<SectionPos> =
                secs.into_iter().filter(|s| self.meshed.contains(s)).collect();
            if let (Some(lo), Some(hi)) =
                (drawn.iter().map(|s| s.y).min(), drawn.iter().map(|s| s.y).max())
            {
                self.remesh_sections(renderer, ChunkPos::new(col.0, col.1), &drawn, lo, hi)?;
            }
        }
        Ok(())
    }

    /// Re-meshes a cluster of sections (all within the 3×3 columns around
    /// `center_col`) from **one** banded light + heat field spanning their Y
    /// range — the window's sky is seeded from the cached column heightmaps. A
    /// section emptied to all air is dropped from the GPU; one with content is
    /// (re)uploaded and joins the drawn set. No-op if the heightmaps aren't ready
    /// (the existing meshes stay until a full pass catches them).
    fn remesh_sections(
        &mut self,
        renderer: &mut Renderer,
        center_col: ChunkPos,
        sections: &[SectionPos],
        lo: i32,
        hi: i32,
    ) -> Result<()> {
        if sections.is_empty() {
            return Ok(());
        }
        let Some((light, heat)) =
            self.band_field(center_col, (lo - 1) * SECTION_SIZE, (hi + 2) * SECTION_SIZE)
        else {
            return Ok(());
        };
        let env = oc_world::env_registry::active();
        for &pos in sections {
            let section = self.world.section(pos).cloned();
            if section.is_none() {
                if self.meshed.remove(&pos) {
                    renderer.remove_chunk(pos);
                }
                continue;
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
                |local: IVec3| light.get(base + local),
                |local: IVec3| {
                    let p = base + local;
                    quantize_heat(glow_delta(world.temperature(p), &heat, p, env))
                },
            );
            renderer.set_chunk(pos, &mesh)?;
            self.meshed.insert(pos);
        }
        Ok(())
    }

    /// Computes one banded light + tier-2 heat field over the 3×3 columns around
    /// `center_col` × the `[min_y, max_y)` band, seeding sky from the cached
    /// column heightmaps. None if a surrounding column's heightmap is missing.
    fn band_field(
        &self,
        center_col: ChunkPos,
        min_y: i32,
        max_y: i32,
    ) -> Option<(LightField, HeatField)> {
        for dz in -1..=1 {
            for dx in -1..=1 {
                if !self.heights.contains_key(&(center_col.x + dx, center_col.z + dz)) {
                    return None;
                }
            }
        }
        let heights = |wx: i32, wz: i32| -> i32 {
            self.heights
                .get(&(wx >> 4, wz >> 4))
                .map_or(i32::MIN, |h| h[((wz & 15) * 16 + (wx & 15)) as usize])
        };
        let light =
            compute_light_banded(|p| self.world.block(p), center_col, min_y, max_y, heights);
        let env = oc_world::env_registry::active();
        let heat = compute_heat_in(light.blocks(), light.base(), light.height(), env);
        Some((light, heat))
    }
}

/// Everything a worker needs to mesh one section: `Arc` snapshots of the section
/// and its 3×3×3 neighbourhood (for border-face culling and the 15-block light
/// range), the surrounding 3×3 columns' sky heightmaps, and the stored
/// temperatures inside the window (so a heated block streams in already glowing).
struct MeshJob {
    pos: SectionPos,
    snapshot: HashMap<SectionPos, Arc<Section>>,
    heights: HashMap<(i32, i32), Vec<i32>>,
    temps: HashMap<BlockPos, f32>,
}

impl MeshJob {
    fn snapshot(
        world: &World,
        heights_cache: &HashMap<(i32, i32), Vec<i32>>,
        pos: SectionPos,
    ) -> Option<Self> {
        let mut heights = HashMap::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let col = (pos.x + dx, pos.z + dz);
                heights.insert(col, heights_cache.get(&col)?.clone());
            }
        }
        let mut snapshot = HashMap::new();
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let np = pos + IVec3::new(dx, dy, dz);
                    if let Some(section) = world.section(np) {
                        snapshot.insert(np, Arc::clone(section));
                    }
                }
            }
        }
        let temps = world
            .temperatures()
            .filter(|(p, _)| {
                let sp = block_to_section(*p);
                (sp.x - pos.x).abs() <= 1 && (sp.y - pos.y).abs() <= 1 && (sp.z - pos.z).abs() <= 1
            })
            .collect();
        Some(Self { pos, snapshot, heights, temps })
    }

    fn run(self) -> MeshJobResult {
        let block_at = |pos: BlockPos| -> BlockId {
            match self.snapshot.get(&block_to_section(pos)) {
                Some(section) => section.get(block_in_section(pos)),
                None => BlockId::AIR,
            }
        };
        let heights = |wx: i32, wz: i32| -> i32 {
            self.heights
                .get(&(wx >> 4, wz >> 4))
                .map_or(i32::MIN, |h| h[((wz & 15) * 16 + (wx & 15)) as usize])
        };
        let min_y = (self.pos.y - 1) * SECTION_SIZE;
        let max_y = (self.pos.y + 2) * SECTION_SIZE;
        let light =
            compute_light_banded(block_at, ChunkPos::new(self.pos.x, self.pos.z), min_y, max_y, heights);
        let env = oc_world::env_registry::active();
        let heat = compute_heat_in(light.blocks(), light.base(), light.height(), env);
        let base = self.pos * SECTION_SIZE;
        let section = &self.snapshot[&self.pos];
        let mesh = mesh_section(
            |local: IVec3| {
                let inside = local.cmpge(IVec3::ZERO).all()
                    && local.cmplt(IVec3::splat(SECTION_SIZE)).all();
                if inside {
                    section.get(local)
                } else {
                    block_at(base + local)
                }
            },
            |local: IVec3| light.get(base + local),
            |local: IVec3| {
                let p = base + local;
                quantize_heat(glow_delta(self.temps.get(&p).copied(), &heat, p, env))
            },
        );
        MeshJobResult { pos: self.pos, mesh }
    }
}

/// The signed glow delta (°C from the base) to bake for a cell: a tier-3 stored
/// temperature (synced from the server) **overrides** — it can sit below the
/// base, so a cool block placed in the hot deep renders dark — otherwise the
/// deterministic tier-2 source delta.
fn glow_delta(stored: Option<f32>, heat: &HeatField, pos: BlockPos, env: &EnvDef) -> f32 {
    match stored {
        Some(t) => t - oc_world::temperature::base(pos, env),
        None => heat.delta(pos),
    }
}

/// Section-Y bounds of the box, clamped to the world's generated range.
fn section_y_bounds(center_sy: i32, v: i32) -> (i32, i32) {
    ((center_sy - v).max(BOTTOM_SECTION_Y), (center_sy + v).min(TOP_SECTION_Y))
}

/// Horizontal Chebyshev distance from a section's column to `center`.
fn chebyshev(pos: SectionPos, center: ChunkPos) -> i32 {
    (pos.x - center.x).abs().max((pos.z - center.z).abs())
}

/// Squared box distance (including the vertical axis), for nearest-first order.
fn dist2(pos: SectionPos, center: ChunkPos, center_sy: i32) -> i64 {
    let (dx, dz, dy) = (
        (pos.x - center.x) as i64,
        (pos.z - center.z) as i64,
        (pos.y - center_sy) as i64,
    );
    dx * dx + dz * dz + dy * dy
}
