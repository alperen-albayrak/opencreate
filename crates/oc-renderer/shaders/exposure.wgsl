// Auto-exposure measurement (graphics roadmap stage E): a 16x16 grid of
// log2-luminance samples over the HDR scene. The CPU reads the grid back
// (two frames later), averages it — a geometric mean, so the sun can't
// drag the whole frame — and eases the tonemap exposure toward the target.

@group(0) @binding(0) var hdr_texture: texture_2d<f32>;
@group(0) @binding(1) var hdr_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    var out: VertexOut;
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    out.uv = uv;
    out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) f32 {
    let color = textureSample(hdr_texture, hdr_sampler, in.uv).rgb;
    let lum = dot(color, vec3(0.2126, 0.7152, 0.0722));
    return log2(clamp(lum, 0.0001, 64.0));
}
