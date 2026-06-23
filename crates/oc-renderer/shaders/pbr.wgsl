// Deferred lighting pass (graphics roadmap Stage E): a fullscreen triangle
// reads the G-buffer written by chunk_gbuffer.wgsl plus the per-frame Scene
// UBO and resolves the opaque world's lit color into the HDR target.
//
// This reproduces the old forward chunk shading exactly — sky ambient + sun
// diffuse (shadow-darkenable) vs. RGB block light, AO on the indirect terms,
// caustics already folded into the G-buffer albedo, distance fog last — but
// now per-pixel from the G-buffer, the seam every later lighting feature
// (Cook-Torrance specular, SSAO, many point lights) plugs into.

struct Scene {
    // xyz: direction toward the sun (scaled by daylight); w: ambient level.
    sun: vec4<f32>,
    // rgb: distance-fog (horizon) color; w: fog saturation distance, blocks.
    fog: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_away: vec4<f32>,
    sky_sun: vec4<f32>,
    // x: time; y: base ambient floor; z: camera world Y; w: thermal point count.
    params: vec4<f32>,
    // Temperature-vs-height curve, ascending by Y, two points per vec4 as
    // (y0, temp0 °C, y1, temp1) — up to 8 points; `params.w` is the count.
    thermal_profile: array<vec4<f32>, 4>,
}
@group(1) @binding(0) var<uniform> scene: Scene;

@group(0) @binding(0) var gb0: texture_2d<f32>;
@group(0) @binding(1) var gb1: texture_2d<f32>;
@group(0) @binding(2) var gb2: texture_2d<f32>;
@group(0) @binding(3) var gbuf_depth: texture_depth_2d;

// Sun shadow cascades (set 2) — the same dormant plumbing the forward chunk
// shader used. `shadow.params.x` (strength) is 0 while shadows are shelved, so
// `sun_visibility` returns 1.0 and this is a no-op until they're toggled on.
struct ShadowData {
    // Camera-relative world -> cascade clip.
    matrices: array<mat4x4<f32>, 3>,
    // Cascade far distances; w: shadow style (0 = soft PCF, 1 = blocky).
    splits: vec4<f32>,
    // x: strength (0 = off/night); yzw: world units per texel, per cascade.
    params: vec4<f32>,
}
@group(2) @binding(0) var<uniform> shadow: ShadowData;
@group(2) @binding(1) var shadow_map: texture_depth_2d_array;
@group(2) @binding(2) var shadow_sampler: sampler_comparison;

// Dynamic point lights (set 3): emissive blocks casting real coloured light +
// specular. Positions are camera-relative — the same space `world_rel` is
// rebuilt in. Mirrors `pointlights::PointLightData`.
struct PointLight {
    // xyz: camera-relative position; w: radius (blocks).
    pos_radius: vec4<f32>,
    // rgb: colour; w: peak intensity.
    color_intensity: vec4<f32>,
}
struct PointLights {
    // x: active light count (0..=64).
    header: vec4<f32>,
    lights: array<PointLight, 64>,
}
@group(3) @binding(0) var<uniform> point_lights: PointLights;

// Rebuilds the camera-relative world position from the G-buffer depth for the
// cascade lookup (the inverse of the view-projection the chunks rendered with).
struct LightPush {
    inv_view_proj: mat4x4<f32>,
}
var<immediate> pc: LightPush;

// Must match the projection in camera.rs.
const NEAR: f32 = 0.05;
const FAR: f32 = 4096.0;
// Must match shadow.rs MAP_SIZE.
const SHADOW_MAP_SIZE: f32 = 2048.0;
const PI: f32 = 3.14159265359;
// Dev diagnostic: when true, fs_main returns the unpacked GB1.w material
// (roughness as grayscale, metal tinted red) instead of the lit color, to
// confirm per-block roughness/metalness flows through the G-buffer.
const DEBUG_MATERIAL: bool = false;

fn linearize(depth: f32) -> f32 {
    return NEAR * FAR / (FAR - depth * (FAR - NEAR));
}

// Inverse of chunk_gbuffer.wgsl's oct_encode.
fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    let f = e * 2.0 - 1.0;
    var n = vec3<f32>(f.x, f.y, 1.0 - abs(f.x) - abs(f.y));
    let t = max(-n.z, 0.0);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return normalize(n);
}

// PCF visibility from one cascade (ported verbatim from the forward chunk
// shader): depth-biased by the cascade's texel size, one bilinear comparison.
fn cascade_lit(cascade: i32, world_rel: vec3<f32>, normal: vec3<f32>, n_dot_l: f32) -> f32 {
    let texel_world = shadow.params[1u + u32(cascade)];
    // Normal-offset bias is the primary acne lever: push the sample off the
    // surface along the exact (flat, noise-free) face normal, scaled by the
    // cascade's world-per-texel and by the grazing factor — the offset a flat
    // surface needs grows as the sun nears the horizon. Stays < 1 block on
    // every cascade, so shadows never detach (peter-pan).
    let grazing = clamp(1.0 - n_dot_l, 0.0, 1.0);
    // Modest normal offset: enough to keep flat ground off its own surface
    // (the near-cascade self-shadow that read as a camera-following seam),
    // small enough that small caster shadows (a tree, a placed block) don't
    // erode away. Texel-scaled so each cascade self-shadows consistently.
    let offset = texel_world * (1.0 + 2.0 * grazing);
    var pos = world_rel + normal * offset;
    // Blocky style: snap the sample to the 1/16-block grid (16 texels per
    // block) so the shadow edge aligns to block texels.
    let blocky = shadow.splits.w > 0.5;
    if (blocky) {
        pos = floor(pos * 16.0) / 16.0;
    }
    let ndc = shadow.matrices[cascade] * vec4<f32>(pos, 1.0);
    let uv = ndc.xy * 0.5 + vec2(0.5);
    // Outside this cascade's box: sentinel so the caller falls through to a
    // larger cascade (NOT "lit" — returning 1.0 here dropped shadows at the
    // screen edges, where wide-angle pixels sit inside the view-Z split but
    // outside the near cascade's light-space box).
    if (any(uv < vec2(0.0)) || any(uv > vec2(1.0))) {
        return -1.0;
    }
    // Minimal residual depth bias — the normal offset does the heavy lifting.
    let d = ndc.z - (0.00015 + texel_world / 400.0);
    if (blocky) {
        return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, cascade, d);
    }
    // Soft PCF (default): four bilinear-comparison taps spread one texel
    // across and averaged — the LINEAR comparison sampler makes each a 2×2, so
    // this is a smooth ~3×3 kernel, cheap on the fullscreen lighting pass.
    let t = 1.0 / SHADOW_MAP_SIZE;
    var s = textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(-0.5, -0.5) * t, cascade, d);
    s += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(0.5, -0.5) * t, cascade, d);
    s += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(-0.5, 0.5) * t, cascade, d);
    s += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(0.5, 0.5) * t, cascade, d);
    return s * 0.25;
}

// How much of the sun reaches this fragment. Picks the FIRST (smallest)
// cascade whose light-space box actually contains the point, falling through
// to larger cascades when a point lies outside a smaller one — the near
// cascade only covers ±radius perpendicular to the sun, so a wide-angle
// screen-edge pixel (small view-Z, large lateral offset) is outside cascade 0
// but inside 1 or 2. Selecting by view-Z alone dropped those edge shadows.
fn sun_visibility(world_rel: vec3<f32>, normal: vec3<f32>, view_dist: f32, n_dot_l: f32) -> f32 {
    let strength = shadow.params.x;
    if (strength <= 0.0) {
        return 1.0;
    }
    var vis = -1.0;
    for (var c = 0; c < 3; c = c + 1) {
        vis = cascade_lit(c, world_rel, normal, n_dot_l);
        if (vis >= 0.0) {
            break;
        }
    }
    // Beyond every cascade's coverage: fully lit.
    if (vis < 0.0) {
        return 1.0;
    }
    // Ease shadows out toward the far cascade's edge (and twilight via strength).
    let range_fade = smoothstep(shadow.splits.z * 0.8, shadow.splits.z, view_dist);
    return mix(1.0, mix(vis, 1.0, range_fade), strength);
}

// Accurate blackbody colour (normalised sRGB) from temperature — the
// Tanner-Helland fit, identical to oc_core::physical::blackbody_rgb. Continuous
// and granular, not stepped: dull red ~800 K, orange ~1500 K, yellow ~1900 K,
// toward white above ~6000 K.
fn blackbody_rgb(temp_k: f32) -> vec3<f32> {
    let t = clamp(temp_k, 1000.0, 40000.0) / 100.0;
    var r: f32;
    var g: f32;
    var b: f32;
    if (t <= 66.0) {
        r = 255.0;
        g = clamp(99.4708025861 * log(t) - 161.1195681661, 0.0, 255.0);
    } else {
        r = clamp(329.698727446 * pow(t - 60.0, -0.1332047592), 0.0, 255.0);
        g = clamp(288.1221695283 * pow(t - 60.0, -0.0755148492), 0.0, 255.0);
    }
    if (t >= 66.0) {
        b = 255.0;
    } else if (t <= 19.0) {
        b = 0.0;
    } else {
        b = clamp(138.5177312231 * log(t - 10.0) - 305.0447927307, 0.0, 255.0);
    }
    return vec3<f32>(r, g, b) / 255.0;
}

// Incandescent self-glow past the Draper point (~525 °C): the matter's real
// blackbody colour, scaled by how strongly it radiates (rises with
// temperature), HDR so hot matter blooms. Only opaque geometry is shaded here,
// so hot air never glows. (Per-material emissivity will scale this once it's a
// material field; rock/lava are near-full emitters for now.)
fn blackbody_glow(temp_c: f32) -> vec3<f32> {
    if (temp_c <= 525.0) {
        return vec3<f32>(0.0);
    }
    let color = blackbody_rgb(temp_c + 273.15);
    let heat = clamp((temp_c - 525.0) / 775.0, 0.0, 1.0);
    return color * pow(heat, 1.5) * 2.5;
}

// Cook-Torrance specular BRDF (GGX/Trowbridge-Reitz D, Smith-Schlick G,
// Fresnel-Schlick F) for one directional light, returned as the reflected
// radiance per unit incoming radiance — i.e. `D·G·F / (4·n·v)`, the cosine
// `n·l` already cancelled against the geometry term, so the caller multiplies
// only by the light's radiance. Dielectrics use F0 = 0.04; metals tint F0 by
// albedo (their "diffuse" colour becomes the reflection colour). Smooth
// surfaces (ice, obsidian) get a tight bright glint; a fully rough surface
// (roughness 1) spreads the lobe so thin the highlight is imperceptible — so
// matte blocks are visually unchanged without any special-casing.
fn ggx_specular(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, albedo: vec3<f32>, roughness: f32, metal: bool) -> vec3<f32> {
    let n_dot_l = dot(n, l);
    if (n_dot_l <= 0.0) {
        return vec3<f32>(0.0);
    }
    let h = normalize(v + l);
    let n_dot_v = max(dot(n, v), 1e-4);
    let n_dot_h = max(dot(n, h), 0.0);
    let v_dot_h = max(dot(v, h), 0.0);

    // Clamp roughness off zero so the mirror-angle peak stays finite (it still
    // spikes to a bright, bloom-catching glint, the intended look).
    let r = max(roughness, 0.045);
    let a = r * r;
    let a2 = a * a;
    let d_denom = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    let d = a2 / (PI * d_denom * d_denom);

    // Smith geometry with the Schlick-GGX direct-lighting remap of k.
    let k = (r + 1.0) * (r + 1.0) / 8.0;
    let g_v = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let g_l = n_dot_l / (n_dot_l * (1.0 - k) + k);
    let g = g_v * g_l;

    let f0 = mix(vec3<f32>(0.04), albedo, select(0.0, 1.0, metal));
    let f = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - v_dot_h, 5.0);

    // n·l fold: full term is D·G·F/(4·n·v·n·l)·n·l, the n·l cancels.
    return (d * g) * f / (4.0 * n_dot_v);
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    // Oversized fullscreen triangle covering the viewport.
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(frag.xy);
    let depth = textureLoad(gbuf_depth, px, 0);
    // Background (no opaque geometry): keep the cleared sky color for the
    // sky dome / clouds drawn in the following forward pass.
    if (depth >= 1.0) {
        discard;
    }

    let g0 = textureLoad(gb0, px, 0);
    let g1 = textureLoad(gb1, px, 0);
    let g2 = textureLoad(gb2, px, 0);

    let albedo = g0.rgb;
    let ao = g0.a;
    let normal = oct_decode(g1.xy);
    let sky_vis = g1.z;
    // Block light already carries its 0.95 trim and AO from the geometry pass.
    let block_light = g2.rgb;

    // Surface material unpacked from GB1.w (texture::pack_material): the top
    // code bit (≥128) is the metal flag, the low 7 bits are roughness×127.
    let mat_code = round(g1.w * 255.0);
    let metal = mat_code >= 127.5;
    let roughness = (mat_code - select(0.0, 128.0, metal)) / 127.0;

    // DEBUG (Stage 4.1 verify): show the unpacked material directly — roughness
    // as grayscale (dark = smooth/shiny, white = matte), metal tinted red — to
    // prove per-block material reaches GB1.w before any specular math lands.
    if (DEBUG_MATERIAL) {
        return vec4<f32>(select(vec3<f32>(roughness), vec3<f32>(1.0, 0.2, 0.0), metal), 1.0);
    }

    let view_dist = linearize(depth);
    // Rebuild the camera-relative world position from depth for the cascade
    // lookup (visually a no-op while shadows are shelved: sun_vis stays 1.0).
    let dims = vec2<f32>(textureDimensions(gb0));
    let ndc = vec3<f32>((frag.xy / dims) * 2.0 - 1.0, depth);
    let world_h = pc.inv_view_proj * vec4<f32>(ndc, 1.0);
    let world_rel = world_h.xyz / world_h.w;
    // Geometric grazing factor from the *unscaled* sun direction (scene.sun.xyz
    // is daylight-scaled, so normalize it): the shadow normal-offset bias
    // grows as this falls toward 0.
    let sun_dir_n = scene.sun.xyz / max(length(scene.sun.xyz), 1e-4);
    let n_dot_l = max(dot(normal, sun_dir_n), 0.0);
    let sun_vis = sun_visibility(world_rel, normal, view_dist, n_dot_l);

    let ambient = scene.sun.w;
    // scene.sun.xyz is pre-scaled by daylight, so the diffuse dies at night.
    let diffuse = max(dot(normal, scene.sun.xyz), 0.0);

    // Sky-ambient fill tinted by the sky colour (horizon→zenith by how
    // up-facing the surface is), so shadowed and indirect-lit surfaces read
    // sky-blue (cool sky fill), never neutral grey. `sun_vis` darkens only
    // the sun term, so a shadowed surface keeps this fill (never pitch black)
    // and it vanishes underground where sky_vis → 0.
    let sky_color = mix(scene.sky_horizon.rgb, scene.sky_zenith.rgb, clamp(normal.y * 0.5 + 0.5, 0.0, 1.0));
    let sky_term = sky_vis * ambient * ao * sky_color;
    let sun_term = sky_vis * (1.0 - ambient) * diffuse * ao * sun_vis;
    // Unconditional ambient floor (params.y, per dimension): nothing renders
    // pure black. Added on top of sky/sun + block light, AO-modulated.
    let floor = scene.params.y * ao;
    let lit = max(sky_term + vec3<f32>(sun_term), block_light) + vec3<f32>(floor);
    // Metals carry no diffuse albedo term — their colour comes from the tinted
    // specular reflection (the sun glint here + the sky reflection in the IBL
    // step). Dielectrics keep the full diffuse.
    let metal_f = select(0.0, 1.0, metal);
    var color = albedo * lit * (1.0 - metal_f);

    // Cook-Torrance GGX sun specular: a sharp highlight where the smooth
    // surface mirrors the sun toward the eye. Gated by sky_vis * sun_vis (none
    // underground or in shadow) and carried by the daylight-scaled sun radiance
    // (so it dies at night); matte surfaces contribute ~nothing (broad lobe).
    let view_dir = normalize(-world_rel);
    let sun_radiance = length(scene.sun.xyz);
    let spec = ggx_specular(normal, view_dir, sun_dir_n, albedo, roughness, metal);
    color += spec * (sky_vis * sun_vis * sun_radiance);

    // Cheap image-based reflection: a smooth surface mirrors the sky gradient
    // along its reflection vector (the only environment we can sample without
    // SSR). Fresnel-Schlick on the view angle gives the bright grazing rim and
    // a faint face-on sheen; squared smoothness restricts it to genuinely
    // polished blocks (matte → none); sky_vis keeps it out of caves. F0 = 0.04
    // for dielectrics, albedo for metals (a metal's colour is its reflection).
    let refl_dir = reflect(-view_dir, normal);
    let sky_refl = mix(scene.sky_horizon.rgb, scene.sky_zenith.rgb, clamp(refl_dir.y, 0.0, 1.0));
    let n_dot_v = max(dot(normal, view_dir), 0.0);
    let f0_ibl = mix(vec3<f32>(0.04), albedo, metal_f);
    let fresnel = f0_ibl + (vec3<f32>(1.0) - f0_ibl) * pow(1.0 - n_dot_v, 5.0);
    let smoothness = 1.0 - roughness;
    color += sky_refl * fresnel * (smoothness * smoothness * sky_vis);

    // Dynamic point lights: emissive blocks (torches, lava, lamps) casting real
    // coloured light + specular glints, distance-attenuated to a hard radius
    // cutoff (bounded cost). Camera-relative positions match `world_rel`. The
    // double-count-safe combine vs the baked block light lands with the
    // data-driven light derivation step; additive here.
    var pl_diffuse = vec3<f32>(0.0);
    var pl_specular = vec3<f32>(0.0);
    let pl_count = min(u32(point_lights.header.x), 64u);
    for (var i = 0u; i < pl_count; i = i + 1u) {
        let pl = point_lights.lights[i];
        let to_light = pl.pos_radius.xyz - world_rel;
        let dist = length(to_light);
        let l = to_light / max(dist, 1e-3);
        let ndl = max(dot(normal, l), 0.0);
        let atten = clamp(1.0 - dist / max(pl.pos_radius.w, 1e-3), 0.0, 1.0);
        let radiance = pl.color_intensity.rgb * (pl.color_intensity.w * atten * atten);
        pl_diffuse += radiance * ndl;
        pl_specular += ggx_specular(normal, view_dir, l, albedo, roughness, metal) * radiance;
    }
    color += albedo * pl_diffuse * (1.0 - metal_f) + pl_specular;

    // Incandescence: the surface's own blackbody glow past the Draper point,
    // baked per-vertex into GB2.a by the geometry pass (0..1 = 525..1500 °C) —
    // smooth, so it never bands on depth quantization. Modulated by albedo so
    // the surface's texture (lava's molten/crust pattern, the rock grain) shows
    // *in* the glow instead of being washed out by a flat bright colour — the
    // texture is the emissive pattern. Hot matter glows + blooms; cold → 0.
    color += albedo * blackbody_glow(525.0 + g2.a * 975.0);

    // Distance fog with aerial perspective: a sky-exposed surface dissolves into
    // the sky colour *in its view direction* — warm toward the low sun, cool
    // away, brightening to the zenith overhead — i.e. atmospheric scattering, not
    // a flat horizon band. (Mirrors the sky dome's horizon→zenith blend so the
    // terrain melts seamlessly into the actual sky behind it.) An enclosed
    // surface still fades into the dark cave medium via baked `sky_vis`, so a
    // tunnel melts to black while a window/shaft stays bright. cave_dark is the
    // per-dimension ambient floor so it's never pitch black.
    let look = normalize(world_rel);
    let sun_h = scene.sky_sun.xyz;
    var toward = 0.5;
    if (length(sun_h.xz) > 1e-4 && length(look.xz) > 1e-4) {
        let f = dot(normalize(look.xz), normalize(sun_h.xz)) * 0.5 + 0.5;
        toward = f * f;
    }
    let horizon_col = mix(scene.sky_away.rgb, scene.sky_horizon.rgb, toward);
    let sky_in_dir = mix(horizon_col, scene.sky_zenith.rgb, pow(max(look.y, 0.0), 0.65));
    let fog_amount = 1.0 - exp(-pow(view_dist * 2.0 / scene.fog.w, 2.0));
    let cave_dark = vec3<f32>(scene.params.y);
    let fog_col = mix(cave_dark, sky_in_dir, sky_vis);
    color = mix(color, fog_col, fog_amount);
    return vec4<f32>(color, 1.0);
}
