// Volumetric god-rays (graphics roadmap VV stage 3.1): a fullscreen raymarch of
// the view ray that accumulates sun-lit in-scattering sampled against the sun
// shadow cascades, then additively blends it into the lit HDR color. Shadowed
// air contributes nothing, so visible shafts form where the ray crosses sunlit
// air near a shadow edge; a Henyey-Greenstein phase makes it peak toward the
// sun (crepuscular rays). Runs in the lighting render pass, after the lighting
// resolve — depth is a sampled input here, not an attachment.

// Mirror of the shared Scene UBO prefix (we only read sky_sun). Must match
// scene.rs SceneData field order.
struct Scene {
    sun: vec4<f32>,
    fog: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_away: vec4<f32>,
    sky_sun: vec4<f32>,
    params: vec4<f32>,
    thermal_profile: array<vec4<f32>, 4>,
}
@group(1) @binding(0) var<uniform> scene: Scene;

@group(0) @binding(0) var gbuf_depth: texture_depth_2d;

// Sun shadow cascades (same set 2 the lighting pass binds).
struct ShadowData {
    matrices: array<mat4x4<f32>, 3>,
    splits: vec4<f32>,
    params: vec4<f32>,
}
@group(2) @binding(0) var<uniform> shadow: ShadowData;
@group(2) @binding(1) var shadow_map: texture_depth_2d_array;
@group(2) @binding(2) var shadow_sampler: sampler_comparison;

struct VolPush {
    // depth -> camera-relative world (same as the lighting pass).
    inv_view_proj: mat4x4<f32>,
    // x: unused, y: Mie g, z: step count, w: max march distance.
    fog_a: vec4<f32>,
    // rgb: Rayleigh scattering coefficient per block (bluish), w: Mie coefficient.
    fog_b: vec4<f32>,
    // x: ground-mist altitude (world Y), y: mist fade thickness, z: intensity; w: reserved.
    fog_c: vec4<f32>,
}
var<immediate> pc: VolPush;

// Must match camera.rs / pbr.wgsl.
const NEAR: f32 = 0.05;
const FAR: f32 = 4096.0;
const FOUR_PI: f32 = 12.5663706;
// Rayleigh phase normalisation: 3 / (16π).
const RAYLEIGH_K: f32 = 0.0596831;

fn henyey_greenstein(cos_t: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (FOUR_PI * pow(max(1.0 + g2 - 2.0 * g * cos_t, 1e-4), 1.5));
}

// Sun visibility at a camera-relative world point (1 = sunlit, 0 = shadowed).
// No normal offset (this is air, not a surface); a small constant bias only.
fn sun_vis_at(world_rel: vec3<f32>, dist: f32) -> f32 {
    if (shadow.params.x <= 0.0 || dist >= shadow.splits.z) {
        return 1.0;
    }
    var cascade = 2;
    if (dist < shadow.splits.x) {
        cascade = 0;
    } else if (dist < shadow.splits.y) {
        cascade = 1;
    }
    let ndc = shadow.matrices[cascade] * vec4<f32>(world_rel, 1.0);
    let uv = ndc.xy * 0.5 + vec2(0.5);
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0))) {
        return 1.0;
    }
    return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, cascade, ndc.z - 0.0005);
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(gbuf_depth));
    let px = vec2<i32>(frag.xy);
    let depth = textureLoad(gbuf_depth, px, 0);
    // Sky pixels: the sky dome (drawn later) owns them; nothing to add here.
    if (depth >= 1.0) {
        return vec4<f32>(0.0);
    }

    // Camera-relative world position of the surface this ray hit.
    let ndc = vec3<f32>((frag.xy / dims) * 2.0 - 1.0, depth);
    let wh = pc.inv_view_proj * vec4<f32>(ndc, 1.0);
    let world_rel = wh.xyz / wh.w;
    let dist = length(world_rel);
    let ray_dir = world_rel / max(dist, 1e-4);

    // March camera -> surface, capped (god-rays are a near/mid effect; the
    // cascades only cover ~200 blocks anyway).
    let max_d = min(dist, pc.fog_a.w);
    let steps = i32(pc.fog_a.z);
    let step_len = max_d / f32(steps);
    // Per-pixel jitter breaks up step banding.
    let jitter = fract(sin(dot(frag.xy, vec2<f32>(12.9898, 78.233))) * 43758.5453);

    let sun_dir = normalize(scene.sky_sun.xyz);
    let daylight = scene.sky_sun.w;
    let cos_t = dot(ray_dir, sun_dir);
    // Two-term single scattering (what Vibrant Visuals does — Rayleigh + Mie/HG,
    // not a flat isotropic floor): Rayleigh's phase is near-isotropic and its
    // coefficient is blue-biased, so it gives the broad haze visible in EVERY
    // view direction; Mie's HG phase is the forward lobe → the god-ray shafts
    // toward the sun. The colour is the scattering coefficients, not a tint.
    let phase_r = RAYLEIGH_K * (1.0 + cos_t * cos_t);
    let phase_m = henyey_greenstein(cos_t, pc.fog_a.y);
    let scatter = pc.fog_b.rgb * phase_r + vec3<f32>(pc.fog_b.w * phase_m);
    let extinction = pc.fog_b.g + pc.fog_b.w; // representative grey extinction/block

    // March the view ray. A height-density ramp pools fog in low ground (full
    // at/below fog_altitude, fading above; sample world Y = camera Y params.z +
    // local offset). Sunlit air scatters; shadowed air doesn't (the shafts).
    // Beer-Lambert transmittance back to the camera weights near samples more
    // and keeps long rays from blowing out.
    var weight = 0.0;
    var tau = 0.0;
    for (var i = 0; i < steps; i = i + 1) {
        let t = (f32(i) + jitter) * step_len;
        let p = ray_dir * t;
        let world_y = scene.params.z + p.y;
        let height = exp(-max(world_y - pc.fog_c.x, 0.0) / max(pc.fog_c.y, 0.1));
        weight += exp(-tau) * sun_vis_at(p, t) * height * step_len;
        tau += extinction * height * step_len;
    }
    let amount = scatter * weight * daylight * pc.fog_c.z;
    return vec4<f32>(amount, 0.0);
}
