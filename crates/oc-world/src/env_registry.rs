//! Data-driven environment / dimension registry — whole worlds as data.
//! Earth/overworld is the first entry; other planets/moons/asteroids are RON
//! files under `data/dimensions/`. Per dimension: gravity, the atmosphere
//! (sky colors + scattering + the underwater medium), the gas-mixture
//! composition + total pressure, an optional geothermal profile, and the
//! celestial bodies.
//!
//! Same loading contract as the block/fluid/gas registries: embedded default,
//! a `LazyLock` table, stable string ids (`oc:overworld`). The Scene UBO and
//! sky/gravity code read the active [`EnvDef`] instead of hardcoded constants.

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::{Context as _, Result};
use serde::Deserialize;

const DEFAULT_DIMENSIONS: &str = include_str!("../../../data/dimensions/overworld.ron");

/// Per-load numeric id for a dimension (an index into the registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DimensionId(pub u16);

/// Sky / atmosphere appearance + the underwater medium. Container-level
/// `#[serde(default)]` means a dimension overrides only the values that differ
/// from Earth (the [`Default`] below). Sky colors are transcribed from sky.rs.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Atmosphere {
    pub sky_day: (f32, f32, f32),
    pub sky_dusk: (f32, f32, f32),
    pub sky_night: (f32, f32, f32),
    pub zenith_day: (f32, f32, f32),
    pub zenith_night: (f32, f32, f32),
    /// Ambient floor at night, and the additional gain by full day.
    pub ambient_night: f32,
    pub ambient_day_gain: f32,
    /// Off-axis sun tilt so noon shadows aren't perfectly vertical.
    pub sun_tilt: f32,
    /// Submerged fog tint and its near/far visibility ramp (blocks).
    pub underwater_color: (f32, f32, f32),
    pub underwater_fog_near: f32,
    pub underwater_fog_far: f32,
    // --- reserved: physical scattering (Step 5) ---
    pub rayleigh: (f32, f32, f32),
    pub mie: f32,
    pub mie_g: f32,
}

impl Default for Atmosphere {
    fn default() -> Self {
        // Earth, transcribed 1:1 from oc-client/src/sky.rs.
        Self {
            sky_day: (0.47, 0.71, 0.99),
            sky_dusk: (0.82, 0.52, 0.31),
            sky_night: (0.012, 0.018, 0.05),
            zenith_day: (0.18, 0.42, 0.86),
            zenith_night: (0.004, 0.007, 0.022),
            ambient_night: 0.16,
            ambient_day_gain: 0.32,
            sun_tilt: 0.25,
            underwater_color: (0.09, 0.30, 0.55),
            underwater_fog_near: 24.0,
            underwater_fog_far: 72.0,
            rayleigh: (5.8, 13.5, 33.1),
            mie: 21.0,
            mie_g: 0.76,
        }
    }
}

/// The default gas *mixture* + total pressure the open-air reservoir relaxes
/// to. `mix` keys are [`crate::gas_registry`] string ids → mole fraction.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AtmosphereComposition {
    #[serde(default)]
    pub pressure_kpa: f32,
    #[serde(default)]
    pub mix: HashMap<String, f32>,
}

/// Optional geothermal profile (a cored planet has one; a small airless moon
/// omits it — uniformly cold). Feeds the Stage-G static base heat map.
#[derive(Debug, Clone, Deserialize)]
pub struct Thermal {
    pub surface_temp: f32,
    /// Kelvin per block of depth below the surface.
    pub geothermal_gradient: f32,
    pub core_temp: f32,
}

/// A sun / moon / star. Reserved for Step 4-5 (directional light + sky).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CelestialBody {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub light_color: (f32, f32, f32),
    #[serde(default)]
    pub lux: f32,
    #[serde(default)]
    pub angular_size: f32,
}

/// A data-driven dimension/world. Every field defaults so a dimension declares
/// only what differs from Earth.
#[derive(Debug, Clone, Deserialize)]
pub struct EnvDef {
    pub id: String,
    pub name: String,

    // --- physics ---
    #[serde(default = "default_gravity")]
    pub gravity: f32,
    #[serde(default = "default_terminal_fall")]
    pub terminal_fall_speed: f32,
    #[serde(default = "default_jump")]
    pub jump_speed: f32,

    // --- environment ---
    #[serde(default)]
    pub atmosphere: Atmosphere,
    #[serde(default)]
    pub atmosphere_composition: AtmosphereComposition,
    #[serde(default)]
    pub thermal: Option<Thermal>,
    #[serde(default)]
    pub celestial: Vec<CelestialBody>,
}

fn default_gravity() -> f32 {
    28.0
}
fn default_terminal_fall() -> f32 {
    60.0
}
fn default_jump() -> f32 {
    8.4
}

/// The loaded dimension registry: defs + the string→id lookup.
pub struct EnvRegistry {
    defs: Vec<EnvDef>,
    by_id: HashMap<String, DimensionId>,
}

impl EnvRegistry {
    fn parse(ron_text: &str) -> Result<Self> {
        // Each dimension file is one `EnvDef`; the embedded default is the
        // overworld. (A pack/world overlay adds more — the §7.5 stack.)
        let def: EnvDef = ron::from_str(ron_text).context("parsing a dimension .ron")?;
        let mut by_id = HashMap::with_capacity(1);
        by_id.insert(def.id.clone(), DimensionId(0));
        Ok(Self { defs: vec![def], by_id })
    }
}

/// The global dimension registry; the embedded overworld is parsed on first use.
pub static DIMENSIONS: LazyLock<EnvRegistry> = LazyLock::new(|| {
    EnvRegistry::parse(DEFAULT_DIMENSIONS).expect("embedded overworld.ron must parse")
});

/// Full definition for a dimension id, if in range.
pub fn def(id: DimensionId) -> Option<&'static EnvDef> {
    DIMENSIONS.defs.get(id.0 as usize)
}

/// Resolve a stable string id (`oc:overworld`) to its runtime numeric id.
pub fn find_dimension(string_id: &str) -> Option<DimensionId> {
    DIMENSIONS.by_id.get(string_id).copied()
}

/// The default/overworld dimension (id 0), always present.
pub fn overworld() -> &'static EnvDef {
    &DIMENSIONS.defs[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gas_registry::find_gas;

    #[test]
    fn overworld_parses_with_earth_values() {
        let env = overworld();
        assert_eq!(env.id, "oc:overworld");
        assert_eq!(env.gravity, 28.0);
        // Sky transcribed from sky.rs.
        assert_eq!(env.atmosphere.sky_day, (0.47, 0.71, 0.99));
        assert_eq!(env.atmosphere.sun_tilt, 0.25);
        // A cored planet has a geothermal profile.
        let thermal = env.thermal.as_ref().expect("overworld has thermal");
        assert!(thermal.core_temp > thermal.surface_temp);
    }

    #[test]
    fn atmosphere_composition_refs_resolve_to_gases() {
        let env = overworld();
        let comp = &env.atmosphere_composition;
        assert!(comp.pressure_kpa > 100.0, "Earth ~101 kPa: {}", comp.pressure_kpa);
        assert!(!comp.mix.is_empty(), "overworld has a gas mixture");
        // Every component string id resolves in the gas registry, and the
        // fractions sum to ~1.
        let mut sum = 0.0;
        for (gas_id, frac) in &comp.mix {
            assert!(find_gas(gas_id).is_some(), "unknown gas {gas_id} in mixture");
            sum += frac;
        }
        assert!((sum - 1.0).abs() < 0.05, "mole fractions should sum to ~1: {sum}");
    }
}
