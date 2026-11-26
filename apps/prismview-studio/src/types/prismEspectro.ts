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
  /** `//@scale`: fração da resolução da camada em que o efeito roda (0.25..1). */
  nativeScale?: number;
  /** `//@particles N`: partículas simuladas em compute na GPU (0 = nenhuma). */
  nativeParticles?: number;
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
/** Teto de partículas por efeito (48 B cada → 9,6 MB de VRAM no máximo). */
export const ESPECTRO_MAX_PARTICLES = 200000;
/** Tamanho de `Particle` no buffer (bytes). */
export const ESPECTRO_PARTICLE_BYTES = 48;
export const ESPECTRO_MAX_PARAMS = 32;
export const ESPECTRO_MAX_CODE_CHARS = 64 * 1024;

/** Tamanho do uniform `prism` (PrismGlobals) em floats; o motor Rust usa o mesmo número. */
export const ESPECTRO_GLOBALS_FLOATS = 40;

/**
 * Prelúdio comum (Studio WebGPU e motor wgpu). Layout de `PrismGlobals` (bytes):
 * 0 resolution · 8 time · 12 delta · 16 cursor · 24 frame_size · 32 layer_rect · 48 backdrop_rect
 * · 64 audio (level, bass, mid, treble) · 80 beat · 84 battery · 88 has_backdrop · 92 backdrop_opaque
 * · 96 click (uv do último clique) · 104 click_age (s; 1e6 = nenhum) · 108 frame
 * · 112 input (mouse_down, key_pulse, typing_energy, idle_s) · 128 media (playing, progress, has_session, 0)
 * · 144 input_rect (área da camada de entrada no quadro, 0..1) → 160.
 * Bindings: 0 prism · 1 backdrop · 2 sampler (clamp) · 3 params · 4 quadro anterior · 5..8 texturas
 * · 9 sampler (repeat) · 10 estado da simulação · 11 entrada (camada de efeito/3D logo abaixo, ou o fundo)
 * · 12 partículas já desenhadas. Grupo 1 (só com //@particles): 0 = buffer de partículas
 * (read_write, compute) · 1 = o mesmo buffer (read, desenho).
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
  input_rect: vec4f,
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
@group(0) @binding(11) var prism_input: texture_2d<f32>;
@group(0) @binding(12) var prism_particles_tex: texture_2d<f32>;

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
// Camada de baixo: saída do efeito Espectro ou da cena 3D logo abaixo desta camada
// (sem nenhuma, é o fundo). Encadeia efeitos: um efeito trabalha sobre o resultado do outro.
fn sample_input(uv: vec2f) -> vec4f {
  let f = frame_uv(uv);
  let b = (f - prism.input_rect.xy) / max(prism.input_rect.zw, vec2f(1e-5));
  return textureSampleLevel(prism_input, prism_sampler, clamp(b, vec2f(0.0), vec2f(1.0)), 0.0);
}
// Partículas desta camada já desenhadas neste quadro (transparente sem //@particles).
fn sample_particles(uv: vec2f) -> vec4f {
  return textureSampleLevel(prism_particles_tex, prism_sampler, uv, 0.0);
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

/** Partículas + física simples (só entra no WGSL com //@particles). Unidades: uv da camada por segundo. */
const ESPECTRO_PARTICLE_PRELUDE = `
// ── Partículas (//@particles) ──
struct Particle {
  pos: vec2f,
  vel: vec2f,
  life: f32,
  age: f32,
  size: f32,
  seed: f32,
  color: vec4f,
};
@group(1) @binding(0) var<storage, read_write> prism_particles_rw: array<Particle>;
@group(1) @binding(1) var<storage, read> prism_particles_ro: array<Particle>;

fn prism_aspect() -> f32 {
  return prism.resolution.x / max(prism.resolution.y, 1.0);
}
// Aleatório por partícula e por quadro (0..1).
fn prand(i: u32, salt: f32) -> f32 {
  return hash21(vec2f(f32(i % 65536u) * 0.0137 + salt, prism.frame * 0.731 + f32(i / 65536u) * 3.1));
}
// Gravidade + arrasto + integração (dt do quadro). Também avança age e consome life.
fn physics_step(p: Particle, gravity: vec2f, drag: f32) -> Particle {
  var o = p;
  let dt = prism.delta;
  o.vel = (o.vel + gravity * dt) * max(0.0, 1.0 - drag * dt);
  o.pos += o.vel * dt;
  o.age += dt;
  o.life -= dt;
  return o;
}
// Quica nas bordas da camada (restitution 0 = gruda, 1 = quique perfeito).
fn bounce_edges(p: Particle, restitution: f32) -> Particle {
  var o = p;
  if (o.pos.x < 0.0) { o.pos.x = 0.0; o.vel.x = abs(o.vel.x) * restitution; }
  if (o.pos.x > 1.0) { o.pos.x = 1.0; o.vel.x = -abs(o.vel.x) * restitution; }
  if (o.pos.y < 0.0) { o.pos.y = 0.0; o.vel.y = abs(o.vel.y) * restitution; }
  if (o.pos.y > 1.0) { o.pos.y = 1.0; o.vel.y = -abs(o.vel.y) * restitution; }
  return o;
}
// Força do mouse: positiva empurra, negativa atrai (raio em altura da camada).
fn mouse_force(p: Particle, radius: f32, strength: f32) -> vec2f {
  let a = prism_aspect();
  let d = (p.pos - prism.cursor) * vec2f(a, 1.0);
  let r = length(d);
  if (r >= radius || r < 1e-5) {
    return vec2f(0.0);
  }
  return d / r / vec2f(a, 1.0) * strength * (1.0 - r / radius);
}
// Vento com turbulência suave (ruído no espaço e no tempo).
fn wind_force(p: Particle, dir: vec2f, turbulence: f32) -> vec2f {
  let n = vec2f(noise2(p.pos * 3.0 + prism.time * 0.3), noise2(p.pos * 3.0 - prism.time * 0.27)) - 0.5;
  return dir + n * turbulence;
}
`;

const ESPECTRO_PARTICLE_ENTRY = `@compute @workgroup_size(64) fn prism_particles_cs(@builtin(global_invocation_id) gid: vec3u) {
  let i = gid.x;
  if (i >= arrayLength(&prism_particles_rw)) {
    return;
  }
  var p = prism_particles_rw[i];
  if (prism.frame < 1.0 || p.life <= 0.0) {
    p = particle_spawn(i);
  } else {
    p = particle_update(p, i);
  }
  prism_particles_rw[i] = p;
}
struct PrismParticleOut {
  @builtin(position) pos: vec4f,
  @location(0) q: vec2f,
  @location(1) @interpolate(flat) id: u32,
};
@vertex fn prism_particle_vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> PrismParticleOut {
  var corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(-1.0, 1.0),
                                vec2f(-1.0, 1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0));
  let p = prism_particles_ro[i];
  let q = corners[v];
  let c = p.pos + q * vec2f(p.size / prism_aspect(), p.size);
  var o: PrismParticleOut;
  o.pos = select(vec4f(2.0, 2.0, 2.0, 1.0), vec4f(c.x * 2.0 - 1.0, 1.0 - c.y * 2.0, 0.0, 1.0), p.life > 0.0);
  o.q = q;
  o.id = i;
  return o;
}
@fragment fn prism_particle_fs(v: PrismParticleOut) -> @location(0) vec4f {
  let c = clamp(particle_color(prism_particles_ro[v.id], v.q), vec4f(0.0), vec4f(1.0));
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
  /** `//@scale 0.5`: o efeito roda nessa fração da resolução (economia; o compositor amplia). */
  scale: number;
  /** `//@particles N` (0 = sem partículas). */
  particles: number;
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
  let scale = 1;
  let particles = 0;
  const lines = code.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const pc = line.match(/^\s*\/\/@particles\s+([0-9]+)\s*$/);
    if (pc) {
      const v = Number(pc[1]);
      if (v >= 1 && v <= ESPECTRO_MAX_PARTICLES) particles = v;
      else errors.push({ line: i + 1, message: `Use //@particles entre 1 e ${ESPECTRO_MAX_PARTICLES}.` });
      continue;
    }
    const sc = line.match(/^\s*\/\/@scale\s+([0-9.]+)\s*$/);
    if (sc) {
      const v = Number(sc[1]);
      if (Number.isFinite(v) && v >= 0.25 && v <= 1) scale = v;
      else errors.push({ line: i + 1, message: "Use //@scale entre 0.25 e 1 (ex.: //@scale 0.5)." });
      continue;
    }
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
  if (particles > 0) {
    for (const fn of ["particle_spawn", "particle_update", "particle_color"]) {
      if (!new RegExp(`\\bfn\\s+${fn}\\s*\\(`).test(code)) {
        errors.push({ line: 1, message: `Com //@particles, defina fn ${fn}(...) (veja "Referência").` });
      }
    }
  }
  return { params, textures, errors, paramsSize, scale, particles };
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
    `${ESPECTRO_PRELUDE}${parse.particles > 0 ? ESPECTRO_PARTICLE_PRELUDE : ""}\nstruct PrismParams {\n${fields}\n};\n` +
    `@group(0) @binding(3) var<uniform> params: PrismParams;\n${texFns}// ── Código do usuário ──\n`;
  const userLineStart = head.split("\n").length;
  const sim = usesSimulation(code) ? ESPECTRO_SIM_ENTRY : "";
  const particles = parse.particles > 0 ? ESPECTRO_PARTICLE_ENTRY : "";
  return { wgsl: head + code + "\n" + ESPECTRO_ENTRY + sim + particles, userLineStart, parse };
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
    nativeScale: build.parse.scale,
    nativeParticles: build.parse.particles,
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
    id: "blank",
    name: "Em branco",
    description: "Ponto de partida: lê o fundo e mostra onde escrever o seu efeito.",
    code: `//@param intensity: float = 0.5 range(0, 1) label("Intensidade")

// Comece aqui. uv vai de (0,0) no canto de cima à esquerda até (1,1) embaixo à direita.
// Abra "Referência" no painel para ver tudo o que dá para usar.
fn effect(uv: vec2f) -> vec4f {
  let bg = sample_backdrop(uv);
  let wave = 0.75 + 0.25 * sin(prism.time * 1.5 + uv.x * 8.0);
  return vec4f(mix(bg.rgb, bg.rgb * wave, params.intensity), 1.0);
}
`,
  },
  {
    id: "rain-glass",
    name: "Chuva no vidro",
    description: "Gotas paradas na janela que refratam o fundo (lente invertida), escorrem devagar e somem, mais a chuva caindo atrás.",
    code: `//@param density: float = 0.75 range(0, 1) label("Quantidade de gotas")
//@param size: float = 1 range(0.3, 2) label("Tamanho das gotas")
//@param life: float = 15 range(2, 40) label("Tempo na janela (s)")
//@param slide: float = 0.35 range(0, 1) label("Escorrer")
//@param refraction: float = 1 range(0, 3) label("Refração")
//@param downpour: float = 0.5 range(0, 1) label("Chuva caindo")
//@param fog: float = 0 range(0, 2) label("Embaçado")

// Mesma técnica das gotas de janela do Wallpaper Engine: cada gota é uma pequena lente que
// amostra o fundo deslocado pela normal da gota (maior = desloca mais). Aqui o "emissor" é uma
// grade: cada célula tem no máximo uma gota viva, que nasce num ponto aleatório, fica
// \`life\` segundos escorrendo devagar e some; depois renasce em outro lugar.
// Retorno: xy = deslocamento em uv, z = cobertura (0..1), w = borda (escurece a beirada).
fn drop_layer(uv: vec2f, cells: f32, seed: f32, big: f32) -> vec4f {
  let aspect = prism.resolution.x / max(prism.resolution.y, 1.0);
  let p = uv * vec2f(aspect, 1.0) * cells;
  let cell = floor(p);
  let f = fract(p);
  let h0 = hash21(cell + seed);
  let period = params.life * (0.67 + 0.66 * h0);
  let t = prism.time + h0 * period;
  let cycle = floor(t / period);
  let age = fract(t / period);
  let id = cell + vec2f(seed, cycle * 17.31);
  if (hash21(id) > params.density) {
    return vec4f(0.0);
  }
  let h2 = hash21(id + 3.7);
  let h3 = hash21(id + 9.1);
  let h4 = hash21(id + 5.3);
  let r = min(0.42, (0.14 + 0.2 * h2) * params.size * big);
  // Escorre para baixo dentro da célula; gotas maiores descem mais.
  let fall = params.slide * h2 * (0.9 - 2.0 * r) * 0.8;
  let room = max(0.0, 1.0 - 2.0 * r - fall);
  let center = vec2f(r + (1.0 - 2.0 * r) * h3, r + room * h4 + fall * age);
  var d = f - center;
  // Contorno irregular e base mais achatada, como água na vertical.
  let ang = atan2(d.y, d.x);
  let wobble = 1.0 + 0.1 * sin(ang * 3.0 + h3 * 6.28) + 0.05 * sin(ang * 5.0 + h4 * 6.28);
  d.y *= select(1.0, 1.18, d.y > 0.0);
  let q = length(d) / (r * wobble);
  if (q >= 1.0) {
    return vec4f(0.0);
  }
  let born = smoothstep(0.0, 0.015, age);
  let dying = smoothstep(1.0, 0.9, age);
  let alpha = (0.35 + 0.65 * hash21(id + 1.9)) * born * dying;
  let mask = smoothstep(1.0, 0.82, q) * alpha;
  let n = d / r;
  let radius_uv = r / cells;
  let offset = -n * radius_uv * (1.2 + params.refraction * 1.8) * vec2f(1.0 / aspect, 1.0);
  let rim = smoothstep(0.55, 1.0, q) * mask;
  return vec4f(offset, mask, rim);
}

// Chuva caindo (atrás do vidro): riscos finos e rápidos, levemente inclinados.
fn streaks(uv: vec2f, cols: f32, speed: f32, seed: f32) -> f32 {
  let aspect = prism.resolution.x / max(prism.resolution.y, 1.0);
  var q = uv * vec2f(aspect, 1.0);
  q.x += q.y * 0.12;
  let x = q.x * cols;
  let col = floor(x);
  let h = hash21(vec2f(col, seed));
  if (h > params.downpour) {
    return 0.0;
  }
  let len = 0.05 + 0.08 * hash21(vec2f(col, seed + 2.0));
  let y = fract(q.y * 0.7 - prism.time * speed * (0.8 + 0.4 * h) + h * 13.0);
  let across = 1.0 - smoothstep(0.0, 0.18, abs(fract(x) - 0.5));
  return across * smoothstep(0.0, len, y) * step(y, len + 0.02) * (1.0 - y / (len + 0.02));
}

fn effect(uv: vec2f) -> vec4f {
  // Da gota maior (na frente) para a menor: a de cima cobre a de baixo.
  var drop = drop_layer(uv, 7.0, 31.0, 1.35);
  let mid = drop_layer(uv, 14.0, 7.0, 1.0);
  drop = select(mid, drop, drop.z > mid.z);
  let small = drop_layer(uv, 28.0, 3.0, 0.8);
  drop = select(small, drop, drop.z > small.z);

  let o = params.fog * 0.006;
  var bg = sample_backdrop(uv).rgb;
  if (o > 0.0) {
    bg = (sample_backdrop(uv + vec2f(o, o)).rgb + sample_backdrop(uv + vec2f(-o, o)).rgb +
          sample_backdrop(uv + vec2f(o, -o)).rgb + sample_backdrop(uv - vec2f(o, o)).rgb) * 0.25;
  }
  let rain = streaks(uv, 150.0, 1.6, 7.0) + streaks(uv, 90.0, 1.2, 3.0) * 0.7;
  bg += sample_backdrop(uv + vec2f(0.003, 0.0)).rgb * rain * 0.22;

  // Dentro da gota: fundo refratado, beirada mais escura e um brilho pequeno no alto.
  let lens = sample_backdrop(uv + drop.xy).rgb;
  let spec = smoothstep(0.012, 0.0, length(drop.xy * vec2f(1.0, -1.0) - vec2f(0.004, 0.006))) * 0.25 * drop.z;
  let inside = lens * (1.0 - drop.w * 0.45) + spec;
  return vec4f(mix(bg, inside, drop.z), 1.0);
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
  {
    id: "snow-physics",
    name: "Neve com física",
    description: "20 mil flocos na GPU: gravidade, vento com turbulência e o mouse empurrando.",
    code: `//@particles 20000
//@param fall: float = 0.08 range(0, 0.4) label("Queda")
//@param wind: float = 0.03 range(-0.2, 0.2) label("Vento")
//@param gust: float = 0.15 range(0, 0.6) label("Turbulência")
//@param push: float = 1.2 range(0, 4) label("Força do mouse")
//@param flake: float = 4 range(1, 15) label("Tamanho do floco")

// Nasce acima da camada, em x aleatório. Tudo roda na GPU (compute), nada passa pela CPU.
fn particle_spawn(i: u32) -> Particle {
  var p: Particle;
  p.pos = vec2f(prand(i, 1.0), -0.05 - prand(i, 2.0) * 1.1);
  p.vel = vec2f(0.0, params.fall * (0.5 + prand(i, 3.0)));
  p.life = 30.0;
  p.age = 0.0;
  p.size = params.flake * 0.001 * (0.4 + prand(i, 4.0));
  p.seed = prand(i, 5.0);
  p.color = vec4f(1.0, 1.0, 1.0, 0.5 + 0.5 * prand(i, 6.0));
  return p;
}

fn particle_update(p: Particle, i: u32) -> Particle {
  var o = p;
  let goal = vec2f(0.0, params.fall * (0.5 + p.seed));
  let force = wind_force(p, vec2f(params.wind, 0.0), params.gust) + mouse_force(p, 0.12, params.push);
  o.vel += (goal - o.vel) * min(1.0, prism.delta * 1.5) + force * prism.delta;
  o = physics_step(o, vec2f(0.0), 0.0);
  if (o.pos.y > 1.05 || o.pos.x < -0.1 || o.pos.x > 1.1) {
    o.life = 0.0;
  }
  return o;
}

// q vai de -1 a 1 no quadradinho do floco: um disco macio.
fn particle_color(p: Particle, q: vec2f) -> vec4f {
  let d = length(q);
  return vec4f(p.color.rgb, p.color.a * smoothstep(1.0, 0.2, d));
}

fn effect(uv: vec2f) -> vec4f {
  let bg = sample_input(uv);
  let flakes = sample_particles(uv);
  return vec4f(bg.rgb * (1.0 - flakes.a) + flakes.rgb, 1.0);
}
`,
  },
  {
    id: "neon-edges",
    name: "Contorno neon (sobre 3D)",
    description: "Coloque acima de um modelo 3D ou de outro efeito: realça as bordas do que está embaixo.",
    code: `//@scale 0.75
//@param glow: float = 1.5 range(0, 4) label("Brilho")
//@param color: color = #6ae6ff label("Cor do neon")
//@param keep: float = 1 range(0, 1) label("Manter a imagem")

// sample_input lê a camada logo abaixo (cena 3D ou outro efeito); sem nenhuma, o fundo.
fn luma(c: vec3f) -> f32 {
  return dot(c, vec3f(0.299, 0.587, 0.114));
}

fn effect(uv: vec2f) -> vec4f {
  let px = pixel();
  let c = sample_input(uv);
  let gx = luma(sample_input(uv + vec2f(px.x, 0.0)).rgb) - luma(sample_input(uv - vec2f(px.x, 0.0)).rgb);
  let gy = luma(sample_input(uv + vec2f(0.0, px.y)).rgb) - luma(sample_input(uv - vec2f(0.0, px.y)).rgb);
  let edge = clamp(length(vec2f(gx, gy)) * 4.0, 0.0, 1.0);
  let base = c.rgb * params.keep;
  return vec4f(base + params.color.rgb * edge * params.glow, max(c.a * params.keep, edge));
}
`,
  },
];

export function createEspectroEffect(templateId = "blank"): EspectroEffect {
  const t = ESPECTRO_TEMPLATES.find((x) => x.id === templateId) ?? ESPECTRO_TEMPLATES[0];
  return { name: t.name, description: t.description, code: t.code, params: {} };
}
