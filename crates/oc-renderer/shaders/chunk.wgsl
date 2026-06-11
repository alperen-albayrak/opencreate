// Chunk rendering: packed 8-byte vertices (ARCHITECTURE.md §4).
//   word 0: x:5 | y:5 | z:5 | face:3 | corner:2   (corner positions, 0..=16)
//   word 1: texture layer:16 | light:8

struct PushConstants {
    // proj * view * translate(chunk_origin - camera), camera-relative.
    mvp: mat4x4<f32>,
    // xyz: direction toward the sun (normalized); w: ambient light level.
    sun: vec4<f32>,
}

// `immediate` is WGSL/naga's name for Vulkan push constants.
var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) shade: f32,
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

    var corner_uv = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
    );
    // Sun-direction diffuse per face until real lighting lands (§4.7):
    // +Y, -Y, +Z, -Z, +X, -X
    var face_normal = array<vec3<f32>, 6>(
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, -1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(-1.0, 0.0, 0.0),
    );

    let ambient = pc.sun.w;
    let diffuse = max(dot(face_normal[face], pc.sun.xyz), 0.0);

    var out: VsOut;
    out.clip = pc.mvp * vec4<f32>(pos, 1.0);
    out.uv = corner_uv[corner];
    out.layer = packed.y & 0xFFFFu;
    out.shade = ambient + (1.0 - ambient) * diffuse;
    return out;
}

@group(0) @binding(0) var block_textures: texture_2d_array<f32>;
@group(0) @binding(1) var block_sampler: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(block_textures, block_sampler, in.uv, i32(in.layer));
    return vec4<f32>(texel.rgb * in.shade, 1.0);
}
