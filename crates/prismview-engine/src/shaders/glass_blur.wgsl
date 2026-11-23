// Pirâmide de blur do vidro óptico (core/glass/blur.rs): downsample 2× anti-aliased +
// Gaussiana separável contínua de 15 taps (passo exato de 1,0 texel — zero ghosting, zero
// artefatos de caixa e paridade 1:1 com `opticalGlassWebGL.ts` do Estúdio).
//
// Preserva os 4 canais RGBA pré-multiplicados quando `a.w < 0.5` (ex.: cena 3D transparente),
// ou força alpha = 1.0 quando `a.w >= 0.5` (vídeo/imagem opacos).

struct Params {
    // fs_down:  (half_texel.xy, 0, force_opaque)
    // fs_gauss: (axis_step.xy, sigma, force_opaque)
    a: vec4<f32>,
    b: vec4<f32>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_smp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var out: VOut;
    out.pos = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    // v = 0 no topo da textura.
    out.uv = vec2<f32>(x, 1.0 - y);
    return out;
}

fn tap(uv: vec2<f32>) -> vec4<f32> {
    let c = clamp(uv, vec2<f32>(0.001), vec2<f32>(0.999));
    let s = textureSampleLevel(src_tex, src_smp, c, 0.0);
    if (p.a.w >= 0.5) {
        return vec4<f32>(s.rgb, 1.0);
    }
    return s;
}

@fragment
fn fs_down(in: VOut) -> @location(0) vec4<f32> {
    let h = p.a.xy;
    var c = tap(in.uv + vec2<f32>(-h.x, -h.y));
    c += tap(in.uv + vec2<f32>(h.x, -h.y));
    c += tap(in.uv + vec2<f32>(-h.x, h.y));
    c += tap(in.uv + vec2<f32>(h.x, h.y));
    return c * 0.25;
}

@fragment
fn fs_gauss(in: VOut) -> @location(0) vec4<f32> {
    let step = p.a.xy;
    let sigma = max(p.a.z, 0.28);
    let inv2s2 = 0.5 / (sigma * sigma);
    var sum = tap(in.uv);
    var wsum = 1.0;

    for (var i = 1; i <= 7; i = i + 1) {
        let fi = f32(i);
        let w = exp(-fi * fi * inv2s2);
        let off = step * fi;
        sum += (tap(in.uv + off) + tap(in.uv - off)) * w;
        wsum += 2.0 * w;
    }

    return sum / wsum;
}
