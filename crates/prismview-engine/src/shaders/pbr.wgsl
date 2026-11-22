// PBR metálico/rugoso (glTF 2.0) para core/scene3d/renderer.rs.
//
// - Cook-Torrance: GGX (D), Smith-GGX correlacionado (V), Schlick (F); difuso Lambert.
// - Luzes: ambiente (AmbientLight), direcionais e pontuais com atenuação do three.js
//   (1/d^decay × janela suave até `range`).
// - IBL split-sum: cubo especular pré-filtrado (mip = rugosidade) + irradiância + LUT do BRDF.
// - Saída HDR linear com alpha pré-multiplicado (tone mapping em tonemap.wgsl).
// - Normal map: TBN pelas tangentes do vértice ou, sem elas (w == 0), por derivadas de tela.
// - Por draw: `instance_index` indexa `draws` (matriz de modelo + matriz normal).

const PI: f32 = 3.14159265359;
const TEX_BASE: u32 = 1u;
const TEX_MR: u32 = 2u;
const TEX_NORMAL: u32 = 4u;
const TEX_OCCLUSION: u32 = 8u;
const TEX_EMISSIVE: u32 = 16u;

struct DirLight {
    direction: vec4<f32>, // xyz = direção PARA a luz
    radiance: vec4<f32>,
};

struct PointLight {
    position: vec4<f32>, // w = alcance (0 = infinito)
    radiance: vec4<f32>, // w = decaimento
};

struct Frame {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    ambient: vec4<f32>,
    // x = mip máximo do cubo especular, y = nº direcionais, z = nº pontuais, w = intensidade IBL
    env: vec4<f32>,
    dir_lights: array<DirLight, 4>,
    point_lights: array<PointLight, 4>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var env_specular: texture_cube<f32>;
@group(0) @binding(2) var env_irradiance: texture_cube<f32>;
@group(0) @binding(3) var brdf_lut: texture_2d<f32>;
@group(0) @binding(4) var env_sampler: sampler;

struct Draw {
    model: mat4x4<f32>,
    normal: mat4x4<f32>, // inversa-transposta (3×3 útil)
    glow: vec4<f32>,     // x = brilho emissivo extra (bindings EMISSIVE_GLOW)
};
@group(1) @binding(0) var<storage, read> draws: array<Draw>;

struct Material {
    base_color: vec4<f32>,
    emissive: vec4<f32>, // w = escala do normal map
    params: vec4<f32>,   // metal, rugosidade, oclusão, alpha cutoff
    flags: vec4<u32>,    // texturas, usa UV1, alpha mode, double-sided
};
@group(2) @binding(0) var<uniform> material: Material;
@group(2) @binding(1) var t_base: texture_2d<f32>;
@group(2) @binding(2) var t_mr: texture_2d<f32>;
@group(2) @binding(3) var t_normal: texture_2d<f32>;
@group(2) @binding(4) var t_occlusion: texture_2d<f32>;
@group(2) @binding(5) var t_emissive: texture_2d<f32>;
@group(2) @binding(6) var s_material: sampler;

struct VsIn {
    @location(0) position: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) color: vec4<f32>,
    @builtin(instance_index) draw: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) color: vec4<f32>,
    @location(6) @interpolate(flat) glow: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let d = draws[in.draw];
    let world = d.model * vec4<f32>(in.position.xyz, 1.0);
    let nm = mat3x3<f32>(d.normal[0].xyz, d.normal[1].xyz, d.normal[2].xyz);
    let mm = mat3x3<f32>(d.model[0].xyz, d.model[1].xyz, d.model[2].xyz);
    var out: VsOut;
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = nm * in.normal.xyz;
    out.tangent = vec4<f32>(mm * in.tangent.xyz, in.tangent.w);
    out.uv0 = in.uv0;
    out.uv1 = in.uv1;
    out.color = in.color;
    out.glow = d.glow.x;
    return out;
}

// ───────────────────────── BRDF ─────────────────────────

fn d_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let f = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * f * f);
}

fn v_smith_correlated(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

fn f_schlick(f0: vec3<f32>, v_dot_h: f32) -> vec3<f32> {
    let f = pow(1.0 - v_dot_h, 5.0);
    return f0 + (vec3<f32>(1.0) - f0) * f;
}

fn f_schlick_roughness(f0: vec3<f32>, n_dot_v: f32, roughness: f32) -> vec3<f32> {
    let f = pow(1.0 - n_dot_v, 5.0);
    return f0 + (max(vec3<f32>(1.0 - roughness), f0) - f0) * f;
}

struct Surface {
    n: vec3<f32>,
    v: vec3<f32>,
    n_dot_v: f32,
    diffuse: vec3<f32>,
    f0: vec3<f32>,
    alpha: f32,
};

fn shade_light(s: Surface, l: vec3<f32>, radiance: vec3<f32>) -> vec3<f32> {
    let n_dot_l = dot(s.n, l);
    if n_dot_l <= 0.0 {
        return vec3<f32>(0.0);
    }
    let h = normalize(l + s.v);
    let n_dot_h = max(dot(s.n, h), 0.0);
    let v_dot_h = max(dot(s.v, h), 0.0);
    let f = f_schlick(s.f0, v_dot_h);
    let spec = f * (d_ggx(n_dot_h, s.alpha) * v_smith_correlated(s.n_dot_v, n_dot_l, s.alpha));
    let diffuse = s.diffuse / PI;
    return (diffuse + spec) * radiance * n_dot_l;
}

fn distance_attenuation(dist: f32, range: f32, decay: f32) -> f32 {
    var falloff = 1.0 / max(pow(dist, decay), 0.01);
    if range > 0.0 {
        let r = dist / range;
        let w = clamp(1.0 - r * r * r * r, 0.0, 1.0);
        falloff = falloff * w * w;
    }
    return falloff;
}

// Base tangente por derivadas (sem atributo TANGENT). glTF: +Y do normal map = -v.
fn cotangent_frame(n: vec3<f32>, p: vec3<f32>, uv: vec2<f32>) -> mat3x3<f32> {
    let dp1 = dpdx(p);
    let dp2 = dpdy(p);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let scale = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
    return mat3x3<f32>(t * scale, -b * scale, n);
}

fn has(bit: u32) -> bool {
    return (material.flags.x & bit) != 0u;
}

fn uv_for(bit: u32, in: VsOut) -> vec2<f32> {
    return select(in.uv0, in.uv1, (material.flags.y & bit) != 0u);
}

// ───────────────────────── Fragment ─────────────────────────

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Todas as amostras em fluxo uniforme (texturas ausentes = 1×1 neutras).
    let base_tex = textureSample(t_base, s_material, uv_for(TEX_BASE, in));
    let mr_tex = textureSample(t_mr, s_material, uv_for(TEX_MR, in));
    let normal_tex = textureSample(t_normal, s_material, uv_for(TEX_NORMAL, in));
    let occ_tex = textureSample(t_occlusion, s_material, uv_for(TEX_OCCLUSION, in));
    let emi_tex = textureSample(t_emissive, s_material, uv_for(TEX_EMISSIVE, in));
    let deriv_tbn = cotangent_frame(normalize(in.normal), in.world_pos, uv_for(TEX_NORMAL, in));

    var base = material.base_color * in.color;
    if has(TEX_BASE) {
        base = base * base_tex;
    }
    let alpha_mode = material.flags.z;
    if alpha_mode == 1u && base.a < material.params.w {
        discard;
    }
    let alpha = select(1.0, clamp(base.a, 0.0, 1.0), alpha_mode == 2u);

    var metallic = material.params.x;
    var roughness = material.params.y;
    if has(TEX_MR) {
        roughness = roughness * mr_tex.g;
        metallic = metallic * mr_tex.b;
    }
    roughness = clamp(roughness, 0.045, 1.0);
    metallic = clamp(metallic, 0.0, 1.0);

    // Normal geométrica (double-sided vira para o observador).
    var n = normalize(in.normal);
    let flip = material.flags.w != 0u && !front;
    if flip {
        n = -n;
    }
    if has(TEX_NORMAL) {
        var tn = normal_tex.xyz * 2.0 - 1.0;
        tn = vec3<f32>(tn.xy * material.emissive.w, tn.z);
        var tbn: mat3x3<f32>;
        if in.tangent.w != 0.0 {
            var t = normalize(in.tangent.xyz - n * dot(n, in.tangent.xyz));
            if flip {
                t = -t;
            }
            let b = cross(n, t) * in.tangent.w;
            tbn = mat3x3<f32>(t, b, n);
        } else {
            tbn = mat3x3<f32>(deriv_tbn[0], deriv_tbn[1], n);
            if flip {
                tbn = mat3x3<f32>(-deriv_tbn[0], -deriv_tbn[1], n);
            }
        }
        let mapped = tbn * tn;
        n = select(n, normalize(mapped), dot(mapped, mapped) > 1e-12);
    }

    let v = normalize(frame.camera_pos.xyz - in.world_pos);
    var s: Surface;
    s.n = n;
    s.v = v;
    s.n_dot_v = clamp(abs(dot(n, v)), 1e-4, 1.0);
    s.diffuse = base.rgb * (1.0 - metallic);
    s.f0 = mix(vec3<f32>(0.04), base.rgb, metallic);
    s.alpha = roughness * roughness;

    var color = vec3<f32>(0.0);

    let dir_count = u32(frame.env.y);
    for (var i = 0u; i < 4u; i = i + 1u) {
        if i >= dir_count {
            break;
        }
        let light = frame.dir_lights[i];
        color = color + shade_light(s, normalize(light.direction.xyz), light.radiance.rgb);
    }
    let point_count = u32(frame.env.z);
    for (var i = 0u; i < 4u; i = i + 1u) {
        if i >= point_count {
            break;
        }
        let light = frame.point_lights[i];
        let to_light = light.position.xyz - in.world_pos;
        let dist = length(to_light);
        let att = distance_attenuation(dist, light.position.w, light.radiance.w);
        color = color + shade_light(s, to_light / max(dist, 1e-4), light.radiance.rgb * att);
    }

    // Indireto: ambiente constante + IBL.
    var ao = 1.0;
    if has(TEX_OCCLUSION) {
        ao = 1.0 + material.params.z * (occ_tex.r - 1.0);
    }
    let r = reflect(-v, n);
    let lod = roughness * frame.env.x;
    let prefiltered = textureSampleLevel(env_specular, env_sampler, r, lod).rgb;
    let irradiance = textureSampleLevel(env_irradiance, env_sampler, n, 0.0).rgb;
    let lut = textureSampleLevel(brdf_lut, env_sampler, vec2<f32>(s.n_dot_v, roughness), 0.0).rg;
    let f_ibl = f_schlick_roughness(s.f0, s.n_dot_v, roughness);
    let spec_ibl = prefiltered * (f_ibl * lut.x + lut.y);
    let kd = (vec3<f32>(1.0) - f_ibl);
    let diffuse_ibl = irradiance * s.diffuse * kd;
    let env_intensity = frame.env.w;
    color = color + (diffuse_ibl + spec_ibl) * env_intensity * ao;
    color = color + frame.ambient.rgb * s.diffuse / PI * ao;

    var emissive = material.emissive.rgb;
    if has(TEX_EMISSIVE) {
        emissive = emissive * emi_tex.rgb;
    }
    color = color + emissive;

    // Brilho reativo: tom do emissivo do material (normalizado) ou, sem emissivo, da cor base.
    if in.glow > 0.0 {
        let e = material.emissive.rgb;
        let peak = max(e.r, max(e.g, e.b));
        let tint = select(material.base_color.rgb, e / max(peak, 1e-4), peak > 0.0);
        color = color + tint * in.glow;
    }

    // Sem NaN/Inf no alvo HDR (um pixel inválido contaminaria o resolve do MSAA).
    color = clamp(select(color, vec3<f32>(0.0), color != color), vec3<f32>(0.0), vec3<f32>(6.0e4));
    return vec4<f32>(color * alpha, alpha);
}
