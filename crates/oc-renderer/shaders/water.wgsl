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
    // The diffuse term follows actual sun strength (pc.sun.xyz is
    // pre-scaled by daylight), so water goes dark at night like land.
    let daylight = length(pc.sun.xyz);
    let shade = max(
        sky_level * (pc.sun.w + (1.0 - pc.sun.w) * 0.6 * daylight),
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

// Surface ripple: wavelengths of a block or two — texture-scale motion
// on each block's surface, never multi-block swells. Integer cycles
// over 256 blocks keep it seamless across chunk origins (mod 256).
fn ripple_height(p: vec2<f32>, t: f32) -> f32 {
    let tau = 6.28318530718;
    var h = 0.0;
    h += 0.5 * sin(tau * dot(p, vec2(160.0, 40.0)) / 256.0 + t * 2.0);
    h += 0.3 * sin(tau * dot(p, vec2(-88.0, 132.0)) / 256.0 + t * 2.6);
    h += 0.2 * sin(tau * dot(p, vec2(208.0, -160.0)) / 256.0 + t * 3.3);
    return h;
}

// Pixel-art shimmer: positions snap to the 16x16 texel grid and time
// steps at 10 fps, so the glint is crisp texel-sized sparkles (visible
// mostly in the light reflection), not smooth gradients.
fn ripple_normal(p: vec2<f32>, t: f32) -> vec3<f32> {
    let cell = floor(p * 16.0) / 16.0;
    let stepped = floor(t * 10.0) / 10.0;
    let e = 1.0 / 16.0;
    let h0 = ripple_height(cell, stepped);
    let hx = ripple_height(cell + vec2(e, 0.0), stepped);
    let hz = ripple_height(cell + vec2(0.0, e), stepped);
    return normalize(vec3((h0 - hx) * 0.9, e * 2.0, (h0 - hz) * 0.9));
}

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

    // Geometry stays flat; only a texture-scale ripple perturbs the top
    // normal (the sparkling sun path), never multi-block swells — and it
    // fades with distance: far water is a calm mirror, like the genre
    // look (shimmer is a near-field effect).
    let t = pc.rel.w;
    let view_dist = length(pc.rel.xyz + in.local);
    let ripple_fade = 1.0 - smoothstep(24.0, 72.0, view_dist);
    var normal: vec3<f32>;
    if (in.face == 0u) {
        let rippled = ripple_normal(pc.wave_origin.xz + in.local.xz, t);
        normal = normalize(mix(vec3(0.0, 1.0, 0.0), rippled, ripple_fade));
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
    let sky_env = mix(vec3(0.22, 0.42, 0.72), pc.sky.rgb, sky_follow);
    let sky_reflect = sky_env * (0.55 + 0.45 * max(reflect_dir.y, 0.0)) * in.shade;

    // Sun glint: tight specular off the perturbed normal. pc.sun.xyz is
    // pre-scaled by daylight, so the glint dies at night.
    let daylight = length(pc.sun.xyz);
    var glint = 0.0;
    if (daylight > 0.01) {
        let sun_dir = pc.sun.xyz / daylight;
        // The ripple breaks the glint into the sparkling sun path.
        glint = pow(max(dot(reflect_dir, sun_dir), 0.0), 500.0) * 1.3 * daylight * in.shade;
    }

    let color = mix(base, sky_reflect, fresnel) + vec3(glint);
    // Coverage: transparent over shallow bottoms, near-solid when deep or
    // at grazing angles. The waterline itself stays crisp — water meets
    // terrain at block boundaries, so the mesh edge IS the shoreline; a
    // hair of fade only suppresses shimmer where thickness reaches zero.
    let shore = clamp(water_depth / 0.05, 0.0, 1.0);
    let alpha = shore * max(mix(0.35, 0.95, absorb), fresnel * 0.95);
    return vec4<f32>(color, alpha);
}
