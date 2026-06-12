// Blocky cloud layer (stage C2): plain shaded slabs, color and opacity
// from the day cycle (pushed per tile).

struct PushConstants {
    mvp: mat4x4<f32>,
    // rgb: cloud color for this moment; a: layer opacity.
    color: vec4<f32>,
}

var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) shade: f32,
}

@vertex
fn vs_main(@location(0) pos: vec3<f32>, @location(1) shade: f32) -> VsOut {
    var out: VsOut;
    out.clip = pc.mvp * vec4<f32>(pos, 1.0);
    out.shade = shade;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(pc.color.rgb * in.shade, pc.color.a);
}
