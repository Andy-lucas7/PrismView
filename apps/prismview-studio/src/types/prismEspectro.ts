// Modo Espectro: efeitos WGSL do usuário (camada SHADER).
//
// Contrato do código do usuário: definir `fn effect(uv: vec2f) -> vec4f` (cor com alfa reto; o
// prelúdio pré-multiplica). Parâmetros são declarados em comentários e viram controles no Studio:
//   //@param forca: float = 0.1 range(0, 1) label("Força")   → `params.forca` no WGSL (nome ASCII)
// Tipos: float (f32) · vec2 (vec2f) · color (vec4f, "#rrggbb[aa]") · bool (f32 0/1).
// O Studio monta o WGSL completo (`buildEspectroShader`) e o motor nativo recebe esse mesmo texto
// pronto (`nativeWgsl`) + os parâmetros já empacotados (`nativeParams`), então o layout dos
// uniforms vive só aqui.

export type EspectroParamType = "float" | "vec2" | "color" | "bool";
export type EspectroParamValue = number | boolean | string | [number, number];

export interface EspectroParamDef {
  name: string;
  type: EspectroParamType;
  label: string;
  default: EspectroParamValue;
  min?: number;
  max?: number;
  step?: number;
  /** Byte offset dentro do uniform `params` (regras de alinhamento do WGSL). */
  offset: number;
}

export interface EspectroEffect {
  name: string;
  author?: string;
  description?: string;
  /** Código WGSL do usuário (sem prelúdio). */
  code: string;
  /** Valores escolhidos no Studio; parâmetro ausente usa o default do `//@param`. */
  params: Record<string, EspectroParamValue>;
  /** Preenchidos só ao aplicar no motor nativo (`withNativeAssets`). */
  nativeWgsl?: string;
  nativeParams?: number[];
}

export const PRISMFX_FORMAT = "prismfx";
export const PRISMFX_VERSION = 1;
export const ESPECTRO_MAX_PARAMS = 32;
export const ESPECTRO_MAX_CODE_CHARS = 64 * 1024;

/** Tamanho do uniform `prism` (PrismGlobals) em floats; o motor Rust usa o mesmo número. */
export const ESPECTRO_GLOBALS_FLOATS = 24;

/**
 * Prelúdio comum (Studio WebGPU e motor wgpu). Layout de `PrismGlobals` (bytes):
 * 0 resolution · 8 time · 12 delta · 16 cursor · 24 frame_size · 32 layer_rect · 48 backdrop_rect
 * · 64 audio (level, bass, mid, treble) · 80 beat · 84 battery · 88 has_backdrop · 92 backdrop_opaque → 96.
 */
export const ESPECTRO_PRELUDE = `// ── Prelúdio Espectro (PrismView) ──
struct PrismGlobals {
  resolution: vec2f,
  time: f32,
  delta: f32,
  cursor: vec2f,
  frame_size: vec2f,
  layer_rect: vec4f,
  backdrop_rect: vec4f,
  audio: vec4f,
  beat: f32,
  battery: f32,
  has_backdrop: f32,
  backdrop_opaque: f32,
};
@group(0) @binding(0) var<uniform> prism: PrismGlobals;
@group(0) @binding(1) var prism_backdrop: texture_2d<f32>;
@group(0) @binding(2) var prism_sampler: sampler;

fn frame_uv(uv: vec2f) -> vec2f {
  return prism.layer_rect.xy + uv * prism.layer_rect.zw;
}
fn sample_backdrop(uv: vec2f) -> vec4f {
  let f = frame_uv(uv);
  let b = (f - prism.backdrop_rect.xy) / max(prism.backdrop_rect.zw, vec2f(1e-5));
  var c = textureSampleLevel(prism_backdrop, prism_sampler, clamp(b, vec2f(0.0), vec2f(1.0)), 0.0);
  if (prism.backdrop_opaque > 0.5) {
    c.a = 1.0;
  }
  return c;
}
fn hash21(p: vec2f) -> f32 {
  var q = fract(p * vec2f(123.34, 456.21));
  q += dot(q, q + 45.32);
  return fract(q.x * q.y);
}
fn noise2(p: vec2f) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let u = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash21(i), hash21(i + vec2f(1.0, 0.0)), u.x),
             mix(hash21(i + vec2f(0.0, 1.0)), hash21(i + vec2f(1.0, 1.0)), u.x), u.y);
}
`;

const ESPECTRO_ENTRY = `
// ── Entrada Espectro (PrismView) ──
struct PrismVsOut {
  @builtin(position) pos: vec4f,
  @location(0) uv: vec2f,
};
@vertex fn prism_vs(@builtin(vertex_index) i: u32) -> PrismVsOut {
  let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  var o: PrismVsOut;
  o.pos = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
  o.uv = vec2f(p.x, 1.0 - p.y);
  return o;
}
@fragment fn prism_fs(v: PrismVsOut) -> @location(0) vec4f {
  let c = clamp(effect(v.uv), vec4f(0.0), vec4f(1.0));
  return vec4f(c.rgb * c.a, c.a);
}
`;

const PARAM_RE =
  /^\s*\/\/@param\s+([A-Za-z][A-Za-z0-9_]*)\s*:\s*(float|vec2|color|bool)\b\s*(?:=\s*("[^"]*"|#[0-9a-fA-F]{6,8}|[-+0-9.eE]+\s*,\s*[-+0-9.eE]+|[-+0-9.eE]+|true|false))?(.*)$/u;

export interface EspectroParseResult {
  params: EspectroParamDef[];
  /** Linhas `//@param` com problema (1-based no código do usuário). */
  errors: { line: number; message: string }[];
  /** Bytes do uniform `params` (múltiplo de 16, mínimo 16). */
  paramsSize: number;
}

function parseOptNumber(rest: string, key: string): number | undefined {
  const m = rest.match(new RegExp(`\\b${key}\\(\\s*([-+0-9.eE]+)\\s*\\)`));
  if (!m) return undefined;
  const v = Number(m[1]);
  return Number.isFinite(v) ? v : undefined;
}

export function parseHexColor(hex: string): [number, number, number, number] {
  const h = hex.replace(/^#/, "");
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  const a = h.length >= 8 ? parseInt(h.slice(6, 8), 16) : 255;
  const ok = [r, g, b, a].every((v) => Number.isFinite(v));
  return ok ? [r / 255, g / 255, b / 255, a / 255] : [1, 1, 1, 1];
}

function defaultFor(type: EspectroParamType, raw: string | undefined): EspectroParamValue {
  const t = raw?.trim();
  switch (type) {
    case "float": {
      const v = Number(t);
      return t !== undefined && Number.isFinite(v) ? v : 0;
    }
    case "vec2": {
      const parts = (t ?? "").split(",").map((s) => Number(s.trim()));
      return parts.length === 2 && parts.every(Number.isFinite) ? [parts[0], parts[1]] : [0, 0];
    }
    case "color":
      return t && /^#[0-9a-fA-F]{6,8}$/.test(t) ? t : "#ffffff";
    case "bool":
      return t === "true";
  }
}

const SIZE_ALIGN: Record<EspectroParamType, [number, number]> = {
  float: [4, 4],
  bool: [4, 4],
  vec2: [8, 8],
  color: [16, 16],
};

/** Lê as declarações `//@param` e calcula o layout do uniform (regras de alinhamento do WGSL). */
export function parseEspectroParams(code: string): EspectroParseResult {
  const params: EspectroParamDef[] = [];
  const errors: EspectroParseResult["errors"] = [];
  const seen = new Set<string>();
  let offset = 0;
  const lines = code.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!line.includes("//@param")) continue;
    const m = line.match(PARAM_RE);
    if (!m) {
      errors.push({ line: i + 1, message: "Declaração inválida. Use: //@param nome: float = 0.5 range(0, 1)" });
      continue;
    }
    const [, name, typeRaw, rawDefault, rest = ""] = m;
    const type = typeRaw as EspectroParamType;
    if (seen.has(name)) {
      errors.push({ line: i + 1, message: `Parâmetro "${name}" repetido.` });
      continue;
    }
    if (params.length >= ESPECTRO_MAX_PARAMS) {
      errors.push({ line: i + 1, message: `Máximo de ${ESPECTRO_MAX_PARAMS} parâmetros.` });
      continue;
    }
    seen.add(name);
    const [size, align] = SIZE_ALIGN[type];
    offset = Math.ceil(offset / align) * align;
    const label = rest.match(/\blabel\(\s*"([^"]*)"\s*\)/)?.[1] ?? name;
    const range = rest.match(/\brange\(\s*([-+0-9.eE]+)\s*,\s*([-+0-9.eE]+)\s*\)/);
    const lo = range ? Number(range[1]) : NaN;
    const hi = range ? Number(range[2]) : NaN;
    params.push({
      name,
      type,
      label,
      default: defaultFor(type, rawDefault?.replace(/^"|"$/g, "")),
      min: Number.isFinite(lo) ? lo : undefined,
      max: Number.isFinite(hi) ? hi : undefined,
      step: parseOptNumber(rest, "step"),
      offset,
    });
    offset += size;
  }
  const paramsSize = Math.max(16, Math.ceil(offset / 16) * 16);
  return { params, errors, paramsSize };
}

const WGSL_TYPE: Record<EspectroParamType, string> = {
  float: "f32",
  bool: "f32",
  vec2: "vec2f",
  color: "vec4f",
};

export interface EspectroBuild {
  wgsl: string;
  /** Linha (1-based) do WGSL completo onde começa o código do usuário. */
  userLineStart: number;
  parse: EspectroParseResult;
}

/** WGSL completo: prelúdio + struct de parâmetros + código do usuário + entradas. */
export function buildEspectroShader(code: string, parse = parseEspectroParams(code)): EspectroBuild {
  const fields = parse.params.length
    ? parse.params.map((p) => `  ${p.name}: ${WGSL_TYPE[p.type]},`).join("\n")
    : "  _unused: vec4f,";
  const head =
    `${ESPECTRO_PRELUDE}\nstruct PrismParams {\n${fields}\n};\n` +
    `@group(0) @binding(3) var<uniform> params: PrismParams;\n// ── Código do usuário ──\n`;
  const userLineStart = head.split("\n").length;
  return { wgsl: head + code + "\n" + ESPECTRO_ENTRY, userLineStart, parse };
}

function clampNum(v: number, def: EspectroParamDef): number {
  let out = v;
  if (def.min !== undefined) out = Math.max(def.min, out);
  if (def.max !== undefined) out = Math.min(def.max, out);
  return out;
}

/** Valor efetivo de um parâmetro (escolhido no Studio ou o default), já validado. */
export function resolveParamValue(def: EspectroParamDef, values: Record<string, EspectroParamValue> | undefined): EspectroParamValue {
  const v = values?.[def.name];
  switch (def.type) {
    case "float":
      return typeof v === "number" && Number.isFinite(v) ? clampNum(v, def) : (def.default as number);
    case "bool":
      return typeof v === "boolean" ? v : (def.default as boolean);
    case "color":
      return typeof v === "string" && /^#[0-9a-fA-F]{6,8}$/.test(v) ? v : (def.default as string);
    case "vec2":
      return Array.isArray(v) && v.length === 2 && v.every((n) => typeof n === "number" && Number.isFinite(n))
        ? [v[0], v[1]]
        : (def.default as [number, number]);
  }
}

/** Escreve os parâmetros em `out` (floats; reaproveitável entre quadros, sem alocar). */
export function packEspectroParams(
  parse: EspectroParseResult,
  values: Record<string, EspectroParamValue> | undefined,
  out: Float32Array
): void {
  out.fill(0);
  for (const def of parse.params) {
    const i = def.offset / 4;
    const v = resolveParamValue(def, values);
    if (def.type === "float") out[i] = v as number;
    else if (def.type === "bool") out[i] = v ? 1 : 0;
    else if (def.type === "vec2") {
      out[i] = (v as [number, number])[0];
      out[i + 1] = (v as [number, number])[1];
    } else {
      const c = parseHexColor(v as string);
      out[i] = c[0];
      out[i + 1] = c[1];
      out[i + 2] = c[2];
      out[i + 3] = c[3];
    }
  }
}

/** Campos nativos (`nativeWgsl`/`nativeParams`) para o motor. */
export function withNativeEspectro(effect: EspectroEffect): EspectroEffect {
  const build = buildEspectroShader(effect.code);
  const packed = new Float32Array(build.parse.paramsSize / 4);
  packEspectroParams(build.parse, effect.params, packed);
  return { ...effect, nativeWgsl: build.wgsl, nativeParams: Array.from(packed) };
}

// ── Arquivo .prismfx ──

export interface PrismFxFile {
  format: typeof PRISMFX_FORMAT;
  version: number;
  effect: Pick<EspectroEffect, "name" | "author" | "description" | "code" | "params">;
}

export function serializePrismFx(effect: EspectroEffect): string {
  const file: PrismFxFile = {
    format: PRISMFX_FORMAT,
    version: PRISMFX_VERSION,
    effect: {
      name: effect.name,
      author: effect.author,
      description: effect.description,
      code: effect.code,
      params: effect.params,
    },
  };
  return JSON.stringify(file, null, 2);
}

/** Lê um `.prismfx`; lança `Error` com mensagem para o usuário se o arquivo não servir. */
export function parsePrismFx(text: string): EspectroEffect {
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    throw new Error("Arquivo .prismfx corrompido (JSON inválido).");
  }
  const file = data as Partial<PrismFxFile>;
  if (!file || file.format !== PRISMFX_FORMAT || !file.effect) {
    throw new Error("Este arquivo não é um efeito Espectro (.prismfx).");
  }
  if (typeof file.version !== "number" || file.version > PRISMFX_VERSION) {
    throw new Error("Efeito feito numa versão mais nova do PrismView. Atualize o app.");
  }
  const e = file.effect;
  if (typeof e.code !== "string" || !e.code.trim()) throw new Error("Efeito sem código.");
  if (e.code.length > ESPECTRO_MAX_CODE_CHARS) throw new Error("Código do efeito grande demais.");
  const params: Record<string, EspectroParamValue> = {};
  if (e.params && typeof e.params === "object") {
    for (const [k, v] of Object.entries(e.params)) {
      if (typeof v === "number" || typeof v === "boolean" || typeof v === "string" || (Array.isArray(v) && v.length === 2)) {
        params[k] = v as EspectroParamValue;
      }
    }
  }
  return {
    name: typeof e.name === "string" && e.name.trim() ? e.name.trim().slice(0, 80) : "Efeito importado",
    author: typeof e.author === "string" ? e.author.slice(0, 80) : undefined,
    description: typeof e.description === "string" ? e.description.slice(0, 400) : undefined,
    code: e.code,
    params,
  };
}

// ── Modelos prontos ──

export interface EspectroTemplate {
  id: string;
  name: string;
  description: string;
  code: string;
}

export const ESPECTRO_TEMPLATES: EspectroTemplate[] = [
  {
    id: "rain-glass",
    name: "Chuva no vidro",
    description: "Gotas escorrendo que refratam o fundo, com vidro embaçado entre elas.",
    code: `//@param density: float = 0.55 range(0, 1) label("Densidade")
//@param speed: float = 0.35 range(0, 2) label("Velocidade")
//@param refraction: float = 0.06 range(0, 0.2) label("Refração")
//@param fog: float = 0.4 range(0, 2) label("Embaçado")
//@param tint: color = #c8dcff label("Tom do vidro")

// xy = desvio da gota, z = máscara (1 dentro da gota/rastro)
fn drop_layer(uv: vec2f, t: f32, scale: f32) -> vec3f {
  let aspect = prism.resolution.x / max(prism.resolution.y, 1.0);
  var p = uv * vec2f(aspect, 1.0) * scale;
  let column = floor(p.x);
  p.y -= t * (0.6 + hash21(vec2f(column, 1.7)) * 0.8);
  let cell = floor(p);
  let f = fract(p) - 0.5;
  let h = hash21(cell);
  if (h > params.density) {
    return vec3f(0.0);
  }
  let wobble = sin(t * 3.0 + h * 40.0) * 0.05;
  let c = vec2f((h - 0.5) * 0.5 + wobble, (fract(h * 13.1) - 0.5) * 0.6);
  let d = f - c;
  let r = 0.12 + fract(h * 7.3) * 0.1;
  let m = smoothstep(r, r * 0.55, length(d * vec2f(1.0, 0.8)));
  let trail = smoothstep(0.05, 0.0, abs(f.x - c.x)) * smoothstep(c.y, c.y - 0.45, f.y) * 0.35;
  return vec3f(d * m, max(m, trail));
}

fn effect(uv: vec2f) -> vec4f {
  let t = prism.time * params.speed;
  let a = drop_layer(uv, t, 9.0);
  let b = drop_layer(uv + vec2f(0.37, 0.11), t * 0.8, 17.0);
  let n = a.xy * a.z + b.xy * b.z * 0.6;
  let mask = clamp(a.z + b.z, 0.0, 1.0);
  let o = params.fog * 0.01 * (1.0 - mask);
  let fogged = (sample_backdrop(uv + vec2f(o, o)) + sample_backdrop(uv + vec2f(-o, o)) +
                sample_backdrop(uv + vec2f(o, -o)) + sample_backdrop(uv - vec2f(o, o))) * 0.25;
  let drop = sample_backdrop(uv - n * params.refraction * 4.0);
  var col = mix(fogged.rgb * params.tint.rgb, drop.rgb * 1.08, mask);
  col += pow(mask, 3.0) * 0.06;
  return vec4f(col, 1.0);
}
`,
  },
  {
    id: "cursor-ripple",
    name: "Onda no cursor",
    description: "Anéis de água que saem do mouse e distorcem o fundo.",
    code: `//@param strength: float = 0.02 range(0, 0.1) label("Força")
//@param rings: float = 28 range(4, 80) label("Anéis")
//@param falloff: float = 4 range(0.5, 12) label("Alcance")

fn effect(uv: vec2f) -> vec4f {
  let aspect = prism.resolution.x / max(prism.resolution.y, 1.0);
  let d = (uv - prism.cursor) * vec2f(aspect, 1.0);
  let r = length(d);
  let w = sin(r * params.rings - prism.time * 6.0) * exp(-r * params.falloff);
  let off = normalize(d + vec2f(1e-5)) * w * params.strength;
  return sample_backdrop(uv + off);
}
`,
  },
  {
    id: "bass-chroma",
    name: "Aberração no grave",
    description: "Separa as cores do fundo no ritmo do grave da música.",
    code: `//@param amount: float = 0.012 range(0, 0.05) label("Separação")
//@param react: float = 1 range(0, 3) label("Reação ao grave")

fn effect(uv: vec2f) -> vec4f {
  let k = params.amount * (0.3 + prism.audio.y * params.react);
  let dir = (uv - 0.5) * k;
  let r = sample_backdrop(uv + dir).r;
  let g = sample_backdrop(uv).g;
  let b = sample_backdrop(uv - dir).b;
  return vec4f(r, g, b, 1.0);
}
`,
  },
];

export function createEspectroEffect(templateId = "rain-glass"): EspectroEffect {
  const t = ESPECTRO_TEMPLATES.find((x) => x.id === templateId) ?? ESPECTRO_TEMPLATES[0];
  return { name: t.name, description: t.description, code: t.code, params: {} };
}
