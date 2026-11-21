// Pirâmide de blur do vidro óptico (core/glass.rs): Dual Kawase em resolução reduzida.
//
// Três passes de fragmento sobre um triângulo de tela cheia, todos lendo `src_tex` e
// escrevendo num alvo Rgba16Float:
//   fs_down_first : fonte em resolução cheia -> 1/4 (4 taps bilineares em ±1 texel = caixa 4x4
//                   exata, sem aliasing num único passe).
//   fs_down       : nível i-1 -> nível i (1/2), filtro Kawase de 5 taps.
//   fs_up         : nível i+1 -> nível i (2x), filtro Kawase de 8 taps; `offset` controla o raio.
//
// `texel` = 1 / tamanho da TEXTURA DE ORIGEM de cada passe. Os valores ficam em codificação de
// tela (sem linearizar), igual ao WebGL2 do Estúdio (opticalGlassWebGL.ts).

// Slot de 32 bytes do `ParamRing` (compartilhado com os passes de glifo): a = (texel.xy, offset, 0).
struct Params {
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
    // Triângulo gigante: (0,0) (2,0) (0,2) -> clip (-1,-1) (3,-1) (-1,3).
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var out: VOut;
    out.pos = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    // v = 0 no topo da textura.
    out.uv = vec2<f32>(x, 1.0 - y);
    return out;
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(src_tex, src_smp, uv, 0.0).rgb;
}

@fragment
fn fs_down_first(in: VOut) -> @location(0) vec4<f32> {
    let t = p.a.xy;
    var c = tap(in.uv + vec2<f32>(-t.x, -t.y));
    c += tap(in.uv + vec2<f32>(t.x, -t.y));
    c += tap(in.uv + vec2<f32>(-t.x, t.y));
    c += tap(in.uv + vec2<f32>(t.x, t.y));
    return vec4<f32>(c * 0.25, 1.0);
}

@fragment
fn fs_down(in: VOut) -> @location(0) vec4<f32> {
    let t = p.a.xy * p.a.z;
    var c = tap(in.uv) * 4.0;
    c += tap(in.uv + vec2<f32>(-t.x, -t.y));
    c += tap(in.uv + vec2<f32>(t.x, -t.y));
    c += tap(in.uv + vec2<f32>(-t.x, t.y));
    c += tap(in.uv + vec2<f32>(t.x, t.y));
    return vec4<f32>(c * 0.125, 1.0);
}

@fragment
fn fs_up(in: VOut) -> @location(0) vec4<f32> {
    let h = p.a.xy * (0.5 * p.a.z);
    var c = tap(in.uv + vec2<f32>(-2.0 * h.x, 0.0));
    c += tap(in.uv + vec2<f32>(-h.x, h.y)) * 2.0;
    c += tap(in.uv + vec2<f32>(0.0, 2.0 * h.y));
    c += tap(in.uv + vec2<f32>(h.x, h.y)) * 2.0;
    c += tap(in.uv + vec2<f32>(2.0 * h.x, 0.0));
    c += tap(in.uv + vec2<f32>(h.x, -h.y)) * 2.0;
    c += tap(in.uv + vec2<f32>(0.0, -2.0 * h.y));
    c += tap(in.uv + vec2<f32>(-h.x, -h.y)) * 2.0;
    return vec4<f32>(c * (1.0 / 12.0), 1.0);
}
