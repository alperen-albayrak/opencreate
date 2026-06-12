// Bloom mip chain (graphics roadmap stage E): dual-Kawase downsample
// and upsample passes over a half-resolution HDR mip pyramid. The first
// downsample soft-thresholds so only HDR highlights (sun, glints, lamps)
// bleed; upsamples blend additively back up the chain, and the tonemap
// pass mixes mip 0 over the scene.

struct PushConstants {
    // xy: source texel size (1 / source dimensions);
    // z: 1.0 on the first downsample (apply the highlight threshold);
    // w: unused.
    params: vec4<f32>,
}

var<immediate> pc: PushConstants;

@group(0) @binding(0) var src_texture: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One triangle covering the screen, no vertex buffer.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    var out: VertexOut;
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    out.uv = uv;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

// Soft-knee highlight pass: zero below (threshold - knee), quadratic
// ramp through the knee, linear above. Keeps faint pixels out of the
// blur without a hard pop at the threshold.
fn threshold(color: vec3<f32>) -> vec3<f32> {
    let t = 1.0; // HDR brightness where bloom starts
    let k = 0.5; // knee width
    let brightness = max(color.r, max(color.g, color.b));
    var soft = clamp(brightness - t + k, 0.0, 2.0 * k);
    soft = soft * soft / (4.0 * k);
    let contribution = max(soft, brightness - t) / max(brightness, 0.0001);
    return color * max(contribution, 0.0);
}

@fragment
fn fs_down(in: VertexOut) -> @location(0) vec4<f32> {
    let t = pc.params.xy;
    var sum = textureSample(src_texture, src_sampler, in.uv).rgb * 4.0;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(-t.x, -t.y)).rgb;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(t.x, -t.y)).rgb;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(-t.x, t.y)).rgb;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(t.x, t.y)).rgb;
    var color = sum / 8.0;
    if (pc.params.z > 0.5) {
        color = threshold(color);
    }
    return vec4<f32>(color, 1.0);
}

// Upsample tent; the pipeline blends this additively into the larger mip.
@fragment
fn fs_up(in: VertexOut) -> @location(0) vec4<f32> {
    let t = pc.params.xy;
    var sum = textureSample(src_texture, src_sampler, in.uv + vec2(-2.0 * t.x, 0.0)).rgb;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(-t.x, t.y)).rgb * 2.0;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(0.0, 2.0 * t.y)).rgb;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(t.x, t.y)).rgb * 2.0;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(2.0 * t.x, 0.0)).rgb;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(t.x, -t.y)).rgb * 2.0;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(0.0, -2.0 * t.y)).rgb;
    sum += textureSample(src_texture, src_sampler, in.uv + vec2(-t.x, -t.y)).rgb * 2.0;
    return vec4<f32>(sum / 12.0, 1.0);
}
