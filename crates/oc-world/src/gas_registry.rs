//! Data-driven gas registry (the third state of matter; ARCHITECTURE §3 +
//! the world-building matter model). Each [`GasDef`] is a single atmospheric
//! *component* (O₂, N₂, CO₂, water vapour, …); air is a *mixture* of them
//! (see [`crate::env_registry`]'s `atmosphere_composition`).
//!
//! Same loading contract as [`crate::registry`] (blocks): authored in
//! `data/gases.ron`, embedded as the built-in default, a `LazyLock` table,
//! stable string ids (`oc:o2`) as the identity. Most fields are reserved
//! (parsed now, consumed by the §6.6 gas-volume sim later).

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::registry::PhaseTransition;

const DEFAULT_GASES: &str = include_str!("../../../data/gases.ron");

/// Per-load numeric id for a gas (an index into the registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GasId(pub u16);

/// A data-driven atmospheric component. Grouped by concern; every field
/// defaults so an entry declares only what differs. Shares the thermal trait
/// fragment with blocks and fluids (matter can change state between them).
#[derive(Debug, Clone, Deserialize)]
pub struct GasDef {
    pub id: String,
    pub name: String,

    // --- physical ---
    /// Molar mass proxy (g/mol): drives vertical layering / buoyancy — heavy
    /// CO₂ sinks, light H₂ rises. Air ≈ 29.
    #[serde(default)]
    pub density: f32,
    /// Sustains the player directly (O₂ yes; CO₂/toxic no).
    #[serde(default)]
    pub breathable: bool,

    // --- render ---
    #[serde(default)]
    pub color: (f32, f32, f32),
    #[serde(default)]
    pub fog_color: (f32, f32, f32),
    #[serde(default)]
    pub light_emission: u8,

    // --- flags ---
    #[serde(default)]
    pub flammable: bool,
    #[serde(default)]
    pub toxic: bool,

    // --- thermal (shared trait) ---
    #[serde(default)]
    pub heat_capacity: f32,
    #[serde(default)]
    pub conductivity: f32,
    /// Cross-registry product when this gas condenses/reacts (e.g. steam →
    /// `oc:water`). Resolved by string id across all matter registries.
    #[serde(default)]
    pub phase_transition: Option<PhaseTransition>,
}

/// The loaded gas registry: defs + the string→id lookup.
pub struct GasRegistry {
    defs: Vec<GasDef>,
    by_id: HashMap<String, GasId>,
}

impl GasRegistry {
    fn parse(ron_text: &str) -> Result<Self> {
        let defs: Vec<GasDef> = ron::from_str(ron_text).context("parsing gases.ron")?;
        let mut by_id = HashMap::with_capacity(defs.len());
        for (index, d) in defs.iter().enumerate() {
            if by_id.insert(d.id.clone(), GasId(index as u16)).is_some() {
                bail!("duplicate gas id {:?}", d.id);
            }
        }
        Ok(Self { defs, by_id })
    }
}

/// The global gas registry, parsed from the embedded `gases.ron` on first use.
pub static GASES: LazyLock<GasRegistry> = LazyLock::new(|| {
    GasRegistry::parse(DEFAULT_GASES).expect("embedded data/gases.ron must parse")
});

/// Full definition for a gas id, if in range.
pub fn def(id: GasId) -> Option<&'static GasDef> {
    GASES.defs.get(id.0 as usize)
}

/// Resolve a stable string id (`oc:o2`) to its runtime numeric id.
pub fn find_gas(string_id: &str) -> Option<GasId> {
    GASES.by_id.get(string_id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gases_ron_parses_and_resolves() {
        // The base components exist and resolve by string id.
        for id in ["oc:o2", "oc:n2", "oc:co2"] {
            assert!(find_gas(id).is_some(), "missing gas {id}");
        }
        // Oxygen is breathable; CO₂ is not and CO₂ is heavier than O₂.
        let o2 = def(find_gas("oc:o2").unwrap()).unwrap();
        let co2 = def(find_gas("oc:co2").unwrap()).unwrap();
        assert!(o2.breathable, "O2 must be breathable");
        assert!(!co2.breathable, "CO2 must not be breathable");
        assert!(co2.density > o2.density, "CO2 ({}) heavier than O2 ({})", co2.density, o2.density);
    }
}
