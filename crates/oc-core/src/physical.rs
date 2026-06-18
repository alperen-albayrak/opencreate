//! Physically-grounded models and baseline constants (the "natural look"
//! grounding from the world-building roadmap). Pure, unit-tested functions live
//! here; features call them so the whole world stays calibrated and consistent.
//! Per-fluid / per-dimension *values* come from the data registries — these are
//! the defaults and the math.
//!
//! Lives in `oc-core` (no deps) so both the renderer **and** the future
//! temperature simulation can share the same blackbody / absorption math.

use std::f32::consts::PI;

use glam::Vec3;

// ---------------------------------------------------------------------------
// Baseline constants
// ---------------------------------------------------------------------------

/// Photographic middle-grey the auto-exposure steers toward (key value).
pub const EXPOSURE_KEY: f32 = 0.18;

/// Draper point: matter begins to glow visibly (dull red) above this
/// temperature. ≈ 798 K (525 °C).
pub const DRAPER_POINT_K: f32 = 798.0;

/// Index of refraction of water.
pub const WATER_IOR: f32 = 1.333;

/// Schlick base reflectance `F0` for an air→water interface (from `WATER_IOR`).
pub const WATER_F0: f32 = 0.02;

/// Pure-water absorption coefficients per channel (1/m), R:G:B ≈ 30:3:1 — red
/// dies within ~2 m, blue survives ~70 m. The physical baseline; the water
/// shader scales these so blue's 1/e depth ≈ render distance.
pub const WATER_ABSORPTION: Vec3 = Vec3::new(0.45, 0.05, 0.014);

/// Rayleigh scattering coefficients per channel (1/m, ×10⁻⁶ baked in) — the
/// ∝ 1/λ⁴ blue-sky tint.
pub const RAYLEIGH_BETA: Vec3 = Vec3::new(5.8e-6, 13.5e-6, 33.1e-6);

/// Mie scattering coefficient (1/m) — aerosol haze / sun glow.
pub const MIE_BETA: f32 = 21e-6;

/// Henyey–Greenstein asymmetry for Mie forward-scatter (sun glow, fog shafts).
pub const MIE_G: f32 = 0.76;

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// Beer–Lambert transmittance through `distance` (m) of a medium with the given
/// `absorption` coefficient (1/m): `exp(-a·d)`, in `0..=1`.
#[inline]
pub fn beer_lambert(absorption: f32, distance: f32) -> f32 {
    (-absorption * distance).exp()
}

/// Per-channel Beer–Lambert transmittance (e.g. [`WATER_ABSORPTION`]).
#[inline]
pub fn beer_lambert_rgb(absorption: Vec3, distance: f32) -> Vec3 {
    Vec3::new(
        beer_lambert(absorption.x, distance),
        beer_lambert(absorption.y, distance),
        beer_lambert(absorption.z, distance),
    )
}

/// Fresnel reflectance via Schlick's approximation: `F0` at normal incidence
/// rising to 1.0 at grazing. `cos_theta` is the angle between view and normal.
#[inline]
pub fn fresnel_schlick(cos_theta: f32, f0: f32) -> f32 {
    let c = (1.0 - cos_theta).clamp(0.0, 1.0);
    f0 + (1.0 - f0) * c.powi(5)
}

/// Critical angle (radians) for total internal reflection going from a denser
/// medium `n_from` into a thinner one `n_to`. `None` when `n_from <= n_to` (no
/// TIR). Water→air gives Snell's window ≈ 48.6°.
#[inline]
pub fn critical_angle(n_from: f32, n_to: f32) -> Option<f32> {
    if n_from > n_to {
        Some((n_to / n_from).asin())
    } else {
        None
    }
}

/// Henyey–Greenstein phase function for scattering asymmetry `g` (−1..1; 0 =
/// isotropic, →1 = forward). `cos_theta` is the scatter angle cosine.
#[inline]
pub fn henyey_greenstein(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    (1.0 - g2) / (4.0 * PI * (1.0 + g2 - 2.0 * g * cos_theta).powf(1.5))
}

/// Approximate blackbody color for a temperature (K), as normalised sRGB in
/// `0..=1` (the glow *hue*; callers scale by intensity for HDR emissive).
/// Tanner-Helland piecewise fit; clamped to 1000–40000 K.
pub fn blackbody_rgb(temp_k: f32) -> Vec3 {
    let t = (temp_k.clamp(1000.0, 40000.0) / 100.0) as f64;
    let red = if t <= 66.0 {
        255.0
    } else {
        (329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)).clamp(0.0, 255.0)
    };
    let green = if t <= 66.0 {
        (99.470_802_586_1 * t.ln() - 161.119_568_166_1).clamp(0.0, 255.0)
    } else {
        (288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)).clamp(0.0, 255.0)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        (138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7).clamp(0.0, 255.0)
    };
    Vec3::new(red as f32, green as f32, blue as f32) / 255.0
}

/// Relative brightness (flux) of a star from its apparent magnitude, Pogson's
/// ratio: each +5 magnitudes is ×1/100, magnitude 0 = 1.0.
#[inline]
pub fn magnitude_to_brightness(mag: f32) -> f32 {
    10.0_f32.powf(-0.4 * mag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn beer_lambert_one_optical_depth() {
        // a·d = 1 → transmittance 1/e.
        assert!(close(beer_lambert(0.5, 2.0), 1.0 / std::f32::consts::E, 1e-4));
        assert!(close(beer_lambert(1.0, 0.0), 1.0, 1e-6));
    }

    #[test]
    fn water_absorption_is_roughly_30_3_1() {
        let a = WATER_ABSORPTION;
        assert!(close(a.x / a.z, 32.0, 2.0), "red/blue ratio {}", a.x / a.z);
        assert!(close(a.y / a.z, 3.6, 0.5), "green/blue ratio {}", a.y / a.z);
        // Blue penetrates far deeper than red.
        assert!(beer_lambert(a.z, 10.0) > beer_lambert(a.x, 10.0));
    }

    #[test]
    fn fresnel_endpoints() {
        assert!(close(fresnel_schlick(1.0, WATER_F0), WATER_F0, 1e-6)); // head-on
        assert!(close(fresnel_schlick(0.0, WATER_F0), 1.0, 1e-6)); // grazing → mirror
    }

    #[test]
    fn snell_window_is_48_6_degrees() {
        let theta = critical_angle(WATER_IOR, 1.0).expect("water→air has TIR");
        assert!(close(theta.to_degrees(), 48.6, 0.1), "{} deg", theta.to_degrees());
        // Going the other way (air→water) has no critical angle.
        assert!(critical_angle(1.0, WATER_IOR).is_none());
    }

    #[test]
    fn henyey_greenstein_isotropic_when_g_zero() {
        let iso = 1.0 / (4.0 * PI);
        assert!(close(henyey_greenstein(0.5, 0.0), iso, 1e-6));
        assert!(close(henyey_greenstein(-1.0, 0.0), iso, 1e-6));
        // Forward scatter (g>0) peaks ahead (cosθ=1) vs behind (cosθ=-1).
        assert!(henyey_greenstein(1.0, MIE_G) > henyey_greenstein(-1.0, MIE_G));
    }

    #[test]
    fn blackbody_hue_shifts_red_to_white() {
        let cool = blackbody_rgb(1000.0); // deep orange-red
        assert!(cool.x > cool.z, "1000 K should be red-dominant: {cool:?}");
        let neutral = blackbody_rgb(6600.0); // ~daylight white
        assert!(neutral.x > 0.9 && neutral.y > 0.9 && neutral.z > 0.9, "{neutral:?}");
    }

    #[test]
    fn pogson_magnitude_scale() {
        assert!(close(magnitude_to_brightness(0.0), 1.0, 1e-6));
        assert!(close(magnitude_to_brightness(5.0), 0.01, 1e-4)); // +5 mag = ×1/100
    }
}
