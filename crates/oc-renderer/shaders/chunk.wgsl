// Chunk rendering: packed 8-byte vertices (ARCHITECTURE.md §4).
//   word 0: x:5 | y:5 | z:5 | face:3 | corner:2 | (su-1):4 | (sv-1):4 | ao:2
//     (corner positions 0..=16; su/sv = greedy quad extent, tiles the UVs;
//      ao = per-vertex ambient occlusion, 0 darkest .. 3 open)
//   word 1: texture layer:16 | light:8 | underwater:1 | surface_top:1 |
//           underwater_surface:1

struct PushConstants {
    // proj * view * translate(chunk_origin - camera), camera-relative.
    mvp: mat4x4<f32>,
    // xyz: direction toward the sun (normalized); w: ambient light level.
    sun: vec4<f32>,
    // xyz: chunk origin mod 256 (caustic phase anchor); w: time, seconds.
    params: vec4<f32>,
    // rgb: fog (horizon) color; w: distance where fog saturates, blocks.
    fog: vec4<f32>,
    // xyz: chunk origin camera-relative (shadow lookups); w: unused.
    rel: vec4<f32>,
}

// `immediate` is WGSL/naga's name for Vulkan push constants.
var<immediate> pc: PushConstants;

// Sun shadow cascades (set 1, written per frame).
struct ShadowData {
    // Camera-relative world -> cascade clip.
    matrices: array<mat4x4<f32>, 3>,
    // Cascade far distances; w unused.
    splits: vec4<f32>,
    // x: strength (0 = off/night); yzw: world units per texel, per cascade.
    params: vec4<f32>,
}

@group(1) @binding(0) var<uniform> shadow: ShadowData;
@group(1) @binding(1) var shadow_map: texture_depth_2d_array;
@group(1) @binding(2) var shadow_sampler: sampler_comparison;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    // Light terms, AO baked in: x = sky ambient, y = sun diffuse (the
    // part shadows darken), z = block light.
    @location(2) light_terms: vec3<f32>,
    // Caustic-plane coords: world position (mod-256 anchored) projected
    // onto the face's plane, so dapples wrap around vertical faces
    // instead of smearing down them.
    @location(3) cpos: vec2<f32>,
    // Bit 0: face is underwater (caustics); bit 2: the adjacent water
    // is the open surface, so caustics stop at the 14/16 waterline.
    @location(4) @interpolate(flat) underwater: u32,
    // Camera-relative world position + flat face normal (shadows).
    @location(5) world_rel: vec3<f32>,
    @location(6) @interpolate(flat) normal: vec3<f32>,
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
    let block_term = block_level * 0.95;

    // Ambient occlusion: corners boxed in by neighbors darken, which is
    // what makes block edges read as solid geometry. Kept gentle — the
    // edge seams read as dirt lines when this gets heavy.
    let ao = f32((w0 >> 28u) & 3u);
    let ao_mul = 0.66 + (0.34 / 3.0) * ao;

    var out: VsOut;
    out.clip = pc.mvp * vec4<f32>(pos, 1.0);
    out.uv = corner_uv[corner] * extent;
    out.layer = packed.y & 0xFFFFu;
    // Split so the fragment can shadow just the sun-diffuse part.
    out.light_terms = vec3<f32>(
        sky_level * ambient * ao_mul,
        sky_level * (1.0 - ambient) * diffuse * ao_mul,
        block_term * ao_mul,
    );
    let world = pc.params.xyz + pos;
    if (face < 2u) {
        out.cpos = world.xz;
    } else if (face < 4u) {
        out.cpos = world.xy;
    } else {
        out.cpos = world.zy;
    }
    out.underwater = (packed.y >> 24u) & 5u;
    out.world_rel = pc.rel.xyz + pos;
    out.normal = face_normal[face];
    return out;
}

fn hash2(c: vec2<f32>) -> vec2<f32> {
    let n = sin(vec2(dot(c, vec2(127.1, 311.7)), dot(c, vec2(269.5, 183.3))));
    return fract(n * 43758.5453);
}

// Caustic dapples: an animated Voronoi web — thin bright cell borders
// over dark interiors, the shape sunlight takes when surface ripples
// focus it. ~5 cells per block; each cell's focus point drifts in a
// small loop. Snapped to a half-texel grid (32 steps per block) with
// stepped time, and dotted per texel so the lines read as pixel art.
// Cell ids wrap every 256 blocks, matching the chunk phase anchor, so
// the pattern is seamless and fp32-exact anywhere in the world.
fn caustic(p_raw: vec2<f32>, t_raw: f32) -> f32 {
    let tau = 6.28318530718;
    let p = floor(p_raw * 32.0) / 32.0;
    let t = floor(t_raw * 5.0) / 5.0;
    let q = p * 5.0;
    let base = floor(q);
    var f1 = 8.0;
    var f2 = 8.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let cell = base + vec2(f32(dx), f32(dy));
            let wrapped = cell - floor(cell / 1280.0) * 1280.0;
            let h = hash2(wrapped);
            let feature = cell + vec2(0.5)
                + 0.38 * vec2(sin(t * 0.8 + h.x * tau), sin(t * 1.0 + h.y * tau));
            let d = distance(q, feature);
            if (d < f1) {
                f2 = f1;
                f1 = d;
            } else if (d < f2) {
                f2 = d;
            }
        }
    }
    // Bright where two cells nearly tie (the border between them).
    let web = 1.0 - smoothstep(0.0, 0.22, f2 - f1);
    // Per-texel flicker breaks the lines into shifting dots, with the
    // occasional fully bright sparkle pixel.
    let tex = p * 32.0 - floor(p * 32.0 / 8192.0) * 8192.0;
    let n = hash2(tex + vec2(t * 7.0, t * 13.0)).x;
    let sparkle = select(0.0, 0.5, n > 0.93);
    return web * (0.3 + 0.35 * n * n + sparkle);
}

@group(0) @binding(0) var block_textures: texture_2d_array<f32>;
@group(0) @binding(1) var block_sampler: sampler;

// Must match the projection in camera.rs.
const NEAR: f32 = 0.05;
const FAR: f32 = 4096.0;

fn linearize(depth: f32) -> f32 {
    return NEAR * FAR / (FAR - depth * (FAR - NEAR));
}

// PCF visibility from one cascade. The depth bias scales with the
// cascade's texel size so every cascade self-shadows identically —
// a flat bias over-biases the fine cascade and the brightness step
// shows up as a line that follows the camera.
fn cascade_lit(cascade: i32, world_rel: vec3<f32>, normal: vec3<f32>) -> f32 {
    let texel_world = shadow.params[1u + u32(cascade)];
    // One texel of normal offset: enough against acne, small enough that
    // shadows stay attached to the walls that cast them.
    let pos = world_rel + normal * texel_world;
    let ndc = shadow.matrices[cascade] * vec4<f32>(pos, 1.0);
    // Vulkan rasterizes NDC y-down into the map; the lookup matches.
    let uv = ndc.xy * 0.5 + vec2(0.5);
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0))) {
        return 1.0;
    }
    // 400 = the cascades' shared light-space depth range, blocks.
    let bias = 0.0003 + texel_world * 2.5 / 400.0;
    // 3x3 PCF on top of the sampler's bilinear comparison.
    var lit = 0.0;
    let step = 1.0 / 2048.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            lit += textureSampleCompareLevel(
                shadow_map,
                shadow_sampler,
                uv + vec2(f32(dx), f32(dy)) * step,
                cascade,
                ndc.z - bias,
            );
        }
    }
    return lit / 9.0;
}

// How much of the sun reaches this fragment. Cascades cross-fade over
// the last 15% of each range — a hard switch reads as a moving line —
// and the whole effect eases out past the far cascade and at twilight.
fn sun_visibility(world_rel: vec3<f32>, normal: vec3<f32>, view_dist: f32) -> f32 {
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
    var lit = cascade_lit(cascade, world_rel, normal);
    let blend = smoothstep(split * 0.85, split, view_dist);
    if (blend > 0.0 && cascade < 2) {
        lit = mix(lit, cascade_lit(cascade + 1, world_rel, normal), blend);
    }
    // Ease back to fully lit at the cascade horizon and with twilight.
    let range_fade = smoothstep(shadow.splits.z * 0.8, shadow.splits.z, view_dist);
    return mix(1.0, mix(lit, 1.0, range_fade), strength);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    let view_dist = linearize(in.clip.z);
    let visibility = sun_visibility(in.world_rel, in.normal, view_dist);
    // Shadow only steals the sun's diffuse; ambient and lamps remain.
    let shade = max(in.light_terms.x + in.light_terms.y * visibility, in.light_terms.z);
    var color = texel.rgb * shade;
    // Water sits at 14/16: side faces against surface water keep their
    // top sliver dry (cpos.y carries world y on side faces).
    let dry = (in.underwater & 4u) != 0u && fract(in.cpos.y) > 0.875;
    if ((in.underwater & 1u) == 1u && !dry) {
        // Sun dapples on submerged surfaces; daylight-gated (pc.sun.xyz
        // is pre-scaled by daylight) and scene-lit so caves stay dark.
        let daylight = length(pc.sun.xyz);
        // Caustics are a near-field effect too: gone past ~100 blocks.
        let dist_fade = 1.0 - smoothstep(40.0, 110.0, view_dist);
        let dapple = caustic(in.cpos, pc.params.w) * daylight * dist_fade;
        // Slightly green-cyan dapples, like sunlight through water —
        // kept faint: a shimmer on the sand, not a pattern painted on it.
        color *= vec3(1.0) + vec3(0.30, 0.44, 0.38) * dapple;
    }
    // Horizon fog, same curve as water: far terrain melts into the sky;
    // underwater the client passes a short distance and deep blue color.
    let fog_amount = 1.0 - exp(-pow(view_dist * 2.0 / pc.fog.w, 2.0));
    return vec4<f32>(mix(color, pc.fog.rgb, fog_amount), 1.0);
}
