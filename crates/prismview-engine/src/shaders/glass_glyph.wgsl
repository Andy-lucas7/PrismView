// Passes de GPU que geram o mapa de normais dos glifos (core/glass.rs). Port do
// `GLYPH_PACK_FRAG_SRC` (Sobel) de opticalGlassWebGL.ts e do blur de altura
// (`ctx.filter = blur(px)`) de glassLens.ts — sem nenhuma passada de CPU.
//
// Passes sobre um triângulo de tela cheia. `pos.xy` = texel de destino (alvo com o tamanho exato da
// região do texto). Slot de 32 B do ParamRing: a = (axis.xy, sigma, raio), b = (região.xy, 0, 0).

struct PassParams {
    a: vec4<f32>,
    b: vec4<f32>,
};

@group(0) @binding(0) var<uniform> pp: PassParams;
@group(0) @binding(1) var src_a: texture_2d<f32>; // gauss: entrada; pack: máscara nítida
@group(0) @binding(2) var src_b: texture_2d<f32>; // pack: altura borrada (gauss: ignorada)

struct FVOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FVOut {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var out: FVOut;
    out.pos = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn in_region(p: vec2<i32>) -> bool {
    let r = vec2<i32>(i32(pp.b.x), i32(pp.b.y));
    return p.x >= 0 && p.y >= 0 && p.x < r.x && p.y < r.y;
}

fn mask_at(p: vec2<i32>) -> f32 {
    if (!in_region(p)) {
        return 0.0;
    }
    return textureLoad(src_a, p, 0).a;
}

// Gaussiana separável (equivale ao `ctx.filter = blur(σpx)` do canvas 2D). Lê o canal alpha
// com zero-padding fora da região para que o bisel nas bordas da caixa decaia suavemente.
@fragment
fn fs_gauss(in: FVOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let axis = vec2<i32>(i32(pp.a.x), i32(pp.a.y));
    let sigma = max(pp.a.z, 0.3);
    let radius = i32(pp.a.w);
    let inv2s2 = 0.5 / (sigma * sigma);
    var sum = mask_at(p);
    var wsum = 1.0;
    for (var i = 1; i <= radius; i = i + 1) {
        let w = exp(-f32(i * i) * inv2s2);
        sum = sum + (mask_at(p + axis * i) + mask_at(p - axis * i)) * w;
        wsum = wsum + 2.0 * w;
    }
    let v = sum / wsum;
    return vec4<f32>(v, v, v, v);
}

fn height_at(p: vec2<i32>) -> f32 {
    if (!in_region(p)) {
        return 0.0;
    }
    return textureLoad(src_b, p, 0).a;
}

// Sobel na GPU: R = nx, G = ny, B = inclinação da borda, A = alpha nítido (igual à passada do TS).
@fragment
fn fs_pack(in: FVOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let sharp_a = mask_at(p);
    if (sharp_a * 255.0 < 3.0) {
        return vec4<f32>(128.0 / 255.0, 128.0 / 255.0, 0.0, 0.0);
    }
    // Texturas nativas não são invertidas: "acima" no canvas = y - 1.
    let tl = height_at(p + vec2<i32>(-1, -1));
    let t = height_at(p + vec2<i32>(0, -1));
    let tr = height_at(p + vec2<i32>(1, -1));
    let l = height_at(p + vec2<i32>(-1, 0));
    let r = height_at(p + vec2<i32>(1, 0));
    let bl = height_at(p + vec2<i32>(-1, 1));
    let b = height_at(p + vec2<i32>(0, 1));
    let br = height_at(p + vec2<i32>(1, 1));
    let gx = (tl + 2.0 * l + bl) - (tr + 2.0 * r + br);
    let gy = (tl + 2.0 * t + tr) - (bl + 2.0 * b + br);
    let slope = length(vec2<f32>(gx, gy));
    let bevel_sigma = max(pp.a.x, 1.5);
    if (slope > 0.002) {
        let gain = min(1.0, slope * max(2.2, bevel_sigma * 0.9));
        let n = vec2<f32>(gx, gy) / slope * gain;
        let packed = clamp(vec2<f32>(128.0) + n * 126.0, vec2<f32>(0.0), vec2<f32>(255.0)) / 255.0;
        return vec4<f32>(packed, gain, sharp_a);
    }
    return vec4<f32>(128.0 / 255.0, 128.0 / 255.0, 0.0, sharp_a);
}
