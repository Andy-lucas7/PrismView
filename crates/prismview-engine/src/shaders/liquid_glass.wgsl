struct FullscreenOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) idx: u32) -> FullscreenOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0)
    );
    let p = positions[idx];
    var out: FullscreenOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return out;
}

// --- Dual Kawase Blur (Downsample & Upsample for frostedTex and lensHeightMap) ---
struct KawaseUniforms {
    texel_size: vec2<f32>,
    offset: f32,
    dim_alpha: f32,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
@group(0) @binding(2) var<uniform> kawase: KawaseUniforms;

@fragment
fn fs_background_blit(in: FullscreenOut) -> @location(0) vec4<f32> {
    let col = textureSample(src_tex, src_sampler, clamp(in.uv, vec2<f32>(0.0), vec2<f32>(1.0))).rgb;
    let dimmed = col * (1.0 - clamp(kawase.dim_alpha, 0.0, 0.85));
    return vec4<f32>(dimmed, 1.0);
}

@fragment
fn fs_kawase_down(in: FullscreenOut) -> @location(0) vec4<f32> {
    let hp = kawase.texel_size * kawase.offset;
    var sum = textureSample(src_tex, src_sampler, in.uv) * 4.0;
    sum += textureSample(src_tex, src_sampler, in.uv - hp);
    sum += textureSample(src_tex, src_sampler, in.uv + hp);
    sum += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(hp.x, -hp.y));
    sum += textureSample(src_tex, src_sampler, in.uv - vec2<f32>(hp.x, -hp.y));
    return sum / 8.0;
}

@fragment
fn fs_kawase_up(in: FullscreenOut) -> @location(0) vec4<f32> {
    let hp = kawase.texel_size * kawase.offset;
    var sum = textureSample(src_tex, src_sampler, in.uv + vec2<f32>(-hp.x * 2.0, 0.0));
    sum += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(-hp.x, hp.y)) * 2.0;
    sum += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(0.0, hp.y * 2.0));
    sum += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(hp.x, hp.y)) * 2.0;
    sum += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(hp.x * 2.0, 0.0));
    sum += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(hp.x, -hp.y)) * 2.0;
    sum += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(0.0, -hp.y * 2.0));
    sum += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(-hp.x, -hp.y)) * 2.0;
    return sum / 12.0;
}

// --- Optical Crystal & Liquid Glass (Port of PrismLock CRYSTAL_AGSL + RGB Dispersion) ---
struct CrystalUniforms {
    resolution: vec2<f32>,
    box_origin: vec2<f32>,
    box_size: vec2<f32>,
    parallax_offset: vec2<f32>,
    refraction_px: f32,
    bevel_radius: f32,
    stroke_radius: f32,
    dispersion_rgb: f32,
    ior_rgb: vec3<f32>,
    frost_amount: f32,
    fresnel_intensity: f32,
    prismatic_rim_intensity: f32,
    light_angle_rad: f32,
    saturation: f32,
    tint_rgba: vec4<f32>,
    solid_rgba: vec4<f32>,
    opacity: f32,
    fill_mode: u32, // 0 = Solid, 1 = ClassicBlur, 2 = OpticalCrystal
    grain: f32,
    corner_radius: f32,
};

@group(0) @binding(0) var bg_sharp_tex: texture_2d<f32>;
@group(0) @binding(1) var bg_frosted_tex: texture_2d<f32>;
@group(0) @binding(2) var lens_height_tex: texture_2d<f32>;
@group(0) @binding(3) var glass_sampler: sampler;
@group(0) @binding(4) var<uniform> u: CrystalUniforms;

struct LayerVertOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) local_uv: vec2<f32>,
    @location(1) screen_px: vec2<f32>,
};

@vertex
fn vs_layer(@builtin(vertex_index) idx: u32) -> LayerVertOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0)
    );
    let uv = quad[idx];
    let px = u.box_origin + u.parallax_offset + uv * u.box_size;
    let ndc = vec2<f32>(
        (px.x / max(u.resolution.x, 1.0)) * 2.0 - 1.0,
        1.0 - (px.y / max(u.resolution.y, 1.0)) * 2.0
    );
    var out: LayerVertOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.local_uv = uv;
    out.screen_px = px;
    return out;
}

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn rounded_box_sdf(p: vec2<f32>, half_size: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn sample_lens_height(local_uv: vec2<f32>) -> vec2<f32> {
    let tex_sample = textureSample(lens_height_tex, glass_sampler, clamp(local_uv, vec2<f32>(0.0), vec2<f32>(1.0)));
    let half_sz = u.box_size * 0.5;
    let r = min(u.corner_radius, min(half_sz.x, half_sz.y));
    let d = rounded_box_sdf((local_uv - 0.5) * u.box_size, half_sz, r);
    let sdf_mask = 1.0 - smoothstep(-1.0, 1.0, d);
    let sdf_height = clamp(-d / max(u.bevel_radius, 1.0), 0.0, 1.0);
    let mask_a = tex_sample.a * sdf_mask;
    let height = mix(sdf_height, tex_sample.r, step(0.01, tex_sample.r)) * mask_a;
    return vec2<f32>(height, mask_a);
}

@fragment
fn fs_liquid_glass(in: LayerVertOut) -> @location(0) vec4<f32> {
    let hm = sample_lens_height(in.local_uv);
    let mask_alpha = hm.y;
    if (mask_alpha <= 0.004) {
        discard;
    }

    let final_alpha = mask_alpha * clamp(u.opacity, 0.0, 1.0);
    if (u.fill_mode == 0u) {
        return vec4<f32>(u.solid_rgba.rgb, u.solid_rgba.a * final_alpha);
    }

    let safe_res = max(u.resolution, vec2<f32>(2.0));
    let base_uv = clamp(in.screen_px / safe_res, vec2<f32>(0.001), vec2<f32>(0.999));

    if (u.fill_mode == 1u) {
        var col = textureSample(bg_frosted_tex, glass_sampler, base_uv).rgb;
        let luma = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
        col = mix(vec3<f32>(luma), col, u.saturation);
        let n = (hash12(in.screen_px) - 0.5) * u.grain;
        col = clamp(col + vec3<f32>(n), vec3<f32>(0.0), vec3<f32>(1.0));
        col = mix(col, u.tint_rgba.rgb, u.tint_rgba.a);
        return vec4<f32>(col, final_alpha);
    }

    // Mode 2: Optical Crystal (3D normal from lensHeightMap + physical RGB IOR dispersion + Fresnel + Prismatic sheen)
    let step_px = clamp(max(u.bevel_radius, u.stroke_radius) * 0.12, 1.5, 5.5);
    let step_uv = vec2<f32>(step_px) / max(u.box_size, vec2<f32>(2.0));
    let h_l = sample_lens_height(in.local_uv - vec2<f32>(step_uv.x, 0.0)).x;
    let h_r = sample_lens_height(in.local_uv + vec2<f32>(step_uv.x, 0.0)).x;
    let h_u = sample_lens_height(in.local_uv - vec2<f32>(0.0, step_uv.y)).x;
    let h_d = sample_lens_height(in.local_uv + vec2<f32>(0.0, step_uv.y)).x;

    let edge_atten = smoothstep(0.02, 0.22, mask_alpha);
    let slope_gain = clamp((max(u.bevel_radius, u.stroke_radius) * 0.45) / max(step_px, 1.0), 1.8, 8.0);
    let dx = (h_l - h_r) * slope_gain * edge_atten;
    let dy = (h_u - h_d) * slope_gain * edge_atten;
    let normal = normalize(vec3<f32>(dx, dy, 1.0));

    let base_refr_px = normal.xy * u.refraction_px;
    let disp_scale = u.dispersion_rgb * (max(u.box_size.x, u.box_size.y) + u.refraction_px * 1.5);

    // Physical per-channel IOR offsets (Red, Green, Blue additive spectral dispersion)
    let uv_r = clamp((in.screen_px + base_refr_px * (u.ior_rgb.x / u.ior_rgb.y) + normal.xy * disp_scale) / safe_res, vec2<f32>(0.001), vec2<f32>(0.999));
    let uv_g = clamp((in.screen_px + base_refr_px) / safe_res, vec2<f32>(0.001), vec2<f32>(0.999));
    let uv_b = clamp((in.screen_px + base_refr_px * (u.ior_rgb.z / u.ior_rgb.y) - normal.xy * disp_scale) / safe_res, vec2<f32>(0.001), vec2<f32>(0.999));

    let sharp_rgb = vec3<f32>(
        textureSample(bg_sharp_tex, glass_sampler, uv_r).r,
        textureSample(bg_sharp_tex, glass_sampler, uv_g).g,
        textureSample(bg_sharp_tex, glass_sampler, uv_b).b
    );
    let frosted_rgb = vec3<f32>(
        textureSample(bg_frosted_tex, glass_sampler, uv_r).r,
        textureSample(bg_frosted_tex, glass_sampler, uv_g).g,
        textureSample(bg_frosted_tex, glass_sampler, uv_b).b
    );

    var col = mix(sharp_rgb, frosted_rgb, clamp(u.frost_amount, 0.0, 1.0));
    let luma = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
    col = mix(vec3<f32>(luma), col, u.saturation);
    col = mix(col, u.tint_rgba.rgb, u.tint_rgba.a * 0.65);

    let light_vec = vec2<f32>(cos(u.light_angle_rad), sin(u.light_angle_rad));
    let n_xy_len = length(normal.xy);
    let dir_light = pow(max(dot(normalize(normal.xy + vec2<f32>(0.0001)), light_vec), 0.0), 3.0) * n_xy_len;
    let fresnel = pow(clamp(1.0 - normal.z, 0.0, 1.0), 2.0);

    // Pure white core specular + subtle RGB spectral dispersion on high-curvature rim
    let white_spec = (dir_light * 0.65 + fresnel * 0.55) * u.fresnel_intensity * edge_atten;
    let prism_r = vec3<f32>(1.0, 0.118, 0.259) * smoothstep(0.18, 0.85, fresnel) * max(normal.x, 0.0);
    let prism_g = vec3<f32>(0.0, 1.0, 0.533) * smoothstep(0.22, 0.88, fresnel) * abs(normal.y);
    let prism_b = vec3<f32>(0.0, 0.824, 1.0) * smoothstep(0.18, 0.85, fresnel) * max(-normal.x, 0.0);
    let prism_rim = (prism_r + prism_g + prism_b) * u.prismatic_rim_intensity * edge_atten * 0.45;

    col = clamp(col + vec3<f32>(white_spec) + prism_rim, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(col, final_alpha);
}
