// Targeted-block outline: a line-list unit cube transformed per draw.

struct PushConstants {
    // proj * view * translate(block_origin - camera), camera-relative.
    mvp: mat4x4<f32>,
}

// `immediate` is WGSL/naga's name for Vulkan push constants.
var<immediate> pc: PushConstants;

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return pc.mvp * vec4<f32>(pos, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.05, 0.05, 0.05, 1.0);
}
