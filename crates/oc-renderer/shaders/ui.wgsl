// HUD text: screen-space quads sampling the font atlas.

struct PushConstants {
    // Framebuffer size in pixels.
    screen: vec2<f32>,
    // Pixel offset applied to every vertex (used for the drop shadow).
    offset: vec2<f32>,
    color: vec4<f32>,
}

// `immediate` is WGSL/naga's name for Vulkan push constants.
var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    // Pixel coordinates, origin top-left. Vulkan NDC y points down, so no
    // flip is needed.
    let ndc = (pos + pc.offset) / pc.screen * 2.0 - 1.0;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var font_atlas: texture_2d<f32>;
@group(0) @binding(1) var font_sampler: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let alpha = textureSample(font_atlas, font_sampler, in.uv).r;
    return vec4<f32>(pc.color.rgb, pc.color.a * alpha);
}
