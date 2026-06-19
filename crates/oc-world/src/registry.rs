//! Data-driven block registry (ARCHITECTURE.md §3).
//!
//! Blocks are defined in `data/blocks.ron` (embedded as the built-in default),
//! not hardcoded. [`BlockId`] keeps its ergonomic methods (`is_solid`, …); their
//! bodies now read this registry, so the ~30 call sites are unchanged. The
//! schema is **forward-looking**: grouped by concern, every field `#[serde(default)]`,
//! so a block declares only what differs and later features (PBR, thermal/phase
//! transitions, gameplay) slot in with no reshaping.
//!
//! Numeric ids are per-load array indices; the **string id** (`oc:stone`) is the
//! stable identity. [`BlockPalette`] is the per-world string↔numeric table used by
//! the save format so reorders/mods never corrupt saves (see `store.rs`).

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::BlockId;

/// Embedded default block definitions; the game runs from any working directory.
const DEFAULT_BLOCKS: &str = include_str!("../../../data/blocks.ron");

/// Legacy block order for `format_version: 1` saves (the old hardcoded ids
/// 0..=10). v1 columns store these numeric ids directly; on load they remap
/// through this palette to the runtime registry.
const LEGACY_IDS: [&str; 11] = [
    "oc:air", "oc:stone", "oc:dirt", "oc:grass", "oc:sand", "oc:water", "oc:log",
    "oc:leaves", "oc:lamp", "oc:snow", "oc:planks",
];

/// Per-face texture layer indices into the block texture array
/// (0 = +Y top, 1 = -Y bottom, 2..=5 sides).
#[derive(Debug, Clone, Deserialize)]
pub enum Faces {
    /// Same layer on every face.
    All(u32),
    /// Distinct top / bottom layers; the four sides share one.
    Sided { top: u32, bottom: u32, side: u32 },
}

impl Default for Faces {
    fn default() -> Self {
        Faces::All(0)
    }
}

impl Faces {
    /// Texture layer for a face index (0 = top, 1 = bottom, 2..=5 = sides).
    pub fn layer(&self, face: usize) -> u32 {
        match self {
            Faces::All(l) => *l,
            Faces::Sided { top, bottom, side } => match face {
                0 => *top,
                1 => *bottom,
                _ => *side,
            },
        }
    }
}

/// Cross-registry phase-transition products (reserved — consumed by the
/// temperature feature). Each names a melt/freeze/boil product as a stable
/// string id into *any* matter registry (block/fluid/gas): `lava` freezes to
/// `oc:obsidian` (fast quench) or `oc:basalt` (slow cool); `ice` melts to
/// `oc:water`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PhaseTransition {
    #[serde(default)]
    pub melt: Option<String>,
    #[serde(default)]
    pub freeze: Option<String>,
    #[serde(default)]
    pub boil: Option<String>,
    /// Fast-quench product (e.g. lava meeting water → obsidian glass), vs the
    /// default slow-cool `freeze` product (e.g. basalt).
    #[serde(default)]
    pub quench: Option<String>,
}

/// A data-driven block definition. Grouped by concern; every field defaults so a
/// block declares only what differs. Most fields are **reserved** (parsed and
/// stored now, consumed by later features) so the schema never reshapes.
#[derive(Debug, Clone, Deserialize)]
pub struct BlockDef {
    /// Namespaced stable id, e.g. `oc:stone`.
    pub id: String,
    pub name: String,

    // --- active now ---
    /// Collides and stops raycasts.
    #[serde(default)]
    pub solid: bool,
    /// Fully covers adjacent faces in meshing.
    #[serde(default)]
    pub opaque: bool,
    /// Cost of light passing through (per block), or `None` if it blocks light.
    #[serde(default)]
    pub light_opacity: Option<u8>,
    /// Light level (0..=15) emitted.
    #[serde(default)]
    pub light_emission: u8,
    /// UI swatch / fallback tint.
    #[serde(default = "magenta")]
    pub color: (u8, u8, u8),
    /// Per-face texture layer indices.
    #[serde(default)]
    pub textures: Faces,
    /// Can be overwritten by placement (grass, tall plants).
    #[serde(default)]
    pub replaceable: bool,
    /// If this block is a fluid voxel, the `FluidDef` id it embodies (`oc:water`,
    /// `oc:lava`). Links the block to its fluid for submersion fog, buoyancy and
    /// breathing — generalising the old `== blocks::WATER` special-cases. None
    /// for ordinary solids.
    #[serde(default)]
    pub fluid: Option<String>,

    // --- reserved: render / material ---
    #[serde(default)]
    pub emissive: (f32, f32, f32),
    #[serde(default = "one")]
    pub roughness: f32,
    #[serde(default)]
    pub metalness: f32,
    #[serde(default)]
    pub subsurface: f32,
    #[serde(default)]
    pub render_layer: Option<String>,

    // --- reserved: gameplay ---
    #[serde(default = "one")]
    pub hardness: f32,
    #[serde(default)]
    pub blast_resistance: f32,
    #[serde(default)]
    pub harvest_tool: Option<String>,
    #[serde(default)]
    pub harvest_tier: u8,
    #[serde(default)]
    pub requires_correct_tool: bool,
    #[serde(default)]
    pub drops: Vec<String>,
    #[serde(default)]
    pub sound_set: Option<String>,
    #[serde(default)]
    pub flammability: u8,

    // --- reserved: physics ---
    #[serde(default = "default_friction")]
    pub friction: f32,
    #[serde(default)]
    pub gravity: bool,
    #[serde(default)]
    pub collision_shape: Option<String>,
    #[serde(default)]
    pub outline_shape: Option<String>,
    #[serde(default)]
    pub random_tick: bool,
    #[serde(default)]
    pub weight_class: Option<String>,
    #[serde(default)]
    pub bouncy: bool,
    #[serde(default)]
    pub sticky: bool,
    #[serde(default)]
    pub slippery: bool,
    #[serde(default)]
    pub fragile: bool,

    // --- reserved: thermal (shared trait; feeds the 3-stage heat function) ---
    #[serde(default)]
    pub heat_capacity: f32,
    #[serde(default)]
    pub conductivity: f32,
    #[serde(default)]
    pub resistivity: f32,
    #[serde(default)]
    pub melting_point: Option<f32>,
    #[serde(default)]
    pub boiling_point: Option<f32>,
    #[serde(default)]
    pub ignitable: bool,
    #[serde(default)]
    pub phase_transition: Option<PhaseTransition>,
}

fn one() -> f32 {
    1.0
}
fn default_friction() -> f32 {
    0.6
}
fn magenta() -> (u8, u8, u8) {
    (255, 0, 255)
}

/// Hot-path block properties: a small `Copy` struct extracted at load so the
/// per-voxel meshing/light/physics loops never touch the heavy `BlockDef`.
#[derive(Debug, Clone, Copy)]
pub struct BlockProps {
    pub solid: bool,
    pub opaque: bool,
    pub light_opacity: Option<u8>,
    pub light_emission: u8,
    /// Per-channel block-light seed (R, G, B, each 0..=15): the emission level
    /// (reach) tinted by the emissive color (hue). Seeds the RGB flood-fill.
    pub light_color: [u8; 3],
    /// This block is a fluid voxel (water, lava). Fluids render but do **not**
    /// occlude their neighbours' faces, so a solid block touching lava keeps
    /// its face (no holes at the boundary).
    pub fluid: bool,
    /// Thermal conductivity (W/m·K), copied from the def so the per-voxel heat
    /// flood-fill never touches the heavy `BlockDef`. Insulators (wood/wool/snow)
    /// are low and shield heat; stone/metal are high and conduct it.
    pub conductivity: f32,
}

/// Fallback for ids the registry doesn't know (out-of-range/stale): treated as a
/// plain solid block — matching the old `_ => solid` catch-all.
const DEFAULT_PROPS: BlockProps = BlockProps {
    solid: true,
    opaque: true,
    light_opacity: None,
    light_emission: 0,
    light_color: [0, 0, 0],
    fluid: false,
    conductivity: 2.5,
};

/// The loaded block registry: full defs + the hot-path props table + the
/// string→id lookup.
pub struct BlockRegistry {
    defs: Vec<BlockDef>,
    props: Vec<BlockProps>,
    by_id: HashMap<String, BlockId>,
}

impl BlockRegistry {
    fn parse(ron_text: &str) -> Result<Self> {
        let defs: Vec<BlockDef> = ron::from_str(ron_text).context("parsing blocks.ron")?;
        let mut by_id = HashMap::with_capacity(defs.len());
        let mut props = Vec::with_capacity(defs.len());
        for (index, d) in defs.iter().enumerate() {
            if by_id.insert(d.id.clone(), BlockId(index as u16)).is_some() {
                bail!("duplicate block id {:?}", d.id);
            }
            // Per-channel block-light seed: the emission level (reach) tinted by
            // the emissive color (hue). A colorless emitter floods every channel
            // equally; a warm lamp floods red fully and blue less, so its cast
            // light carries the source's color (the RGB-light contract).
            let (er, eg, eb) = d.emissive;
            let m = er.max(eg).max(eb);
            let e = d.light_emission as f32;
            let light_color = if d.light_emission == 0 {
                [0, 0, 0]
            } else if m <= 0.0 {
                [d.light_emission; 3]
            } else {
                [
                    (e * er / m).round().clamp(0.0, 15.0) as u8,
                    (e * eg / m).round().clamp(0.0, 15.0) as u8,
                    (e * eb / m).round().clamp(0.0, 15.0) as u8,
                ]
            };
            props.push(BlockProps {
                solid: d.solid,
                opaque: d.opaque,
                light_opacity: d.light_opacity,
                light_emission: d.light_emission,
                light_color,
                fluid: d.fluid.is_some(),
                conductivity: d.conductivity,
            });
        }
        Ok(Self { defs, props, by_id })
    }
}

/// The global block registry, parsed from the embedded `blocks.ron` on first use.
pub static BLOCKS: LazyLock<BlockRegistry> = LazyLock::new(|| {
    BlockRegistry::parse(DEFAULT_BLOCKS).expect("embedded data/blocks.ron must parse")
});

/// The built-in `format_version: 1` legacy palette (old ids 0..=10), for
/// migrating pre-registry saves on load.
pub static LEGACY_PALETTE: LazyLock<BlockPalette> = LazyLock::new(BlockPalette::legacy);

/// Hot-path properties for a block (cheap `Copy`; falls back for unknown ids).
#[inline]
pub fn props(id: BlockId) -> BlockProps {
    BLOCKS.props.get(id.0 as usize).copied().unwrap_or(DEFAULT_PROPS)
}

/// Full definition for a block, if the id is in range.
pub fn def(id: BlockId) -> Option<&'static BlockDef> {
    BLOCKS.defs.get(id.0 as usize)
}

/// Resolve a stable string id (`oc:stone`) to its runtime numeric id.
pub fn find_block(string_id: &str) -> Option<BlockId> {
    BLOCKS.by_id.get(string_id).copied()
}

/// The stable string id for a runtime numeric id.
pub fn string_id(id: BlockId) -> Option<&'static str> {
    BLOCKS.defs.get(id.0 as usize).map(|d| d.id.as_str())
}

/// True if a block can never be broken (`hardness < 0`, e.g. bedrock) — the
/// world floor survival players can never dig past. Unknown ids are breakable.
#[inline]
pub fn is_unbreakable(id: BlockId) -> bool {
    def(id).is_some_and(|d| d.hardness < 0.0)
}

/// The current registry's string ids in numeric order — the palette a freshly
/// created (or migrated) world is saved with.
pub fn palette_strings() -> Vec<String> {
    BLOCKS.defs.iter().map(|d| d.id.clone()).collect()
}

/// Per-world string↔numeric block table. On-disk column ids are *palette-local*
/// (indices into `strings`); on load they remap to runtime [`BlockId`]s via the
/// stable string ids, so a registry reorder or added mod blocks never corrupt an
/// existing save. For the base game the palette equals the registry order, so the
/// remap is the identity.
pub struct BlockPalette {
    strings: Vec<String>,
    to_runtime: Vec<BlockId>,
    from_runtime: HashMap<u16, u16>,
}

impl BlockPalette {
    /// Build a palette from its ordered string ids (disk index → string).
    pub fn from_strings(strings: Vec<String>) -> Self {
        let mut to_runtime = Vec::with_capacity(strings.len());
        let mut from_runtime = HashMap::with_capacity(strings.len());
        for (disk, s) in strings.iter().enumerate() {
            let rt = find_block(s).unwrap_or(BlockId::AIR);
            to_runtime.push(rt);
            from_runtime.entry(rt.0).or_insert(disk as u16);
        }
        Self { strings, to_runtime, from_runtime }
    }

    /// The palette matching the current runtime registry order (identity remap).
    pub fn current() -> Self {
        Self::from_strings(palette_strings())
    }

    /// The built-in `format_version: 1` legacy palette (old ids 0..=10).
    pub fn legacy() -> Self {
        Self::from_strings(LEGACY_IDS.iter().map(|s| s.to_string()).collect())
    }

    /// The ordered string ids, for persisting in the world header.
    pub fn strings(&self) -> &[String] {
        &self.strings
    }

    /// Remap an on-disk (palette-local) id to a runtime block.
    #[inline]
    pub fn decode_id(&self, disk: u16) -> BlockId {
        self.to_runtime.get(disk as usize).copied().unwrap_or(BlockId::AIR)
    }

    /// Remap a runtime block to its on-disk (palette-local) id. Blocks absent
    /// from the palette fall back to their runtime id (base-game identity).
    #[inline]
    pub fn encode_id(&self, id: BlockId) -> u16 {
        self.from_runtime.get(&id.0).copied().unwrap_or(id.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;

    #[test]
    fn blocks_ron_parses_in_legacy_order() {
        // The embedded blocks.ron must list the 11 base blocks in the exact
        // order the hardcoded `blocks::` consts assume.
        assert_eq!(find_block("oc:air"), Some(blocks::AIR));
        assert_eq!(find_block("oc:stone"), Some(blocks::STONE));
        assert_eq!(find_block("oc:dirt"), Some(blocks::DIRT));
        assert_eq!(find_block("oc:grass"), Some(blocks::GRASS));
        assert_eq!(find_block("oc:sand"), Some(blocks::SAND));
        assert_eq!(find_block("oc:water"), Some(blocks::WATER));
        assert_eq!(find_block("oc:log"), Some(blocks::LOG));
        assert_eq!(find_block("oc:leaves"), Some(blocks::LEAVES));
        assert_eq!(find_block("oc:lamp"), Some(blocks::LAMP));
        assert_eq!(find_block("oc:snow"), Some(blocks::SNOW));
        assert_eq!(find_block("oc:planks"), Some(blocks::PLANKS));
        // Deep-world content appends after the original 11.
        assert_eq!(find_block("oc:bedrock"), Some(blocks::BEDROCK));
        assert_eq!(find_block("oc:lava"), Some(blocks::LAVA));
    }

    #[test]
    fn properties_match_the_old_hardcoded_behavior() {
        // air: not solid, not opaque, lets light through at cost 1
        assert!(!blocks::AIR.is_solid());
        assert!(!blocks::AIR.is_opaque());
        assert_eq!(blocks::AIR.light_opacity(), Some(1));
        // water: not solid, not opaque, light cost 1
        assert!(!blocks::WATER.is_solid());
        assert!(!blocks::WATER.is_opaque());
        assert_eq!(blocks::WATER.light_opacity(), Some(1));
        // stone: solid, opaque, blocks light
        assert!(blocks::STONE.is_solid());
        assert!(blocks::STONE.is_opaque());
        assert_eq!(blocks::STONE.light_opacity(), None);
        // lamp emits 15, others emit 0
        assert_eq!(blocks::LAMP.light_emission(), 15);
        assert_eq!(blocks::STONE.light_emission(), 0);
    }

    #[test]
    fn legacy_palette_is_a_prefix_of_the_current_registry() {
        let legacy = BlockPalette::legacy();
        let current = BlockPalette::current();
        // v1 saves only knew the first 11 blocks. The registry only ever
        // *appends* (bedrock, lava, …), so the legacy palette must stay a
        // prefix of the current one — old disk ids 0..=10 decode/encode
        // identically, and added blocks never corrupt a pre-v2 save.
        assert_eq!(legacy.strings().len(), 11);
        assert!(current.strings().len() >= 11);
        assert_eq!(&current.strings()[..11], legacy.strings());
        for n in 0..11u16 {
            assert_eq!(legacy.decode_id(n), BlockId(n));
            assert_eq!(legacy.encode_id(BlockId(n)), n);
        }
    }

    #[test]
    fn bedrock_is_unbreakable_and_lava_is_an_opaque_nonsolid_emitter() {
        // Bedrock (hardness -1) can never be broken; everything else can.
        assert!(is_unbreakable(blocks::BEDROCK));
        assert!(!is_unbreakable(blocks::STONE));
        assert!(!is_unbreakable(blocks::LAVA));
        // Lava is opaque (you can't see through it → renders in the solid pass)
        // but not solid (you fall into it), and emits full-strength light.
        assert!(blocks::LAVA.is_opaque());
        assert!(!blocks::LAVA.is_solid());
        assert_eq!(blocks::LAVA.light_emission(), 15);
        // Its cast light carries the ~1200 °C blackbody hue: red full, no blue.
        let lc = blocks::LAVA.light_color();
        assert_eq!(lc[0], 15, "red reaches full emission: {lc:?}");
        assert_eq!(lc[2], 0, "no blue in lava's glow: {lc:?}");
        assert!(lc[1] < lc[0] && lc[1] > 0, "orange (some green): {lc:?}");
    }

    #[test]
    fn emissive_blocks_cast_tinted_light() {
        // The lamp emits 15 with a warm emissive (1.0, 0.87, 0.59): red floods
        // fully, green and blue progressively less — the hue of its cast light.
        let lc = blocks::LAMP.light_color();
        assert_eq!(lc[0], 15, "red reaches the full emission level");
        assert!(lc[1] < lc[0] && lc[1] >= 12, "green is slightly dimmer: {lc:?}");
        assert!(lc[2] < lc[1], "blue is the dimmest channel: {lc:?}");
        // Non-emitters cast no light.
        assert_eq!(blocks::STONE.light_color(), [0, 0, 0]);
    }
}
