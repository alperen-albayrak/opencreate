// Far terrain LOD: coarse colored heightmap tiles beyond the detailed
// chunks. Colors are pre-shaded per vertex; the day cycle and horizon
// fog apply here so the ring matches the world it extends.

struct PushConstants {
    // proj * view * translate(tile_origin - camera), camera-relative.
    mvp: mat4x4<f32>,
    // rgb: fog (horizon) color; w: distance where fog saturates, blocks.
    fog: vec4<f32>,
    // x: daylight (sun strength + ambient floor); yzw: tile origin
    // minus camera (x, z, y) — yes, y rides in w.
    params: vec4<f32>,
    // The loaded-chunk square, camera-relative: (min x, min z, max x,
    // max z). Fragments inside it discard — real terrain lives there.
    cut: vec4<f32>,
}

var<immediate> pc: PushConstants;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // rgb: vertex color; a < 0.5 marks water (view-dependent shading).
    @location(0) color: vec4<f32>,
    // Camera-relative world position.
    @location(1) world_rel: vec3<f32>,
}

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
) -> VsOut {
    var out: VsOut;
    out.clip = pc.mvp * vec4<f32>(position, 1.0);
    out.color = vec4<f32>(color.rgb * pc.params.x, color.a);
    out.world_rel = vec3<f32>(
        pc.params.y + position.x,
        pc.params.w + position.y,
        pc.params.z + position.z,
    );
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
    // The ring exists only beyond the loaded chunks: inside the square,
    // the coarse approximation would poke through real terrain.
    if (in.world_rel.x > pc.cut.x && in.world_rel.z > pc.cut.y
        && in.world_rel.x < pc.cut.z && in.world_rel.z < pc.cut.w) {
        discard;
    }
    var color = in.color.rgb;
    if (in.color.a < 0.5) {
        // Far sea: real water is mostly sky mirror plus a bright seabed,
        // so the ring's sea leans heavily into the sky color — a floor
        // of 35% even head-on, rising with the fresnel grazing term.
        // (Camera sits at the origin of camera-relative space.)
        let view = normalize(in.world_rel);
        let cos_view = max(-view.y, 0.0);
        let fresnel = 0.02 + 0.58 * pow(1.0 - cos_view, 4.0);
        let toward_sky = clamp(0.35 + fresnel * 1.5, 0.0, 1.0);
        color = mix(in.color.rgb, pc.fog.rgb, toward_sky);
    }
    let fog_amount = 1.0 - exp(-pow(linearize(in.clip.z) * 2.0 / pc.fog.w, 2.0));
    return vec4<f32>(mix(color, pc.fog.rgb, fog_amount), 1.0);
}
