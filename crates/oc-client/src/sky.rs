//! Day/night cycle (§4.7): sun direction, ambient level, and sky color as a
//! pure function of the time of day. The server will own world time once it
//! exists; until then the client advances it locally.

use glam::{Vec3, Vec4};

/// One full day, in real seconds.
pub const DAY_LENGTH_SECS: f64 = 600.0;

/// What the sky and sun look like at a moment of the day.
#[derive(Debug, Clone, Copy)]
pub struct SkyState {
    /// xyz: direction toward the sun (normalized); w: ambient light level.
    pub sun: Vec4,
    /// Horizon color — also the fog color and the water's environment.
    pub sky_color: [f32; 4],
    /// Overhead color (deeper blue by day, near-black at night).
    pub zenith: [f32; 4],
    /// Cloud slab color (rgb) and opacity (a).
    pub clouds: [f32; 4],
}

const DAY_SKY: Vec3 = Vec3::new(0.47, 0.71, 0.99);
const DUSK_SKY: Vec3 = Vec3::new(0.82, 0.52, 0.31);
const NIGHT_SKY: Vec3 = Vec3::new(0.012, 0.018, 0.05);
const DAY_ZENITH: Vec3 = Vec3::new(0.18, 0.42, 0.86);
const NIGHT_ZENITH: Vec3 = Vec3::new(0.004, 0.007, 0.022);

/// Computes the sky for `day_fraction` in [0, 1): 0.0 = sunrise at the
/// horizon, 0.25 = noon, 0.5 = sunset, 0.75 = midnight.
pub fn sky_at(day_fraction: f64) -> SkyState {
    let angle = day_fraction as f32 * std::f32::consts::TAU;
    // Sun travels an east-west arc, slightly tilted off the axis so noon
    // shadows aren't perfectly vertical.
    let elevation = angle.sin();
    let sun_dir = Vec3::new(angle.cos(), elevation, 0.25).normalize();

    // Daylight ramps in around the horizon (smoothstep over elevation).
    let daylight = smoothstep(-0.06, 0.22, elevation);
    // A warm dusk band when the sun is near the horizon.
    let dusk = smoothstep(-0.25, -0.02, elevation) * (1.0 - smoothstep(0.02, 0.35, elevation));

    let sky = NIGHT_SKY.lerp(DAY_SKY, daylight).lerp(DUSK_SKY, dusk * 0.85);
    // The zenith keeps its blue while the horizon warms at dusk.
    let zenith = NIGHT_ZENITH.lerp(DAY_ZENITH, daylight).lerp(DUSK_SKY * 0.35, dusk * 0.3);
    // Never fully dark: moonlight floor at night.
    let ambient = 0.16 + 0.32 * daylight;

    // Clouds: white by day, warm-tinted at dusk, dim at night.
    let cloud = Vec3::splat(0.06 + 0.94 * daylight).lerp(DUSK_SKY * 1.05, dusk * 0.55);

    SkyState {
        sun: (sun_dir * daylight).extend(ambient),
        sky_color: [sky.x, sky.y, sky.z, 1.0],
        zenith: [zenith.x, zenith.y, zenith.z, 1.0],
        clouds: [cloud.x, cloud.y, cloud.z, 0.82],
    }
}

/// How far you can see underwater before the fog saturates, in blocks.
pub const UNDERWATER_FOG_DISTANCE: f32 = 40.0;

/// Submerged camera: dense blue fog swallows the sky and the horizon —
/// both fade to deep water blue, scaled by daylight so night dives are
/// dark. The sun keeps shining through as a bright glow overhead.
pub fn underwater(state: &SkyState) -> SkyState {
    let daylight = Vec3::new(state.sun.x, state.sun.y, state.sun.z).length();
    let water = Vec3::new(0.05, 0.22, 0.42) * (0.04 + 0.96 * daylight);
    SkyState {
        sun: state.sun,
        sky_color: [water.x, water.y, water.z, 1.0],
        zenith: [water.x * 0.7, water.y * 0.75, water.z * 0.85, 1.0],
        clouds: state.clouds,
    }
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_is_bright_and_blue() {
        let sky = sky_at(0.25);
        assert!(sky.sun.w > 0.4, "noon ambient too dark: {}", sky.sun.w);
        assert!(sky.sun.y > 0.8, "noon sun should be overhead: {}", sky.sun.y);
        assert!(sky.sky_color[2] > 0.8, "noon sky should be blue");
    }

    #[test]
    fn midnight_is_dark_with_no_sun() {
        let sky = sky_at(0.75);
        assert!(sky.sun.w < 0.2, "midnight ambient too bright: {}", sky.sun.w);
        let sun_strength = Vec3::new(sky.sun.x, sky.sun.y, sky.sun.z).length();
        assert!(sun_strength < 0.01, "sun should be off at midnight: {sun_strength}");
        assert!(sky.sky_color[2] < 0.1, "midnight sky should be near-black");
    }

    #[test]
    fn sunset_is_warm() {
        let sky = sky_at(0.5);
        assert!(
            sky.sky_color[0] > sky.sky_color[2],
            "sunset should be warmer than blue: {:?}",
            sky.sky_color
        );
    }

    #[test]
    fn underwater_is_blue_fog_and_tracks_daylight() {
        let noon = underwater(&sky_at(0.25));
        assert!(
            noon.sky_color[2] > noon.sky_color[0] * 3.0,
            "underwater fog should be deep blue: {:?}",
            noon.sky_color
        );
        let night = underwater(&sky_at(0.75));
        assert!(
            night.sky_color[2] < noon.sky_color[2] * 0.2,
            "night dives should be dark: {:?} vs {:?}",
            night.sky_color,
            noon.sky_color
        );
    }

    #[test]
    fn cycle_is_continuous_at_wraparound() {
        let end = sky_at(0.999);
        let start = sky_at(0.001);
        assert!((end.sun.w - start.sun.w).abs() < 0.05);
        assert!((end.sky_color[2] - start.sky_color[2]).abs() < 0.1);
    }
}
