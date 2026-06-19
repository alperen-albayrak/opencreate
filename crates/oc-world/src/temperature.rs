//! Effective temperature (docs/world-building/temperature.md), three tiers:
//!  1. **static base** — a pure function of position + the dimension's
//!     geothermal profile (`EnvDef.thermal`): deep = hot toward the core,
//!     high = cold; an airless body (no profile) is uniformly cold.
//!  2. source delta — a sparse flood-fill from heat sources (later).
//!  3. stored — per-block heat on actively-heated cells (later).
//!
//! `effective T = base(pos) + source_delta(pos) [+ stored]`. Only tier 1 is
//! built here. Temperatures are degrees Celsius (the blackbody glow converts
//! to Kelvin via [`to_kelvin`]).
//!
//! The geothermal gradient is **gameplay-scaled** (°C per block of depth), not
//! real rock's ~0.025 K/m — so the deepest playable rock actually reaches the
//! Draper point (~525 °C) and glows, instead of needing kilometres of depth.

use oc_core::BlockPos;

use crate::env_registry::EnvDef;
use crate::terrain::SEA_LEVEL;

/// Draper point in Celsius (~798 K): matter hotter than this glows visibly,
/// so it both self-emits (the lighting-pass blackbody glow) and casts warm
/// light (seeded into the block-light flood-fill).
pub const DRAPER_C: f32 = 525.0;
/// Uniform temperature of an airless body with no geothermal profile (cold).
pub const AIRLESS_TEMP_C: f32 = -50.0;
/// Cooling per block of altitude above sea level (TFC-style lapse rate).
pub const ELEVATION_LAPSE_C: f32 = 0.16;

/// Tier-1 static base temperature (°C) at a world position: a pure function of
/// position and the dimension's geothermal profile. Cheap and queryable
/// anywhere; never stored (recomputed like terrain, so it freezes offline for
/// free). This is the static height/depth-based environmental heat map.
pub fn base(pos: BlockPos, env: &EnvDef) -> f32 {
    match &env.thermal {
        // Airless / coreless body: uniformly cold, no deep-hot zone.
        None => AIRLESS_TEMP_C,
        Some(t) => {
            let depth = (SEA_LEVEL - pos.y).max(0) as f32;
            let altitude = (pos.y - SEA_LEVEL).max(0) as f32;
            // Geothermal heating with depth (clamped to the core); a gentle
            // lapse-rate cooling with altitude.
            let geo = t.surface_temp + t.geothermal_gradient * depth;
            (geo - ELEVATION_LAPSE_C * altitude).min(t.core_temp)
        }
    }
}

/// The effective temperature (°C) at a position. Currently the tier-1 base;
/// tier-2 source deltas and tier-3 stored heat add into this as they land.
pub fn effective(pos: BlockPos, env: &EnvDef) -> f32 {
    base(pos, env)
}

/// Celsius → Kelvin, for the blackbody glow (physical.rs works in Kelvin).
pub fn to_kelvin(celsius: f32) -> f32 {
    celsius + 273.15
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env_registry;
    use glam::IVec3;

    #[test]
    fn deep_is_hot_surface_is_mild_high_is_cold() {
        let env = env_registry::overworld();
        let thermal = env.thermal.as_ref().unwrap();
        let surface = base(IVec3::new(0, SEA_LEVEL, 0), env);
        let deep = base(IVec3::new(0, -64, 0), env);
        let high = base(IVec3::new(0, SEA_LEVEL + 120, 0), env);
        assert!((surface - thermal.surface_temp).abs() < 0.01, "sea level ≈ surface: {surface}");
        // The deep rock must reach glowing temperatures (past the Draper point).
        assert!(deep > 300.0, "the deep is hot enough to glow: {deep}");
        assert!(deep <= thermal.core_temp, "clamped to the core: {deep}");
        assert!(high < surface, "high altitude is colder: {high} vs {surface}");
    }

    #[test]
    fn airless_world_is_uniformly_cold() {
        let moon = env_registry::def(env_registry::find_dimension("oc:moon").unwrap()).unwrap();
        assert_eq!(base(IVec3::new(0, -64, 0), moon), AIRLESS_TEMP_C);
        assert_eq!(base(IVec3::new(0, 100, 0), moon), AIRLESS_TEMP_C);
    }
}
