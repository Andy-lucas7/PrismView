// PMREM / IBL em compute shaders (core/scene3d/environment.rs). Roda só quando o ambiente muda.
//
// Entradas (cada uma com o próprio bind group; bindings não se repetem entre entradas):
//   cs_sky_procedural  : fundo + cards emissivos (interseção raio×retângulo)  → cubo base mip 0
//   cs_sky_equirect    : HDRI equirretangular (rgba32float, bilinear manual)  → cubo base mip 0
//   cs_downsample      : box 2×2 por face                                     → cubo base mip N
//   cs_prefilter       : GGX importance sampling filtrado (Karis/Křivánek)    → cubo especular mip N
//   cs_irradiance      : convolução de cosseno                                → cubo irradiância
//   cs_brdf_lut        : integral split-sum (A, B) do BRDF                    → LUT 2D
//
// Convenção de faces = cubemap D3D/wgpu (+X, -X, +Y, -Y, +Z, -Z), a mesma usada na amostragem,
// então gerar e amostrar pela direção de mundo é consistente.

const PI: f32 = 3.14159265359;

struct EnvParams {
    background: vec4<f32>,
    card_center: array<vec4<f32>, 4>,
    card_u: array<vec4<f32>, 4>,
    card_v: array<vec4<f32>, 4>,
    card_radiance: array<vec4<f32>, 4>,
    // x = tamanho da face de saída, y = rugosidade, z = nº de amostras, w = tamanho da face base
    info: vec4<f32>,
    // x = nº de mips do cubo base, y = escala do HDRI, z = nº de cards, w = livre
    info2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: EnvParams;
@group(0) @binding(1) var out_cube: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(2) var equirect: texture_2d<f32>;
@group(0) @binding(3) var src_mip: texture_2d_array<f32>;
@group(0) @binding(4) var env_cube: texture_cube<f32>;
@group(0) @binding(5) var env_sampler: sampler;
@group(0) @binding(6) var out_lut: texture_storage_2d<rgba16float, write>;

fn face_dir(face: u32, uv: vec2<f32>) -> vec3<f32> {
    // uv em [-1, 1], v cresce para baixo.
    let u = uv.x;
    let v = uv.y;
    var d: vec3<f32>;
    switch face {
        case 0u: { d = vec3<f32>(1.0, -v, -u); }
        case 1u: { d = vec3<f32>(-1.0, -v, u); }
        case 2u: { d = vec3<f32>(u, 1.0, v); }
        case 3u: { d = vec3<f32>(u, -1.0, -v); }
        case 4u: { d = vec3<f32>(u, -v, 1.0); }
        default: { d = vec3<f32>(-u, -v, -1.0); }
    }
    return normalize(d);
}

fn texel_dir(id: vec3<u32>, size: f32) -> vec3<f32> {
    let uv = (vec2<f32>(id.xy) + 0.5) / size * 2.0 - 1.0;
    return face_dir(id.z, uv);
}

// ───────────────────────── Céu procedural ─────────────────────────

fn procedural_radiance(dir: vec3<f32>) -> vec3<f32> {
    var best_t = 1e30;
    var color = params.background.rgb;
    let count = u32(params.info2.z);
    for (var i = 0u; i < 4u; i = i + 1u) {
        if i >= count {
            break;
        }
        let c = params.card_center[i].xyz;
        let hu = params.card_u[i].xyz;
        let hv = params.card_v[i].xyz;
        let n = cross(hu, hv);
        let denom = dot(dir, n);
        if abs(denom) < 1e-6 {
            continue;
        }
        let t = dot(c, n) / denom; // raio a partir da origem (câmera do PMREM)
        if t <= 0.0 || t >= best_t {
            continue;
        }
        let p = dir * t - c;
        let a = dot(p, hu) / max(dot(hu, hu), 1e-8);
        let b = dot(p, hv) / max(dot(hv, hv), 1e-8);
        if abs(a) <= 1.0 && abs(b) <= 1.0 {
            best_t = t;
            color = params.card_radiance[i].rgb; // planos double-sided, não iluminados
        }
    }
    return color;
}

@compute @workgroup_size(8, 8, 1)
fn cs_sky_procedural(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = params.info.x;
    if f32(id.x) >= size || f32(id.y) >= size || id.z >= 6u {
        return;
    }
    // Super-amostragem 2×2 suaviza as bordas dos cards (o PMREM do three usa sigma 0,04).
    var acc = vec3<f32>(0.0);
    for (var sy = 0u; sy < 2u; sy = sy + 1u) {
        for (var sx = 0u; sx < 2u; sx = sx + 1u) {
            let off = (vec2<f32>(f32(sx), f32(sy)) - 0.5) * 0.5;
            let uv = (vec2<f32>(id.xy) + 0.5 + off) / size * 2.0 - 1.0;
            acc = acc + procedural_radiance(face_dir(id.z, uv));
        }
    }
    textureStore(out_cube, vec2<i32>(id.xy), i32(id.z), vec4<f32>(acc * 0.25, 1.0));
}

// ───────────────────────── HDRI equirretangular ─────────────────────────

fn equirect_fetch(p: vec2<i32>, dims: vec2<i32>) -> vec3<f32> {
    let x = ((p.x % dims.x) + dims.x) % dims.x; // repete em longitude
    let y = clamp(p.y, 0, dims.y - 1);
    return textureLoad(equirect, vec2<i32>(x, y), 0).rgb;
}

@compute @workgroup_size(8, 8, 1)
fn cs_sky_equirect(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = params.info.x;
    if f32(id.x) >= size || f32(id.y) >= size || id.z >= 6u {
        return;
    }
    let dir = texel_dir(id, size);
    let dims_u = textureDimensions(equirect, 0);
    let dims = vec2<i32>(dims_u);
    let u = atan2(dir.z, dir.x) / (2.0 * PI) + 0.5;
    let v = 0.5 - asin(clamp(dir.y, -1.0, 1.0)) / PI;
    let pos = vec2<f32>(u, v) * vec2<f32>(dims_u) - 0.5;
    let base = vec2<i32>(floor(pos));
    let f = fract(pos);
    let c00 = equirect_fetch(base, dims);
    let c10 = equirect_fetch(base + vec2<i32>(1, 0), dims);
    let c01 = equirect_fetch(base + vec2<i32>(0, 1), dims);
    let c11 = equirect_fetch(base + vec2<i32>(1, 1), dims);
    var c = mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y) * params.info2.y;
    // Sem NaN/Inf e com teto: um pixel de sol 1e6 geraria fireflies no prefiltro.
    c = clamp(select(c, vec3<f32>(0.0), c != c), vec3<f32>(0.0), vec3<f32>(6.0e4));
    textureStore(out_cube, vec2<i32>(id.xy), i32(id.z), vec4<f32>(c, 1.0));
}

// ───────────────────────── Mips do cubo base ─────────────────────────

@compute @workgroup_size(8, 8, 1)
fn cs_downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = params.info.x;
    if f32(id.x) >= size || f32(id.y) >= size || id.z >= 6u {
        return;
    }
    let p = vec2<i32>(id.xy) * 2;
    let layer = i32(id.z);
    let a = textureLoad(src_mip, p, layer, 0);
    let b = textureLoad(src_mip, p + vec2<i32>(1, 0), layer, 0);
    let c = textureLoad(src_mip, p + vec2<i32>(0, 1), layer, 0);
    let d = textureLoad(src_mip, p + vec2<i32>(1, 1), layer, 0);
    textureStore(out_cube, vec2<i32>(id.xy), layer, (a + b + c + d) * 0.25);
}

// ───────────────────────── Amostragem ─────────────────────────

fn radical_inverse(bits_in: u32) -> f32 {
    var bits = bits_in;
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return f32(bits) * 2.3283064365386963e-10;
}

fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), radical_inverse(i));
}

fn tangent_frame(n: vec3<f32>) -> mat3x3<f32> {
    let up = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(n.z) < 0.999);
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    return mat3x3<f32>(t, b, n);
}

fn importance_ggx(xi: vec2<f32>, alpha: f32) -> vec3<f32> {
    let a2 = alpha * alpha;
    let phi = 2.0 * PI * xi.x;
    let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a2 - 1.0) * xi.y));
    let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
    return vec3<f32>(cos(phi) * sin_t, sin(phi) * sin_t, cos_t);
}

fn d_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let f = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * f * f);
}

// ───────────────────────── Especular pré-filtrado ─────────────────────────

@compute @workgroup_size(8, 8, 1)
fn cs_prefilter(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = params.info.x;
    if f32(id.x) >= size || f32(id.y) >= size || id.z >= 6u {
        return;
    }
    let n = texel_dir(id, size);
    let roughness = params.info.y;
    let base_size = params.info.w;
    let max_lod = params.info2.x - 1.0;

    if roughness <= 0.001 {
        // Espelho: amostra direta do cubo base (mip que casa com a resolução de saída).
        let lod = clamp(log2(base_size / size), 0.0, max_lod);
        let c = textureSampleLevel(env_cube, env_sampler, n, lod).rgb;
        textureStore(out_cube, vec2<i32>(id.xy), i32(id.z), vec4<f32>(c, 1.0));
        return;
    }

    let alpha = roughness * roughness;
    let count = u32(params.info.z);
    let frame = tangent_frame(n);
    // Ângulo sólido de um texel do cubo base (filtered importance sampling).
    let sa_texel = 4.0 * PI / (6.0 * base_size * base_size);
    var acc = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        let h = frame * importance_ggx(hammersley(i, count), alpha);
        let l = normalize(2.0 * dot(n, h) * h - n); // V = N
        let n_dot_l = dot(n, l);
        if n_dot_l > 0.0 {
            let n_dot_h = max(dot(n, h), 0.0);
            let pdf = d_ggx(n_dot_h, alpha) * 0.25 + 1e-4; // D·NdH/(4·VdH), V = N
            let sa_sample = 1.0 / (f32(count) * pdf);
            let lod = clamp(0.5 * log2(sa_sample / sa_texel) + 1.0, 0.0, max_lod);
            acc = acc + textureSampleLevel(env_cube, env_sampler, l, lod).rgb * n_dot_l;
            weight = weight + n_dot_l;
        }
    }
    let c = acc / max(weight, 1e-4);
    textureStore(out_cube, vec2<i32>(id.xy), i32(id.z), vec4<f32>(c, 1.0));
}

// ───────────────────────── Irradiância difusa ─────────────────────────

@compute @workgroup_size(8, 8, 1)
fn cs_irradiance(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = params.info.x;
    if f32(id.x) >= size || f32(id.y) >= size || id.z >= 6u {
        return;
    }
    let n = texel_dir(id, size);
    let frame = tangent_frame(n);
    let count = u32(params.info.z);
    let base_size = params.info.w;
    let max_lod = params.info2.x - 1.0;
    let sa_texel = 4.0 * PI / (6.0 * base_size * base_size);
    var acc = vec3<f32>(0.0);
    for (var i = 0u; i < count; i = i + 1u) {
        let xi = hammersley(i, count);
        // Amostragem de cosseno: pdf = cosθ/π.
        let r = sqrt(xi.y);
        let phi = 2.0 * PI * xi.x;
        let local = vec3<f32>(r * cos(phi), r * sin(phi), sqrt(max(1.0 - xi.y, 0.0)));
        let l = frame * local;
        let pdf = max(local.z, 1e-4) / PI;
        let lod = clamp(0.5 * log2(1.0 / (f32(count) * pdf) / sa_texel) + 1.0, 0.0, max_lod);
        acc = acc + textureSampleLevel(env_cube, env_sampler, l, lod).rgb;
    }
    // Média da radiância ponderada por cosseno = E/π (o shader multiplica pelo albedo).
    textureStore(out_cube, vec2<i32>(id.xy), i32(id.z), vec4<f32>(acc / f32(count), 1.0));
}

// ───────────────────────── LUT do BRDF ─────────────────────────

fn g_smith_ibl(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let k = alpha * 0.5;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

@compute @workgroup_size(8, 8, 1)
fn cs_brdf_lut(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(out_lut);
    if id.x >= dims.x || id.y >= dims.y {
        return;
    }
    let n_dot_v = max((f32(id.x) + 0.5) / f32(dims.x), 1e-3);
    let roughness = (f32(id.y) + 0.5) / f32(dims.y);
    let alpha = roughness * roughness;
    let v = vec3<f32>(sqrt(1.0 - n_dot_v * n_dot_v), 0.0, n_dot_v);
    let count = 512u;
    var a = 0.0;
    var b = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        let h = importance_ggx(hammersley(i, count), alpha);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let n_dot_l = max(l.z, 0.0);
        let n_dot_h = max(h.z, 0.0);
        let v_dot_h = max(dot(v, h), 0.0);
        if n_dot_l > 0.0 {
            let g = g_smith_ibl(n_dot_v, n_dot_l, alpha);
            let g_vis = g * v_dot_h / max(n_dot_h * n_dot_v, 1e-5);
            let fc = pow(1.0 - v_dot_h, 5.0);
            a = a + (1.0 - fc) * g_vis;
            b = b + fc * g_vis;
        }
    }
    textureStore(out_lut, vec2<i32>(id.xy), vec4<f32>(a / f32(count), b / f32(count), 0.0, 1.0));
}
