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
//! The base is a per-dimension **temperature-vs-height curve** (`EnvDef.thermal`,
//! authored keypoints, piecewise-linear, clamped beyond the ends). A uniform-
//! conductivity world is a straight line; a layered world (insulating shell over
//! a hot core, a hot-cold-hot sandwich) is just different points — the shape is
//! data, not code. The lethal/glowing deep heat does **not** come from this
//! gentle base; it comes from real hot matter (lava) via the source delta.

use oc_core::BlockPos;

use crate::env_registry::{EnvDef, TempPoint};

/// Draper point in Celsius (~798 K): matter hotter than this glows visibly, so
/// it both self-emits and casts warm light (seeded into the block-light flood-
/// fill). Glow is a property of hot *matter*, not of the ambient base field.
pub const DRAPER_C: f32 = 525.0;

/// Tier-1 static base temperature (°C) at a world position: the dimension's
/// temperature-vs-height curve sampled at `pos.y`. A pure function of position;
/// never stored (recomputed like terrain, so it freezes offline for free). This
/// is the static height-based environmental heat map.
pub fn base(pos: BlockPos, env: &EnvDef) -> f32 {
    sample_profile(&env.thermal.profile, pos.y as f32)
}

/// Sample a temperature curve (points sorted ascending by Y) at world `y`:
/// piecewise-linear between points, clamped to the end values beyond the
/// outermost points (the deepest point is the held "core").
fn sample_profile(profile: &[TempPoint], y: f32) -> f32 {
    match profile {
        [] => 14.0, // no data: a mild default (registry fills this in practice)
        [only] => only.temp,
        _ => {
            let lo = &profile[0];
            let hi = &profile[profile.len() - 1];
            if y <= lo.y {
                return lo.temp;
            }
            if y >= hi.y {
                return hi.temp;
            }
            for w in profile.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                if y >= a.y && y <= b.y {
                    let t = if b.y > a.y { (y - a.y) / (b.y - a.y) } else { 0.0 };
                    return a.temp + t * (b.temp - a.temp);
                }
            }
            hi.temp // unreachable (y is within [lo.y, hi.y])
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
    use crate::terrain::SEA_LEVEL;
    use glam::IVec3;

    #[test]
    fn temperature_follows_the_curve_and_clamps_at_the_ends() {
        let env = env_registry::overworld();
        let surface = base(IVec3::new(0, SEA_LEVEL, 0), env);
        // The survivable band ends at 50 °C; the curve is authored so that's
        // reached ~200 blocks down (the rock above is safe to mine).
        let hazard_depth = base(IVec3::new(0, SEA_LEVEL - 200, 0), env);
        // Below the deepest point the curve holds flat (no runaway extrapolation
        // toward a glowing core — the deep heat comes from lava, not the base).
        let deepest = base(IVec3::new(0, -368, 0), env);
        let below_floor = base(IVec3::new(0, SEA_LEVEL - 100_000, 0), env);
        let high = base(IVec3::new(0, SEA_LEVEL + 120, 0), env);
        assert!((surface - 14.0).abs() < 0.01, "sea level ≈ 14 °C: {surface}");
        assert!((hazard_depth - 50.0).abs() < 1.0, "~50 °C at 200 deep: {hazard_depth}");
        assert!(hazard_depth > surface, "deeper is hotter");
        assert_eq!(below_floor, deepest, "clamps to the deepest point below it");
        assert!(deepest < 100.0, "the base stays gentle (lava does the heat): {deepest}");
        assert!(high < surface, "high altitude is colder: {high} vs {surface}");
    }

    #[test]
    fn a_single_point_curve_is_a_uniform_world() {
        // The airless moon is one cold point: the same temperature at every Y.
        let moon = env_registry::def(env_registry::find_dimension("oc:moon").unwrap()).unwrap();
        let deep = base(IVec3::new(0, -64, 0), moon);
        let high = base(IVec3::new(0, 100, 0), moon);
        assert_eq!(deep, high, "uniform at all heights");
        assert!(deep < 0.0, "the airless moon is cold: {deep}");
    }
}
