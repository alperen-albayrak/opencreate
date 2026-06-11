//! World coordinates (ARCHITECTURE.md §3).
//!
//! The world is centered on the origin: block coordinates are signed `i32` on
//! all three axes, with sea level at Y = 0. Section coordinates address 16³
//! chunk sections; column coordinates address vertical 16×16 stacks.

use glam::IVec3;

use crate::SECTION_SHIFT;

/// Absolute position of a block in a grid, in blocks.
pub type BlockPos = IVec3;

/// Position of a 16³ section within a grid, in sections.
pub type SectionPos = IVec3;

/// Horizontal position of a 16×16 chunk column, in sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
}

/// Section containing a block. Arithmetic shift floors correctly for
/// negative coordinates ( -1 >> 4 == -1, i.e. block -1 is in section -1).
pub fn block_to_section(block: BlockPos) -> SectionPos {
    IVec3::new(
        block.x >> SECTION_SHIFT,
        block.y >> SECTION_SHIFT,
        block.z >> SECTION_SHIFT,
    )
}

/// Column containing a block.
pub fn block_to_chunk(block: BlockPos) -> ChunkPos {
    ChunkPos::new(block.x >> SECTION_SHIFT, block.z >> SECTION_SHIFT)
}

/// A block's offset within its section, each component in `0..16`.
pub fn block_in_section(block: BlockPos) -> IVec3 {
    IVec3::new(block.x & 15, block.y & 15, block.z & 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_floor_for_negative_coords() {
        assert_eq!(block_to_section(IVec3::new(0, 0, 0)), IVec3::new(0, 0, 0));
        assert_eq!(block_to_section(IVec3::new(15, 15, 15)), IVec3::new(0, 0, 0));
        assert_eq!(block_to_section(IVec3::new(16, 16, 16)), IVec3::new(1, 1, 1));
        assert_eq!(
            block_to_section(IVec3::new(-1, -1, -1)),
            IVec3::new(-1, -1, -1)
        );
        assert_eq!(
            block_to_section(IVec3::new(-16, -16, -16)),
            IVec3::new(-1, -1, -1)
        );
        assert_eq!(
            block_to_section(IVec3::new(-17, -17, -17)),
            IVec3::new(-2, -2, -2)
        );
    }

    #[test]
    fn in_section_offsets_are_always_non_negative() {
        assert_eq!(block_in_section(IVec3::new(-1, -1, -1)), IVec3::new(15, 15, 15));
        assert_eq!(block_in_section(IVec3::new(-16, 5, 31)), IVec3::new(0, 5, 15));
    }
}
