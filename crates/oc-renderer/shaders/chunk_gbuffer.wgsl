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
//   GB1: octahedral normal.xy | sky_visibility | packed roughness+metalness
//   GB2: block_light.rgb | blackbody-glow temperature

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
    // Intrinsic emissive temperature (°C) per block-texture layer, up to 32
    // packed into 8 vec4 (layer L → emissive_temp[L/4][L%4]). Lava ≈ 1200; 0 = none.
    emissive_temp: array<vec4<f32>, 8>,
    // Surface material per block-texture layer (layer L → material[L/4][L%4]),
    // each scalar already packed by Rust `pack_material(roughness, metalness)`
    // and written verbatim into GB1.w. See pbr.wgsl for the decode.
    material: array<vec4<f32>, 8>,
    // Subsurface (translucency) amount per block-texture layer (layer L →
    // subsurface[L/4][L%4]); 0 = opaque. Folded into the signed GB2.a below;
    // pbr.wgsl reads it back for the backlit-foliage transmittance glow.
    subsurface: array<vec4<f32>, 8>,
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
    // Camera-relative fragment position (camera at origin) — the view direction
    // for parallax occlusion mapping is normalize(-view_rel).
    @location(8) view_rel: vec3<f32>,
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

    // word 2: low 16 = light (sky:4 << 12 | r:4 << 8 | g:4 << 4 | b:4);
    // high 16 = the baked tier-2 source-heat delta °C above the base.
    let light = packed.z & 0xFFFFu;
    let sky_level = f32((light >> 12u) & 15u) / 15.0;
    let block_rgb = vec3<f32>(
        f32((light >> 8u) & 15u),
        f32((light >> 4u) & 15u),
        f32(light & 15u),
    ) / 15.0;
    // Signed glow delta (°C from the base): 0 ⇒ −MAX, ~32768 ⇒ 0, 65535 ⇒ +MAX.
    // Signed so tier-3 stored heat can pull a cell below the depth glow (a cool
    // block placed deep renders dark). Must match quantize_heat in mesh.rs.
    let heat_delta = (f32((packed.z >> 16u) & 0xFFFFu) / 65535.0 * 2.0 - 1.0) * 1500.0;

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
    // Camera-relative fragment position (pc.rel = chunk origin camera-relative).
    out.view_rel = pc.rel.xyz + pos;
    // Absolute world Y = camera world Y (params.z) + camera-relative section
    // origin (pc.rel) + local pos. Drives the per-vertex blackbody glow. Hot
    // matter glows at the hotter of its effective ambient temperature (the
    // geothermal base + the baked tier-2 source-heat delta, so rock near lava
    // glows) and its own intrinsic emissive temperature (lava ~1200 °C).
    let world_y = scene.params.z + pc.rel.y + pos.y;
    let layer = packed.y & 0xFFFFu;
    let mat_temp = scene.emissive_temp[layer / 4u][layer % 4u];
    let temp = max(base_temp(world_y) + heat_delta, mat_temp);
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
// Per-texel material (MER) array, sampled with block_sampler: R = metalness,
// G = roughness (linear UNORM). Packed into GB1.w below.
@group(0) @binding(2) var mer_textures: texture_2d_array<f32>;
// Per-texel tangent-space normal map array (linear UNORM); perturbs the flat
// face normal for surface relief.
@group(0) @binding(3) var normal_textures: texture_2d_array<f32>;

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

// Parallax occlusion mapping: march the heightmap (stored in normal_textures.a)
// along the tangent-space view direction so the surface shows true
// view-dependent depth — valleys recede behind ridges, the texture "sinks in"
// as you look across it. Steepest linear search; returns the offset UV that the
// albedo / MER / normal samples should use. `textureSampleLevel` (explicit LOD)
// keeps it valid in the data-dependent loop.
fn parallax_uv(uv: vec2<f32>, layer: i32, view_ts: vec3<f32>) -> vec2<f32> {
    let depth_scale = 0.06;     // max recess depth, in texture-tile units
    let num_layers = 12.0;
    // UV shift across the full depth, toward the eye in tangent XY (clamp z so
    // grazing views don't explode the offset).
    let uv_step = view_ts.xy / max(abs(view_ts.z), 0.2) * depth_scale / num_layers;
    let layer_step = 1.0 / num_layers;
    var cur_uv = uv;
    var cur_layer = 0.0;
    var cur_depth = 1.0 - textureSampleLevel(normal_textures, block_sampler, cur_uv, layer, 0.0).a;
    for (var i = 0; i < 12; i = i + 1) {
        if (cur_layer >= cur_depth) {
            break;
        }
        cur_uv = cur_uv - uv_step;
        cur_layer = cur_layer + layer_step;
        cur_depth = 1.0 - textureSampleLevel(normal_textures, block_sampler, cur_uv, layer, 0.0).a;
    }
    return cur_uv;
}

@fragment
fn fs_main(in: VsOut) -> GBufferOut {
    // Per-face tangent frame matching the cpos UV projection (±Y: U=x,V=z;
    // ±Z: U=x,V=y; ±X: U=z,V=y) — drives both parallax and the normal map.
    var tan_u: vec3<f32>;
    var tan_v: vec3<f32>;
    if (abs(in.normal.y) > 0.5) {
        tan_u = vec3<f32>(1.0, 0.0, 0.0);
        tan_v = vec3<f32>(0.0, 0.0, 1.0);
    } else if (abs(in.normal.z) > 0.5) {
        tan_u = vec3<f32>(1.0, 0.0, 0.0);
        tan_v = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        tan_u = vec3<f32>(0.0, 0.0, 1.0);
        tan_v = vec3<f32>(0.0, 1.0, 0.0);
    }

    // Parallax occlusion: offset the sampling UV by the view direction through
    // the heightmap, for true view-dependent depth. All texture samples below
    // use this offset `uv` (caustics stay on the world-space cpos).
    let view = normalize(-in.view_rel);
    let view_ts = vec3<f32>(dot(view, tan_u), dot(view, tan_v), dot(view, in.normal));
    let uv = parallax_uv(in.uv, i32(in.layer), view_ts);

    let texel = textureSample(block_textures, block_sampler, uv, i32(in.layer));
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

    // Per-texel surface material from the MER array (R = metalness, G =
    // roughness), packed into GB1.w in the same 8-bit code pbr.wgsl decodes
    // (top bit = metal flag, low 7 = roughness × 127). Per-texel, so detail
    // varies within a face — e.g. the metallic iron-ore flecks.
    let mer = textureSample(mer_textures, block_sampler, uv, i32(in.layer));
    let metal_code = select(0.0, 128.0, mer.r >= 0.5);
    let material = (metal_code + round(mer.g * 127.0)) / 255.0;

    // Perturb the flat face normal by the tangent-space normal map (full
    // per-texel: smooth blocks sparkle, matte get diffuse relief), then encode.
    let nt = textureSample(normal_textures, block_sampler, uv, i32(in.layer)).xyz * 2.0 - 1.0;
    let normal = normalize(nt.x * tan_u + nt.y * tan_v + nt.z * in.normal);

    // GB2.a is a signed channel centred on 0.5: the upper half (>0.5) carries
    // the blackbody-glow temperature (emissive), the lower half (<0.5) carries
    // foliage subsurface (translucency). A block is one or the other — hot
    // matter never has foliage SSS — so they share one channel with no extra
    // G-buffer target. Emissive wins if both are somehow set. pbr.wgsl decodes.
    let sss = clamp(scene.subsurface[in.layer / 4u][in.layer % 4u], 0.0, 1.0);
    var gb2a = 0.5 + in.emissive * 0.5;
    if (in.emissive <= 0.0 && sss > 0.0) {
        gb2a = 0.5 - sss * 0.5;
    }

    var out: GBufferOut;
    out.gb0 = vec4<f32>(albedo, in.ao_sky.x);
    out.gb1 = vec4<f32>(oct_encode(normal), in.ao_sky.y, material);
    out.gb2 = vec4<f32>(in.block_light, gb2a);
    return out;
}
