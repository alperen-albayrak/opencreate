//! Sparse world storage: generated chunk columns of 16³ sections.
//!
//! Milestone-2 simplification: a flat section map keyed by section position.
//! The §3 column-with-sparse-Y layout replaces the backing storage later
//! without changing this API.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::IVec3;
use oc_core::coords::{block_in_section, block_to_chunk, block_to_section};
use oc_core::{BlockPos, ChunkPos, SECTION_SHIFT, SECTION_SIZE, SectionPos};

use crate::terrain::{
    BOTTOM_SECTION_Y, ColumnInfo, SEA_LEVEL, TerrainGenerator, WORLD_MAX_Y, WORLD_MIN_Y,
};
use crate::{BlockId, Section};

/// Vertical section range of a generated column, inclusive.
#[derive(Debug, Clone, Copy)]
pub struct ColumnSpan {
    pub min_section_y: i32,
    pub max_section_y: i32,
}

/// Freshly generated terrain for one column, before it joins a `World`.
/// Produced by [`generate_column_data`], which is a pure function of
/// (generator, position) and safe to run on worker threads.
#[derive(Clone)]
pub struct GeneratedColumn {
    pub chunk: ChunkPos,
    pub span: ColumnSpan,
    pub sections: Vec<(SectionPos, Section)>,
}

/// Generates one column's terrain. Pure: no world access, deterministic.
pub fn generate_column_data(generator: &TerrainGenerator, chunk: ChunkPos) -> GeneratedColumn {
    let base_x = chunk.x * SECTION_SIZE;
    let base_z = chunk.z * SECTION_SIZE;
    let mut infos = Vec::with_capacity(256);
    let mut max_height = SEA_LEVEL; // water reaches sea level even offshore
    for dz in 0..16 {
        for dx in 0..16 {
            let info: ColumnInfo = generator.column(base_x + dx, base_z + dz);
            max_height = max_height.max(info.surface);
            infos.push(info);
        }
    }

    // Trees rooted in this column or a neighbor may reach into this column
    // (the classic cross-chunk feature problem, solved by scanning the
    // 3×3 neighborhood's deterministic origins). Trees never replace
    // terrain, only fill air.
    let mut overlay: HashMap<BlockPos, BlockId> = HashMap::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            let neighbor = ChunkPos::new(chunk.x + dx, chunk.z + dz);
            for origin in generator.tree_origins(neighbor) {
                for (pos, block) in generator.tree_blocks(origin) {
                    let in_column = pos.x >> SECTION_SHIFT == chunk.x
                        && pos.z >> SECTION_SHIFT == chunk.z;
                    if in_column {
                        max_height = max_height.max(pos.y);
                        // Logs win over leaves where adjacent trees touch.
                        if block == crate::blocks::LOG {
                            overlay.insert(pos, block);
                        } else {
                            overlay.entry(pos).or_insert(block);
                        }
                    }
                }
            }
        }
    }

    // Village houses use the same cross-chunk origin scan, but their
    // blocks are authoritative: AIR entries carve interiors out of
    // terrain (and out of any tree that strayed inside).
    let mut structures: HashMap<BlockPos, BlockId> = HashMap::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            let neighbor = ChunkPos::new(chunk.x + dx, chunk.z + dz);
            for origin in generator.house_origins(neighbor) {
                for (pos, block) in generator.house_blocks(origin) {
                    let in_column = pos.x >> SECTION_SHIFT == chunk.x
                        && pos.z >> SECTION_SHIFT == chunk.z;
                    if in_column {
                        max_height = max_height.max(pos.y);
                        structures.insert(pos, block);
                    }
                }
            }
        }
    }

    let span = ColumnSpan {
        min_section_y: BOTTOM_SECTION_Y,
        max_section_y: max_height.div_euclid(SECTION_SIZE),
    };
    let mut sections = Vec::new();
    for section_y in span.min_section_y..=span.max_section_y {
        let base_y = section_y * SECTION_SIZE;
        let mut section = Section::empty();
        let mut any = false;
        for dz in 0..16usize {
            for dx in 0..16usize {
                let (x, z) = (base_x + dx as i32, base_z + dz as i32);
                let info = &infos[dz * 16 + dx];
                for dy in 0..SECTION_SIZE {
                    let y = base_y + dy;
                    let pos = IVec3::new(x, y, z);
                    // Carve caves + deep bands (hellish air, lava lake) in one
                    // pass; bedrock and non-solid blocks pass through.
                    let mut block =
                        generator.carve(pos, info.surface, generator.block_in_column(info, y));
                    if block.is_air()
                        && let Some(&tree) = overlay.get(&IVec3::new(x, y, z))
                    {
                        block = tree;
                    }
                    if let Some(&structure) = structures.get(&IVec3::new(x, y, z)) {
                        block = structure;
                    }
                    if !block.is_air() {
                        section.set(IVec3::new(dx as i32, dy, dz as i32), block);
                        any = true;
                    }
                }
            }
        }
        if any {
            sections.push((IVec3::new(chunk.x, section_y, chunk.z), section));
        }
    }
    GeneratedColumn { chunk, span, sections }
}

/// All loaded voxel data plus the generator that fills it.
pub struct World {
    generator: TerrainGenerator,
    /// Only sections containing at least one non-air block are stored;
    /// absence within a generated column means all-air. `Arc` so meshing
    /// jobs on worker threads can hold cheap snapshots; block edits go
    /// through `Arc::make_mut` (copy-on-write if a job holds a reference).
    sections: HashMap<SectionPos, Arc<Section>>,
    columns: HashMap<ChunkPos, ColumnSpan>,
    /// Columns edited since they were generated/loaded; these (and only
    /// these) need saving — pristine terrain regenerates from the seed.
    dirty: HashSet<ChunkPos>,
    /// Tier-3 stored temperature (°C) on the sparse set of blocks currently out
    /// of thermal equilibrium — a placed block heating toward the deep ambient.
    /// Server-authoritative dynamic state, ticked by the server and persisted
    /// with its column; empty for most of the world (see [`crate::heat`]).
    temperatures: HashMap<BlockPos, f32>,
}

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            generator: TerrainGenerator::new(seed),
            sections: HashMap::new(),
            columns: HashMap::new(),
            dirty: HashSet::new(),
            temperatures: HashMap::new(),
        }
    }

    /// Stored temperature (°C) at a block, if it is tracked as out of
    /// equilibrium (else the cell is at its pure base temperature).
    pub fn temperature(&self, pos: BlockPos) -> Option<f32> {
        self.temperatures.get(&pos).copied()
    }

    /// Tracks (or updates) a block's stored temperature; marks its column dirty
    /// so the value is saved.
    pub fn set_temperature(&mut self, pos: BlockPos, temp: f32) {
        self.temperatures.insert(pos, temp);
        self.dirty.insert(block_to_chunk(pos));
    }

    /// Drops a block's stored temperature (it equilibrated, or the block was
    /// replaced); marks its column dirty if anything was removed.
    pub fn remove_temperature(&mut self, pos: BlockPos) {
        if self.temperatures.remove(&pos).is_some() {
            self.dirty.insert(block_to_chunk(pos));
        }
    }

    /// All tracked `(position, °C)` pairs — the server's relaxation tick set.
    pub fn temperatures(&self) -> impl Iterator<Item = (BlockPos, f32)> + '_ {
        self.temperatures.iter().map(|(&p, &t)| (p, t))
    }

    pub fn is_dirty(&self, chunk: ChunkPos) -> bool {
        self.dirty.contains(&chunk)
    }

    pub fn dirty_columns(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.dirty.iter().copied()
    }

    /// Clears a column's dirty flag (call after persisting it).
    pub fn mark_saved(&mut self, chunk: ChunkPos) {
        self.dirty.remove(&chunk);
    }

    /// Snapshot of a loaded column for persistence.
    pub fn export_column(&self, chunk: ChunkPos) -> Option<crate::store::StoredColumn> {
        let span = *self.columns.get(&chunk)?;
        let sections = (span.min_section_y..=span.max_section_y)
            .filter_map(|y| {
                let section = self.sections.get(&IVec3::new(chunk.x, y, chunk.z))?;
                Some((y, Section::clone(section)))
            })
            .collect();
        Some(crate::store::StoredColumn { span, sections })
    }

    pub fn is_generated(&self, chunk: ChunkPos) -> bool {
        self.columns.contains_key(&chunk)
    }

    pub fn loaded_columns(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.columns.keys().copied()
    }

    /// Topmost solid Y at a block column (pure; works before generation).
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        self.generator.surface_height(x, z)
    }

    pub fn section(&self, pos: SectionPos) -> Option<&Arc<Section>> {
        self.sections.get(&pos)
    }

    /// Air for any position outside stored sections.
    pub fn block(&self, pos: BlockPos) -> BlockId {
        match self.sections.get(&block_to_section(pos)) {
            Some(section) => section.get(block_in_section(pos)),
            None => BlockId::AIR,
        }
    }

    /// Section positions of a generated column that contain blocks.
    pub fn column_sections(&self, chunk: ChunkPos) -> Vec<SectionPos> {
        let Some(span) = self.columns.get(&chunk) else {
            return Vec::new();
        };
        (span.min_section_y..=span.max_section_y)
            .map(|y| IVec3::new(chunk.x, y, chunk.z))
            .filter(|pos| self.sections.contains_key(pos))
            .collect()
    }

    pub fn generator(&self) -> &TerrainGenerator {
        &self.generator
    }

    /// Adds generated terrain to the world. No-op if the column is loaded.
    pub fn insert_column(&mut self, column: GeneratedColumn) {
        if self.columns.contains_key(&column.chunk) {
            return;
        }
        for (pos, section) in column.sections {
            self.sections.insert(pos, Arc::new(section));
        }
        self.columns.insert(column.chunk, column.span);
    }

    /// Generates a column's terrain synchronously if it isn't loaded yet.
    pub fn generate_column(&mut self, chunk: ChunkPos) {
        if self.columns.contains_key(&chunk) {
            return;
        }
        let column = generate_column_data(&self.generator, chunk);
        self.insert_column(column);
    }

    /// Writes one block. Returns false (no-op) if the column isn't
    /// generated. Creates the backing section and extends the column span
    /// when building above/below existing content.
    pub fn set_block(&mut self, pos: BlockPos, block: BlockId) -> bool {
        // Hard build limits: nothing exists outside the world's vertical range.
        if pos.y < WORLD_MIN_Y || pos.y > WORLD_MAX_Y {
            return false;
        }
        let chunk = block_to_chunk(pos);
        // Any edit supersedes a stored temperature at this cell (the block
        // changed); the server re-tracks it if the new block is off-ambient.
        self.temperatures.remove(&pos);
        let Some(span) = self.columns.get_mut(&chunk) else {
            return false;
        };
        let section_pos = block_to_section(pos);
        if block.is_air() && !self.sections.contains_key(&section_pos) {
            return true; // clearing air in an all-air section
        }
        span.min_section_y = span.min_section_y.min(section_pos.y);
        span.max_section_y = span.max_section_y.max(section_pos.y);
        let section = self
            .sections
            .entry(section_pos)
            .or_insert_with(|| Arc::new(Section::empty()));
        // Copy-on-write: clones only if a mesh job still holds this section.
        Arc::make_mut(section).set(block_in_section(pos), block);
        self.dirty.insert(chunk);
        true
    }

    /// Drops a column from memory. The caller is responsible for saving it
    /// first if it was dirty (`is_dirty`/`export_column`).
    pub fn unload_column(&mut self, chunk: ChunkPos) {
        let Some(span) = self.columns.remove(&chunk) else {
            return;
        };
        for y in span.min_section_y..=span.max_section_y {
            self.sections.remove(&IVec3::new(chunk.x, y, chunk.z));
        }
        self.temperatures.retain(|p, _| block_to_chunk(*p) != chunk);
        self.dirty.remove(&chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;

    #[test]
    fn generated_column_matches_heightmap() {
        let mut world = World::new(99);
        let chunk = ChunkPos::new(3, -2);
        world.generate_column(chunk);
        assert!(world.is_generated(chunk));

        let generator = world.generator().clone();
        for (dx, dz) in [(0, 0), (7, 11), (15, 15)] {
            let (x, z) = (chunk.x * 16 + dx, chunk.z * 16 + dz);
            let h = world.surface_height(x, z);
            let info = generator.column(x, z);
            let expect = |y: i32| generator.block_in_column(&info, y);
            assert_eq!(world.block(IVec3::new(x, h, z)), expect(h));
            assert_eq!(world.block(IVec3::new(x, h - 1, z)), expect(h - 1));
            // Above the surface: terrain rules, or part of a tree.
            let above = world.block(IVec3::new(x, h + 1, z));
            assert!(
                above == expect(h + 1) || above == blocks::LOG || above == blocks::LEAVES,
                "unexpected block above surface: {above:?}"
            );
        }
    }

    #[test]
    fn ungenerated_positions_read_as_air() {
        let world = World::new(99);
        assert_eq!(world.block(IVec3::new(0, -5, 0)), BlockId::AIR);
    }

    #[test]
    fn unload_removes_all_sections() {
        let mut world = World::new(99);
        let chunk = ChunkPos::new(0, 0);
        world.generate_column(chunk);
        assert!(!world.column_sections(chunk).is_empty());
        world.unload_column(chunk);
        assert!(world.column_sections(chunk).is_empty());
        assert_eq!(world.block(IVec3::new(8, world.surface_height(8, 8), 8)), BlockId::AIR);
    }

    #[test]
    fn set_block_extends_the_column_span() {
        let mut world = World::new(99);
        let chunk = ChunkPos::new(0, 0);
        world.generate_column(chunk);

        // Far above any generated terrain: needs a new section + wider span.
        let high = IVec3::new(8, 500, 8);
        assert!(world.set_block(high, blocks::STONE));
        assert_eq!(world.block(high), blocks::STONE);
        let high_section = IVec3::new(0, 500 >> 4, 0);
        assert!(world.column_sections(chunk).contains(&high_section));

        assert!(world.set_block(high, BlockId::AIR));
        assert_eq!(world.block(high), BlockId::AIR);

        // Ungenerated column: rejected.
        assert!(!world.set_block(IVec3::new(1000, 0, 1000), blocks::STONE));
    }

    #[test]
    fn village_houses_generate_into_columns() {
        let generator = crate::terrain::TerrainGenerator::new(42);
        for rx in -20..20 {
            for rz in -20..20 {
                let Some(center) = generator.village_center(rx, rz) else { continue };
                for dcx in -2..=2 {
                    for dcz in -2..=2 {
                        let chunk = ChunkPos::new(center.x + dcx, center.z + dcz);
                        let Some(&origin) = generator.house_origins(chunk).first() else {
                            continue;
                        };
                        let mut world = World::new(42);
                        world.generate_column(chunk);
                        let at = |d: IVec3| world.block(origin + d);
                        assert_eq!(at(IVec3::new(0, -1, 0)), blocks::PLANKS, "floor");
                        assert_eq!(at(IVec3::new(0, 0, 0)), BlockId::AIR, "interior");
                        assert_eq!(at(IVec3::new(2, 0, -2)), blocks::LAMP, "lamp");
                        assert_eq!(at(IVec3::new(0, 3, 0)), blocks::PLANKS, "roof");
                        assert_eq!(at(IVec3::new(3, 0, 3)), blocks::LOG, "corner");
                        return; // one verified house is enough
                    }
                }
            }
        }
        panic!("no village house found in the scan area");
    }

    #[test]
    fn column_sections_cover_the_surface_band() {
        let mut world = World::new(7);
        let chunk = ChunkPos::new(-4, 9);
        world.generate_column(chunk);
        let sections = world.column_sections(chunk);
        let h = world.surface_height(chunk.x * 16 + 8, chunk.z * 16 + 8);
        let surface_section = IVec3::new(chunk.x, h.div_euclid(16), chunk.z);
        assert!(sections.contains(&surface_section), "surface section missing: {sections:?}");
    }
}

#[cfg(test)]
mod surface_invariant {
    use super::*;
    use crate::blocks;

    /// Every generated column's surface follows the surface rules; above
    /// the surface only air, water, or tree blocks.
    #[test]
    fn surface_blocks_match_biome_rules() {
        let mut world = World::new(20260611);
        for cx in -3..3 {
            for cz in -3..3 {
                world.generate_column(ChunkPos::new(cx, cz));
            }
        }
        let mut breaches = 0;
        for x in -40..40 {
            for z in -40..40 {
                let h = world.surface_height(x, z);
                let surface = world.block(IVec3::new(x, h, z));
                if surface == BlockId::AIR {
                    // A cave mouth carved through the heightmap surface —
                    // legitimate, but should be rare.
                    breaches += 1;
                    continue;
                }
                let info = world.generator().column(x, z);
                let expected = world.generator().block_in_column(&info, h);
                assert_eq!(surface, expected, "wrong surface at ({x},{h},{z})");

                let above = world.block(IVec3::new(x, h + 1, z));
                let above_ok = if h < SEA_LEVEL {
                    above == blocks::WATER
                } else {
                    above == BlockId::AIR || above == blocks::LOG || above == blocks::LEAVES
                };
                assert!(above_ok, "unexpected {above:?} above surface at ({x},{z})");
            }
        }
        let total = 80 * 80;
        assert!(
            breaches < total / 20,
            "cave mouths should be rare: {breaches}/{total}"
        );
    }
}
