//! Data-driven fluid registry (the liquid state of matter). Water is just the
//! first entry; oil, lava, milk, mud, blood … are data, never code.
//!
//! Same loading contract as [`crate::registry`] (blocks): authored in
//! `data/fluids.ron`, embedded default, a `LazyLock` table, stable string ids
//! (`oc:water`). Generalises the hardcoded water special-cases (rendering
//! absorption/fog, buoyancy/swim, breathing) into [`FluidDef`] queries.

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::registry::PhaseTransition;

const DEFAULT_FLUIDS: &str = include_str!("../../../data/fluids.ron");

/// Per-load numeric id for a fluid (an index into the registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FluidId(pub u16);

/// A data-driven fluid definition. Grouped by concern; every field defaults so
/// an entry declares only what differs. Opts into the optical-surface,
/// volumetric-medium, respiration, mass/flow and thermal trait fragments.
#[derive(Debug, Clone, Deserialize)]
pub struct FluidDef {
    pub id: String,
    pub name: String,

    // --- render: volumetric medium + optical surface ---
    /// Body/fog tint (linear rgb 0..1).
    #[serde(default)]
    pub color: (f32, f32, f32),
    /// 0 = clear, 1 = opaque (milk); the source of truth for the render layer.
    #[serde(default)]
    pub opacity: f32,
    /// Beer–Lambert per-channel absorption (water ≈ 30:3:1, red dies first).
    #[serde(default)]
    pub absorption: (f32, f32, f32),
    /// Distance (blocks) at which the medium's fog saturates.
    #[serde(default)]
    pub fog_distance: f32,
    /// Index of refraction (water 1.333 → Fresnel F0 0.02).
    #[serde(default = "default_ior")]
    pub ior: f32,
    /// Self-glow (HDR rgb; lava emits). Drives cast light + bloom.
    #[serde(default)]
    pub emissive: (f32, f32, f32),
    #[serde(default = "one")]
    pub roughness: f32,
    #[serde(default)]
    pub metalness: f32,
    /// Light attenuation cost per block for the baked light field, or `None`
    /// if the fluid blocks light entirely.
    #[serde(default = "default_light_opacity")]
    pub light_opacity: Option<u8>,
    /// Block-light level emitted (lava).
    #[serde(default)]
    pub light_emission: u8,

    // --- physics: mass + flow + buoyancy/swim ---
    /// Mass per unit volume (relative; water = 1.0). Drives buoyancy.
    #[serde(default = "one")]
    pub density: f32,
    /// Flow resistance (higher = sluggish, like lava/honey).
    #[serde(default)]
    pub viscosity: f32,
    /// Downward acceleration while submerged (blocks/s²; water 10.0).
    #[serde(default)]
    pub submerged_gravity: f32,
    /// Terminal sink speed when not swimming (blocks/s).
    #[serde(default)]
    pub sink_speed: f32,
    /// Upward swim speed (blocks/s).
    #[serde(default)]
    pub swim_up_speed: f32,
    /// Horizontal move speed multiplier while submerged.
    #[serde(default = "one")]
    pub swim_speed_factor: f32,

    // --- respiration ---
    /// Breathe directly without gear (0 = drown; water 0).
    #[serde(default)]
    pub breathability: u8,
    /// Gear/machine-extractable O₂ content (0..15; water high).
    #[serde(default)]
    pub oxygen_content: u8,

    // --- thermal (shared trait) ---
    /// Intrinsic operating temperature (°C), if the fluid holds one (lava
    /// ~1200). Drives the heat hazard when the player is in it; `None` means it
    /// sits at the ambient temperature (water).
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub heat_capacity: f32,
    #[serde(default)]
    pub conductivity: f32,
    #[serde(default)]
    pub melting_point: Option<f32>,
    #[serde(default)]
    pub boiling_point: Option<f32>,
    #[serde(default)]
    pub ignitable: bool,
    /// Cross-registry products: lava →[freeze]→ obsidian/basalt, water
    /// →[freeze]→ ice, →[boil]→ steam. Resolved by string id across registries.
    #[serde(default)]
    pub phase_transition: Option<PhaseTransition>,
}

fn one() -> f32 {
    1.0
}
fn default_ior() -> f32 {
    1.0
}
fn default_light_opacity() -> Option<u8> {
    Some(1)
}

/// The loaded fluid registry: defs + the string→id lookup.
pub struct FluidRegistry {
    defs: Vec<FluidDef>,
    by_id: HashMap<String, FluidId>,
}

impl FluidRegistry {
    fn parse(ron_text: &str) -> Result<Self> {
        let defs: Vec<FluidDef> = ron::from_str(ron_text).context("parsing fluids.ron")?;
        let mut by_id = HashMap::with_capacity(defs.len());
        for (index, d) in defs.iter().enumerate() {
            if by_id.insert(d.id.clone(), FluidId(index as u16)).is_some() {
                bail!("duplicate fluid id {:?}", d.id);
            }
        }
        Ok(Self { defs, by_id })
    }
}

/// The global fluid registry, parsed from the embedded `fluids.ron` on first use.
pub static FLUIDS: LazyLock<FluidRegistry> = LazyLock::new(|| {
    FluidRegistry::parse(DEFAULT_FLUIDS).expect("embedded data/fluids.ron must parse")
});

/// Full definition for a fluid id, if in range.
pub fn def(id: FluidId) -> Option<&'static FluidDef> {
    FLUIDS.defs.get(id.0 as usize)
}

/// Resolve a stable string id (`oc:water`) to its runtime numeric id.
pub fn find_fluid(string_id: &str) -> Option<FluidId> {
    FLUIDS.by_id.get(string_id).copied()
}

/// The fluid a block embodies, if any (via `BlockDef.fluid`): `oc:water` →
/// the water def, `oc:lava` → the lava def, ordinary solids → None. The
/// generalised replacement for `== blocks::WATER` checks.
pub fn for_block(block: crate::BlockId) -> Option<&'static FluidDef> {
    let id = crate::registry::def(block)?.fluid.as_deref()?;
    def(find_fluid(id)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fluids_ron_parses_and_water_is_first() {
        let water = find_fluid("oc:water").expect("water exists");
        let w = def(water).unwrap();
        // Swim constants transcribed verbatim from player.rs (used in F3).
        assert_eq!(w.submerged_gravity, 10.0);
        assert_eq!(w.sink_speed, 3.5);
        assert_eq!(w.swim_up_speed, 4.5);
        assert!((w.swim_speed_factor - 0.55).abs() < 1e-6);
        // Water drowns (breathability 0) but holds extractable oxygen.
        assert_eq!(w.breathability, 0);
        assert!(w.oxygen_content > 0);
        // Beer–Lambert: blue survives far longer than red (30:3:1).
        assert!(w.absorption.0 > w.absorption.2 * 10.0, "red dies first: {:?}", w.absorption);
    }
}
