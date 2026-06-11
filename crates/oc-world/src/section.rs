//! 16³ chunk sections.

use glam::IVec3;
use oc_core::SECTION_SIZE;

use crate::{BlockId, blocks};

const VOLUME: usize = (SECTION_SIZE * SECTION_SIZE * SECTION_SIZE) as usize;

/// A 16³ block volume. Voxels are indexed `(y * 16 + z) * 16 + x`.
pub struct Section {
    voxels: Box<[BlockId; VOLUME]>,
}

impl Section {
    pub fn empty() -> Self {
        Self {
            voxels: vec![BlockId::AIR; VOLUME].into_boxed_slice().try_into().unwrap(),
        }
    }

    /// `pos` components must be in `0..16`.
    pub fn get(&self, pos: IVec3) -> BlockId {
        self.voxels[Self::index(pos)]
    }

    pub fn set(&mut self, pos: IVec3, block: BlockId) {
        self.voxels[Self::index(pos)] = block;
    }

    fn index(pos: IVec3) -> usize {
        debug_assert!(
            pos.cmpge(IVec3::ZERO).all() && pos.cmplt(IVec3::splat(SECTION_SIZE)).all(),
            "section-local position out of range: {pos}"
        );
        ((pos.y * SECTION_SIZE + pos.z) * SECTION_SIZE + pos.x) as usize
    }

    /// A test pattern: rolling stone/dirt/grass terrain inside one section.
    pub fn test_terrain() -> Self {
        let mut section = Self::empty();
        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                // Cheap deterministic "hills" without pulling in a noise crate.
                let h = 6.0
                    + 3.0 * ((x as f32 * 0.7).sin() + (z as f32 * 0.5).cos())
                    + ((x + z) as f32 * 0.45).sin();
                let height = (h as i32).clamp(1, SECTION_SIZE - 1);
                for y in 0..height {
                    let block = if y == height - 1 {
                        blocks::GRASS
                    } else if y >= height - 3 {
                        blocks::DIRT
                    } else {
                        blocks::STONE
                    };
                    section.set(IVec3::new(x, y, z), block);
                }
            }
        }
        section
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_set_roundtrip() {
        let mut s = Section::empty();
        assert_eq!(s.get(IVec3::new(3, 4, 5)), BlockId::AIR);
        s.set(IVec3::new(3, 4, 5), blocks::STONE);
        assert_eq!(s.get(IVec3::new(3, 4, 5)), blocks::STONE);
        assert_eq!(s.get(IVec3::new(5, 4, 3)), BlockId::AIR);
    }

    #[test]
    fn test_terrain_has_grass_surface() {
        let s = Section::test_terrain();
        let mut grass = 0;
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..16 {
                    if s.get(IVec3::new(x, y, z)) == blocks::GRASS {
                        grass += 1;
                    }
                }
            }
        }
        assert_eq!(grass, 16 * 16, "every column should have exactly one grass cap");
    }
}
