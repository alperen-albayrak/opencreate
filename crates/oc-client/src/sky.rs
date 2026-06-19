//! Day/night cycle (§4.7): sun direction, ambient level, and sky color as a
//! pure function of the time of day. The server will own world time once it
//! exists; until then the client advances it locally.

use glam::{Vec3, Vec4};
use oc_world::env_registry;

/// One full day, in real seconds — the server owns the value; the client
/// only uses it to advance smoothly between Time broadcasts.
pub use oc_server::DAY_LENGTH_SECS;

/// `(f32, f32, f32)` data tuple -> `Vec3` (the RON atmosphere colors).
fn v3(t: (f32, f32, f32)) -> Vec3 {
    Vec3::new(t.0, t.1, t.2)
}

/// What the sky and sun look like at a moment of the day.
#[derive(Debug, Clone, Copy)]
pub struct SkyState {
    /// xyz: direction toward the sun pre-scaled by daylight (terrain and
    /// water lighting); w: ambient light level.
    pub sun: Vec4,
    /// xyz: unscaled sun direction; w: daylight 0..1 (sky dome).
    pub sun_dir: [f32; 4],
    /// Horizon color on the sun's side — also fog and the water's
    /// environment.
    pub sky_color: [f32; 4],
    /// Horizon color opposite the sun: at dusk the east darkens while
    /// the west still glows, like the real sky.
    pub horizon_away: [f32; 4],
    /// Overhead color (deeper blue by day, near-black at night).
    pub zenith: [f32; 4],
    /// Cloud slab color (rgb) and opacity (a).
    pub clouds: [f32; 4],
    /// Where the sky's spin stands, radians (stars rotate with the sun).
    pub angle: f32,
    /// Moon phase in eighths: 0 new, 0.5 full.
    pub moon_phase: f32,
    /// Star visibility, 0 by day to 1 at night.
    pub stars: f32,
}

/// Computes the sky for a cumulative `day` (whole part = day count for
/// the moon phase; fraction = time of day): .0 = sunrise at the horizon,
/// .25 = noon, .5 = sunset, .75 = midnight. Colors come from the active
/// dimension's [`Atmosphere`] (data), not hardcoded constants.
pub fn sky_at(day: f64) -> SkyState {
    let atm = &env_registry::active().atmosphere;
    let day_sky = v3(atm.sky_day);
    let dusk_sky = v3(atm.sky_dusk);
    let night_sky = v3(atm.sky_night);
    let day_zenith = v3(atm.zenith_day);
    let night_zenith = v3(atm.zenith_night);

    let day_count = day.floor();
    let day_fraction = day - day_count;
    let angle = day_fraction as f32 * std::f32::consts::TAU;
    // Sun travels an east-west arc, slightly tilted off the axis so noon
    // shadows aren't perfectly vertical.
    let elevation = angle.sin();
    let sun_dir = Vec3::new(angle.cos(), elevation, atm.sun_tilt).normalize();

    // Daylight ramps in around the horizon (smoothstep over elevation).
    let daylight = smoothstep(-0.06, 0.22, elevation);
    // A warm dusk band when the sun is near the horizon.
    let dusk = smoothstep(-0.25, -0.02, elevation) * (1.0 - smoothstep(0.02, 0.35, elevation));

    let sky = night_sky.lerp(day_sky, daylight).lerp(dusk_sky, dusk * 0.85);
    // Opposite the sun the day fades sooner and barely warms: dusk and
    // dawn sweep across the sky instead of dimming it evenly.
    let away = night_sky
        .lerp(day_sky, daylight * daylight)
        .lerp(dusk_sky * 0.30, dusk * 0.25);
    // The zenith keeps its blue while the horizon warms at dusk.
    let zenith = night_zenith.lerp(day_zenith, daylight).lerp(dusk_sky * 0.35, dusk * 0.3);
    // Never fully dark: moonlight floor at night.
    let ambient = atm.ambient_night + atm.ambient_day_gain * daylight;

    // Clouds: white by day, warm-tinted at dusk, dim at night.
    let cloud = Vec3::splat(0.06 + 0.94 * daylight).lerp(dusk_sky * 1.05, dusk * 0.55);

    // Day 0 opens on a full moon; one phase step per day, 8 per cycle.
    let moon_phase = ((day_count as i64 + 4).rem_euclid(8)) as f32 / 8.0;

    SkyState {
        sun: (sun_dir * daylight).extend(ambient),
        sun_dir: [sun_dir.x, sun_dir.y, sun_dir.z, daylight],
        sky_color: [sky.x, sky.y, sky.z, 1.0],
        horizon_away: [away.x, away.y, away.z, 1.0],
        zenith: [zenith.x, zenith.y, zenith.z, 1.0],
        clouds: [cloud.x, cloud.y, cloud.z, 0.82],
        angle,
        moon_phase,
        stars: (1.0 - daylight * 1.25 - dusk * 0.2).clamp(0.0, 1.0),
    }
}

/// Underwater visibility ramps like Minecraft's (dense on the dive,
/// clearing as the eyes adjust; Java settles past 90 blocks, Bedrock
/// at 60 — we sit between, scaled to our render distances).
pub fn underwater_fog_distance(submerged_secs: f32) -> f32 {
    let atm = &env_registry::active().atmosphere;
    let t = (submerged_secs / 12.0).clamp(0.0, 1.0);
    atm.underwater_fog_near
        + (atm.underwater_fog_far - atm.underwater_fog_near) * (t * t * (3.0 - 2.0 * t))
}

/// Submerged camera: the fluid's color swallows the sky and horizon. The tint
/// is the fluid's own `color` (per-fluid: water blue, lava orange, oil yellow…).
/// A sun-lit fluid (water) dims with daylight so dusk dives stay readable; a
/// self-lit fluid (lava, which glows) keeps full brightness day or night.
pub fn submerged(state: &SkyState, color: Vec3, self_lit: bool) -> SkyState {
    let daylight = Vec3::new(state.sun.x, state.sun.y, state.sun.z).length();
    let tint = if self_lit { color } else { color * (0.22 + 0.78 * daylight) };
    SkyState {
        sky_color: [tint.x, tint.y, tint.z, 1.0],
        horizon_away: [tint.x, tint.y, tint.z, 1.0],
        zenith: [tint.x * 0.7, tint.y * 0.75, tint.z * 0.85, 1.0],
        stars: 0.0, // the fog owns the view down here
        ..*state
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
    fn dusk_darkens_opposite_the_sun_first() {
        let sunset = sky_at(0.5);
        let toward: f32 = sunset.sky_color[..3].iter().sum();
        let away: f32 = sunset.horizon_away[..3].iter().sum();
        assert!(
            away < toward * 0.7,
            "the anti-sun horizon should darken first: away {away} vs toward {toward}"
        );
        let noon = sky_at(0.25);
        let toward: f32 = noon.sky_color[..3].iter().sum();
        let away: f32 = noon.horizon_away[..3].iter().sum();
        assert!((toward - away).abs() < 0.2, "by day the horizons should match");
    }

    #[test]
    fn moon_steps_through_phases_and_stars_come_out() {
        assert_eq!(sky_at(0.75).moon_phase, 0.5, "day 0 should open on a full moon");
        assert_eq!(sky_at(1.75).moon_phase, 0.625);
        assert_eq!(sky_at(4.75).moon_phase, 0.0, "new moon 4 days in");
        assert_eq!(sky_at(8.75).moon_phase, 0.5, "the cycle is 8 days");
        assert!(sky_at(0.75).stars > 0.9, "stars out at midnight");
        assert_eq!(sky_at(0.25).stars, 0.0, "no stars at noon");
        // The time of day still works on cumulative days.
        assert!(sky_at(5.25).sun.y > 0.8, "noon sun overhead on day 5");
    }

    #[test]
    fn submerged_water_is_blue_fog_and_tracks_daylight() {
        let blue = Vec3::new(0.09, 0.30, 0.55); // oc:water's color
        let noon = submerged(&sky_at(0.25), blue, false);
        assert!(
            noon.sky_color[2] > noon.sky_color[0] * 3.0,
            "underwater fog should be deep blue: {:?}",
            noon.sky_color
        );
        // Sun-lit fluid dims at night.
        let night = submerged(&sky_at(0.75), blue, false);
        assert!(
            night.sky_color[2] < noon.sky_color[2] * 0.3,
            "night dives should be dark: {:?} vs {:?}",
            night.sky_color,
            noon.sky_color
        );
        // The visibility ramp: dense on the dive, clear once adjusted.
        assert!(underwater_fog_distance(0.0) < 30.0);
        assert!(underwater_fog_distance(20.0) > 70.0);
    }

    #[test]
    fn submerged_self_lit_fluid_stays_bright_at_night() {
        let orange = Vec3::new(0.70, 0.20, 0.02); // oc:lava's color
        let day = submerged(&sky_at(0.25), orange, true);
        let night = submerged(&sky_at(0.75), orange, true);
        // Lava glows: full brightness regardless of the sun, and red-dominant.
        assert_eq!(day.sky_color, night.sky_color, "self-lit fluid ignores daylight");
        assert!(day.sky_color[0] > day.sky_color[2] * 3.0, "lava fog is orange: {:?}", day.sky_color);
    }

    #[test]
    fn cycle_is_continuous_at_wraparound() {
        let end = sky_at(0.999);
        let start = sky_at(0.001);
        assert!((end.sun.w - start.sun.w).abs() < 0.05);
        assert!((end.sky_color[2] - start.sky_color[2]).abs() < 0.1);
    }
}
