//! Data-driven environment / dimension registry — whole worlds as data.
//! Earth/overworld is the first entry; other planets/moons/asteroids are RON
//! files under `data/dimensions/`. Per dimension: gravity, the atmosphere
//! (sky colors + scattering + the underwater medium), the gas-mixture
//! composition + total pressure, a temperature-vs-height profile, and the
//! celestial bodies.
//!
//! Same loading contract as the block/fluid/gas registries: embedded default,
//! a `LazyLock` table, stable string ids (`oc:overworld`). The Scene UBO and
//! sky/gravity code read the active [`EnvDef`] instead of hardcoded constants.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU16, Ordering};

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

const OVERWORLD_RON: &str = include_str!("../../../data/dimensions/overworld.ron");
const MOON_RON: &str = include_str!("../../../data/dimensions/moon.ron");
/// Built-in dimensions in id order; id 0 is always the overworld. A pack/world
/// filesystem overlay can add more later (the §7.5 stack).
const BUILTIN_DIMENSIONS: [&str; 2] = [OVERWORLD_RON, MOON_RON];

/// The process's active dimension (index into the registry). The server sets
/// it on world load (from LevelMeta); the client sets it from the Welcome
/// message. Read by the sky / gravity / physics code via [`active`].
static ACTIVE: AtomicU16 = AtomicU16::new(0);

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
    /// Unconditional minimum light on every surface — nothing renders pure
    /// black (the always-on base brightness; sealed caves and night stay
    /// dimly visible). The plan's `ambient_floor`.
    pub ambient_floor: f32,
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
    // --- volumetric god-rays / ground mist (graphics stage 3) ---
    /// Game-scale atmospheric scattering strength per block: the renderer scales
    /// the `rayleigh`/`mie` coefficient *ratios* by this to get visible in-scatter
    /// over block (not km) distances. 0 disables the effect (e.g. an airless moon).
    pub fog_density: f32,
    /// Ground-mist band top, world Y: density is full at/below this height and
    /// fades above it, so mist pools in low ground / valleys (sea level is 0).
    pub fog_altitude: f32,
    /// Mist fade scale height in blocks (how fast density falls off above the band).
    pub fog_thickness: f32,
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
            ambient_floor: 0.045,
            sun_tilt: 0.25,
            underwater_color: (0.09, 0.30, 0.55),
            underwater_fog_near: 24.0,
            underwater_fog_far: 72.0,
            rayleigh: (5.8, 13.5, 33.1),
            mie: 21.0,
            mie_g: 0.76,
            fog_density: 0.03,
            fog_altitude: 12.0,
            fog_thickness: 16.0,
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

/// One point on a dimension's temperature-vs-height curve: degrees Celsius at a
/// world Y. Sea level is `y = 0`.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TempPoint {
    pub y: f32,
    pub temp: f32,
}

/// A dimension's static (tier-1) temperature profile: ambient °C as a function
/// of height, **authored as keypoints**, piecewise-linear between them and
/// **clamped beyond both ends** (the deepest point is the held "core"; a high
/// cold point is the lapse). The shape is data, not code: a uniform-conductivity
/// world is a straight line, but a layered world — an insulating shell over a
/// hot core, or a hot-cold-hot sandwich — is just a different set of points. A
/// single point is a uniform world (e.g. an airless moon). Points may be in any
/// order; the registry sorts them by `y` on load. Feeds the Stage-G heat map.
#[derive(Debug, Clone, Deserialize)]
pub struct Thermal {
    pub profile: Vec<TempPoint>,
}

impl Default for Thermal {
    /// A dimension that omits `thermal` gets a mild uniform 14 °C.
    fn default() -> Self {
        Self { profile: vec![TempPoint { y: 0.0, temp: 14.0 }] }
    }
}

/// The deep vertical layers of a dimension: ordered bands below the normal
/// terrain where worldgen carves big caverns and fills them with a chosen block
/// (air for hostile caves, lava for a lava lake). Modular per world — a volcanic
/// planet raises the lava, an airless moon has none (default empty → just rock
/// to the bedrock floor).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeepLayers {
    pub bands: Vec<DeepBand>,
}

/// One deep band: caverns are carved from `top` downward (to the next band's
/// top, or the bedrock floor for the last) and filled with `fill` (a stable
/// block id, e.g. `oc:air` or `oc:lava`); the un-carved rock stays stone.
#[derive(Debug, Clone, Deserialize)]
pub struct DeepBand {
    /// Top world-Y of the band (bands are sorted descending on load).
    pub top: i32,
    /// Block id filling the carved caverns (`oc:air`, `oc:lava`, …).
    pub fill: String,
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
    /// The temperature-vs-height curve. Defaults to a mild uniform 14 °C; an
    /// airless world is just a single cold point (no sentinel needed).
    #[serde(default)]
    pub thermal: Thermal,
    /// Deep vertical layers (hellish caves, lava lake). Empty = none (rock to
    /// the bedrock floor). Read by the terrain generator.
    #[serde(default)]
    pub layers: DeepLayers,
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
    fn load() -> Result<Self> {
        // Each dimension file is one `EnvDef`; id 0 is the overworld.
        let mut defs = Vec::with_capacity(BUILTIN_DIMENSIONS.len());
        let mut by_id = HashMap::with_capacity(BUILTIN_DIMENSIONS.len());
        for ron_text in BUILTIN_DIMENSIONS {
            let mut def: EnvDef = ron::from_str(ron_text).context("parsing a dimension .ron")?;
            if by_id.insert(def.id.clone(), DimensionId(defs.len() as u16)).is_some() {
                bail!("duplicate dimension id {:?}", def.id);
            }
            // The temperature curve is sampled assuming ascending Y; authors may
            // list points in any order, so sort once at load.
            def.thermal
                .profile
                .sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
            if def.thermal.profile.is_empty() {
                def.thermal = Thermal::default();
            }
            defs.push(def);
        }
        Ok(Self { defs, by_id })
    }
}

/// The global dimension registry; the built-in dimensions parse on first use.
pub static DIMENSIONS: LazyLock<EnvRegistry> = LazyLock::new(|| {
    EnvRegistry::load().expect("embedded dimension RON must parse")
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

/// Set the process's active dimension (server: from LevelMeta on world load;
/// client: from the Welcome message). Out-of-range ids are ignored.
pub fn set_active(id: DimensionId) {
    if (id.0 as usize) < DIMENSIONS.defs.len() {
        ACTIVE.store(id.0, Ordering::Relaxed);
    }
}

/// Set the active dimension by stable string id; returns false if unknown
/// (the active dimension is left unchanged).
pub fn set_active_by_id(string_id: &str) -> bool {
    match find_dimension(string_id) {
        Some(id) => {
            set_active(id);
            true
        }
        None => false,
    }
}

/// The process's active dimension (defaults to the overworld). Sky, gravity
/// and physics read their constants from this.
pub fn active() -> &'static EnvDef {
    let i = ACTIVE.load(Ordering::Relaxed) as usize;
    DIMENSIONS.defs.get(i).unwrap_or(&DIMENSIONS.defs[0])
}

/// The stable string id of the active dimension.
pub fn active_id() -> &'static str {
    &active().id
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
        // A cored planet warms with depth: the deepest profile point is hotter
        // than sea level (the curve is sorted ascending by Y on load).
        let p = &env.thermal.profile;
        assert!(p.len() >= 2, "overworld has a multi-point curve");
        assert!(p.first().unwrap().temp > p.last().unwrap().temp, "deep is hotter than high");
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

    #[test]
    fn second_dimension_differs_proving_worlds_are_data() {
        let moon = def(find_dimension("oc:moon").expect("moon dimension exists")).unwrap();
        let earth = overworld();
        assert!(moon.gravity < earth.gravity, "moon is lower-gravity: {} vs {}", moon.gravity, earth.gravity);
        // An airless moon is uniformly cold: a single-point curve (no warming
        // with depth, unlike Earth's multi-point profile).
        assert_eq!(moon.thermal.profile.len(), 1, "moon is uniform-temperature");
        assert!(moon.thermal.profile[0].temp < 0.0, "moon is cold");
        assert!(moon.atmosphere.sky_day != earth.atmosphere.sky_day, "moon sky differs from Earth");
        assert!(moon.atmosphere_composition.mix.is_empty(), "the moon is a vacuum");
        // The active-dimension switch resolves and reads back the moon.
        assert!(set_active_by_id("oc:moon"));
        assert_eq!(active().id, "oc:moon");
        set_active(DimensionId(0)); // restore so other tests see the overworld
        assert_eq!(active().id, "oc:overworld");
    }
}
