//! Sparse world storage: generated chunk columns of 16³ sections.
//!
//! Milestone-2 simplification: a flat section map keyed by section position.
//! The §3 column-with-sparse-Y layout replaces the backing storage later
//! without changing this API.

use std::collections::HashMap;

use glam::IVec3;
use oc_core::coords::{block_in_section, block_to_section};
use oc_core::{BlockPos, ChunkPos, SECTION_SIZE, SectionPos};

use crate::terrain::{BOTTOM_SECTION_Y, TerrainGenerator};
use crate::{BlockId, Section};

/// Vertical section range of a generated column, inclusive.
#[derive(Debug, Clone, Copy)]
pub struct ColumnSpan {
    pub min_section_y: i32,
    pub max_section_y: i32,
}

/// All loaded voxel data plus the generator that fills it.
pub struct World {
    generator: TerrainGenerator,
    /// Only sections containing at least one non-air block are stored;
    /// absence within a generated column means all-air.
    sections: HashMap<SectionPos, Section>,
    columns: HashMap<ChunkPos, ColumnSpan>,
}

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            generator: TerrainGenerator::new(seed),
            sections: HashMap::new(),
            columns: HashMap::new(),
        }
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

    pub fn section(&self, pos: SectionPos) -> Option<&Section> {
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

    /// Generates a column's terrain if it isn't loaded yet.
    pub fn generate_column(&mut self, chunk: ChunkPos) {
        if self.columns.contains_key(&chunk) {
            return;
        }

        let base_x = chunk.x * SECTION_SIZE;
        let base_z = chunk.z * SECTION_SIZE;
        let mut heights = [[0i32; 16]; 16];
        let mut max_height = i32::MIN;
        for (dz, row) in heights.iter_mut().enumerate() {
            for (dx, h) in row.iter_mut().enumerate() {
                *h = self.generator.surface_height(base_x + dx as i32, base_z + dz as i32);
                max_height = max_height.max(*h);
            }
        }

        let span = ColumnSpan {
            min_section_y: BOTTOM_SECTION_Y,
            max_section_y: max_height.div_euclid(SECTION_SIZE),
        };
        for section_y in span.min_section_y..=span.max_section_y {
            let base_y = section_y * SECTION_SIZE;
            let mut section = Section::empty();
            let mut any = false;
            for (dz, row) in heights.iter().enumerate() {
                for (dx, &surface) in row.iter().enumerate() {
                    for dy in 0..SECTION_SIZE {
                        let block = self.generator.block_at(surface, base_y + dy);
                        if !block.is_air() {
                            section.set(IVec3::new(dx as i32, dy, dz as i32), block);
                            any = true;
                        }
                    }
                }
            }
            if any {
                self.sections.insert(IVec3::new(chunk.x, section_y, chunk.z), section);
            }
        }
        self.columns.insert(chunk, span);
    }

    pub fn unload_column(&mut self, chunk: ChunkPos) {
        let Some(span) = self.columns.remove(&chunk) else {
            return;
        };
        for y in span.min_section_y..=span.max_section_y {
            self.sections.remove(&IVec3::new(chunk.x, y, chunk.z));
        }
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

        for (dx, dz) in [(0, 0), (7, 11), (15, 15)] {
            let (x, z) = (chunk.x * 16 + dx, chunk.z * 16 + dz);
            let h = world.surface_height(x, z);
            assert_eq!(world.block(IVec3::new(x, h, z)), blocks::GRASS);
            assert_eq!(world.block(IVec3::new(x, h + 1, z)), BlockId::AIR);
            assert_eq!(world.block(IVec3::new(x, h - 1, z)), blocks::DIRT);
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
