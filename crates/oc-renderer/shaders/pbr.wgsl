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
    // x: time; y: base ambient floor; z, w: reserved.
    params: vec4<f32>,
}
@group(1) @binding(0) var<uniform> scene: Scene;

@group(0) @binding(0) var gb0: texture_2d<f32>;
@group(0) @binding(1) var gb1: texture_2d<f32>;
@group(0) @binding(2) var gb2: texture_2d<f32>;
@group(0) @binding(3) var gbuf_depth: texture_depth_2d;

// Must match the projection in camera.rs.
const NEAR: f32 = 0.05;
const FAR: f32 = 4096.0;

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

    let ambient = scene.sun.w;
    // scene.sun.xyz is pre-scaled by daylight, so the diffuse dies at night.
    let diffuse = max(dot(normal, scene.sun.xyz), 0.0);
    // E4 revives the shadow cascades here; fully lit until then.
    let sun_vis = 1.0;

    let sky_term = sky_vis * ambient * ao;
    let sun_term = sky_vis * (1.0 - ambient) * diffuse * ao * sun_vis;
    let lit = max(vec3<f32>(sky_term + sun_term), block_light);
    var color = albedo * lit;

    // Distance fog: far terrain melts into the sky, same curve as the
    // forward path (and the water pass).
    let view_dist = linearize(depth);
    let fog_amount = 1.0 - exp(-pow(view_dist * 2.0 / scene.fog.w, 2.0));
    color = mix(color, scene.fog.rgb, fog_amount);
    return vec4<f32>(color, 1.0);
}
