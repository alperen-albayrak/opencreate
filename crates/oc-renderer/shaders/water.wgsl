// Water pass (graphics roadmap stage B): same packed vertices as chunk.wgsl,
// drawn blended after opaques in its own render pass. Scrolling procedural
// wave normals, Schlick fresnel, sky reflection, sun glint, and — from the
// sampled opaque depth — Beer-Lambert absorption (shallow turquoise to deep
// blue), soft shorelines and in-shader occlusion (the pass has no depth
// attachment). Refraction/SSR arrive with the stage-E snapshot tier.

struct PushConstants {
    // proj * view * translate(chunk_origin - camera), camera-relative.
    mvp: mat4x4<f32>,
    // xyz: direction toward the sun, pre-scaled by daylight; w: ambient.
    sun: vec4<f32>,
    // Sky/horizon color this frame (rgb) — the reflection environment.
    sky: vec4<f32>,
    // xyz: chunk origin camera-relative (view vector); w: time in seconds.
    rel: vec4<f32>,
    // xyz: chunk origin mod 256 (wave phase stays fp32-exact far from
    // the origin because every wave period divides 256).
    wave_origin: vec4<f32>,
}

var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) shade: f32,
    @location(3) local: vec3<f32>,
    @location(4) @interpolate(flat) face: u32,
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

    let light = (packed.y >> 16u) & 0xFFu;
    let sky_level = f32(light >> 4u) / 15.0;
    let block_level = f32(light & 15u) / 15.0;
    let shade = max(sky_level * (pc.sun.w + (1.0 - pc.sun.w) * 0.6), block_level * 0.95);

    var out: VsOut;
    out.clip = pc.mvp * vec4<f32>(pos, 1.0);
    out.uv = corner_uv[corner] * extent;
    out.layer = packed.y & 0xFFFFu;
    out.shade = shade;
    out.local = pos;
    out.face = face;
    return out;
}

@group(0) @binding(0) var block_textures: texture_2d_array<f32>;
@group(0) @binding(1) var block_sampler: sampler;
@group(1) @binding(0) var opaque_depth: texture_depth_2d;

// Must match the projection in camera.rs.
const NEAR: f32 = 0.05;
const FAR: f32 = 4096.0;

fn linearize(depth: f32) -> f32 {
    return NEAR * FAR / (FAR - depth * (FAR - NEAR));
}

// Sum of directional sines; wave vectors are integer cycles over 256
// blocks, so phase is seamless across the mod-256 chunk origins.
fn wave_height(p: vec2<f32>, t: f32) -> f32 {
    let tau = 6.28318530718;
    var h = 0.0;
    h += 0.50 * sin(tau * (dot(p, vec2(16.0, 4.0)) / 256.0) + t * 1.3);
    h += 0.30 * sin(tau * (dot(p, vec2(-6.0, 14.0)) / 256.0) + t * 1.7);
    h += 0.20 * sin(tau * (dot(p, vec2(9.0, -11.0)) / 256.0) + t * 2.3);
    // Short chop: breaks the sun glint into sparkle.
    h += 0.08 * sin(tau * (dot(p, vec2(43.0, 51.0)) / 256.0) + t * 3.1);
    h += 0.06 * sin(tau * (dot(p, vec2(-57.0, 38.0)) / 256.0) + t * 3.7);
    return h;
}

fn wave_normal(p: vec2<f32>, t: f32) -> vec3<f32> {
    let e = 0.35;
    let h0 = wave_height(p, t);
    let hx = wave_height(p + vec2(e, 0.0), t);
    let hz = wave_height(p + vec2(0.0, e), t);
    // Height units are visual only; the slope strength sets sparkle.
    return normalize(vec3((h0 - hx) * 0.55, e, (h0 - hz) * 0.55));
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

    let t = pc.rel.w;
    let wave_pos = pc.wave_origin.xz + in.local.xz;

    // Perturbed normal on horizontal surfaces; flat on the sides.
    var normal: vec3<f32>;
    if (in.face == 0u) {
        normal = wave_normal(wave_pos, t);
    } else if (in.face == 1u) {
        normal = vec3(0.0, -1.0, 0.0);
    } else {
        var side = array<vec3<f32>, 6>(
            vec3(0.0, 1.0, 0.0), vec3(0.0, -1.0, 0.0),
            vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0),
            vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0),
        );
        normal = side[in.face];
    }

    // Camera sits at the origin of camera-relative space.
    let to_fragment = normalize(pc.rel.xyz + in.local);
    let cos_view = max(dot(normal, -to_fragment), 0.0);
    // Schlick fresnel, F0 = 0.02 (water), capped so distant water stays
    // water-colored instead of a full sky mirror.
    let fresnel = 0.02 + 0.68 * pow(1.0 - cos_view, 5.0);

    // Water body color: Beer-Lambert-ish — red dies first, so shallow
    // water reads turquoise and deep water converges to deep blue.
    let absorb = 1.0 - exp(-water_depth * 0.22);
    let texel = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    let ripple = 0.85 + 0.30 * texel.b;
    let base = mix(vec3(0.16, 0.46, 0.52), vec3(0.03, 0.13, 0.32), absorb)
        * ripple * in.shade;

    // Reflection: the sky environment, blue-shifted and rippled by the
    // wave normal (R.y varies per pixel), lit by the same shade so cave
    // water doesn't mirror a bright sky.
    let reflect_dir = reflect(to_fragment, normal);
    let sky_reflect = pc.sky.rgb * vec3(0.75, 0.85, 1.0)
        * (0.30 + 0.70 * max(reflect_dir.y, 0.0)) * in.shade;

    // Sun glint: tight specular off the perturbed normal. pc.sun.xyz is
    // pre-scaled by daylight, so the glint dies at night.
    let daylight = length(pc.sun.xyz);
    var glint = 0.0;
    if (daylight > 0.01) {
        let sun_dir = pc.sun.xyz / daylight;
        glint = pow(max(dot(reflect_dir, sun_dir), 0.0), 600.0) * 1.4 * daylight * in.shade;
    }

    let color = mix(base, sky_reflect, fresnel) + vec3(glint);
    // Coverage: transparent over shallow bottoms, near-solid when deep
    // or seen at grazing angles; fades out entirely at the waterline.
    let shore = clamp(water_depth / 0.7, 0.0, 1.0);
    let alpha = shore * max(mix(0.30, 0.95, absorb), fresnel * 0.95);
    return vec4<f32>(color, alpha);
}
