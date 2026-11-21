// Compositor 2D do motor nativo (Fase 1).
//
// Um quad instanciado por elemento (triangle strip, 4 vértices, sem vertex buffer). Os dados de
// cada instância vêm de um storage buffer (`insts`), lido no vértice e no fragmento.
//
// - Forma: caixa com cantos arredondados por SDF (retângulo/círculo/pílula/linha) + borda interna.
// - Transformação: afim 2D (rotação + escala uniforme) aplicada no vértice; o SDF é avaliado no
//   espaço local (antes da transformação), então AA e bordas ficam nítidas em qualquer escala.
// - Preenchimento: sólido ou gradiente (linear/radial/sweep).
// - Textura: `TEXTURE` (imagem × tint) e `MASK` (preenchimento × alpha da textura: texto/ícones).
// - Máscara: clip retangular arredondado em espaço de tela (`clip`).
// - Sombra suave: SDF com transição larga (`SHADOW`).
//
// Saída: alpha PRÉ-MULTIPLICADO (blend states do compositor assumem isso). Tudo na codificação
// da tela (superfície não-sRGB): não há conversão de gama aqui.

struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};

struct Inst {
    // xy = centro em px de tela, zw = meio tamanho local (px).
    center_half: vec4<f32>,
    // Colunas da matriz 2x2: (eixo X: xy) (eixo Y: zw).
    basis: vec4<f32>,
    // Cor de preenchimento / tint (alpha reto, já multiplicada pela opacidade).
    color: vec4<f32>,
    // Segunda cor do gradiente.
    color2: vec4<f32>,
    // Cor da borda (alpha reto).
    border: vec4<f32>,
    // u0 v0 u1 v1 (região da textura mapeada no quad).
    uv_rect: vec4<f32>,
    // Máscara: centro xy, meio tamanho zw (px de tela). zw < 0 = sem máscara.
    clip: vec4<f32>,
    // x = raio dos cantos, y = largura da borda, z = margem extra do quad (AA/sombra), w = raio do clip.
    shape: vec4<f32>,
    // xy = direção do gradiente (local), z = largura de AA (px locais), w = blur da sombra (px locais).
    extra: vec4<f32>,
    // x = tipo, y = gradiente (0 none, 1 linear, 2 radial, 3 sweep).
    mode: vec4<u32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var<storage, read> insts: array<Inst>;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

const KIND_SOLID: u32 = 0u;
const KIND_TEXTURE: u32 = 1u;
const KIND_MASK: u32 = 2u;
const KIND_SHADOW: u32 = 3u;
const TAU: f32 = 6.28318530718;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) world: vec2<f32>,
    @location(2) @interpolate(flat) idx: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let inst = insts[ii];
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u)) * 2.0 - 1.0;
    let hs = inst.center_half.zw;
    let local = corner * (hs + vec2<f32>(inst.shape.z));
    let basis = mat2x2<f32>(inst.basis.xy, inst.basis.zw);
    let world = inst.center_half.xy + basis * local;

    var out: VOut;
    out.pos = vec4<f32>(
        world.x / g.viewport.x * 2.0 - 1.0,
        1.0 - world.y / g.viewport.y * 2.0,
        0.0,
        1.0,
    );
    out.local = local;
    out.world = world;
    out.idx = ii;
    return out;
}

// Distância assinada até uma caixa de meio tamanho `b` com cantos de raio `r` (negativa dentro).
fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn gradient_t(inst: Inst, local: vec2<f32>) -> f32 {
    let hs = max(inst.center_half.zw, vec2<f32>(1e-4));
    let dir = inst.extra.xy;
    let kind = inst.mode.y;
    if (kind == 1u) {
        let ext = max(abs(dir.x) * hs.x + abs(dir.y) * hs.y, 1e-4);
        return dot(local, dir) / (2.0 * ext) + 0.5;
    }
    if (kind == 2u) {
        return length(local / hs);
    }
    let a = atan2(local.y, local.x) - atan2(dir.y, dir.x);
    return fract(a / TAU + 1.0);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let inst = insts[in.idx];
    let hs = inst.center_half.zw;
    let radius = clamp(inst.shape.x, 0.0, min(hs.x, hs.y));
    let aa = max(inst.extra.z, 1e-5);
    let d = sd_round_box(in.local, hs, radius);
    let kind = inst.mode.x;

    var out = vec4<f32>(0.0);

    if (kind == KIND_SHADOW) {
        let blur = max(inst.extra.w, aa);
        let cov = 1.0 - smoothstep(-blur, blur, d);
        out = vec4<f32>(inst.color.rgb * inst.color.a, inst.color.a) * cov;
    } else {
        let cov = 1.0 - smoothstep(-0.5 * aa, 0.5 * aa, d);

        var fill = inst.color;
        if (inst.mode.y != 0u) {
            fill = mix(inst.color, inst.color2, clamp(gradient_t(inst, in.local), 0.0, 1.0));
        }

        // `textureSampleLevel` é permitido em fluxo não uniforme (o tipo é por instância).
        let uvn = in.local / (2.0 * max(hs, vec2<f32>(1e-4))) + vec2<f32>(0.5);
        let uv = mix(inst.uv_rect.xy, inst.uv_rect.zw, uvn);
        let t = textureSampleLevel(tex, samp, uv, 0.0);

        var rgba: vec4<f32>;
        if (kind == KIND_TEXTURE) {
            // Textura pré-multiplicada × tint (rgb) × opacidade (a).
            rgba = t * vec4<f32>(inst.color.rgb, 1.0) * inst.color.a;
        } else if (kind == KIND_MASK) {
            // Preenchimento (sólido/gradiente) recortado pelo alpha da textura.
            rgba = vec4<f32>(fill.rgb * fill.a, fill.a) * t.a;
        } else {
            rgba = vec4<f32>(fill.rgb * fill.a, fill.a);
        }

        // Borda interna: faixa de largura `bw` junto ao contorno, por cima do preenchimento.
        let bw = inst.shape.y;
        if (bw > 0.0) {
            let bmask = smoothstep(-bw - 0.5 * aa, -bw + 0.5 * aa, d);
            let bc = inst.border;
            let bp = vec4<f32>(bc.rgb * bc.a, bc.a);
            rgba = rgba * (1.0 - bmask * bc.a) + bp * bmask;
        }

        out = rgba * cov;
    }

    // Máscara retangular arredondada em espaço de tela (AA de 1 px).
    if (inst.clip.z >= 0.0) {
        let cd = sd_round_box(in.world - inst.clip.xy, inst.clip.zw, inst.shape.w);
        out = out * (1.0 - smoothstep(-0.5, 0.5, cd));
    }

    return out;
}
