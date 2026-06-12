// Sky dome (graphics roadmap stage C): drawn after the opaques at the far
// plane, so only empty pixels shade. Horizon-to-zenith gradient, sun disc
// with glow and a warm band when the sun sits low. HDR values — the sun
// disc overshoots 1.0 on purpose; the tonemap (and later bloom) eat it.

struct PushConstants {
    // Inverse of the camera-relative view-projection (no translation):
    // unprojects a clip position into a world-space view direction.
    inv_view_proj: mat4x4<f32>,
    // xyz: direction toward the sun pre-scaled by daylight; w: ambient.
    sun: vec4<f32>,
    // Horizon and zenith colors for the current moment.
    horizon: vec4<f32>,
    zenith: vec4<f32>,
}

var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var out: VsOut;
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    let ndc = uv * 2.0 - 1.0;
    // z = w: the dome sits exactly on the far plane (depth 1.0), so the
    // LESS_EQUAL test keeps it behind everything that drew geometry.
    out.position = vec4<f32>(ndc, 1.0, 1.0);
    out.ndc = ndc;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let world = pc.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(world.xyz / world.w);

    // Horizon-to-zenith gradient; below the horizon, settle darker.
    let up = clamp(dir.y, -1.0, 1.0);
    var color = mix(pc.horizon.rgb, pc.zenith.rgb, pow(max(up, 0.0), 0.65));
    color = mix(color, pc.horizon.rgb * 0.35, smoothstep(0.0, -0.3, up));

    let daylight = length(pc.sun.xyz);
    if (daylight > 0.005) {
        let sun_dir = pc.sun.xyz / daylight;
        let cos_sun = dot(dir, sun_dir);
        // Warm glow widens and strengthens when the sun rides low.
        let low_sun = 1.0 - smoothstep(0.05, 0.45, sun_dir.y);
        let glow = pow(max(cos_sun, 0.0), 24.0) * (0.18 + 0.55 * low_sun);
        color += vec3(1.0, 0.55, 0.25) * glow * daylight;
        // The disc itself: ~1 degree across, HDR-bright.
        let disc = smoothstep(0.99975, 0.99989, cos_sun);
        color += vec3(4.5, 3.9, 3.0) * disc * daylight;
    }
    return vec4<f32>(color, 1.0);
}
