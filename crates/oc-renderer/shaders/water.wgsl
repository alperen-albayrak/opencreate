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
    // xyz: chunk origin mod 256 (reserved for texture-scale surface
    // animation; keeps the Rust push layout stable).
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

    // Flat face normals: vanilla-style calm water (no geometric waves —
    // surface motion returns later as texture-scale animation).
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
    let fresnel = 0.02 + 0.40 * pow(1.0 - cos_view, 5.0);

    // Water body color: Beer-Lambert-ish — red dies first, so shallow
    // water reads turquoise and deep water converges to deep blue.
    let absorb = 1.0 - exp(-water_depth * 0.22);
    let texel = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    let ripple = 0.85 + 0.30 * texel.b;
    let base = mix(vec3(0.16, 0.46, 0.52), vec3(0.03, 0.13, 0.32), absorb)
        * ripple * in.shade;

    // Reflection: the sky, pulled toward blue so a pale dawn sky doesn't
    // wash the ocean white; lit by the same shade so cave water doesn't
    // mirror a bright sky. (Stage C's real sky() function replaces this.)
    let reflect_dir = reflect(to_fragment, normal);
    let sky_env = mix(vec3(0.22, 0.42, 0.72), pc.sky.rgb, 0.45);
    let sky_reflect = sky_env * (0.55 + 0.45 * max(reflect_dir.y, 0.0)) * in.shade;

    // Sun glint: tight specular off the perturbed normal. pc.sun.xyz is
    // pre-scaled by daylight, so the glint dies at night.
    let daylight = length(pc.sun.xyz);
    var glint = 0.0;
    if (daylight > 0.01) {
        let sun_dir = pc.sun.xyz / daylight;
        // Flat normals: the glint is the sun's mirror highlight.
        let _anim = pc.rel.w + pc.wave_origin.x; // reserved (see push struct)
        glint = pow(max(dot(reflect_dir, sun_dir), 0.0), 500.0) * 1.2 * daylight * in.shade;
    }

    let color = mix(base, sky_reflect, fresnel) + vec3(glint);
    // Coverage: transparent over shallow bottoms, near-solid when deep
    // or seen at grazing angles; fades out entirely at the waterline.
    let shore = clamp(water_depth / 0.7, 0.0, 1.0);
    let alpha = shore * max(mix(0.30, 0.95, absorb), fresnel * 0.95);
    return vec4<f32>(color, alpha);
}
