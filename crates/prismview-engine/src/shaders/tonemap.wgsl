// HDR (linear, alpha pré-multiplicado) → saída RGBA8 para o compositor 2D: ACES Filmic idêntico ao
// `ACESFilmicToneMapping` do three.js, codificação sRGB e alpha pré-multiplicado (formato
// não-sRGB, como compositor2d.wgsl espera). Fundo sólido opcional composto por baixo, sem tone
// mapping (como `scene.background = Color` no three.js).

struct Params {
    // x = exposição, y = 1 se fundo sólido
    info: vec4<f32>,
    // fundo em sRGB (não linear)
    background: vec4<f32>,
};

@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    return out;
}

fn rrt_odt_fit(v: vec3<f32>) -> vec3<f32> {
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return a / b;
}

fn aces_filmic(color_in: vec3<f32>, exposure: f32) -> vec3<f32> {
    // Matrizes em colunas (mesma disposição do GLSL do three.js).
    let input_mat = mat3x3<f32>(
        vec3<f32>(0.59719, 0.07600, 0.02840),
        vec3<f32>(0.35458, 0.90834, 0.13383),
        vec3<f32>(0.04823, 0.01566, 0.83777),
    );
    let output_mat = mat3x3<f32>(
        vec3<f32>(1.60475, -0.10208, -0.00327),
        vec3<f32>(-0.53108, 1.10813, -0.07276),
        vec3<f32>(-0.07367, -0.00605, 1.07602),
    );
    var c = color_in * (exposure / 0.6);
    c = input_mat * c;
    c = rrt_odt_fit(c);
    c = output_mat * c;
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureLoad(hdr, vec2<i32>(floor(in.pos.xy)), 0);
    let a = clamp(texel.a, 0.0, 1.0);
    var rgb = vec3<f32>(0.0);
    if a > 1e-5 {
        rgb = linear_to_srgb(aces_filmic(texel.rgb / a, params.info.x)) * a; // pré-multiplicado
    }
    if params.info.y > 0.5 {
        return vec4<f32>(rgb + params.background.rgb * (1.0 - a), 1.0);
    }
    return vec4<f32>(rgb, a);
}
