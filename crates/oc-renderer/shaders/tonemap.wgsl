// Fullscreen resolve: samples the offscreen HDR world target, tonemaps
// (ACES fit), dithers, and writes to the sRGB swapchain (hardware encodes).

@group(0) @binding(0) var hdr_texture: texture_2d<f32>;
@group(0) @binding(1) var hdr_sampler: sampler;
@group(0) @binding(2) var bloom_texture: texture_2d<f32>;

struct PushConstants {
    // x: auto-exposure multiplier; y: measured scene luminance (Purkinje gate);
    // z: contrast amount; w: saturation.
    params: vec4<f32>,
    // rgb: white-balance multiplier (Kelvin); w: Purkinje night-shift strength.
    // All neutral (1,1,1 / 0) when the colour grade is off → plain ACES.
    grade: vec4<f32>,
}

var<immediate> pc: PushConstants;

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

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
    // Bloom: thresholded highlight pyramid, gently added before the
    // tonemap so the sun, glints and lamps glow.
    let bloom = textureSample(bloom_texture, hdr_sampler, in.uv).rgb;
    // Auto-exposure scales the whole frame before the tonemap: the eye
    // opens up in caves and at night, stops down against bright snow.
    var lit = (hdr + bloom * 0.35) * pc.params.x;
    // White balance (Kelvin), in linear before the filmic curve.
    lit *= pc.grade.rgb;
    var color = aces(lit);

    // --- colour grade (P6), all neutral when the grade is off ---
    // Saturation about luma.
    let luma = dot(color, LUMA);
    color = max(mix(vec3<f32>(luma), color, pc.params.w), vec3<f32>(0.0));
    // Gentle contrast S-curve (smoothstep) about mid-grey.
    let scurve = color * color * (3.0 - 2.0 * color);
    color = clamp(mix(color, scurve, pc.params.z), vec3<f32>(0.0), vec3<f32>(1.0));
    // Purkinje scotopic night-shift: as the *scene* darkens, human vision moves
    // to the rods — desaturated, blue-shifted, red-weak. Gated on absolute scene
    // luminance (params.y), so daylight stays untouched even after the eye has
    // adapted and the screen looks bright.
    let night = smoothstep(0.07, 0.012, pc.params.y) * pc.grade.w;
    let rod = dot(color, vec3<f32>(0.10, 0.50, 0.40)); // scotopic: blue-green peak, red-blind
    let scotopic = rod * vec3<f32>(0.60, 0.78, 1.12);  // cool blue-grey night cast
    color = mix(color, scotopic, night);

    // 1-LSB dither breaks up sky-gradient banding.
    let noise = fract(sin(dot(in.position.xy, vec2(12.9898, 78.233))) * 43758.5453);
    color += (noise - 0.5) / 255.0;
    return vec4<f32>(color, 1.0);
}
