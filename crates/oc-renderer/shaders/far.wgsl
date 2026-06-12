// Far terrain LOD: coarse colored heightmap tiles beyond the detailed
// chunks. Colors are pre-shaded per vertex; the day cycle and horizon
// fog apply here so the ring matches the world it extends.

struct PushConstants {
    // proj * view * translate(tile_origin - camera), camera-relative.
    mvp: mat4x4<f32>,
    // rgb: fog (horizon) color; w: distance where fog saturates, blocks.
    fog: vec4<f32>,
    // x: daylight (sun strength + ambient floor); yzw: unused.
    params: vec4<f32>,
}

var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
}

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.clip = pc.mvp * vec4<f32>(position, 1.0);
    out.color = color.rgb * pc.params.x;
    return out;
}

// Must match the projection in camera.rs.
const NEAR: f32 = 0.05;
const FAR: f32 = 4096.0;

fn linearize(depth: f32) -> f32 {
    return NEAR * FAR / (FAR - depth * (FAR - NEAR));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let fog_amount = 1.0 - exp(-pow(linearize(in.clip.z) * 2.0 / pc.fog.w, 2.0));
    return vec4<f32>(mix(in.color, pc.fog.rgb, fog_amount), 1.0);
}
