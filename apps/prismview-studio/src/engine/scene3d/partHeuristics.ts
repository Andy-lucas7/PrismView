import type { ReactiveBinding3D, Scene3DAutoSuggestion, Scene3DAxis } from "../../types/prism";

// Semantic part auto-detection (PT + EN + Blender/Mixamo/VRM). Pure data in → suggestions out:
// no Three.js import, runs ONCE per model load (never per frame) and is unit-tested by `npm test`.

export interface PartNodeInput {
  name: string;
  isBone: boolean;
  isMesh: boolean;
}

export interface PartMaterialInput {
  name: string;
  /** Material already glows (emissive color ≠ black or has an emissiveMap) */
  emissive: boolean;
}

export interface PartMeshInput {
  name: string;
  parentName: string;
  materialNames: string[];
}

export interface PartDetectionInput {
  nodes: PartNodeInput[];
  morphNames: Iterable<string>;
  materials: PartMaterialInput[];
  meshes: PartMeshInput[];
}

/** Default tuning of the quick "follow the mouse" actions (shared with quickActions.ts) */
export const HEAD_FOLLOW_DEFAULTS = { strength: 24, smoothing: 0.12 } as const;
export const NECK_FOLLOW_DEFAULTS = { strength: 15, smoothing: 0.1 } as const;
/** Torso lean: low amplitude + softer spring so the pose (arms, held props) never pops */
export const BODY_FOLLOW_DEFAULTS = { strength: 8, smoothing: 0.07 } as const;

export function splitNameTokens(raw: string): string[] {
  return raw
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .toLowerCase()
    .split(/[^a-z0-9À-ɏ]+/)
    .filter(Boolean);
}

export const END_EFFECTOR_RE = /(?:^|[_\-\s.])(end|nub|tip|target|ik|pole|helper|collider)(?:$|[_\-\s.])/i;
export const SCENE_WRAPPER_RE = /^(scene|rootnode|auxscene|sketchfab_model|sketchfab_scene|modelroot_.+|objpivot_.+)$/i;
export const GENERIC_MESH_RE = /^(mesh|primitive|object|node|geometry|polysurface|pcube|psphere)[_.\-\s]?\d*$/i;

const hasAny = (tokens: string[], words: readonly string[]) => tokens.some((t) => words.includes(t));

const HEAD_WORDS = ["head", "cabeca", "cabeça", "skull", "cranio", "crânio"] as const;
export function matchHead(raw: string, tokens: string[]): boolean {
  if (/headphone|headset|headlight|header|headband/i.test(raw)) return false;
  if (hasAny(tokens, HEAD_WORDS)) return true;
  // "J_Bip_C_Head" (VRM), "mixamorig:Head", "HeadBone"
  return /head(?!phone|set|light|er|band)/i.test(raw);
}

const NECK_WORDS = ["neck", "pescoco", "pescoço"] as const;
export function matchNeck(raw: string, tokens: string[]): boolean {
  return hasAny(tokens, NECK_WORDS) || /neck/i.test(raw);
}

const BODY_WORDS = ["body", "corpo", "spine", "torso", "chest", "peito", "tronco", "upperbody", "upperchest"] as const;
const NOT_BODY_WORDS = ["lower", "lowerbody", "hip", "hips", "pelvis", "quadril", "shape", "collider"] as const;
/** Torso / spine chain (Mixamo `Spine`, `Spine1`, `Spine2`; VRM `J_Bip_C_Chest`/`UpperChest`) */
export function matchBody(raw: string, tokens: string[]): boolean {
  if (hasAny(tokens, NOT_BODY_WORDS)) return false;
  if (matchHead(raw, tokens) || matchNeck(raw, tokens)) return false;
  if (hasAny(tokens, BODY_WORDS)) return true;
  // "spine1", "spine02", "Spine_2"
  return tokens.some((t) => /^(spine|chest|torso)\d+$/.test(t)) || /upper.?(body|chest)/i.test(raw);
}

/** Prefers the base of the spine chain (Spine > Spine1 > Spine2 > Chest) so the whole upper body leans rigidly */
function bodyRank(tokens: string[]): number {
  for (const t of tokens) {
    if (t === "spine" || t === "torso" || t === "tronco") return 6;
    if (/^spine0*1$/.test(t)) return 5;
    if (t === "body" || t === "corpo" || t === "upperbody") return 4;
    if (/^spine\d+$/.test(t)) return 3;
    if (t === "chest" || t === "peito" || t === "upperchest") return 2;
  }
  return 0;
}

export function matchEye(raw: string, tokens: string[]): boolean {
  if (/eyebrow|eyelid|eyelash|eyeshadow|eyewear/i.test(raw)) return false;
  if (hasAny(tokens, ["eye", "eyes", "eyeball", "olho", "olhos"])) return true;
  return /(?:^|left|right)eyes?(?:$|\d)/i.test(raw);
}

export function matchJaw(raw: string, tokens: string[]): boolean {
  if (hasAny(tokens, ["jaw", "jaws", "mandible", "mandibula", "mandíbula"])) return true;
  return /jaw/i.test(raw);
}

function matchJawMorph(raw: string, tokens: string[]): boolean {
  return matchJaw(raw, tokens) || /mouth.?open|viseme.?aa/i.test(raw);
}

function matchWing(raw: string, tokens: string[]): boolean {
  if (tokens.includes("swing") || /swing/i.test(raw)) return false;
  if (hasAny(tokens, ["wing", "wings", "asa", "asas"])) return true;
  return /wing/i.test(raw);
}

function matchFan(_raw: string, tokens: string[]): boolean {
  return hasAny(tokens, ["fan", "fans", "ventoinha", "rotor", "propeller", "helice", "hélice"]);
}

function matchWheel(raw: string, tokens: string[]): boolean {
  return hasAny(tokens, ["wheel", "wheels", "roda", "rodas", "turntable", "vinyl"]) || /wheel/i.test(raw);
}

function matchSpeaker(raw: string, tokens: string[]): boolean {
  return hasAny(tokens, ["speaker", "speakers", "woofer", "subwoofer", "tweeter"]) || /speaker|woofer/i.test(raw);
}

const SCREEN_STRONG = [
  "screen", "screens", "tela", "display", "monitor", "lcd", "oled", "tv",
  "placa", "plaquinha", "sign", "signboard", "cover", "album", "capa", "poster",
] as const;
const SCREEN_WEAK = ["board", "frame", "quadro", "canvas", "painel", "panel", "cartaz", "plate"] as const;

/** 0 = not a screen; 1 = weak hint (frame/board/canvas); 2 = explicit screen/sign/cover */
export function screenScore(raw: string): number {
  if (!raw) return 0;
  const tokens = splitNameTokens(raw);
  if (hasAny(tokens, SCREEN_STRONG) || /screen|display/i.test(raw)) return 2;
  if (hasAny(tokens, SCREEN_WEAK)) return 1;
  return 0;
}

export function matchScreen(raw: string): boolean {
  return screenScore(raw) > 0;
}

function pickBestNode(
  candidates: readonly (PartNodeInput & { tokens: string[] })[],
  matcher: (raw: string, tokens: string[]) => boolean,
  preferBone: boolean,
  rank?: (tokens: string[]) => number
): (PartNodeInput & { tokens: string[] }) | null {
  let best: (PartNodeInput & { tokens: string[] }) | null = null;
  let bestScore = -Infinity;
  for (const c of candidates) {
    if (!matcher(c.name, c.tokens)) continue;
    let score = 0;
    if (preferBone) score += c.isBone ? 24 : c.isMesh ? 6 : 12;
    else score += c.isBone ? 14 : c.isMesh ? 10 : 12;
    if (END_EFFECTOR_RE.test(c.name)) score -= 30;
    if (rank) score += rank(c.tokens) * 2;
    score -= c.name.length * 0.1;
    if (score > bestScore) {
      bestScore = score;
      best = c;
    }
  }
  return best;
}

function lookBinding(
  targetName: string,
  tuning: { strength: number; smoothing: number }
): Omit<ReactiveBinding3D, "id" | "objectId"> {
  return {
    enabled: true,
    source: "CURSOR_LOOK",
    sourceNode: "*",
    targetType: "NODE_LOOK_AT",
    targetName,
    strength: tuning.strength,
    smoothing: tuning.smoothing,
    invertX: false,
    invertY: false,
  };
}

/** Best material to receive the live album cover; null when nothing looks like a screen/sign */
export function detectCoverMaterial(
  materials: readonly PartMaterialInput[],
  meshes: readonly PartMeshInput[]
): { materialName: string; label: string } | null {
  // 1) Material named like a screen/sign (strong beats weak; first wins on ties)
  let best: { materialName: string; label: string } | null = null;
  let bestScore = 0;
  for (const m of materials) {
    const s = screenScore(m.name);
    if (s > bestScore) {
      bestScore = s;
      best = { materialName: m.name, label: m.name };
    }
  }
  if (bestScore === 2) return best;

  // 2) Mesh (or its parent group) named like a screen/sign → its first named material
  let meshBest: { materialName: string; label: string } | null = null;
  let meshScore = 0;
  for (const mesh of meshes) {
    const parent = mesh.parentName && !SCENE_WRAPPER_RE.test(mesh.parentName) ? mesh.parentName : "";
    const s = Math.max(screenScore(mesh.name), screenScore(parent));
    if (s <= meshScore) continue;
    const firstNamed = mesh.materialNames.find(Boolean);
    meshScore = s;
    meshBest = { materialName: firstNamed || "*", label: firstNamed || mesh.name || parent || "Tela" };
  }
  if (meshScore > bestScore) return meshBest;
  if (best) return best;

  // 3) Material that already glows (a lit panel in the source model)
  const glowing = materials.find((m) => m.emissive && m.name);
  return glowing ? { materialName: glowing.name, label: glowing.name } : null;
}

export function detectPartSuggestions(input: PartDetectionInput): Scene3DAutoSuggestion[] {
  const candidates: Array<PartNodeInput & { tokens: string[] }> = [];
  for (const n of input.nodes) {
    if (!n.name || n.name === "__ROOT__" || SCENE_WRAPPER_RE.test(n.name)) continue;
    candidates.push({ ...n, tokens: splitNameTokens(n.name) });
  }

  const out: Scene3DAutoSuggestion[] = [];
  const used = new Set<string>();

  const head = pickBestNode(candidates, matchHead, true);
  if (head) {
    used.add(head.name);
    out.push({
      id: `sug-head-${head.name}`,
      role: "HEAD",
      kind: "BINDING",
      label: "Cabeça → Mouse",
      detail: `Olhar p/ Cursor em "${head.name}" (${HEAD_FOLLOW_DEFAULTS.strength}° · suav. ${HEAD_FOLLOW_DEFAULTS.smoothing})`,
      targetName: head.name,
      binding: lookBinding(head.name, HEAD_FOLLOW_DEFAULTS),
    });
  }

  const neck = pickBestNode(candidates, matchNeck, true);
  if (neck && !used.has(neck.name)) {
    used.add(neck.name);
    out.push({
      id: `sug-neck-${neck.name}`,
      role: "NECK",
      kind: "BINDING",
      label: "Pescoço → Mouse",
      detail: `Olhar p/ Cursor em "${neck.name}" (${NECK_FOLLOW_DEFAULTS.strength}° · suav. ${NECK_FOLLOW_DEFAULTS.smoothing})`,
      targetName: neck.name,
      binding: lookBinding(neck.name, NECK_FOLLOW_DEFAULTS),
    });
  }

  const body = pickBestNode(candidates, matchBody, true, bodyRank);
  if (body && !used.has(body.name)) {
    used.add(body.name);
    out.push({
      id: `sug-body-${body.name}`,
      role: "BODY",
      kind: "BINDING",
      label: "Corpo → Mouse (leve)",
      detail: `Inclinar "${body.name}" de leve p/ o Cursor (${BODY_FOLLOW_DEFAULTS.strength}° · suav. ${BODY_FOLLOW_DEFAULTS.smoothing})`,
      targetName: body.name,
      binding: lookBinding(body.name, BODY_FOLLOW_DEFAULTS),
    });
  }

  const eye = pickBestNode(candidates, matchEye, true);
  if (eye && !used.has(eye.name)) {
    used.add(eye.name);
    out.push({
      id: `sug-eye-${eye.name}`,
      role: "EYE",
      kind: "BINDING",
      label: "Olho → Mouse",
      detail: `Olhar p/ Cursor em "${eye.name}" (18° · suav. 0.18)`,
      targetName: eye.name,
      binding: lookBinding(eye.name, { strength: 18, smoothing: 0.18 }),
    });
  }

  // Jaw: prefer Morph Target on AUDIO_MID, else bone/node NODE_ROTATE_SWING on AUDIO_MID
  let bestJawMorph: string | null = null;
  let bestJawMorphScore = -Infinity;
  for (const mName of input.morphNames) {
    const toks = splitNameTokens(mName);
    if (!matchJawMorph(mName, toks)) continue;
    const score = (matchJaw(mName, toks) ? 20 : 10) - mName.length * 0.1;
    if (score > bestJawMorphScore) {
      bestJawMorphScore = score;
      bestJawMorph = mName;
    }
  }
  if (bestJawMorph) {
    out.push({
      id: `sug-jaw-morph-${bestJawMorph}`,
      role: "JAW",
      kind: "BINDING",
      label: "Mandíbula → Médios",
      detail: `Morph "${bestJawMorph}" no Áudio Médio (peso 0.85 · suav. 0.22)`,
      targetName: bestJawMorph,
      binding: {
        enabled: true,
        source: "AUDIO_MID",
        sourceNode: "*",
        targetType: "MORPH_WEIGHT",
        targetName: bestJawMorph,
        strength: 0.85,
        smoothing: 0.22,
      },
    });
  } else {
    const jaw = pickBestNode(candidates, matchJaw, true);
    if (jaw && !used.has(jaw.name)) {
      used.add(jaw.name);
      out.push({
        id: `sug-jaw-${jaw.name}`,
        role: "JAW",
        kind: "BINDING",
        label: "Mandíbula → Médios",
        detail: `Balançar "${jaw.name}" no eixo X com Áudio Médio (22° · suav. 0.22)`,
        targetName: jaw.name,
        binding: {
          enabled: true,
          source: "AUDIO_MID",
          sourceNode: "*",
          targetType: "NODE_ROTATE_SWING",
          targetName: jaw.name,
          strength: 22,
          smoothing: 0.22,
          axis: "X",
          invertAxis: false,
        },
      });
    }
  }

  const wing = pickBestNode(candidates, matchWing, true);
  if (wing && !used.has(wing.name)) {
    used.add(wing.name);
    out.push({
      id: `sug-wing-${wing.name}`,
      role: "WING",
      kind: "BINDING",
      label: "Asa → Grave",
      detail: `Balançar "${wing.name}" no eixo Z com Áudio Grave (24° · suav. 0.20)`,
      targetName: wing.name,
      binding: {
        enabled: true,
        source: "AUDIO_BASS",
        sourceNode: "*",
        targetType: "NODE_ROTATE_SWING",
        targetName: wing.name,
        strength: 24,
        smoothing: 0.2,
        axis: "Z",
        invertAxis: false,
      },
    });
  }

  const fan = pickBestNode(candidates, matchFan, false);
  if (fan && !used.has(fan.name)) {
    used.add(fan.name);
    out.push({
      id: `sug-fan-${fan.name}`,
      role: "FAN",
      kind: "BINDING",
      label: "Ventoinha → CPU",
      detail: `Girar "${fan.name}" no eixo Z conforme Uso de CPU (4.5 rad/s · suav. 0.14)`,
      targetName: fan.name,
      binding: {
        enabled: true,
        source: "CPU_USAGE",
        sourceNode: "*",
        targetType: "NODE_ROTATE_SPIN",
        targetName: fan.name,
        strength: 4.5,
        smoothing: 0.14,
        axis: "Z",
        invertAxis: false,
      },
    });
  }

  const wheel = pickBestNode(candidates, matchWheel, false);
  if (wheel && !used.has(wheel.name)) {
    used.add(wheel.name);
    const isTurntable = /turntable|vinyl/i.test(wheel.name);
    const axis: Scene3DAxis = isTurntable ? "Y" : "X";
    out.push({
      id: `sug-wheel-${wheel.name}`,
      role: "WHEEL",
      kind: "BINDING",
      label: "Roda → Mídia",
      detail: `Girar "${wheel.name}" no eixo ${axis} enquanto Música Toca (3.2 rad/s · suav. 0.12)`,
      targetName: wheel.name,
      binding: {
        enabled: true,
        source: "MEDIA_PLAYING",
        sourceNode: "*",
        targetType: "NODE_ROTATE_SPIN",
        targetName: wheel.name,
        strength: 3.2,
        smoothing: 0.12,
        axis,
        invertAxis: false,
      },
    });
  }

  const speaker = pickBestNode(candidates, matchSpeaker, false);
  if (speaker && !used.has(speaker.name)) {
    used.add(speaker.name);
    const isCone = /cone|woofer|subwoofer|driver|diaphragm/i.test(speaker.name);
    out.push(
      isCone
        ? {
            id: `sug-speaker-${speaker.name}`,
            role: "SPEAKER",
            kind: "BINDING",
            label: "Speaker → Pistão Grave",
            detail: `Deslocar "${speaker.name}" no eixo Z com Áudio Grave (0.08 · suav. 0.26)`,
            targetName: speaker.name,
            binding: {
              enabled: true,
              source: "AUDIO_BASS",
              sourceNode: "*",
              targetType: "NODE_MOVE_OFFSET",
              targetName: speaker.name,
              strength: 0.08,
              smoothing: 0.26,
              axis: "Z",
              invertAxis: false,
            },
          }
        : {
            id: `sug-speaker-${speaker.name}`,
            role: "SPEAKER",
            kind: "BINDING",
            label: "Speaker → Pulso Grave",
            detail: `Pulso de Escala em "${speaker.name}" com Áudio Grave (0.16× · suav. 0.24)`,
            targetName: speaker.name,
            binding: {
              enabled: true,
              source: "AUDIO_BASS",
              sourceNode: "*",
              targetType: "NODE_SCALE_PULSE",
              targetName: speaker.name,
              strength: 0.16,
              smoothing: 0.24,
            },
          }
    );
  }

  const cover = detectCoverMaterial(input.materials, input.meshes);
  if (cover) {
    out.push({
      id: `sug-screen-${cover.materialName}`,
      role: "SCREEN",
      kind: "DYNAMIC_TEXTURE",
      label: "Tela → Capa Música",
      detail: `Aplicar capa da música ao vivo no material "${cover.label}"`,
      targetName: cover.materialName,
      dynamicTexture: {
        materialName: cover.materialName,
        source: "MEDIA_COVER",
        emissiveMap: true,
      },
    });
  }

  return out;
}
