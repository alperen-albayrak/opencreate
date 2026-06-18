// Water pass (graphics roadmap stage B): same packed vertices as chunk.wgsl,
// drawn blended after opaques in its own render pass. Scrolling procedural
// wave normals, Schlick fresnel, sky reflection, sun glint, and — from the
// sampled opaque depth — Beer-Lambert absorption (shallow turquoise to deep
// blue), soft shorelines and in-shader occlusion (the pass has no depth
// attachment). Refraction/SSR arrive with the stage-E snapshot tier.

struct PushConstants {
    // proj * view * translate(chunk_origin - camera), camera-relative.
    mvp: mat4x4<f32>,
    // xyz: chunk origin camera-relative (view vector); w: unused.
    rel: vec4<f32>,
    // xyz: chunk origin mod 256 (shimmer phase anchor); w: unused.
    wave_origin: vec4<f32>,
}

var<immediate> pc: PushConstants;

// Per-frame scene/environment data (set 2); mirrors `scene::SceneData`.
// sun, the reflection-environment color (sky_horizon), time, and fog
// distance used to ride in push constants.
struct Scene {
    // xyz: direction toward the sun, pre-scaled by daylight; w: ambient.
    sun: vec4<f32>,
    // rgb: distance-fog (horizon) color; w: fog saturation distance, blocks.
    fog: vec4<f32>,
    // rgb: sky/horizon reflection environment color; w: celestial angle.
    sky_horizon: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_away: vec4<f32>,
    sky_sun: vec4<f32>,
    // x: time (seconds); y: base ambient floor.
    params: vec4<f32>,
}
@group(2) @binding(0) var<uniform> scene: Scene;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) shade: f32,
    @location(3) local: vec3<f32>,
    @location(4) @interpolate(flat) face: u32,
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

    // word 2: sky:4 << 12 | r:4 << 8 | g:4 << 4 | b:4 (RGB block light).
    let light = packed.z & 0xFFFFu;
    let sky_level = f32((light >> 12u) & 15u) / 15.0;
    // Water tints stay subtle: track the brightest block-light channel.
    let block_level = f32(max(max((light >> 8u) & 15u, (light >> 4u) & 15u), light & 15u)) / 15.0;
    // The diffuse term follows actual sun strength (scene.sun.xyz is
    // pre-scaled by daylight), so water goes dark at night like land.
    let daylight = length(scene.sun.xyz);
    let shade = max(
        sky_level * (scene.sun.w + (1.0 - scene.sun.w) * 0.6 * daylight),
        block_level * 0.95,
    );

    // Open water surfaces sit at 14/16 block height (the inset rim
    // against shores): drop the whole top face, and the top corners of
    // side faces whose top edge is the surface (corners 2 and 3).
    var lowered = pos;
    let surface_top = (packed.y >> 25u) & 1u;
    if (surface_top == 1u && (face == 0u || corner >= 2u)) {
        lowered.y -= 0.125;
    }

    var out: VsOut;
    out.clip = pc.mvp * vec4<f32>(lowered, 1.0);
    out.uv = corner_uv[corner] * extent;
    out.layer = packed.y & 0xFFFFu;
    out.shade = shade;
    out.local = lowered;
    out.face = face;
    return out;
}

@group(0) @binding(0) var block_textures: texture_2d_array<f32>;
@group(0) @binding(1) var block_sampler: sampler;
@group(1) @binding(0) var opaque_depth: texture_depth_2d;
@group(1) @binding(1) var scene_color: texture_2d<f32>;
@group(1) @binding(2) var scene_sampler: sampler;

// Per-frame data for the reflection march.
struct WaterFrame {
    // Camera-relative world -> clip (same projection the chunks use).
    view_proj: mat4x4<f32>,
    // x, y: render extent in pixels; z: SSR enabled; w: unused.
    params: vec4<f32>,
}

@group(1) @binding(3) var<uniform> frame: WaterFrame;

// Lite surface ripple: two slow components at block-or-two wavelengths
// (integer cycles over 256 blocks, seamless across chunk origins). It
// exists only to sparkle the sun glint — calm, unhurried water.
fn ripple_height(p: vec2<f32>, t: f32) -> f32 {
    let tau = 6.28318530718;
    var h = 0.0;
    h += 0.6 * sin(tau * dot(p, vec2(144.0, 48.0)) / 256.0 + t * 0.55);
    h += 0.4 * sin(tau * dot(p, vec2(-80.0, 120.0)) / 256.0 + t * 0.85);
    return h;
}

// Pixel-art twinkle: positions snap to the 16x16 texel grid and time
// steps gently (5 fps), so sparse texel-sized sparkles drift slowly
// through the light reflection.
fn ripple_normal(p: vec2<f32>, t: f32) -> vec3<f32> {
    let cell = floor(p * 16.0) / 16.0;
    let stepped = floor(t * 5.0) / 5.0;
    let e = 1.0 / 16.0;
    let h0 = ripple_height(cell, stepped);
    let hx = ripple_height(cell + vec2(e, 0.0), stepped);
    let hz = ripple_height(cell + vec2(0.0, e), stepped);
    return normalize(vec3((h0 - hx) * 0.35, e * 2.0, (h0 - hz) * 0.35));
}

// Must match the projection in camera.rs.
const NEAR: f32 = 0.05;
const FAR: f32 = 4096.0;

fn linearize(depth: f32) -> f32 {
    return NEAR * FAR / (FAR - depth * (FAR - NEAR));
}

// Screen-space reflection: march the reflected ray through the opaque
// depth buffer and return (scene uv, confidence). Confidence fades at
// screen edges and drops to zero on a miss, where the sky fallback
// takes over. Steps grow geometrically: crisp nearby, cheap far away.
fn ssr_trace(origin: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    var t = 0.3;
    for (var i = 0; i < 16; i++) {
        if (t > 140.0) {
            break;
        }
        let p = origin + dir * t;
        let clip = frame.view_proj * vec4<f32>(p, 1.0);
        if (clip.w <= 0.05) {
            break;
        }
        let ndc = clip.xyz / clip.w;
        let uv = ndc.xy * 0.5 + vec2(0.5);
        if (any(uv < vec2(0.0)) || any(uv > vec2(1.0)) || ndc.z >= 1.0) {
            break;
        }
        let texel = vec2<i32>(uv * frame.params.xy);
        let scene_z = textureLoad(opaque_depth, texel, 0);
        let scene_dist = linearize(scene_z);
        let ray_dist = linearize(ndc.z);
        if (ray_dist > scene_dist + 0.05) {
            // Behind something. Reject paper-thin overhangs the ray
            // actually passed behind, then refine one step back.
            if (ray_dist - scene_dist > max(2.0, t * 0.35)) {
                break;
            }
            let edge = min(
                smoothstep(0.0, 0.08, min(uv.x, 1.0 - uv.x)),
                smoothstep(0.0, 0.08, min(uv.y, 1.0 - uv.y)),
            );
            return vec3<f32>(uv, edge);
        }
        t *= 1.5;
    }
    return vec3<f32>(0.0, 0.0, 0.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The opaque scene at this pixel (same resolution as this pass).
    let scene_depth = textureLoad(opaque_depth, vec2<i32>(in.clip.xy), 0);
    if (scene_depth < in.clip.z) {
        discard; // terrain in front of the water
    }
    // View-space distance the eye ray travels through water before
    // hitting whatever is behind it (sky counts as "very far").
    let water_depth = max(linearize(scene_depth) - linearize(in.clip.z), 0.0);

    // The body, fresnel and sky reflection all use the FLAT face normal:
    // the water sheet stays perfectly smooth at every distance (no
    // light/dark wave strips). The ripple only sparkles the sun glint.
    var side = array<vec3<f32>, 6>(
        vec3(0.0, 1.0, 0.0), vec3(0.0, -1.0, 0.0),
        vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0),
        vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0),
    );
    let normal = side[in.face];

    // Camera sits at the origin of camera-relative space.
    let to_fragment = normalize(pc.rel.xyz + in.local);
    let cos_view = max(dot(normal, -to_fragment), 0.0);
    // Schlick fresnel, F0 = 0.02 (water), capped low: with flat normals
    // every distant pixel hits grazing angle at once, and vanilla water
    // should stay blue with a mild sheen, not become a sky mirror.
    let fresnel = 0.02 + 0.58 * pow(1.0 - cos_view, 4.0);

    // Water body color: Beer-Lambert-ish — red dies first, so shallow
    // water reads turquoise and deep water converges to deep blue.
    let absorb = 1.0 - exp(-water_depth * 0.11);
    let texel = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    let ripple = 0.85 + 0.30 * texel.b;
    let base = mix(vec3(0.15, 0.40, 0.48), vec3(0.05, 0.17, 0.35), absorb)
        * ripple * in.shade;

    // Reflection: blue water head-on, but at grazing angles (where the
    // mirror dominates) it reflects the REAL sky color — sunset water
    // mirrors the sunset. (Stage C's sky() function enriches this.)
    let reflect_dir = reflect(to_fragment, normal);
    let sky_follow = clamp(fresnel * 2.2, 0.25, 1.0);
    let sky_env = mix(vec3(0.22, 0.42, 0.72), scene.sky_horizon.rgb, sky_follow);
    var sky_reflect = sky_env * (0.55 + 0.45 * max(reflect_dir.y, 0.0)) * in.shade;
    // SSR: top faces march the depth buffer for real scene reflections
    // (trees, mountains, clouds on the far shore); the sky fallback
    // remains wherever the ray misses or leaves the screen.
    // Gated on fresnel: head-on water is body color, the mirror only
    // matters at grazing angles — skipping the march there wins back
    // most of its cost.
    if (frame.params.z > 0.5 && in.face == 0u && fresnel > 0.08) {
        let hit = ssr_trace(pc.rel.xyz + in.local, reflect_dir);
        if (hit.z > 0.0) {
            let reflected = textureSampleLevel(scene_color, scene_sampler, hit.xy, 0.0).rgb;
            sky_reflect = mix(sky_reflect, reflected * (0.8 + 0.2 * in.shade), hit.z * 0.85);
        }
    }

    // Sun glint: the ONLY place the ripple acts — a tight specular off
    // the rippled normal breaks into the pixel-sparkle sun path, fading
    // with distance (far water is a calm mirror). scene.sun.xyz is
    // pre-scaled by daylight, so the glint dies at night.
    let daylight = length(scene.sun.xyz);
    var glint = 0.0;
    if (daylight > 0.01 && in.face == 0u) {
        let view_dist = length(pc.rel.xyz + in.local);
        let ripple_fade = 1.0 - smoothstep(24.0, 64.0, view_dist);
        let t = scene.params.x;
        let rippled = ripple_normal(pc.wave_origin.xz + in.local.xz, t);
        let glint_normal = normalize(mix(normal, rippled, ripple_fade));
        let glint_dir = reflect(to_fragment, glint_normal);
        let sun_dir = scene.sun.xyz / daylight;
        glint = pow(max(dot(glint_dir, sun_dir), 0.0), 500.0) * 0.95 * daylight * in.shade;
    }

    var color = mix(base, sky_reflect, fresnel) + vec3(glint);
    // The same horizon fog as terrain, so far water melts into the sky.
    let fog_amount = 1.0 - exp(-pow(linearize(in.clip.z) * 2.0 / scene.fog.w, 2.0));
    color = mix(color, scene.sky_horizon.rgb, fog_amount);
    // Coverage: transparent over shallow bottoms, near-solid when deep or
    // at grazing angles. The waterline itself stays crisp — water meets
    // terrain at block boundaries, so the mesh edge IS the shoreline; a
    // hair of fade only suppresses shimmer where thickness reaches zero.
    let shore = clamp(water_depth / 0.05, 0.0, 1.0);
    let alpha = shore * max(mix(0.35, 0.95, absorb), fresnel * 0.95);
    return vec4<f32>(color, alpha);
}
