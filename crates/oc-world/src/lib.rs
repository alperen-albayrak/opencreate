//! Voxel world storage: block states and chunk sections (ARCHITECTURE.md §3).
//!
//! Milestone-1 simplification: sections store a flat `u16` block id per voxel.
//! Palette compression replaces the backing storage later without changing
//! this crate's API.

pub mod env_registry;
pub mod fluid_registry;
pub mod gas_registry;
pub mod light;
pub mod physics;
pub mod raycast;
pub mod registry;
pub mod section;
pub mod store;
pub mod temperature;
pub mod terrain;
pub mod world;

pub use section::Section;
pub use world::World;

/// A block state id. 0 is always air.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BlockId(pub u16);

impl BlockId {
    pub const AIR: BlockId = BlockId(0);

    pub fn is_air(self) -> bool {
        self == Self::AIR
    }

    /// Solid blocks collide and stop raycasts; water does neither.
    pub fn is_solid(self) -> bool {
        registry::props(self).solid
    }

    /// Opaque blocks fully cover adjacent faces in meshing.
    pub fn is_opaque(self) -> bool {
        registry::props(self).opaque
    }

    /// Cost of light passing through this block, or `None` if it blocks
    /// light entirely.
    pub fn light_opacity(self) -> Option<u8> {
        registry::props(self).light_opacity
    }

    /// Light level (0..=15) this block emits.
    pub fn light_emission(self) -> u8 {
        registry::props(self).light_emission
    }

    /// Per-channel block-light seed (R, G, B, each 0..=15): the block's
    /// emission level tinted by its emissive color (hue). A warm lamp casts
    /// warm light; non-emitters return zeros. Seeds the RGB flood-fill.
    pub fn light_color(self) -> [u8; 3] {
        registry::props(self).light_color
    }
}

/// Stable numeric ids for the base-game blocks, matching `data/blocks.ron`'s
/// order. Properties are now data (see [`registry`]); these consts stay as
/// convenient handles for worldgen and code that places known blocks. A unit
/// test in [`registry`] locks the order against the string ids.
pub mod blocks {
    use super::BlockId;

    pub const AIR: BlockId = BlockId(0);
    pub const STONE: BlockId = BlockId(1);
    pub const DIRT: BlockId = BlockId(2);
    pub const GRASS: BlockId = BlockId(3);
    pub const SAND: BlockId = BlockId(4);
    pub const WATER: BlockId = BlockId(5);
    pub const LOG: BlockId = BlockId(6);
    pub const LEAVES: BlockId = BlockId(7);
    pub const LAMP: BlockId = BlockId(8);
    pub const SNOW: BlockId = BlockId(9);
    pub const PLANKS: BlockId = BlockId(10);
}
