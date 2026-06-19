// Deferred geometry pass (graphics roadmap Stage E): opaque chunks write
// their material attributes into a thin G-buffer instead of shading inline.
// The fullscreen `pbr.wgsl` lighting pass then reads the G-buffer + the
// Scene UBO + baked light and computes the final lit color.
//
// Vertex unpacking matches chunk.wgsl exactly (same 12-byte packed vertex):
//   word 0: x:5 | y:5 | z:5 | face:3 | corner:2 | (su-1):4 | (sv-1):4 | ao:2
//   word 1: texture layer:16 | reserved:8 | underwater:1 | surface_top:1 |
//           underwater_surface:1
//   word 2: light:16 (sky:4 << 12 | r:4 << 8 | g:4 << 4 | b:4) | reserved:16
//
// G-buffer (3x RGBA8):
//   GB0: albedo.rgb (caustics folded in) | ao_mul
//   GB1: octahedral normal.xy | sky_visibility | roughness (reserved, =1)
//   GB2: block_light.rgb | metalness (reserved, =0)

struct PushConstants {
    // proj * view * translate(chunk_origin - camera), camera-relative.
    mvp: mat4x4<f32>,
    // xyz: chunk origin mod 256 (caustic phase anchor); w: unused.
    params: vec4<f32>,
    // xyz: chunk origin camera-relative; w: unused.
    rel: vec4<f32>,
}
var<immediate> pc: PushConstants;

// Per-frame scene/environment data (set 1); mirrors `scene::SceneData`. Only
// time + daylight are needed here (the caustic animation); the rest of the
// lighting reads it in the deferred pass.
struct Scene {
    sun: vec4<f32>,
    fog: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_away: vec4<f32>,
    sky_sun: vec4<f32>,
    // x: time (seconds); y: base ambient floor; z: camera world Y; w: thermal
    // point count.
    params: vec4<f32>,
    // Temperature-vs-height curve, ascending Y, two points per vec4 as
    // (y0, temp0 °C, y1, temp1) — up to 8 points; params.w is the count.
    thermal_profile: array<vec4<f32>, 4>,
    // Intrinsic emissive temperature (°C) per block-texture layer, 16 packed
    // into 4 vec4 (layer L → emissive_temp[L/4][L%4]). Lava ≈ 1200; 0 = none.
    emissive_temp: array<vec4<f32>, 4>,
}
@group(1) @binding(0) var<uniform> scene: Scene;

// One temperature-curve point (y, °C); the curve is ascending by y.
fn profile_point(i: i32) -> vec2<f32> {
    let v = scene.thermal_profile[i / 2];
    if ((i & 1) == 0) { return vec2<f32>(v.x, v.y); }
    return vec2<f32>(v.z, v.w);
}

// Tier-1 ambient temperature (°C) at a world Y from the dimension's curve;
// piecewise-linear, clamped beyond the ends. Mirrors oc_world::temperature::base
// and pbr.wgsl, but evaluated per-vertex here (smooth) so the glow it drives
// doesn't band on the depth buffer's quantization.
fn base_temp(world_y: f32) -> f32 {
    let n = i32(scene.params.w);
    if (n <= 0) { return 14.0; }
    // Walk ascending points, carrying the previous one. The first point past
    // `world_y` closes the bracketing segment [prev, cur]; below the deepest
    // point we clamp to it, above the shallowest we clamp to the last `prev`.
    // (Single forward scan — no separate first-iteration segment, which a
    // two-lookup `pp(i)`/`pp(i+1)` loop turned into a fall-through to the cold
    // end value for the deepest band, leaving deep rock un-glowing below -560.)
    var prev = profile_point(0);
    if (world_y <= prev.x) { return prev.y; }
    for (var i = 1; i < n; i = i + 1) {
        let cur = profile_point(i);
        if (world_y <= cur.x) {
            let t = (world_y - prev.x) / max(cur.x - prev.x, 0.0001);
            return prev.y + t * (cur.y - prev.y);
        }
        prev = cur;
    }
    return prev.y;
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    // x: ao multiplier (0.66..1); y: sky visibility 0..1.
    @location(2) ao_sky: vec2<f32>,
    @location(3) block_light: vec3<f32>,
    // Caustic-plane coords (mod-256 anchored world projected onto the face).
    @location(4) cpos: vec2<f32>,
    // Bit 0: underwater (caustics); bit 2: adjacent water is the open surface.
    @location(5) @interpolate(flat) underwater: u32,
    @location(6) @interpolate(flat) normal: vec3<f32>,
    // Blackbody glow drive, 0..1 = temperature 525..1500 °C past the Draper
    // point. Per-vertex (smooth) so the glow doesn't band; the lighting pass
    // blooms it. 0 = no glow.
    @location(7) emissive: f32,
}

@vertex
fn vs_main(@location(0) packed: vec3<u32>) -> VsOut {
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
    // +Y, -Y, +Z, -Z, +X, -X
    var face_normal = array<vec3<f32>, 6>(
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, -1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(-1.0, 0.0, 0.0),
    );

    // word 2: sky:4 << 12 | r:4 << 8 | g:4 << 4 | b:4.
    let light = packed.z & 0xFFFFu;
    let sky_level = f32((light >> 12u) & 15u) / 15.0;
    let block_rgb = vec3<f32>(
        f32((light >> 8u) & 15u),
        f32((light >> 4u) & 15u),
        f32(light & 15u),
    ) / 15.0;

    let ao = f32((w0 >> 28u) & 3u);
    let ao_mul = 0.66 + (0.34 / 3.0) * ao;

    var out: VsOut;
    out.clip = pc.mvp * vec4<f32>(pos, 1.0);
    out.uv = corner_uv[corner] * extent;
    out.layer = packed.y & 0xFFFFu;
    out.ao_sky = vec2<f32>(ao_mul, sky_level);
    // Block light keeps the 0.95 trim baked from the forward path.
    out.block_light = block_rgb * 0.95 * ao_mul;
    let world = pc.params.xyz + pos;
    if (face < 2u) {
        out.cpos = world.xz;
    } else if (face < 4u) {
        out.cpos = world.xy;
    } else {
        out.cpos = world.zy;
    }
    out.underwater = (packed.y >> 24u) & 5u;
    out.normal = face_normal[face];
    // Absolute world Y = camera world Y (params.z) + camera-relative section
    // origin (pc.rel) + local pos. Drives the per-vertex blackbody glow. Hot
    // matter glows at the hotter of its ambient temperature and its own
    // intrinsic emissive temperature (lava ~1200 °C, not the ambient ~666).
    let world_y = scene.params.z + pc.rel.y + pos.y;
    let layer = packed.y & 0xFFFFu;
    let mat_temp = scene.emissive_temp[layer / 4u][layer % 4u];
    let temp = max(base_temp(world_y), mat_temp);
    out.emissive = clamp((temp - 525.0) / 975.0, 0.0, 1.0);
    return out;
}

fn hash2(c: vec2<f32>) -> vec2<f32> {
    let n = sin(vec2(dot(c, vec2(127.1, 311.7)), dot(c, vec2(269.5, 183.3))));
    return fract(n * 43758.5453);
}

// Caustic dapples (identical to chunk.wgsl): an animated Voronoi web.
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
    let web = 1.0 - smoothstep(0.0, 0.22, f2 - f1);
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

// Sign-preserving octahedral normal encode (n is unit). Round-trips the six
// axis-aligned face normals exactly; ready for normal maps later.
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let s = abs(n.x) + abs(n.y) + abs(n.z);
    let p = n.xy / s;
    var e = p;
    if (n.z < 0.0) {
        let snz = vec2<f32>(
            select(-1.0, 1.0, p.x >= 0.0),
            select(-1.0, 1.0, p.y >= 0.0),
        );
        e = (vec2<f32>(1.0) - abs(p.yx)) * snz;
    }
    return e * 0.5 + 0.5;
}

struct GBufferOut {
    @location(0) gb0: vec4<f32>,
    @location(1) gb1: vec4<f32>,
    @location(2) gb2: vec4<f32>,
}

@fragment
fn fs_main(in: VsOut) -> GBufferOut {
    let texel = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    var albedo = texel.rgb;

    // Caustics fold into albedo (multiplicative): albedo*caustic, then * light
    // in the lighting pass, reproduces the forward path's lit*caustic exactly.
    let dry = (in.underwater & 4u) != 0u && fract(in.cpos.y) > 0.875;
    if ((in.underwater & 1u) == 1u && !dry) {
        let daylight = length(scene.sun.xyz);
        let view_dist = linearize(in.clip.z);
        let dist_fade = 1.0 - smoothstep(40.0, 110.0, view_dist);
        let dapple = caustic(in.cpos, scene.params.x) * daylight * dist_fade;
        albedo *= vec3(1.0) + vec3(0.30, 0.44, 0.38) * dapple;
    }

    var out: GBufferOut;
    out.gb0 = vec4<f32>(albedo, in.ao_sky.x);
    out.gb1 = vec4<f32>(oct_encode(in.normal), in.ao_sky.y, 1.0);
    // GB2.a carries the blackbody-glow temperature (was metalness-reserved).
    out.gb2 = vec4<f32>(in.block_light, in.emissive);
    return out;
}
