// Modo Espectro: efeitos WGSL do usuário (camada SHADER).
//
// Contrato do código do usuário: definir `fn effect(uv: vec2f) -> vec4f` (cor com alfa reto; o
// prelúdio pré-multiplica). Parâmetros são declarados em comentários e viram controles no Studio:
//   //@param forca: float = 0.1 range(0, 1) label("Força")   → `params.forca` no WGSL (nome ASCII)
// Tipos: float (f32) · vec2 (vec2f) · color (vec4f, "#rrggbb[aa]") · bool (f32 0/1).
// Texturas do usuário (até 4): //@texture ruido label("Ruído") → `tex_ruido(uv)` e `tex_ruido_size()`.
// Memória entre quadros: `sample_previous(uv)` devolve o quadro anterior do próprio efeito.
// Simulação: definir `fn simulate(uv: vec2f) -> vec4f` cria um buffer de estado (rgba16float) que
// roda antes do efeito; `sample_state(uv)` lê o estado anterior dentro de `simulate` e o estado novo
// dentro de `effect` (água, fluidos, jogo da vida, partículas em grade...).
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
  /** Imagens das declarações `//@texture` (caminho local em `customSrc`, como as texturas do 3D). */
  textures?: EspectroTextureRef[];
  /** Preenchidos só ao aplicar no motor nativo (`withNativeAssets`). */
  nativeWgsl?: string;
  nativeParams?: number[];
  /** Caminho de cada slot `prism_tex0..3` ("" = vazio). */
  nativeTextures?: string[];
  /** O código usa `sample_previous`: o motor guarda o quadro anterior. */
  nativeFeedback?: boolean;
  /** O código define `simulate`: o motor roda o passe de estado (`prism_sim`). */
  nativeSimulate?: boolean;
}

export interface EspectroTextureRef {
  name: string;
  customSrc: string;
}

export interface EspectroTextureDef {
  name: string;
  label: string;
  /** Slot `prism_texN` (0..3). */
  slot: number;
}

export const PRISMFX_FORMAT = "prismfx";
export const PRISMFX_VERSION = 2;
export const ESPECTRO_MAX_TEXTURES = 4;
export const ESPECTRO_MAX_PARAMS = 32;
export const ESPECTRO_MAX_CODE_CHARS = 64 * 1024;

/** Tamanho do uniform `prism` (PrismGlobals) em floats; o motor Rust usa o mesmo número. */
export const ESPECTRO_GLOBALS_FLOATS = 36;

/**
 * Prelúdio comum (Studio WebGPU e motor wgpu). Layout de `PrismGlobals` (bytes):
 * 0 resolution · 8 time · 12 delta · 16 cursor · 24 frame_size · 32 layer_rect · 48 backdrop_rect
 * · 64 audio (level, bass, mid, treble) · 80 beat · 84 battery · 88 has_backdrop · 92 backdrop_opaque
 * · 96 click (uv do último clique) · 104 click_age (s; 1e6 = nenhum) · 108 frame
 * · 112 input (mouse_down, key_pulse, typing_energy, idle_s) · 128 media (playing, progress, has_session, 0) → 144.
 * Bindings: 0 prism · 1 backdrop · 2 sampler (clamp) · 3 params · 4 quadro anterior · 5..8 texturas
 * · 9 sampler (repeat) · 10 estado da simulação.
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
  click: vec2f,
  click_age: f32,
  frame: f32,
  input: vec4f,
  media: vec4f,
};
@group(0) @binding(0) var<uniform> prism: PrismGlobals;
@group(0) @binding(1) var prism_backdrop: texture_2d<f32>;
@group(0) @binding(2) var prism_sampler: sampler;
@group(0) @binding(4) var prism_prev: texture_2d<f32>;
@group(0) @binding(5) var prism_tex0: texture_2d<f32>;
@group(0) @binding(6) var prism_tex1: texture_2d<f32>;
@group(0) @binding(7) var prism_tex2: texture_2d<f32>;
@group(0) @binding(8) var prism_tex3: texture_2d<f32>;
@group(0) @binding(9) var prism_repeat: sampler;
@group(0) @binding(10) var prism_state: texture_2d<f32>;

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
// Quadro anterior deste efeito (cor final pré-multiplicada; transparente no 1º quadro).
fn sample_previous(uv: vec2f) -> vec4f {
  return textureSampleLevel(prism_prev, prism_sampler, uv, 0.0);
}
// Estado da simulação (anterior dentro de \`simulate\`, atual dentro de \`effect\`).
fn sample_state(uv: vec2f) -> vec4f {
  return textureSampleLevel(prism_state, prism_sampler, uv, 0.0);
}
// Um pixel do efeito em uv (para vizinhança em simulações).
fn pixel() -> vec2f {
  return 1.0 / max(prism.resolution, vec2f(1.0));
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

const ESPECTRO_SIM_ENTRY = `@fragment fn prism_sim(v: PrismVsOut) -> @location(0) vec4f {
  return simulate(v.uv);
}
`;

const PARAM_RE =
  /^\s*\/\/@param\s+([A-Za-z][A-Za-z0-9_]*)\s*:\s*(float|vec2|color|bool)\b\s*(?:=\s*("[^"]*"|#[0-9a-fA-F]{6,8}|[-+0-9.eE]+\s*,\s*[-+0-9.eE]+|[-+0-9.eE]+|true|false))?(.*)$/u;

export interface EspectroParseResult {
  params: EspectroParamDef[];
  textures: EspectroTextureDef[];
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
  const textures: EspectroTextureDef[] = [];
  const errors: EspectroParseResult["errors"] = [];
  const seen = new Set<string>();
  let offset = 0;
  const lines = code.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (line.includes("//@texture")) {
      const t = line.match(TEXTURE_RE);
      if (!t) {
        errors.push({ line: i + 1, message: 'Declaração inválida. Use: //@texture ruido label("Ruído")' });
      } else if (seen.has(t[1])) {
        errors.push({ line: i + 1, message: `Nome "${t[1]}" repetido.` });
      } else if (textures.length >= ESPECTRO_MAX_TEXTURES) {
        errors.push({ line: i + 1, message: `Máximo de ${ESPECTRO_MAX_TEXTURES} texturas.` });
      } else {
        seen.add(t[1]);
        textures.push({ name: t[1], label: t[2]?.match(/\blabel\(\s*"([^"]*)"\s*\)/)?.[1] ?? t[1], slot: textures.length });
      }
      continue;
    }
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
  return { params, textures, errors, paramsSize };
}

const TEXTURE_RE = /^\s*\/\/@texture\s+([A-Za-z][A-Za-z0-9_]*)\b(.*)$/;

/** O código define o passe de simulação. */
export function usesSimulation(code: string): boolean {
  return /\bfn\s+simulate\s*\(/.test(code);
}

/** Formato do buffer de estado (motor e Studio). */
export const ESPECTRO_STATE_FORMAT = "rgba16float";

/** O código lê o quadro anterior (o motor só guarda a cópia quando precisa). */
export function usesPrevious(code: string): boolean {
  return /\bsample_previous\s*\(/.test(code);
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
  const texFns = parse.textures
    .map(
      (t) =>
        `fn tex_${t.name}(uv: vec2f) -> vec4f {\n  return textureSampleLevel(prism_tex${t.slot}, prism_repeat, uv, 0.0);\n}\n` +
        `fn tex_${t.name}_size() -> vec2f {\n  return vec2f(textureDimensions(prism_tex${t.slot}));\n}\n`
    )
    .join("");
  const head =
    `${ESPECTRO_PRELUDE}\nstruct PrismParams {\n${fields}\n};\n` +
    `@group(0) @binding(3) var<uniform> params: PrismParams;\n${texFns}// ── Código do usuário ──\n`;
  const userLineStart = head.split("\n").length;
  const sim = usesSimulation(code) ? ESPECTRO_SIM_ENTRY : "";
  return { wgsl: head + code + "\n" + ESPECTRO_ENTRY + sim, userLineStart, parse };
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
  const nativeTextures = Array.from({ length: ESPECTRO_MAX_TEXTURES }, () => "");
  for (const t of build.parse.textures) {
    nativeTextures[t.slot] = effect.textures?.find((x) => x.name === t.name)?.customSrc ?? "";
  }
  return {
    ...effect,
    nativeWgsl: build.wgsl,
    nativeParams: Array.from(packed),
    nativeTextures,
    nativeFeedback: usesPrevious(effect.code),
    nativeSimulate: usesSimulation(effect.code),
  };
}

/** Caminho da imagem escolhida para a textura `name` ("" = nenhuma). */
export function textureSrc(effect: EspectroEffect, name: string): string {
  return effect.textures?.find((t) => t.name === name)?.customSrc ?? "";
}

export function setTextureSrc(effect: EspectroEffect, name: string, src: string): EspectroEffect {
  const rest = (effect.textures ?? []).filter((t) => t.name !== name);
  return { ...effect, textures: src ? [...rest, { name, customSrc: src }] : rest };
}

// ── Arquivo .prismfx ──

/** Textura embutida no `.prismfx` (o arquivo leva as imagens; importar grava de volta no disco). */
export interface PrismFxTexture {
  name: string;
  fileName: string;
  /** `data:image/...;base64,...` */
  dataUrl: string;
}

export interface PrismFxFile {
  format: typeof PRISMFX_FORMAT;
  version: number;
  effect: Pick<EspectroEffect, "name" | "author" | "description" | "code" | "params"> & {
    textures?: PrismFxTexture[];
  };
}

/** Limite do `.prismfx` (código + imagens em base64). */
export const PRISMFX_MAX_BYTES = 24 * 1024 * 1024;

export function serializePrismFx(effect: EspectroEffect, textures: PrismFxTexture[] = []): string {
  const file: PrismFxFile = {
    format: PRISMFX_FORMAT,
    version: PRISMFX_VERSION,
    effect: {
      name: effect.name,
      author: effect.author,
      description: effect.description,
      code: effect.code,
      params: effect.params,
      textures: textures.length ? textures : undefined,
    },
  };
  return JSON.stringify(file, null, 2);
}

export interface ParsedPrismFx {
  effect: EspectroEffect;
  /** Imagens a gravar no disco; o chamador preenche `effect.textures` com os caminhos. */
  textures: PrismFxTexture[];
}

/** Lê um `.prismfx`; lança `Error` com mensagem para o usuário se o arquivo não servir. */
export function parsePrismFx(text: string): EspectroEffect {
  return parsePrismFxFull(text).effect;
}

export function parsePrismFxFull(text: string): ParsedPrismFx {
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
  const textures: PrismFxTexture[] = [];
  for (const t of Array.isArray(e.textures) ? e.textures.slice(0, ESPECTRO_MAX_TEXTURES) : []) {
    if (
      t &&
      typeof t.name === "string" &&
      /^[A-Za-z][A-Za-z0-9_]*$/.test(t.name) &&
      typeof t.dataUrl === "string" &&
      /^data:image\/(png|jpeg|webp);base64,[A-Za-z0-9+/=]+$/.test(t.dataUrl)
    ) {
      const fileName = typeof t.fileName === "string" ? t.fileName.replace(/[^\w.-]+/g, "_").slice(0, 80) : `${t.name}.png`;
      textures.push({ name: t.name, fileName, dataUrl: t.dataUrl });
    }
  }
  return {
    effect: {
      name: typeof e.name === "string" && e.name.trim() ? e.name.trim().slice(0, 80) : "Efeito importado",
      author: typeof e.author === "string" ? e.author.slice(0, 80) : undefined,
      description: typeof e.description === "string" ? e.description.slice(0, 400) : undefined,
      code: e.code,
      params,
    },
    textures,
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
  {
    id: "light-trail",
    name: "Rastro de luz",
    description: "O mouse deixa um rastro brilhante que some devagar (memória entre quadros).",
    code: `//@param decay: float = 0.94 range(0.8, 0.995) label("Duração do rastro")
//@param size: float = 0.035 range(0.005, 0.2) label("Tamanho do pincel")
//@param color: color = #7cc8ff label("Cor")

// O efeito é transparente: o wallpaper continua visível por baixo do rastro.
fn effect(uv: vec2f) -> vec4f {
  let aspect = prism.resolution.x / max(prism.resolution.y, 1.0);
  let d = length((uv - prism.cursor) * vec2f(aspect, 1.0));
  let brush = smoothstep(params.size, 0.0, d);
  let trail = sample_previous(uv).a * params.decay;
  let a = max(trail, brush);
  let hue = 0.5 + 0.5 * sin(prism.time + uv.x * 3.0);
  return vec4f(mix(params.color.rgb, vec3f(1.0, 0.6, 0.9), hue * 0.35), a);
}
`,
  },
  {
    id: "water",
    name: "Água interativa",
    description: "Superfície de água simulada: clique, segure o mouse ou deixe chover.",
    code: `//@param damping: float = 0.985 range(0.9, 0.999) label("Duração das ondas")
//@param brush: float = 0.02 range(0.005, 0.08) label("Tamanho do toque")
//@param rain: float = 0.3 range(0, 1) label("Chuva")
//@param refraction: float = 0.08 range(0, 0.3) label("Refração")

// Estado: r = altura, g = velocidade. Roda antes de effect() a cada quadro.
fn simulate(uv: vec2f) -> vec4f {
  let px = pixel();
  let s = sample_state(uv);
  let n = sample_state(uv + vec2f(px.x, 0.0)).r + sample_state(uv - vec2f(px.x, 0.0)).r +
          sample_state(uv + vec2f(0.0, px.y)).r + sample_state(uv - vec2f(0.0, px.y)).r;
  var vel = (s.g + (n * 0.25 - s.r) * 1.6) * params.damping;
  var h = s.r + vel;
  let aspect = prism.resolution.x / max(prism.resolution.y, 1.0);
  let touch = smoothstep(params.brush, 0.0, length((uv - prism.cursor) * vec2f(aspect, 1.0)));
  h += touch * prism.input.x * 0.4;
  let clicked = select(0.0, 1.0, prism.click_age < 0.05);
  h += smoothstep(params.brush * 2.0, 0.0, length((uv - prism.click) * vec2f(aspect, 1.0))) * clicked;
  let cell = floor(uv * 40.0);
  let t = floor(prism.time * 8.0);
  let drop = step(1.0 - params.rain * 0.004, hash21(cell + t * 1.37));
  h += drop * smoothstep(0.012, 0.0, length((fract(uv * 40.0) - 0.5) / 40.0)) * 0.6;
  return vec4f(clamp(h, -2.0, 2.0), clamp(vel, -2.0, 2.0), 0.0, 1.0);
}

fn effect(uv: vec2f) -> vec4f {
  let px = pixel() * 2.0;
  let hx = sample_state(uv + vec2f(px.x, 0.0)).r - sample_state(uv - vec2f(px.x, 0.0)).r;
  let hy = sample_state(uv + vec2f(0.0, px.y)).r - sample_state(uv - vec2f(0.0, px.y)).r;
  let n = vec2f(hx, hy);
  let col = sample_backdrop(uv + n * params.refraction).rgb;
  let spec = pow(clamp(dot(normalize(vec3f(-n * 6.0, 1.0)), normalize(vec3f(0.4, -0.6, 1.0))), 0.0, 1.0), 60.0);
  return vec4f(col + spec * 0.5, 1.0);
}
`,
  },
  {
    id: "life",
    name: "Jogo da vida",
    description: "Autômato celular de Conway por cima do fundo. Segure o mouse para semear.",
    code: `//@param cell: float = 8 range(2, 24) label("Tamanho da célula (px)")
//@param speed: float = 12 range(1, 60) label("Gerações por segundo")
//@param color: color = #9dffb0 label("Cor das células")

fn grid() -> vec2f {
  return floor(prism.resolution / params.cell);
}
fn cell_center(uv: vec2f) -> vec2f {
  let g = grid();
  return (floor(uv * g) + 0.5) / g;
}

// r = viva (0/1), g = tempo acumulado até a próxima geração.
fn simulate(uv: vec2f) -> vec4f {
  let g = grid();
  let c = cell_center(uv);
  let s = sample_state(c);
  if (prism.frame < 2.0) {
    return vec4f(step(0.78, hash21(floor(uv * g) + 0.37)), 0.0, 0.0, 1.0);
  }
  var alive = step(0.5, s.r);
  let aspect = prism.resolution.x / max(prism.resolution.y, 1.0);
  let near_mouse = step(length((c - prism.cursor) * vec2f(aspect, 1.0)), 0.03) * prism.input.x;
  var acc = s.g + prism.delta * params.speed;
  if (acc >= 1.0) {
    acc -= 1.0;
    var count = 0.0;
    for (var y = -1; y <= 1; y++) {
      for (var x = -1; x <= 1; x++) {
        if (x != 0 || y != 0) {
          count += step(0.5, sample_state(c + vec2f(f32(x), f32(y)) / g).r);
        }
      }
    }
    alive = select(select(0.0, 1.0, count == 3.0), select(0.0, 1.0, count == 2.0 || count == 3.0), alive > 0.5);
  }
  return vec4f(max(alive, near_mouse), min(acc, 1.0), 0.0, 1.0);
}

fn effect(uv: vec2f) -> vec4f {
  let alive = step(0.5, sample_state(cell_center(uv)).r);
  let f = fract(uv * grid());
  let inner = step(0.08, f.x) * step(0.08, f.y);
  let bg = sample_backdrop(uv).rgb * 0.55;
  return vec4f(mix(bg, params.color.rgb, alive * inner), 1.0);
}
`,
  },
  {
    id: "living-texture",
    name: "Textura viva",
    description: "Uma imagem sua escorrendo por cima do fundo, distorcida por ruído.",
    code: `//@texture padrao label("Imagem")
//@param scale: float = 2 range(0.25, 8) label("Repetição")
//@param flow: float = 0.06 range(0, 0.3) label("Distorção")
//@param amount: float = 0.6 range(0, 1) label("Mistura")

// Sem imagem escolhida a textura é branca: escolha uma em Texturas, logo abaixo.
fn effect(uv: vec2f) -> vec4f {
  let n = vec2f(noise2(uv * 4.0 + prism.time * 0.2), noise2(uv * 4.0 - prism.time * 0.17)) - 0.5;
  let t = tex_padrao(uv * params.scale + n * params.flow + vec2f(0.0, prism.time * 0.02));
  let bg = sample_backdrop(uv + n * 0.01);
  return vec4f(mix(bg.rgb, t.rgb, t.a * params.amount), 1.0);
}
`,
  },
];

export function createEspectroEffect(templateId = "rain-glass"): EspectroEffect {
  const t = ESPECTRO_TEMPLATES.find((x) => x.id === templateId) ?? ESPECTRO_TEMPLATES[0];
  return { name: t.name, description: t.description, code: t.code, params: {} };
}
