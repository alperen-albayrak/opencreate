// Deferred lighting pass (graphics roadmap Stage E): a fullscreen triangle
// reads the G-buffer written by chunk_gbuffer.wgsl plus the per-frame Scene
// UBO and resolves the opaque world's lit color into the HDR target.
//
// This reproduces the old forward chunk shading exactly — sky ambient + sun
// diffuse (shadow-darkenable) vs. RGB block light, AO on the indirect terms,
// caustics already folded into the G-buffer albedo, distance fog last — but
// now per-pixel from the G-buffer, the seam every later lighting feature
// (Cook-Torrance specular, SSAO, many point lights) plugs into.

struct Scene {
    // xyz: direction toward the sun (scaled by daylight); w: ambient level.
    sun: vec4<f32>,
    // rgb: distance-fog (horizon) color; w: fog saturation distance, blocks.
    fog: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_away: vec4<f32>,
    sky_sun: vec4<f32>,
    // x: time; y: base ambient floor; z: camera world Y; w: thermal point count.
    params: vec4<f32>,
    // Temperature-vs-height curve, ascending by Y, two points per vec4 as
    // (y0, temp0 °C, y1, temp1) — up to 8 points; `params.w` is the count.
    thermal_profile: array<vec4<f32>, 4>,
}
@group(1) @binding(0) var<uniform> scene: Scene;

@group(0) @binding(0) var gb0: texture_2d<f32>;
@group(0) @binding(1) var gb1: texture_2d<f32>;
@group(0) @binding(2) var gb2: texture_2d<f32>;
@group(0) @binding(3) var gbuf_depth: texture_depth_2d;

// Sun shadow cascades (set 2) — the same dormant plumbing the forward chunk
// shader used. `shadow.params.x` (strength) is 0 while shadows are shelved, so
// `sun_visibility` returns 1.0 and this is a no-op until they're toggled on.
struct ShadowData {
    // Camera-relative world -> cascade clip.
    matrices: array<mat4x4<f32>, 3>,
    // Cascade far distances; w: shadow style (0 = soft PCF, 1 = blocky).
    splits: vec4<f32>,
    // x: strength (0 = off/night); yzw: world units per texel, per cascade.
    params: vec4<f32>,
}
@group(2) @binding(0) var<uniform> shadow: ShadowData;
@group(2) @binding(1) var shadow_map: texture_depth_2d_array;
@group(2) @binding(2) var shadow_sampler: sampler_comparison;

// Rebuilds the camera-relative world position from the G-buffer depth for the
// cascade lookup (the inverse of the view-projection the chunks rendered with).
struct LightPush {
    inv_view_proj: mat4x4<f32>,
}
var<immediate> pc: LightPush;

// Must match the projection in camera.rs.
const NEAR: f32 = 0.05;
const FAR: f32 = 4096.0;
// Must match shadow.rs MAP_SIZE.
const SHADOW_MAP_SIZE: f32 = 2048.0;

fn linearize(depth: f32) -> f32 {
    return NEAR * FAR / (FAR - depth * (FAR - NEAR));
}

// Inverse of chunk_gbuffer.wgsl's oct_encode.
fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    let f = e * 2.0 - 1.0;
    var n = vec3<f32>(f.x, f.y, 1.0 - abs(f.x) - abs(f.y));
    let t = max(-n.z, 0.0);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return normalize(n);
}

// PCF visibility from one cascade (ported verbatim from the forward chunk
// shader): depth-biased by the cascade's texel size, one bilinear comparison.
fn cascade_lit(cascade: i32, world_rel: vec3<f32>, normal: vec3<f32>, n_dot_l: f32) -> f32 {
    let texel_world = shadow.params[1u + u32(cascade)];
    // Normal-offset bias is the primary acne lever: push the sample off the
    // surface along the exact (flat, noise-free) face normal, scaled by the
    // cascade's world-per-texel and by the grazing factor — the offset a flat
    // surface needs grows as the sun nears the horizon. Stays < 1 block on
    // every cascade, so shadows never detach (peter-pan).
    let grazing = clamp(1.0 - n_dot_l, 0.0, 1.0);
    // Modest normal offset: enough to keep flat ground off its own surface
    // (the near-cascade self-shadow that read as a camera-following seam),
    // small enough that small caster shadows (a tree, a placed block) don't
    // erode away. Texel-scaled so each cascade self-shadows consistently.
    let offset = texel_world * (1.0 + 2.0 * grazing);
    var pos = world_rel + normal * offset;
    // Blocky style (VV): snap the sample to the 1/16-block grid (VV
    // texel_size = 16) so the shadow edge aligns to block texels.
    let blocky = shadow.splits.w > 0.5;
    if (blocky) {
        pos = floor(pos * 16.0) / 16.0;
    }
    let ndc = shadow.matrices[cascade] * vec4<f32>(pos, 1.0);
    let uv = ndc.xy * 0.5 + vec2(0.5);
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0))) {
        return 1.0;
    }
    // Minimal residual depth bias — the normal offset does the heavy lifting.
    let d = ndc.z - (0.00015 + texel_world / 400.0);
    if (blocky) {
        return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, cascade, d);
    }
    // Soft PCF (VV default): four bilinear-comparison taps spread one texel
    // across and averaged — the LINEAR comparison sampler makes each a 2×2, so
    // this is a smooth ~3×3 kernel, cheap on the fullscreen lighting pass.
    let t = 1.0 / SHADOW_MAP_SIZE;
    var s = textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(-0.5, -0.5) * t, cascade, d);
    s += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(0.5, -0.5) * t, cascade, d);
    s += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(-0.5, 0.5) * t, cascade, d);
    s += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(0.5, 0.5) * t, cascade, d);
    return s * 0.25;
}

// How much of the sun reaches this fragment, with cross-faded cascades that
// ease out at the far cascade and at twilight.
fn sun_visibility(world_rel: vec3<f32>, normal: vec3<f32>, view_dist: f32, n_dot_l: f32) -> f32 {
    let strength = shadow.params.x;
    if (strength <= 0.0 || view_dist >= shadow.splits.z) {
        return 1.0;
    }
    var cascade = 2;
    var split = shadow.splits.z;
    if (view_dist < shadow.splits.x) {
        cascade = 0;
        split = shadow.splits.x;
    } else if (view_dist < shadow.splits.y) {
        cascade = 1;
        split = shadow.splits.y;
    }
    var lit = cascade_lit(cascade, world_rel, normal, n_dot_l);
    let blend = smoothstep(split * 0.85, split, view_dist);
    if (blend > 0.0 && cascade < 2) {
        lit = mix(lit, cascade_lit(cascade + 1, world_rel, normal, n_dot_l), blend);
    }
    let range_fade = smoothstep(shadow.splits.z * 0.8, shadow.splits.z, view_dist);
    return mix(1.0, mix(lit, 1.0, range_fade), strength);
}

// Accurate blackbody colour (normalised sRGB) from temperature — the
// Tanner-Helland fit, identical to oc_core::physical::blackbody_rgb. Continuous
// and granular, not stepped: dull red ~800 K, orange ~1500 K, yellow ~1900 K,
// toward white above ~6000 K.
fn blackbody_rgb(temp_k: f32) -> vec3<f32> {
    let t = clamp(temp_k, 1000.0, 40000.0) / 100.0;
    var r: f32;
    var g: f32;
    var b: f32;
    if (t <= 66.0) {
        r = 255.0;
        g = clamp(99.4708025861 * log(t) - 161.1195681661, 0.0, 255.0);
    } else {
        r = clamp(329.698727446 * pow(t - 60.0, -0.1332047592), 0.0, 255.0);
        g = clamp(288.1221695283 * pow(t - 60.0, -0.0755148492), 0.0, 255.0);
    }
    if (t >= 66.0) {
        b = 255.0;
    } else if (t <= 19.0) {
        b = 0.0;
    } else {
        b = clamp(138.5177312231 * log(t - 10.0) - 305.0447927307, 0.0, 255.0);
    }
    return vec3<f32>(r, g, b) / 255.0;
}

// Incandescent self-glow past the Draper point (~525 °C): the matter's real
// blackbody colour, scaled by how strongly it radiates (rises with
// temperature), HDR so hot matter blooms. Only opaque geometry is shaded here,
// so hot air never glows. (Per-material emissivity will scale this once it's a
// material field; rock/lava are near-full emitters for now.)
fn blackbody_glow(temp_c: f32) -> vec3<f32> {
    if (temp_c <= 525.0) {
        return vec3<f32>(0.0);
    }
    let color = blackbody_rgb(temp_c + 273.15);
    let heat = clamp((temp_c - 525.0) / 775.0, 0.0, 1.0);
    return color * pow(heat, 1.5) * 2.5;
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    // Oversized fullscreen triangle covering the viewport.
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let depth = textureLoad(gbuf_depth, px, 0);
    // Background (no opaque geometry): keep the cleared sky color for the
    // sky dome / clouds drawn in the following forward pass.
    if (depth >= 1.0) {
        discard;
    }

    let g0 = textureLoad(gb0, px, 0);
    let g1 = textureLoad(gb1, px, 0);
    let g2 = textureLoad(gb2, px, 0);

    let albedo = g0.rgb;
    let ao = g0.a;
    let normal = oct_decode(g1.xy);
    let sky_vis = g1.z;
    // Block light already carries its 0.95 trim and AO from the geometry pass.
    let block_light = g2.rgb;

    let view_dist = linearize(depth);
    // Rebuild the camera-relative world position from depth for the cascade
    // lookup (visually a no-op while shadows are shelved: sun_vis stays 1.0).
    let dims = vec2<f32>(textureDimensions(gb0));
    let ndc = vec3<f32>((frag.xy / dims) * 2.0 - 1.0, depth);
    let world_h = pc.inv_view_proj * vec4<f32>(ndc, 1.0);
    let world_rel = world_h.xyz / world_h.w;
    // Geometric grazing factor from the *unscaled* sun direction (scene.sun.xyz
    // is daylight-scaled, so normalize it): the shadow normal-offset bias
    // grows as this falls toward 0.
    let sun_dir_n = scene.sun.xyz / max(length(scene.sun.xyz), 1e-4);
    let n_dot_l = max(dot(normal, sun_dir_n), 0.0);
    let sun_vis = sun_visibility(world_rel, normal, view_dist, n_dot_l);

    let ambient = scene.sun.w;
    // scene.sun.xyz is pre-scaled by daylight, so the diffuse dies at night.
    let diffuse = max(dot(normal, scene.sun.xyz), 0.0);

    // Sky-ambient fill tinted by the sky colour (horizon→zenith by how
    // up-facing the surface is), so shadowed and indirect-lit surfaces read
    // sky-blue like Vibrant Visuals, never neutral grey. `sun_vis` darkens only
    // the sun term, so a shadowed surface keeps this fill (never pitch black)
    // and it vanishes underground where sky_vis → 0.
    let sky_color = mix(scene.sky_horizon.rgb, scene.sky_zenith.rgb, clamp(normal.y * 0.5 + 0.5, 0.0, 1.0));
    let sky_term = sky_vis * ambient * ao * sky_color;
    let sun_term = sky_vis * (1.0 - ambient) * diffuse * ao * sun_vis;
    // Unconditional ambient floor (params.y, per dimension): nothing renders
    // pure black. Added on top of sky/sun + block light, AO-modulated.
    let floor = scene.params.y * ao;
    let lit = max(sky_term + vec3<f32>(sun_term), block_light) + vec3<f32>(floor);
    var color = albedo * lit;

    // Incandescence: the surface's own blackbody glow past the Draper point,
    // baked per-vertex into GB2.a by the geometry pass (0..1 = 525..1500 °C) —
    // smooth, so it never bands on depth quantization. Modulated by albedo so
    // the surface's texture (lava's molten/crust pattern, the rock grain) shows
    // *in* the glow instead of being washed out by a flat bright colour — the
    // texture is the emissive pattern. Hot matter glows + blooms; cold → 0.
    color += albedo * blackbody_glow(525.0 + g2.a * 975.0);

    // Distance fog, coloured per-pixel by the destination's baked sky visibility
    // (the same `sky_vis` that gates lighting): a sky-exposed surface fades into
    // the real sky colour, while an enclosed one fades into the dark cave medium.
    // So a deep tunnel melts to black, but the view out a window or up a shaft
    // stays bright — "the deep is dark" with no camera/depth heuristic. cave_dark
    // is the per-dimension ambient floor so it's never pitch black.
    let fog_amount = 1.0 - exp(-pow(view_dist * 2.0 / scene.fog.w, 2.0));
    let cave_dark = vec3<f32>(scene.params.y);
    let fog_col = mix(cave_dark, scene.fog.rgb, sky_vis);
    color = mix(color, fog_col, fog_amount);
    return vec4<f32>(color, 1.0);
}
