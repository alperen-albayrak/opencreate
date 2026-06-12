// Sky dome (graphics roadmap stage C): drawn after the opaques at the far
// plane, so only empty pixels shade. Directional horizon gradient (the
// sunset side keeps its light while the opposite horizon darkens first),
// sun disc with glow, moon with phases and a soft halo, and a star field:
// a small real bright-star catalog (Orion, the Dippers, Cassiopeia, the
// Southern Cross...) over a procedural backdrop, all rotating with the
// day. HDR values — the discs overshoot 1.0; the tonemap eats them.

struct PushConstants {
    // Inverse of the camera-relative view-projection (no translation):
    // unprojects a clip position into a world-space view direction.
    inv_view_proj: mat4x4<f32>,
    // xyz: direction toward the sun (unscaled); w: daylight 0..1.
    sun: vec4<f32>,
    // rgb: horizon color on the sun's side; w: celestial angle, radians.
    horizon: vec4<f32>,
    // rgb: horizon color opposite the sun; w: moon phase 0..1.
    away: vec4<f32>,
    // rgb: zenith color; w: star visibility 0..1.
    zenith: vec4<f32>,
}

var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var out: VsOut;
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    let ndc = uv * 2.0 - 1.0;
    // z = w: the dome sits exactly on the far plane (depth 1.0), so the
    // LESS_EQUAL test keeps it behind everything that drew geometry.
    out.position = vec4<f32>(ndc, 1.0, 1.0);
    out.ndc = ndc;
    return out;
}

// The ~46 brightest stars that draw the famous shapes, as unit vectors
// in celestial coordinates (computed from real RA/Dec; w = brightness).
// Dec +90 maps to +Z, so the celestial pole sits on the north horizon —
// the sky turns about the same axis the sun travels.
const STAR_COUNT: u32 = 46u;
const STARS = array<vec4<f32>, 46>(
    vec4<f32>(0.0210, 0.9914, 0.1289, 0.96),
    vec4<f32>(0.1951, 0.9703, -0.1427, 1.02),
    vec4<f32>(0.1508, 0.9824, 0.1106, 0.69),
    vec4<f32>(0.0526, 0.9844, -0.1680, 0.60),
    vec4<f32>(0.0839, 0.9959, -0.0339, 0.66),
    vec4<f32>(0.1035, 0.9944, -0.0210, 0.68),
    vec4<f32>(0.1220, 0.9925, -0.0052, 0.56),
    vec4<f32>(-0.4591, 0.1151, 0.8809, 0.66),
    vec4<f32>(-0.5359, 0.1390, 0.8327, 0.53),
    vec4<f32>(-0.5919, 0.0160, 0.8059, 0.51),
    vec4<f32>(-0.5429, -0.0366, 0.8390, 0.45),
    vec4<f32>(-0.5443, -0.1307, 0.8286, 0.66),
    vec4<f32>(-0.5365, -0.2058, 0.8184, 0.55),
    vec4<f32>(-0.5815, -0.2948, 0.7583, 0.64),
    vec4<f32>(0.5124, 0.0205, 0.8585, 0.55),
    vec4<f32>(0.5428, 0.0969, 0.8342, 0.56),
    vec4<f32>(0.4742, 0.1198, 0.8722, 0.51),
    vec4<f32>(0.4621, 0.1815, 0.8681, 0.46),
    vec4<f32>(0.3894, 0.2124, 0.8963, 0.45),
    vec4<f32>(0.0101, 0.0079, 0.9999, 0.61),
    vec4<f32>(-0.1873, 0.9392, -0.2876, 1.30),
    vec4<f32>(-0.4181, 0.9038, 0.0911, 0.98),
    vec4<f32>(-0.0632, 0.6027, -0.7954, 1.21),
    vec4<f32>(0.1252, -0.7694, 0.6264, 1.04),
    vec4<f32>(0.4556, -0.5362, 0.7106, 0.78),
    vec4<f32>(0.4591, -0.8749, 0.1542, 0.88),
    vec4<f32>(0.4439, -0.6208, 0.6462, 0.56),
    vec4<f32>(0.3406, -0.8150, 0.4689, 0.45),
    vec4<f32>(-0.3448, -0.8264, -0.4451, 0.84),
    vec4<f32>(-0.0917, -0.7923, -0.6033, 0.69),
    vec4<f32>(-0.4494, -0.0524, -0.8918, 0.88),
    vec4<f32>(-0.4938, -0.1043, -0.8633, 0.78),
    vec4<f32>(-0.5380, -0.0736, -0.8397, 0.69),
    vec4<f32>(-0.5177, -0.0342, -0.8549, 0.45),
    vec4<f32>(-0.3739, -0.3126, -0.8732, 1.11),
    vec4<f32>(-0.4239, -0.2543, -0.8693, 0.92),
    vec4<f32>(-0.8644, 0.4580, 0.2073, 0.75),
    vec4<f32>(-0.9667, 0.0461, 0.2516, 0.58),
    vec4<f32>(0.3438, 0.8950, 0.2842, 0.86),
    vec4<f32>(0.1305, 0.6823, 0.7193, 1.03),
    vec4<f32>(-0.3407, 0.7777, 0.5283, 0.69),
    vec4<f32>(-0.3915, 0.7912, 0.4699, 0.80),
    vec4<f32>(-0.7838, -0.5270, 0.3286, 1.06),
    vec4<f32>(-0.9141, -0.3564, -0.1936, 0.84),
    vec4<f32>(0.8373, -0.2336, -0.4943, 0.79),
    vec4<f32>(0.4927, 0.2239, -0.8409, 0.95),
);

fn hash31(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453);
}

fn hash33(p: vec3<f32>) -> vec3<f32> {
    return fract(
        sin(vec3(
            dot(p, vec3(127.1, 311.7, 74.7)),
            dot(p, vec3(269.5, 183.3, 246.1)),
            dot(p, vec3(113.5, 271.9, 124.6)),
        )) * 43758.5453,
    );
}

// Procedural faint stars: a hash grid over the celestial sphere. Star
// points stay near their cell's center so a single lookup never clips.
fn faint_stars(cel: vec3<f32>) -> f32 {
    let n = 26.0;
    let cell = floor(cel * n);
    let h = hash33(cell);
    if (h.x > 0.30) {
        return 0.0; // most cells hold no star
    }
    let center = (cell + 0.5 + (h.yzx - 0.5) * 0.5) / n;
    let d = distance(cel, normalize(center));
    let core = 1.0 - smoothstep(0.0008, 0.0035, d);
    return core * (0.25 + 0.6 * h.y);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let world = pc.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(world.xyz / world.w);

    let daylight = pc.sun.w;
    let sun_dir = pc.sun.xyz;

    // The horizon color depends on where you look: toward the low sun it
    // keeps the warm light, opposite it the dark rises first — dusk and
    // dawn sweep across the sky instead of falling straight down.
    var toward = 0.5;
    if (length(sun_dir.xz) > 1e-4 && length(dir.xz) > 1e-4) {
        let f = dot(normalize(dir.xz), normalize(sun_dir.xz)) * 0.5 + 0.5;
        toward = f * f;
    }
    let horizon = mix(pc.away.rgb, pc.horizon.rgb, toward);

    // Horizon-to-zenith gradient; below the horizon, settle darker.
    let up = clamp(dir.y, -1.0, 1.0);
    var color = mix(horizon, pc.zenith.rgb, pow(max(up, 0.0), 0.65));
    color = mix(color, horizon * 0.35, smoothstep(0.0, -0.3, up));

    // Stars fade in at dusk and sit behind everything else. They live on
    // the rotating celestial sphere: undo the day's spin about +Z.
    let stars_vis = pc.zenith.w;
    if (stars_vis > 0.001 && up > 0.0) {
        let a = -pc.horizon.w;
        let c = cos(a);
        let s = sin(a);
        let cel = vec3(dir.x * c - dir.y * s, dir.x * s + dir.y * c, dir.z);
        var star = faint_stars(cel);
        for (var i = 0u; i < STAR_COUNT; i++) {
            let st = STARS[i];
            let d = distance(cel, st.xyz);
            star += (1.0 - smoothstep(0.001, 0.004, d)) * st.w * 1.4;
        }
        // Melt near the horizon like the real sky does.
        color += vec3(0.9, 0.95, 1.0) * star * stars_vis * smoothstep(0.0, 0.18, up);
    }

    if (daylight > 0.005) {
        let cos_sun = dot(dir, sun_dir);
        // Warm glow widens and strengthens when the sun rides low.
        let low_sun = 1.0 - smoothstep(0.05, 0.45, sun_dir.y);
        let glow = pow(max(cos_sun, 0.0), 24.0) * (0.18 + 0.55 * low_sun);
        color += vec3(1.0, 0.55, 0.25) * glow * daylight;
        // The disc itself: ~1 degree across, HDR-bright.
        let disc = smoothstep(0.99975, 0.99989, cos_sun);
        color += vec3(4.5, 3.9, 3.0) * disc * daylight;
    }

    // The moon rides opposite the sun; its lit shape steps through the
    // phase cycle, and a soft halo lightens the sky around it.
    let night = 1.0 - daylight;
    if (night > 0.01) {
        let moon_dir = -sun_dir;
        let cos_moon = dot(dir, moon_dir);
        if (cos_moon > 0.0) {
            // Disc-local frame, scaled so the moon radius is 1.
            let radius = 0.024;
            let t1 = normalize(cross(moon_dir, vec3(0.0, 0.0, 1.0)));
            let t2 = cross(moon_dir, t1);
            let p = vec2(dot(dir, t1), dot(dir, t2)) / radius;
            let r = length(p);
            // Phase = how far the shadow disc has slid off the lit one:
            // 0 new, 0.5 full, then back. 8 steps over the cycle.
            let slide = sin(pc.away.w * 3.14159265) * 2.4;
            let shadow = length(p - vec2(slide, 0.0));
            let lit = (1.0 - smoothstep(0.92, 1.0, r)) * smoothstep(1.0, 1.15, shadow);
            color += vec3(1.05, 1.02, 0.92) * lit * night;
            // Halo: the sky brightens near the moon, fading with distance.
            let illum = 0.15 + 0.85 * sin(pc.away.w * 3.14159265);
            let halo = pow(max(cos_moon, 0.0), 350.0) * 0.22 * illum;
            color += vec3(0.55, 0.6, 0.7) * halo * night;
        }
    }

    return vec4<f32>(color, 1.0);
}
