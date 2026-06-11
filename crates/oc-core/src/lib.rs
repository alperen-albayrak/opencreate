//! Shared vocabulary for OpenCreate: coordinate types, world constants.
//!
//! This crate is dependency-light and everything depends on it; nothing in it
//! depends on the rest of the workspace (see ARCHITECTURE.md §2).

pub mod coords;

pub use coords::{BlockPos, ChunkPos, SectionPos};

/// Edge length of a cubic chunk section, in blocks.
pub const SECTION_SIZE: i32 = 16;

/// log2 of [`SECTION_SIZE`], for shift-based coordinate math.
pub const SECTION_SHIFT: i32 = 4;

/// Fixed server simulation rate, in ticks per second (ARCHITECTURE.md §1).
pub const TICKS_PER_SECOND: u32 = 30;
