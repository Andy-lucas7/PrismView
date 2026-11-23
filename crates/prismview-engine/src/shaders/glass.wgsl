// Vidro óptico nativo (core/glass.rs). Port fiel de `GLASS_FRAG_SRC` de
// apps/prismview-studio/src/utils/opticalGlassWebGL.ts (o mapa de normais dos glifos fica em
// glass_glyph.wgsl).
//
// Convenções do port:
//  * Espaço "CSS top-down" (y para baixo) em todo lugar; as texturas nativas NÃO são invertidas em Y
//    (o WebGL2 as subia com UNPACK_FLIP_Y).
//  * `in.pos.xy` (posição de fragmento, y para baixo) faz o papel de `vpPx` do TS.
//  * Alpha de saída pré-multiplicado (blend One / OneMinusSrcAlpha).
//
// Entradas: grupo 0 = globals + instâncias; grupo 1 = backdrop nítido, backdrop borrado (pirâmide
// Kawase), mapa de normais do glifo e sampler linear/clamp.

struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};

struct Inst {
    xf: vec4<f32>,      // base afim [m00, m10, m01, m11]
    geo: vec4<f32>,     // centro (px de tela) xy, meio tamanho local zw
    bg: vec4<f32>,      // uv_rect do backdrop (u0, v0, u1, v1) cobrindo a viewport inteira
    p0: vec4<f32>,      // blur_mix, refração px, bevel px, raio px
    p1: vec4<f32>,      // chroma px, frost, ruído, especular
    p2: vec4<f32>,      // borda, brilho, contraste, saturação
    p3: vec4<f32>,      // opacidade, luz.x, luz.y, modo (0 placa, 1 glifo)
    tint: vec4<f32>,    // rgb do véu/borda; w = 1.0 se backdrop opaco (imagem/vídeo), 0.0 se 3D transparente
    pigment: vec4<f32>, // rgb + alpha do pigmento (só glifo)
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var<storage, read> insts: array<Inst>;

@group(1) @binding(0) var sharp_tex: texture_2d<f32>;
@group(1) @binding(1) var blur_tex: texture_2d<f32>;
@group(1) @binding(2) var glyph_tex: texture_2d<f32>;
@group(1) @binding(3) var smp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) idx: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let inst = insts[ii];
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u)) * 2.0 - 1.0;
    let scale = max(length(vec2<f32>(inst.xf.x, inst.xf.y)), 1e-3);
    // 1,5 px de margem para o antialiasing da borda SDF.
    let margin = 1.5 / scale;
    let local = corner * (inst.geo.zw + vec2<f32>(margin));
    let screen = inst.geo.xy + vec2<f32>(
        inst.xf.x * local.x + inst.xf.z * local.y,
        inst.xf.y * local.x + inst.xf.w * local.y,
    );
    var out: VOut;
    out.pos = vec4<f32>(
        screen.x / globals.viewport.x * 2.0 - 1.0,
        1.0 - screen.y / globals.viewport.y * 2.0,
        0.0,
        1.0,
    );
    out.local = local;
    out.idx = ii;
    return out;
}

fn sd_rounded_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn sample_backdrop(inst: Inst, vp: vec2<f32>) -> vec4<f32> {
    let css_uv = vp / max(globals.viewport, vec2<f32>(1.0));
    let uv = inst.bg.xy + css_uv * (inst.bg.zw - inst.bg.xy);
    let eps = 0.0015 * (inst.bg.zw - inst.bg.xy);
    let tuv = clamp(uv, inst.bg.xy + eps, inst.bg.zw - eps);
    let opaque_bg = inst.tint.w >= 0.5;
    var sharp = textureSampleLevel(sharp_tex, smp, tuv, 0.0);
    if (opaque_bg) {
        sharp.a = 1.0;
    }
    let blur_mix = inst.p0.x;
    if (blur_mix <= 0.001) {
        return sharp;
    }
    var blurred = textureSampleLevel(blur_tex, smp, tuv, 0.0);
    if (opaque_bg) {
        blurred.a = 1.0;
    }
    return mix(sharp, blurred, blur_mix);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let inst = insts[in.idx];
    let half_size = inst.geo.zw;
    let layer_size = half_size * 2.0;
    let local_px = in.local;
    let scale = max(length(vec2<f32>(inst.xf.x, inst.xf.y)), 1e-3);
    let aa = 0.75 / scale;

    let refraction_px = inst.p0.y;
    let bevel_px = inst.p0.z;
    let radius_px = inst.p0.w;
    let chroma_px = inst.p1.x;
    let frost = inst.p1.y;
    let noise_opacity = inst.p1.z;
    let specular = inst.p1.w;
    let border_opacity = inst.p2.x;
    let brightness = inst.p2.y;
    let contrast = inst.p2.z;
    let saturation = inst.p2.w;
    let opacity = inst.p3.x;
    let light_dir = normalize(vec2<f32>(inst.p3.y, inst.p3.z));
    let glyph_mode = inst.p3.w > 0.5;

    var alpha = 1.0;
    var normal = vec2<f32>(0.0);
    var edge_strength = 0.0;
    var border_mask = 0.0;
    var lens_mag = 0.0;

    if (!glyph_mode) {
        let r = clamp(radius_px, 0.0, min(half_size.x, half_size.y) - 0.5);
        let d = sd_rounded_box(local_px, half_size, r);
        alpha = 1.0 - smoothstep(-aa, aa, d);
        if (alpha <= 0.002) {
            discard;
        }

        let depth = max(0.0, -d);
        let bevel = clamp(bevel_px, 3.0, min(half_size.x, half_size.y) * 0.92);

        // Gradiente analítico do SDF (normal externa em espaço top-down).
        let eps = 0.85;
        let gx = sd_rounded_box(local_px + vec2<f32>(eps, 0.0), half_size, r)
            - sd_rounded_box(local_px - vec2<f32>(eps, 0.0), half_size, r);
        let gy = sd_rounded_box(local_px + vec2<f32>(0.0, eps), half_size, r)
            - sd_rounded_box(local_px - vec2<f32>(0.0, eps), half_size, r);
        let grad = vec2<f32>(gx, gy);
        let glen = length(grad);
        var out_n = vec2<f32>(0.0);
        if (glen > 0.0001) {
            out_n = grad / glen;
        }

        // Bisel convexo curvo + leve domo esférico no centro.
        let dome_n = local_px / max(half_size, vec2<f32>(1.0));
        if (depth < bevel) {
            let t = 1.0 - depth / bevel; // 1 na borda, 0 no fim do bisel
            let bevel_curve = pow(t, 1.25) * sin(t * 1.5707963);
            normal = out_n * bevel_curve + dome_n * 0.14;
            lens_mag = bevel_curve;
        } else {
            normal = dome_n * 0.14;
            lens_mag = 0.0;
        }

        edge_strength = exp(-depth / max(2.0, bevel * 0.34));
        border_mask = 1.0 - smoothstep(0.0, 1.5, depth);
    } else {
        // Mapa empacotado: R = nx, G = ny, B = inclinação da borda, A = alpha nítido.
        let guv = local_px / max(layer_size, vec2<f32>(1.0)) + vec2<f32>(0.5);
        if (guv.x < 0.0 || guv.y < 0.0 || guv.x > 1.0 || guv.y > 1.0) {
            discard;
        }
        let g = textureSampleLevel(glyph_tex, smp, guv, 0.0);
        alpha = g.a;
        if (alpha <= 0.004) {
            discard;
        }
        // Decodifica 128/255 → 0 exato e soma domo esférico (paridade 1:1 com `opticalGlassWebGL.ts`).
        let sobel_n = clamp((g.rg * 255.0 - vec2<f32>(128.0)) / 126.0, vec2<f32>(-1.0), vec2<f32>(1.0));
        let dome_n = local_px / max(half_size, vec2<f32>(1.0));
        normal = clamp(sobel_n + dome_n * 0.28, vec2<f32>(-1.0), vec2<f32>(1.0));
        edge_strength = g.b;
        lens_mag = edge_strength;
        border_mask = smoothstep(0.35, 0.95, edge_strength);
    }

    // Rotação da camada (parte linear da base afim, normalizada) para girar os deslocamentos.
    let cs = inst.xf.x / scale;
    let sn = inst.xf.y / scale;

    // Deslocamento de lente real (px de tela, girado com a camada).
    let disp_local = -normal * (refraction_px * 1.35);
    let disp_vp = vec2<f32>(
        disp_local.x * cs - disp_local.y * sn,
        disp_local.x * sn + disp_local.y * cs,
    );

    let vp = in.pos.xy;
    var bg: vec4<f32>;
    if (chroma_px > 0.05) {
        let ca = clamp(chroma_px * 0.075, 0.0, 0.45);
        var dir = disp_vp;
        if (length(dir) < 0.5) {
            let fb = -normal * (chroma_px * 2.2);
            dir = vec2<f32>(fb.x * cs - fb.y * sn, fb.x * sn + fb.y * cs);
        }
        let r_s = sample_backdrop(inst, vp + dir * (1.0 + ca));
        let g_s = sample_backdrop(inst, vp + dir);
        let b_s = sample_backdrop(inst, vp + dir * (1.0 - ca));
        bg = vec4<f32>(r_s.r, g_s.g, b_s.b, clamp((r_s.a + g_s.a + b_s.a) * (1.0 / 3.0), 0.0, 1.0));
    } else {
        bg = sample_backdrop(inst, vp + disp_vp);
    }

    // Transmissão óptica (brilho, contraste, saturação) preservando alpha pré-multiplicado:
    // áreas transparentes de cena 3D (bg.a == 0) permanecem transparentes em vez de virar preto.
    var res = vec4<f32>(0.0);
    if (bg.a > 0.001) {
        var rgb = clamp(bg.rgb / bg.a, vec3<f32>(0.0), vec3<f32>(1.0));
        rgb = rgb * brightness;
        rgb = (rgb - vec3<f32>(0.5)) * contrast + vec3<f32>(0.5);
        let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = clamp(mix(vec3<f32>(luma), rgb, saturation), vec3<f32>(0.0), vec3<f32>(1.0));
        res = vec4<f32>(rgb * bg.a, bg.a);
    }

    // Véu de tint (Porter-Duff src-over: 0% = cristal incolor, 80% = vidro leitoso/fumê).
    let uv01 = local_px / max(layer_size, vec2<f32>(1.0)) + vec2<f32>(0.5); // top-down
    let diag = clamp(uv01.x * 0.5 + uv01.y * 0.5, 0.0, 1.0);
    if (frost > 0.002) {
        let tint_amt = clamp(mix(frost * 1.18, frost * 0.82, diag), 0.0, 0.94);
        res = vec4<f32>(inst.tint.rgb * tint_amt, tint_amt) + res * (1.0 - tint_amt);
    }

    // Micro-grão fosco determinístico (escalado pela cobertura real do vidro).
    if (noise_opacity > 0.002 && res.a > 0.002) {
        let n1 = hash12(in.pos.xy + vec2<f32>(17.3, 41.7)) - 0.5;
        let n2 = hash12(floor(in.pos.xy * 0.5) + vec2<f32>(91.1, 13.7)) - 0.5;
        let grain = n1 * 0.65 + n2 * 0.35;
        res = vec4<f32>(
            clamp(res.rgb + vec3<f32>(grain * noise_opacity * 0.88) * res.a, vec3<f32>(0.0), vec3<f32>(res.a)),
            res.a,
        );
    }

    // Pigmento do primeiro plano no modo glifo (src-over real: 0% = transparente, 100% = cor cheia).
    if (glyph_mode && inst.pigment.a > 0.001) {
        let pa = clamp(inst.pigment.a, 0.0, 1.0);
        res = vec4<f32>(inst.pigment.rgb * pa, pa) + res * (1.0 - pa);
    }

    // Realce especular direcional + reflexo interno oposto.
    let n_len = length(normal);
    var unit_n = vec2<f32>(0.0);
    if (n_len > 0.001) {
        unit_n = normal / n_len;
    }
    let ndotl = dot(unit_n, light_dir);

    if (specular > 0.005) {
        let top_spec = pow(max(0.0, ndotl), 1.45) * edge_strength * specular * 0.92;
        let bottom_bounce = pow(max(0.0, -ndotl), 1.7) * edge_strength * specular * 0.30;
        var sheen_k = 0.08;
        if (!glyph_mode) {
            sheen_k = 0.14;
        }
        let sheen = smoothstep(0.62, 0.0, diag) * specular * sheen_k;
        let bevel_glow = lens_mag * specular * 0.12;
        let spec_rgb = vec3<f32>(1.0) * (top_spec + sheen + bevel_glow)
            + vec3<f32>(0.86, 0.90, 1.0) * bottom_bounce;
        let spec_a = clamp(max(spec_rgb.r, max(spec_rgb.g, spec_rgb.b)), 0.0, 1.0);
        res = vec4<f32>(res.rgb + spec_rgb, clamp(res.a + spec_a * (1.0 - res.a), 0.0, 1.0));
    }

    // Borda sutil tingida no bisel externo (src-over).
    if (border_opacity > 0.005) {
        var rim = border_mask * 0.65;
        if (!glyph_mode) {
            rim = border_mask;
        }
        let ba = clamp(rim * border_opacity, 0.0, 0.85);
        res = vec4<f32>(inst.tint.rgb * ba, ba) + res * (1.0 - ba);
    }

    let mask = clamp(alpha * opacity, 0.0, 1.0);
    return vec4<f32>(clamp(res.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * mask, clamp(res.a, 0.0, 1.0) * mask);
}
