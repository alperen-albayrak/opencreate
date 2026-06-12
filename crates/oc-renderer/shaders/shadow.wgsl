// Shadow cascade pass (graphics roadmap stage D): depth-only re-render
// of solid chunk geometry from the sun's orthographic view. Same packed
// vertex as chunk.wgsl; only the position bits matter here.

struct PushConstants {
    // cascade view-proj * translate(chunk_origin - camera)
    mvp: mat4x4<f32>,
}

var<immediate> pc: PushConstants;

@vertex
fn vs_main(@location(0) packed: vec2<u32>) -> @builtin(position) vec4<f32> {
    let w0 = packed.x;
    let pos = vec3<f32>(
        f32(w0 & 31u),
        f32((w0 >> 5u) & 31u),
        f32((w0 >> 10u) & 31u),
    );
    return pc.mvp * vec4<f32>(pos, 1.0);
}

@fragment
fn fs_main() {}
