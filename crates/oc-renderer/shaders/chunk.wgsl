// Chunk rendering: packed 8-byte vertices (ARCHITECTURE.md §4).
//   word 0: x:5 | y:5 | z:5 | face:3 | corner:2 | (su-1):4 | (sv-1):4
//     (corner positions 0..=16; su/sv = greedy quad extent, tiles the UVs)
//   word 1: texture layer:16 | light:8

struct PushConstants {
    // proj * view * translate(chunk_origin - camera), camera-relative.
    mvp: mat4x4<f32>,
    // xyz: direction toward the sun (normalized); w: ambient light level.
    sun: vec4<f32>,
    // xyz: chunk origin mod 256 (caustic phase anchor); w: time, seconds.
    params: vec4<f32>,
}

// `immediate` is WGSL/naga's name for Vulkan push constants.
var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) shade: f32,
    @location(3) local: vec3<f32>,
    @location(4) @interpolate(flat) underwater: u32,
}

@vertex
fn vs_main(@location(0) packed: vec2<u32>) -> VsOut {
    let w0 = packed.x;
    let pos = vec3<f32>(
        f32(w0 & 31u),
        f32((w0 >> 5u) & 31u),
        f32((w0 >> 10u) & 31u),
    );
    let face = (w0 >> 15u) & 7u;
    let corner = (w0 >> 18u) & 3u;
    let extent = vec2<f32>(
        f32(((w0 >> 20u) & 15u) + 1u),
        f32(((w0 >> 24u) & 15u) + 1u),
    );

    var corner_uv = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
    );
    // Sun-direction diffuse per face until real lighting lands (§4.7):
    // +Y, -Y, +Z, -Z, +X, -X
    var face_normal = array<vec3<f32>, 6>(
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, -1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(-1.0, 0.0, 0.0),
    );

    // Per-face brightness = whichever wins of:
    //  - sky light scaled by the day cycle (ambient floor + sun diffuse;
    //    pc.sun.xyz is pre-scaled by daylight, so night kills the diffuse)
    //  - block light (lamps), constant through the day.
    let light = (packed.y >> 16u) & 0xFFu;
    let sky_level = f32(light >> 4u) / 15.0;
    let block_level = f32(light & 15u) / 15.0;

    let ambient = pc.sun.w;
    let diffuse = max(dot(face_normal[face], pc.sun.xyz), 0.0);
    let sky_term = sky_level * (ambient + (1.0 - ambient) * diffuse);
    let block_term = block_level * 0.95;

    var out: VsOut;
    out.clip = pc.mvp * vec4<f32>(pos, 1.0);
    out.uv = corner_uv[corner] * extent;
    out.layer = packed.y & 0xFFFFu;
    out.shade = max(sky_term, block_term);
    out.local = pos;
    out.underwater = (packed.y >> 24u) & 1u;
    return out;
}

// Caustic dapples: bright cell lines (a sum of waves crossing zero) plus
// a fine sparkle octave, snapped to the 16x16 texel grid with stepped
// time — dense pixel-art shimmer over submerged surfaces. Wave vectors
// are integer cycles over 256 blocks, seamless across chunks.
fn caustic(p_raw: vec2<f32>, t_raw: f32) -> f32 {
    let tau = 6.28318530718;
    let p = floor(p_raw * 16.0) / 16.0;
    let t = floor(t_raw * 10.0) / 10.0;
    let a = sin(tau * dot(p, vec2(64.0, 24.0)) / 256.0 + t * 1.6);
    let b = sin(tau * dot(p, vec2(-32.0, 72.0)) / 256.0 + t * 2.1);
    let c = sin(tau * dot(p, vec2(48.0, -56.0)) / 256.0 + t * 1.2);
    let web = pow(1.0 - abs((a + b + c) / 3.0), 5.0);
    // Fine grain (~1-block wavelength) that rides on the web.
    let d = sin(tau * dot(p, vec2(168.0, 200.0)) / 256.0 + t * 2.6);
    let e = sin(tau * dot(p, vec2(-216.0, 144.0)) / 256.0 + t * 3.4);
    let fine = pow(1.0 - abs((d + e) / 2.0), 3.0);
    return web * (0.55 + 0.45 * fine) + 0.25 * fine * web;
}

@group(0) @binding(0) var block_textures: texture_2d_array<f32>;
@group(0) @binding(1) var block_sampler: sampler;

// Must match the projection in camera.rs.
const NEAR: f32 = 0.05;
const FAR: f32 = 4096.0;

fn linearize(depth: f32) -> f32 {
    return NEAR * FAR / (FAR - depth * (FAR - NEAR));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    var shade = in.shade;
    if (in.underwater == 1u) {
        // Sun dapples on submerged surfaces; daylight-gated (pc.sun.xyz
        // is pre-scaled by daylight) and scene-lit so caves stay dark.
        let daylight = length(pc.sun.xyz);
        // Caustics are a near-field effect too: gone past ~100 blocks.
        let dist_fade = 1.0 - smoothstep(40.0, 110.0, linearize(in.clip.z));
        let p = pc.params.xz + in.local.xz;
        let dapple = caustic(p, pc.params.w) * daylight * dist_fade;
        // Slightly green-cyan dapples, like sunlight through water.
        return vec4<f32>(
            texel.rgb * shade * (vec3(1.0) + vec3(0.55, 0.80, 0.70) * dapple),
            1.0,
        );
    }
    return vec4<f32>(texel.rgb * shade, 1.0);
}
