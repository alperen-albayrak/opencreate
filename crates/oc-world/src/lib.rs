//! Voxel world storage: block states and chunk sections (ARCHITECTURE.md §3).
//!
//! Milestone-1 simplification: sections store a flat `u16` block id per voxel.
//! Palette compression replaces the backing storage later without changing
//! this crate's API.

pub mod section;

pub use section::Section;

/// A block state id. 0 is always air.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BlockId(pub u16);

impl BlockId {
    pub const AIR: BlockId = BlockId(0);

    pub fn is_air(self) -> bool {
        self == Self::AIR
    }
}

/// Hardcoded test blocks until the data-driven registry (§3) exists.
pub mod blocks {
    use super::BlockId;

    pub const AIR: BlockId = BlockId(0);
    pub const STONE: BlockId = BlockId(1);
    pub const DIRT: BlockId = BlockId(2);
    pub const GRASS: BlockId = BlockId(3);
}
