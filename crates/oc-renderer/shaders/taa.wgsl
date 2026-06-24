// Temporal anti-aliasing resolve (graphics roadmap P5). A fullscreen pass run
// after the scene fully composites into the HDR target: it blends the current
// (sub-pixel-jittered) frame with the reprojected history, so voxel edges,
// per-texel normal maps and specular highlights stop shimmering ("blinks by
// pixel") — the accumulated samples converge to a clean, anti-aliased image.
//
// Camera-only reprojection (no velocity buffer yet): the previous-frame screen
// position of each pixel is found from its depth and a single reprojection
// matrix `VP_prev · translate(camera_delta) · inv(VP_cur)` (the camera delta is
// folded in CPU-side, so camera-relative rendering reprojects correctly). A
// YCoCg neighborhood clamp rejects stale history (disocclusions, moving
// shadows), trading a little ghosting-resistance for sharpness.

@group(0) @binding(0) var current: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var history: texture_2d<f32>;
@group(0) @binding(3) var depth_tex: texture_depth_2d;

struct TaaPush {
    // VP_prev · translate(camera_delta) · inv(VP_cur): current NDC → previous NDC.
    reproj: mat4x4<f32>,
    // x: history valid (1 = blend, 0 = passthrough — first frame / resize /
    // teleport / TAA off); y: history feedback weight (≈0.9); zw: unused.
    params: vec4<f32>,
}
var<immediate> pc: TaaPush;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

// Reversible RGB↔YCoCg (luma + two chroma); the neighborhood clamp works in
// this space because the luma axis bounds most temporal error tightly.
fn rgb_to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}
fn ycocg_to_rgb(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let cur = textureLoad(current, px, 0).rgb;

    // Passthrough: first frame after a history reset, or TAA disabled. The
    // resolved image still feeds exposure/bloom/tonemap, so this is a straight
    // copy of the (un-jittered) current frame.
    if (pc.params.x < 0.5) {
        return vec4<f32>(cur, 1.0);
    }

    let dims = vec2<f32>(textureDimensions(current));
    let depth = textureLoad(depth_tex, px, 0);
    // Reproject this pixel into the previous frame (sky at depth==1 reprojects by
    // rotation, which is what we want — the camera delta is negligible at infinity).
    let ndc = vec3<f32>((frag.xy / dims) * 2.0 - 1.0, depth);
    let clip_prev = pc.reproj * vec4<f32>(ndc, 1.0);
    let uv_prev = (clip_prev.xy / clip_prev.w) * 0.5 + 0.5;
    // History off-screen (disocclusion at the frame edge) or behind the camera:
    // no usable history, take the current sample.
    if (clip_prev.w <= 0.0 || any(uv_prev < vec2<f32>(0.0)) || any(uv_prev > vec2<f32>(1.0))) {
        return vec4<f32>(cur, 1.0);
    }

    // Variance clip (Salvi/Karis) of the history toward the 3×3 neighborhood,
    // in YCoCg: bound by the colour distribution's mean ± γ·σ rather than a hard
    // min/max box. This is what keeps a *still* screen stable on high-frequency
    // (filtered-but-busy) content — a min/max box pumps as the jitter shifts the
    // extremes frame to frame; the statistical bound centred on the mean does not.
    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let s = rgb_to_ycocg(textureLoad(current, px + vec2<i32>(dx, dy), 0).rgb);
            m1 += s;
            m2 += s * s;
        }
    }
    let inv = 1.0 / 9.0;
    let mean = m1 * inv;
    let sigma = sqrt(max(m2 * inv - mean * mean, vec3<f32>(0.0)));
    let gamma = 1.25;
    let mn = mean - gamma * sigma;
    let mx = mean + gamma * sigma;

    let cur_y = rgb_to_ycocg(cur);
    // Explicit LOD: single-mip history, and textureSample's implicit LOD is
    // illegal after the conditional returns above (non-uniform control flow).
    var hist = rgb_to_ycocg(textureSampleLevel(history, samp, uv_prev, 0.0).rgb);
    hist = clamp(hist, mn, mx);

    let resolved = mix(cur_y, hist, pc.params.y);
    return vec4<f32>(max(ycocg_to_rgb(resolved), vec3<f32>(0.0)), 1.0);
}
