// Fullscreen resolve: samples the offscreen HDR world target, tonemaps
// (ACES fit), dithers, and writes to the sRGB swapchain (hardware encodes).

@group(0) @binding(0) var hdr_texture: texture_2d<f32>;
@group(0) @binding(1) var hdr_sampler: sampler;

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

// Narkowicz ACES approximation: filmic shoulder, keeps saturated voxel
// colors from clipping to white too early.
fn aces(color: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((color * (a * color + b)) / (color * (c * color + d) + e), vec3(0.0), vec3(1.0));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let hdr = textureSample(hdr_texture, hdr_sampler, in.uv).rgb;
    var color = aces(hdr);
    // 1-LSB dither breaks up sky-gradient banding.
    let noise = fract(sin(dot(in.position.xy, vec2(12.9898, 78.233))) * 43758.5453);
    color += (noise - 0.5) / 255.0;
    return vec4<f32>(color, 1.0);
}
