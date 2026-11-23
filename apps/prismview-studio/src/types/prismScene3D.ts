// Cena 3D: objetos, bindings reativos (8 ações), validação e normalização do schema v3.
import { evaluateFxTemplate } from "../engine/fx/fxEngine";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { LEGACY_DEFAULT_AUTO_ROTATE, PrismProject, ReactiveBinding3D, SCENE3D_SCHEMA_VERSION, Scene3DAxis, Scene3DBackgroundMode, Scene3DBindingTarget, Scene3DConfig, Scene3DDynamicTexture, Scene3DFpsMode, Scene3DInspectInfo, Scene3DObject, Scene3DObjectInspectInfo, Scene3DObjectKind, Scene3DPickedPart, Scene3DShowcaseVariant, SceneLayer, clampScene3DFps } from "./prismBase";

export function createBinding3D(
  preset:
    | "LOOK_AT"
    | "HOVER_GLOW"
    | "CLICK_CLIP"
    | "HOVER_PULSE"
    | "AUDIO_BASS"
    | "KEY_PULSE"
    | "AUDIO_SWING"
    | "AUDIO_PISTON",
  info?: Scene3DObjectInspectInfo | Scene3DInspectInfo | null,
  objectId?: string
): ReactiveBinding3D {
  const id = `b3d-${Math.random().toString(36).slice(2, 9)}`;
  const objPatch = objectId ? { objectId } : {};
  if (preset === "LOOK_AT") {
    const headBone =
      info?.boneNames.find((b) => /head|neck|eye|cabeca|pescoco/i.test(b)) ??
      info?.boneNames[0] ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "CURSOR_LOOK",
      sourceNode: "*",
      targetType: "NODE_LOOK_AT",
      targetName: headBone,
      strength: 24,
      smoothing: 0.12,
      invertX: false,
      invertY: false,
    };
  }
  if (preset === "HOVER_GLOW") {
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "HOVER_MESH",
      sourceNode: "*",
      targetType: "EMISSIVE_GLOW",
      targetName: info?.materialNames[0] ?? "*",
      strength: 1.4,
      smoothing: 0.14,
    };
  }
  if (preset === "AUDIO_BASS") {
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType: "NODE_SCALE_PULSE",
      targetName: "__ROOT__",
      strength: 0.18,
      smoothing: 0.22,
    };
  }
  if (preset === "AUDIO_SWING") {
    // Hinge-like parts first (wing, jaw, arm, lid, needle…), whole model as fallback
    const names = [...(info?.boneNames ?? []), ...(info?.meshNames ?? [])];
    const hinge =
      names.find((n) => /wing|asa|jaw|mandib|arm|braco|lid|tampa|needle|agulha|tail|cauda/i.test(n)) ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType: "NODE_ROTATE_SWING",
      targetName: hinge,
      strength: 22,
      smoothing: 0.2,
      axis: "Z",
    };
  }
  if (preset === "AUDIO_PISTON") {
    const names = [...(info?.meshNames ?? []), ...(info?.boneNames ?? [])];
    const cone =
      names.find((n) => /speaker|woofer|cone|caixa|alto.?falante|key|tecla|piston/i.test(n)) ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType: "NODE_MOVE_OFFSET",
      targetName: cone,
      strength: 0.12,
      smoothing: 0.26,
      axis: "Z",
    };
  }
  if (preset === "KEY_PULSE") {
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "KEY_PULSE",
      sourceNode: "*",
      targetType: "EMISSIVE_GLOW",
      targetName: info?.materialNames[0] ?? "*",
      strength: 1.6,
      smoothing: 0.22,
    };
  }
  if (preset === "CLICK_CLIP") {
    const hasClips = Boolean(info && info.clipNames.length > 0);
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "CLICK_MESH",
      sourceNode: "*",
      targetType: hasClips ? "PLAY_CLIP" : "NODE_SCALE_PULSE",
      targetName: hasClips ? info!.clipNames[Math.min(1, info!.clipNames.length - 1)] : "__ROOT__",
      strength: hasClips ? 1 : 0.16,
      smoothing: 0.15,
    };
  }
  return {
    id,
    enabled: true,
    ...objPatch,
    source: "HOVER_MESH",
    sourceNode: "*",
    targetType: "NODE_SCALE_PULSE",
    targetName: "__ROOT__",
    strength: 0.1,
    smoothing: 0.14,
  };
}

/** Creates a sensible ReactiveBinding3D targeting a specific picked bone/mesh/group (Alt+Click or Outliner). */
export function createBindingForPart(part: Scene3DPickedPart): ReactiveBinding3D {
  const id = `b3d-${Math.random().toString(36).slice(2, 9)}`;
  const targetName = part.name && part.name !== "(sem nome)" ? part.name : "__ROOT__";
  const lower = targetName.toLowerCase();

  if (/head(?!phone|set|light|er)|cabeca|cabeça/i.test(lower)) {
    return {
      id,
      enabled: true,
      objectId: part.objectId,
      source: "CURSOR_LOOK",
      sourceNode: "*",
      targetType: "NODE_LOOK_AT",
      targetName,
      strength: 24,
      smoothing: 0.12,
      invertX: false,
      invertY: false,
    };
  }
  if (/neck|pescoco|pescoço/i.test(lower)) {
    return {
      id,
      enabled: true,
      objectId: part.objectId,
      source: "CURSOR_LOOK",
      sourceNode: "*",
      targetType: "NODE_LOOK_AT",
      targetName,
      strength: 15,
      smoothing: 0.1,
      invertX: false,
      invertY: false,
    };
  }
  if (/(?:^|[^a-z])eyes?(?:$|[^a-z])|eyeball|olho/i.test(lower) && !/eyebrow|eyelid|eyelash/i.test(lower)) {
    return {
      id,
      enabled: true,
      objectId: part.objectId,
      source: "CURSOR_LOOK",
      sourceNode: "*",
      targetType: "NODE_LOOK_AT",
      targetName,
      strength: 18,
      smoothing: 0.18,
      invertX: false,
      invertY: false,
    };
  }
  if (/jaw|mandib/i.test(lower)) {
    return {
      id,
      enabled: true,
      objectId: part.objectId,
      source: "AUDIO_MID",
      sourceNode: "*",
      targetType: "NODE_ROTATE_SWING",
      targetName,
      strength: 22,
      smoothing: 0.22,
      axis: "X",
      invertAxis: false,
    };
  }
  if ((/(?:^|[^a-z])wings?(?:$|[^a-z])|wing|asa/i.test(lower)) && !/swing/i.test(lower)) {
    return {
      id,
      enabled: true,
      objectId: part.objectId,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType: "NODE_ROTATE_SWING",
      targetName,
      strength: 24,
      smoothing: 0.2,
      axis: "Z",
      invertAxis: false,
    };
  }
  if (/(?:^|[^a-z])fans?(?:$|[^a-z])|ventoinha|rotor|propeller|helice|hélice/i.test(lower)) {
    return {
      id,
      enabled: true,
      objectId: part.objectId,
      source: "CPU_USAGE",
      sourceNode: "*",
      targetType: "NODE_ROTATE_SPIN",
      targetName,
      strength: 4.5,
      smoothing: 0.14,
      axis: "Z",
      invertAxis: false,
    };
  }
  if (/wheel|roda|turntable|vinyl/i.test(lower)) {
    return {
      id,
      enabled: true,
      objectId: part.objectId,
      source: "MEDIA_PLAYING",
      sourceNode: "*",
      targetType: "NODE_ROTATE_SPIN",
      targetName,
      strength: 3.2,
      smoothing: 0.12,
      axis: /turntable|vinyl/i.test(lower) ? "Y" : "X",
      invertAxis: false,
    };
  }
  if (/speaker|woofer|subwoofer|cone/i.test(lower)) {
    const isCone = /cone|woofer|subwoofer|driver|diaphragm/i.test(lower);
    return isCone
      ? {
          id,
          enabled: true,
          objectId: part.objectId,
          source: "AUDIO_BASS",
          sourceNode: "*",
          targetType: "NODE_MOVE_OFFSET",
          targetName,
          strength: 0.08,
          smoothing: 0.26,
          axis: "Z",
          invertAxis: false,
        }
      : {
          id,
          enabled: true,
          objectId: part.objectId,
          source: "AUDIO_BASS",
          sourceNode: "*",
          targetType: "NODE_SCALE_PULSE",
          targetName,
          strength: 0.16,
          smoothing: 0.24,
        };
  }

  if (part.type === "bone") {
    return {
      id,
      enabled: true,
      objectId: part.objectId,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType: "NODE_ROTATE_SWING",
      targetName,
      strength: 20,
      smoothing: 0.18,
      axis: "Z",
      invertAxis: false,
    };
  }

  return {
    id,
    enabled: true,
    objectId: part.objectId,
    source: "AUDIO_BASS",
    sourceNode: "*",
    targetType: "NODE_SCALE_PULSE",
    targetName,
    strength: 0.16,
    smoothing: 0.2,
  };
}

function defaultShowcaseLabel(variant?: Scene3DShowcaseVariant): string {
  if (variant === "GYROSCOPE") return "Astrolábio Solar";
  if (variant === "KINETIC_SLAB") return "Display Slab 3D";
  return "Obsidian Monolith";
}

function fileBaseName(pathStr: string): string {
  const leaf = pathStr.split(/[\\/]/).pop() || "";
  return leaf.replace(/\.[^.]+$/, "") || "Modelo .glb";
}

export function createScene3DObject(
  kind: Scene3DObjectKind,
  opts?: {
    id?: string;
    name?: string;
    modelSrc?: string;
    showcaseVariant?: Scene3DShowcaseVariant;
    position?: [number, number, number];
    rotation?: [number, number, number];
    scale?: [number, number, number];
    autoRotateSpeed?: number;
    dynamicTextures?: Scene3DDynamicTexture[];
  }
): Scene3DObject {
  const modelSrc = (opts?.modelSrc ?? "").trim();
  const variant = opts?.showcaseVariant ?? "MONOLITH";
  return {
    id: opts?.id ?? `obj3d-${Math.random().toString(36).slice(2, 9)}`,
    name:
      opts?.name ||
      (kind === "GLB" && modelSrc ? fileBaseName(modelSrc) : defaultShowcaseLabel(variant)),
    kind,
    modelSrc,
    showcaseVariant: kind === "SHOWCASE" ? variant : undefined,
    position: opts?.position ?? [0, 0, 0],
    rotation: opts?.rotation ?? [0, 0, 0],
    scale: opts?.scale ?? [1, 1, 1],
    visible: true,
    autoRotateSpeed: opts?.autoRotateSpeed ?? 0,
    playback: {
      clipName: "",
      speed: 1,
      loop: true,
    },
    dynamicTextures: opts?.dynamicTextures ?? [],
  };
}

/** v2→v3: the old 0.22 rad/s default spin becomes 0; any other user value is kept */
function migrateLegacySpin(speed: unknown): number {
  const n = finiteOr(speed, 0);
  return Math.abs(n - LEGACY_DEFAULT_AUTO_ROTATE) < 1e-6 ? 0 : n;
}

function finiteTriple(v: unknown, fallback: [number, number, number]): [number, number, number] {
  if (!Array.isArray(v) || v.length !== 3) return fallback;
  return [finiteOr(v[0], fallback[0]), finiteOr(v[1], fallback[1]), finiteOr(v[2], fallback[2])];
}

function sameTriple(a: unknown, b: [number, number, number]): boolean {
  return Array.isArray(a) && a.length === 3 && a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
}

/** Fills missing/invalid fields of one room object; returns the same reference when already valid */
function normalizeScene3DObject(raw: Scene3DObject, index: number, migrateSpin: boolean): Scene3DObject {
  const kind: Scene3DObjectKind = raw.kind === "SHOWCASE" ? "SHOWCASE" : "GLB";
  const modelSrc = typeof raw.modelSrc === "string" ? raw.modelSrc : "";
  const variant = raw.showcaseVariant ?? "MONOLITH";
  const position = finiteTriple(raw.position, [0, 0, 0]);
  const rotation = finiteTriple(raw.rotation, [0, 0, 0]);
  const scale = finiteTriple(raw.scale, [1, 1, 1]);
  const spin = migrateSpin ? migrateLegacySpin(raw.autoRotateSpeed) : finiteOr(raw.autoRotateSpeed, 0);
  const pb = raw.playback;
  const playbackOk =
    !!pb && typeof pb.clipName === "string" && Number.isFinite(pb.speed) && typeof pb.loop === "boolean";
  const id = typeof raw.id === "string" && raw.id ? raw.id : `obj-${index}`;
  const name =
    typeof raw.name === "string" && raw.name
      ? raw.name
      : kind === "GLB" && modelSrc
      ? fileBaseName(modelSrc)
      : defaultShowcaseLabel(variant);
  const parentOk = raw.parentId === undefined || (typeof raw.parentId === "string" && raw.parentId !== id);
  if (
    raw.kind === kind &&
    raw.modelSrc === modelSrc &&
    raw.id === id &&
    raw.name === name &&
    sameTriple(raw.position, position) &&
    sameTriple(raw.rotation, rotation) &&
    sameTriple(raw.scale, scale) &&
    typeof raw.visible === "boolean" &&
    raw.autoRotateSpeed === spin &&
    playbackOk &&
    Array.isArray(raw.dynamicTextures) &&
    parentOk &&
    (kind === "SHOWCASE" ? raw.showcaseVariant === variant : raw.showcaseVariant === undefined)
  ) {
    return raw;
  }
  const out: Scene3DObject = {
    ...raw,
    id,
    name,
    kind,
    modelSrc,
    showcaseVariant: kind === "SHOWCASE" ? variant : undefined,
    position,
    rotation,
    scale,
    visible: typeof raw.visible === "boolean" ? raw.visible : true,
    autoRotateSpeed: spin,
    playback: playbackOk
      ? pb
      : {
          clipName: typeof pb?.clipName === "string" ? pb.clipName : "",
          speed: finiteOr(pb?.speed, 1),
          loop: typeof pb?.loop === "boolean" ? pb.loop : true,
        },
    dynamicTextures: Array.isArray(raw.dynamicTextures) ? raw.dynamicTextures : [],
  };
  if (!out.showcaseVariant) delete out.showcaseVariant;
  if (!parentOk) delete out.parentId;
  return out;
}

/** Legacy (schema ≤2) single-model fields → objects[0] */
function legacyPrimaryObject(cfg: Scene3DConfig): Scene3DObject {
  const src = (cfg.modelSrc ?? "").trim();
  const hasModel = Boolean(src);
  const variant = cfg.showcaseVariant ?? "MONOLITH";
  const rawScale = finiteOr(cfg.modelScale, 1);
  const sc = rawScale > 0 ? rawScale : 1;
  const obj = createScene3DObject(hasModel ? "GLB" : "SHOWCASE", {
    id: "obj-primary",
    modelSrc: src,
    showcaseVariant: variant,
    position: [0, finiteOr(cfg.modelOffsetY, 0), 0],
    scale: [sc, sc, sc],
    autoRotateSpeed: migrateLegacySpin(cfg.autoRotateSpeed),
    dynamicTextures: Array.isArray(cfg.dynamicTextures) ? cfg.dynamicTextures : [],
  });
  if (cfg.playback) {
    obj.playback = {
      clipName: typeof cfg.playback.clipName === "string" ? cfg.playback.clipName : "",
      speed: finiteOr(cfg.playback.speed, 1),
      loop: typeof cfg.playback.loop === "boolean" ? cfg.playback.loop : true,
    };
  }
  return obj;
}

/** Room objects of a config (legacy single-model configs yield one synthetic primary object) */
export function normalizeScene3DObjects(cfg: Scene3DConfig): Scene3DObject[] {
  if (Array.isArray(cfg.objects) && cfg.objects.length > 0) {
    return cfg.objects;
  }
  return [legacyPrimaryObject(cfg)];
}

const LEGACY_SCENE3D_KEYS = [
  "modelSrc",
  "showcaseVariant",
  "autoRotateSpeed",
  "modelScale",
  "modelOffsetY",
  "playback",
  "dynamicTextures",
] as const;

function hasLegacyScene3DKeys(cfg: Scene3DConfig): boolean {
  for (const k of LEGACY_SCENE3D_KEYS) {
    if (cfg[k] !== undefined) return true;
  }
  return false;
}

function stripLegacyScene3DKeys(cfg: Scene3DConfig): Scene3DConfig {
  const out = { ...cfg };
  for (const k of LEGACY_SCENE3D_KEYS) delete out[k];
  return out;
}

/**
 * True when `parentId` of `index` points to a missing object or its ancestor chain comes back
 * to it. A chain that loops further up (not through `index`) is left to that loop's members.
 */
function hasBrokenParent(objects: Scene3DObject[], index: number): boolean {
  let parentId = objects[index].parentId;
  for (let steps = 0; parentId !== undefined; steps++) {
    if (steps >= objects.length) return false;
    const parent = objects.find((p) => p.id === parentId);
    if (!parent || parent === objects[index]) return true;
    parentId = parent.parentId;
  }
  return false;
}

/** Drops dangling/cyclic `parentId`s (objects become roots); same array when already valid */
function sanitizeScene3DParents(objects: Scene3DObject[]): Scene3DObject[] {
  let out = objects;
  for (let i = 0; i < out.length; i++) {
    if (!hasBrokenParent(out, i)) continue;
    if (out === objects) out = objects.slice();
    const root = { ...out[i] };
    delete root.parentId;
    out[i] = root;
  }
  return out;
}

function isBackgroundMode(v: unknown): v is Scene3DBackgroundMode {
  return v === "TRANSPARENT" || v === "SOLID" || v === "ENVIRONMENT";
}

export function defaultScene3D(
  modelSrc = "",
  showcaseVariant: Scene3DShowcaseVariant = "MONOLITH"
): Scene3DConfig {
  const hasExternalModel = Boolean(modelSrc.trim());
  const primary = createScene3DObject(hasExternalModel ? "GLB" : "SHOWCASE", {
    id: "obj-primary",
    modelSrc,
    showcaseVariant,
    autoRotateSpeed: 0,
  });
  return {
    objects: [primary],
    backgroundMode: "TRANSPARENT",
    backgroundColor: "#07080b",
    envPreset: "STUDIO_DARK",
    envIntensity: 1.25,
    exposure: 1.08,
    renderScale: 0.85,
    maxFps: 60,
    camera: {
      fov: 42,
      distance: 4.8,
      azimuthDeg: hasExternalModel ? 0 : 22,
      elevationDeg: hasExternalModel ? 8 : 14,
      targetY: 0,
      target: [0, 0, 0],
      orbitParallaxX: 12,
      orbitParallaxY: 7,
      damping: 0.08,
    },
    bindings: [
      {
        id: "b3d-default-look",
        enabled: true,
        objectId: "obj-primary",
        source: "CURSOR_LOOK",
        sourceNode: "*",
        targetType: "NODE_LOOK_AT",
        targetName: "__ROOT__",
        strength: 18,
        smoothing: 0.1,
      },
      {
        id: "b3d-default-click",
        enabled: true,
        objectId: "obj-primary",
        source: "CLICK_MESH",
        sourceNode: "*",
        targetType: "NODE_SCALE_PULSE",
        targetName: "__ROOT__",
        strength: 0.14,
        smoothing: 0.16,
      },
    ],
    schemaVersion: SCENE3D_SCHEMA_VERSION,
    fpsMode: "CAP",
  };
}

function finiteOr(n: unknown, fallback: number): number {
  return typeof n === "number" && Number.isFinite(n) ? n : fallback;
}

export type Scene3DBindingTargetKind = "NODE" | "MORPH" | "MATERIAL" | "CLIP";

export interface Scene3DActionMeta {
  targetType: Scene3DBindingTarget;
  label: string;
  shortLabel: string;
  example: string;
  targetKind: Scene3DBindingTargetKind;
  usesAxis: boolean;
  usesStrength: boolean;
  usesSmoothing: boolean;
  usesInvertXY: boolean;
  strengthLabel: string;
  strengthMin: number;
  strengthMax: number;
  strengthStep: number;
  strengthUnit: string;
  defaultStrength: number;
  defaultSmoothing: number;
  defaultAxis?: Scene3DAxis;
}

export const SCENE3D_ACTION_META: Record<Scene3DBindingTarget, Scene3DActionMeta> = {
  NODE_LOOK_AT: {
    targetType: "NODE_LOOK_AT",
    label: "Olhar p/ Cursor",
    shortLabel: "Olhar Mouse",
    example: "ex.: cabeça ou olho segue o mouse",
    targetKind: "NODE",
    usesAxis: false,
    usesStrength: true,
    usesSmoothing: true,
    usesInvertXY: true,
    strengthLabel: "Ângulo Máx.",
    strengthMin: 2,
    strengthMax: 65,
    strengthStep: 1,
    strengthUnit: "°",
    defaultStrength: 22,
    defaultSmoothing: 0.12,
  },
  NODE_ROTATE_SWING: {
    targetType: "NODE_ROTATE_SWING",
    label: "Balançar (Dobradiça)",
    shortLabel: "Balançar",
    example: "ex.: asa, mandíbula ou agulha no áudio",
    targetKind: "NODE",
    usesAxis: true,
    usesStrength: true,
    usesSmoothing: true,
    usesInvertXY: false,
    strengthLabel: "Ângulo do Balanço",
    strengthMin: 2,
    strengthMax: 120,
    strengthStep: 1,
    strengthUnit: "°",
    defaultStrength: 25,
    defaultSmoothing: 0.2,
    defaultAxis: "Z",
  },
  NODE_MOVE_OFFSET: {
    targetType: "NODE_MOVE_OFFSET",
    label: "Deslocar (Pistão)",
    shortLabel: "Deslocar",
    example: "ex.: cone de caixa de som ou tecla 3D",
    targetKind: "NODE",
    usesAxis: true,
    usesStrength: true,
    usesSmoothing: true,
    usesInvertXY: false,
    strengthLabel: "Deslocamento",
    strengthMin: 0.01,
    strengthMax: 1.5,
    strengthStep: 0.01,
    strengthUnit: "",
    defaultStrength: 0.15,
    defaultSmoothing: 0.24,
    defaultAxis: "Z",
  },
  NODE_ROTATE_SPIN: {
    targetType: "NODE_ROTATE_SPIN",
    label: "Girar Contínuo",
    shortLabel: "Girar",
    example: "ex.: ventoinha na CPU ou roda na música",
    targetKind: "NODE",
    usesAxis: true,
    usesStrength: true,
    usesSmoothing: true,
    usesInvertXY: false,
    strengthLabel: "Velocidade Giro",
    strengthMin: 0.2,
    strengthMax: 8,
    strengthStep: 0.2,
    strengthUnit: "rad/s",
    defaultStrength: 2.4,
    defaultSmoothing: 0.14,
    defaultAxis: "Z",
  },
  NODE_SCALE_PULSE: {
    targetType: "NODE_SCALE_PULSE",
    label: "Pulso de Escala",
    shortLabel: "Pulso Escala",
    example: "ex.: peça ou modelo pulsa no grave",
    targetKind: "NODE",
    usesAxis: false,
    usesStrength: true,
    usesSmoothing: true,
    usesInvertXY: false,
    strengthLabel: "Pulso de Escala",
    strengthMin: 0.02,
    strengthMax: 0.45,
    strengthStep: 0.01,
    strengthUnit: "×",
    defaultStrength: 0.16,
    defaultSmoothing: 0.2,
  },
  MORPH_WEIGHT: {
    targetType: "MORPH_WEIGHT",
    label: "Morph / Shape Key",
    shortLabel: "Morph",
    example: "ex.: boca abre nos médios ou expressão",
    targetKind: "MORPH",
    usesAxis: false,
    usesStrength: true,
    usesSmoothing: true,
    usesInvertXY: false,
    strengthLabel: "Peso Morph",
    strengthMin: 0.1,
    strengthMax: 1,
    strengthStep: 0.05,
    strengthUnit: "",
    defaultStrength: 1,
    defaultSmoothing: 0.2,
  },
  EMISSIVE_GLOW: {
    targetType: "EMISSIVE_GLOW",
    label: "Brilho Emissivo",
    shortLabel: "Brilho LED",
    example: "ex.: LED ou neon acende no teclado/áudio",
    targetKind: "MATERIAL",
    usesAxis: false,
    usesStrength: true,
    usesSmoothing: true,
    usesInvertXY: false,
    strengthLabel: "Intensidade Brilho",
    strengthMin: 0.2,
    strengthMax: 4,
    strengthStep: 0.1,
    strengthUnit: "",
    defaultStrength: 1.4,
    defaultSmoothing: 0.16,
  },
  PLAY_CLIP: {
    targetType: "PLAY_CLIP",
    label: "Tocar Animação",
    shortLabel: "Animação",
    example: "ex.: dispara clip .glb no clique ou hover",
    targetKind: "CLIP",
    usesAxis: false,
    usesStrength: false,
    usesSmoothing: false,
    usesInvertXY: false,
    strengthLabel: "Peso",
    strengthMin: 0.1,
    strengthMax: 1,
    strengthStep: 0.1,
    strengthUnit: "",
    defaultStrength: 1,
    defaultSmoothing: 0.15,
  },
};

export const SCENE3D_ACTION_LIST: readonly Scene3DActionMeta[] = [
  SCENE3D_ACTION_META.NODE_LOOK_AT,
  SCENE3D_ACTION_META.NODE_ROTATE_SWING,
  SCENE3D_ACTION_META.NODE_MOVE_OFFSET,
  SCENE3D_ACTION_META.NODE_ROTATE_SPIN,
  SCENE3D_ACTION_META.NODE_SCALE_PULSE,
  SCENE3D_ACTION_META.MORPH_WEIGHT,
  SCENE3D_ACTION_META.EMISSIVE_GLOW,
  SCENE3D_ACTION_META.PLAY_CLIP,
];

export interface Scene3DBindingValidation {
  valid: boolean;
  missingObject: boolean;
  missingTarget: boolean;
  missingSourceNode: boolean;
  reasons: string[];
}

/**
 * Validates that a binding's `objectId`, `targetName` (strictly for its action's target kind)
 * and raycast `sourceNode` exist on the loaded scene, without ever throwing or breaking render.
 */
export function validateReactiveBinding3D(
  binding: ReactiveBinding3D,
  objects: readonly Scene3DObject[],
  objInfo?: Scene3DObjectInspectInfo | Scene3DInspectInfo | null,
  extraNodeNames?: ReadonlySet<string> | null
): Scene3DBindingValidation {
  const reasons: string[] = [];
  const hasExplicitObj = Boolean(binding.objectId);
  const resolvedObj = hasExplicitObj
    ? objects.find((o) => o.id === binding.objectId) ?? null
    : objects[0] ?? null;
  const missingObject = !resolvedObj;
  if (missingObject) {
    reasons.push(
      hasExplicitObj
        ? `Objeto "${binding.objectId}" não existe mais na sala.`
        : "Sala sem objeto alvo disponível."
    );
  }

  let missingTarget = false;
  let missingSourceNode = false;
  if (!missingObject && objInfo) {
    const meta = SCENE3D_ACTION_META[binding.targetType];
    if (!meta) {
      missingTarget = true;
      reasons.push(`Tipo de ação desconhecido: "${String(binding.targetType)}".`);
    } else {
      const t = binding.targetName || "";
      if (meta.targetKind === "NODE") {
        if (t && t !== "__ROOT__") {
          const found =
            objInfo.boneNames.includes(t) ||
            objInfo.meshNames.includes(t) ||
            Boolean(extraNodeNames?.has(t));
          if (!found) {
            missingTarget = true;
            reasons.push(`Peça/osso "${t}" não encontrado no objeto.`);
          }
        }
      } else if (meta.targetKind === "MORPH") {
        if (!t) {
          missingTarget = true;
          reasons.push(
            objInfo.morphNames.length === 0
              ? "Objeto não possui Shape Keys / Morphs."
              : "Selecione um Morph válido."
          );
        } else if (!objInfo.morphNames.includes(t)) {
          missingTarget = true;
          reasons.push(`Morph "${t}" não encontrado no objeto.`);
        }
      } else if (meta.targetKind === "MATERIAL") {
        if (t && t !== "*" && !objInfo.materialNames.includes(t)) {
          missingTarget = true;
          reasons.push(`Material "${t}" não encontrado no objeto.`);
        }
      } else if (meta.targetKind === "CLIP") {
        if (!t) {
          missingTarget = true;
          reasons.push(
            objInfo.clipNames.length === 0
              ? "Objeto não possui clips de animação."
              : "Selecione uma animação válida."
          );
        } else if (!objInfo.clipNames.includes(t)) {
          missingTarget = true;
          reasons.push(`Animação "${t}" não encontrada no objeto.`);
        }
      }
    }

    if (
      (binding.source === "HOVER_MESH" || binding.source === "CLICK_MESH") &&
      binding.sourceNode &&
      binding.sourceNode !== "*"
    ) {
      const hitMesh =
        objInfo.meshNames.includes(binding.sourceNode) ||
        Boolean(extraNodeNames?.has(binding.sourceNode));
      if (!hitMesh) {
        missingSourceNode = true;
        reasons.push(`Malha gatilho "${binding.sourceNode}" não encontrada.`);
      }
    }
  }

  return {
    valid: !missingObject && !missingTarget && !missingSourceNode,
    missingObject,
    missingTarget,
    missingSourceNode,
    reasons,
  };
}

/** Creates a new ReactiveBinding3D for any of the 8 action types with sensible defaults. */
export function createBindingForTargetType(
  targetType: Scene3DBindingTarget,
  info?: Scene3DObjectInspectInfo | Scene3DInspectInfo | null,
  objectId?: string
): ReactiveBinding3D {
  const id = `b3d-${Math.random().toString(36).slice(2, 9)}`;
  const objPatch = objectId ? { objectId } : {};
  const meta = SCENE3D_ACTION_META[targetType];

  if (targetType === "NODE_LOOK_AT") {
    const head =
      info?.boneNames.find((b) => /head|neck|eye|cabeca|pescoco/i.test(b)) ??
      info?.boneNames[0] ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "CURSOR_LOOK",
      sourceNode: "*",
      targetType,
      targetName: head,
      strength: meta.defaultStrength,
      smoothing: meta.defaultSmoothing,
      invertX: false,
      invertY: false,
    };
  }
  if (targetType === "NODE_ROTATE_SWING") {
    const names = [...(info?.boneNames ?? []), ...(info?.meshNames ?? [])];
    const hinge =
      names.find((n) => /wing|asa|jaw|mandib|arm|braco|lid|tampa|needle|agulha|tail|cauda/i.test(n)) ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType,
      targetName: hinge,
      strength: meta.defaultStrength,
      smoothing: meta.defaultSmoothing,
      axis: meta.defaultAxis ?? "Z",
      invertAxis: false,
    };
  }
  if (targetType === "NODE_MOVE_OFFSET") {
    const names = [...(info?.meshNames ?? []), ...(info?.boneNames ?? [])];
    const piston =
      names.find((n) => /speaker|woofer|cone|caixa|alto.?falante|key|tecla|piston/i.test(n)) ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType,
      targetName: piston,
      strength: meta.defaultStrength,
      smoothing: meta.defaultSmoothing,
      axis: meta.defaultAxis ?? "Z",
      invertAxis: false,
    };
  }
  if (targetType === "NODE_ROTATE_SPIN") {
    const names = [...(info?.boneNames ?? []), ...(info?.meshNames ?? [])];
    const spinner =
      names.find((n) => /fan|ventoinha|rotor|wheel|roda|propeller|turntable|vinyl/i.test(n)) ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: /wheel|roda|turntable|vinyl/i.test(spinner) ? "MEDIA_PLAYING" : "CPU_USAGE",
      sourceNode: "*",
      targetType,
      targetName: spinner,
      strength: meta.defaultStrength,
      smoothing: meta.defaultSmoothing,
      axis: meta.defaultAxis ?? "Z",
      invertAxis: false,
    };
  }
  if (targetType === "NODE_SCALE_PULSE") {
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType,
      targetName: "__ROOT__",
      strength: meta.defaultStrength,
      smoothing: meta.defaultSmoothing,
    };
  }
  if (targetType === "MORPH_WEIGHT") {
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_MID",
      sourceNode: "*",
      targetType,
      targetName: info?.morphNames[0] ?? "",
      strength: meta.defaultStrength,
      smoothing: meta.defaultSmoothing,
    };
  }
  if (targetType === "EMISSIVE_GLOW") {
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "KEY_PULSE",
      sourceNode: "*",
      targetType,
      targetName: info?.materialNames[0] ?? "*",
      strength: meta.defaultStrength,
      smoothing: meta.defaultSmoothing,
    };
  }
  return {
    id,
    enabled: true,
    ...objPatch,
    source: "CLICK_MESH",
    sourceNode: "*",
    targetType: "PLAY_CLIP",
    targetName: info?.clipNames[0] ?? "",
    strength: meta.defaultStrength,
    smoothing: meta.defaultSmoothing,
  };
}

/** Default `strength` per target (same fallbacks the Scene3DLayer runtime uses) */
export const SCENE3D_BINDING_DEFAULT_STRENGTH: Record<Scene3DBindingTarget, number> = {
  NODE_LOOK_AT: 20,
  PLAY_CLIP: 1,
  EMISSIVE_GLOW: 1.4,
  NODE_SCALE_PULSE: 0.16,
  MORPH_WEIGHT: 1,
  NODE_ROTATE_SPIN: 2,
  NODE_ROTATE_SWING: 25,
  NODE_MOVE_OFFSET: 0.15,
};
export const SCENE3D_BINDING_SMOOTHING_MIN = 0.02;
export const SCENE3D_BINDING_SMOOTHING_MAX = 0.45;

function isScene3DAxis(v: unknown): v is Scene3DAxis {
  return v === "X" || v === "Y" || v === "Z";
}

function usesBindingAxis(t: Scene3DBindingTarget): boolean {
  return t === "NODE_ROTATE_SWING" || t === "NODE_MOVE_OFFSET" || t === "NODE_ROTATE_SPIN";
}

/**
 * Binding normalizer (idempotent: same ref when already valid). Axis-driven targets
 * (SWING / MOVE_OFFSET / SPIN) always get `axis` ("Y") and a boolean `invertAxis`;
 * strength/smoothing are made finite and smoothing clamped to the spring range.
 */
export function normalizeReactiveBinding3D(b: ReactiveBinding3D): ReactiveBinding3D {
  const fallback = SCENE3D_BINDING_DEFAULT_STRENGTH[b.targetType] ?? 1;
  const strength = finiteOr(b.strength, fallback);
  const smoothing = Math.max(
    SCENE3D_BINDING_SMOOTHING_MIN,
    Math.min(SCENE3D_BINDING_SMOOTHING_MAX, finiteOr(b.smoothing, 0.18))
  );
  const axisTarget = usesBindingAxis(b.targetType);
  const axis: Scene3DAxis | undefined = isScene3DAxis(b.axis) ? b.axis : axisTarget ? "Y" : undefined;
  const invertAxis: boolean | undefined =
    typeof b.invertAxis === "boolean" ? b.invertAxis : axisTarget ? false : undefined;
  const objectId = typeof b.objectId === "string" && b.objectId ? b.objectId : undefined;
  const targetName = typeof b.targetName === "string" ? b.targetName : "";
  const sourceNode = typeof b.sourceNode === "string" ? b.sourceNode : "";
  if (
    strength === b.strength &&
    smoothing === b.smoothing &&
    axis === b.axis &&
    invertAxis === b.invertAxis &&
    objectId === b.objectId &&
    targetName === b.targetName &&
    sourceNode === b.sourceNode &&
    typeof b.enabled === "boolean"
  ) {
    return b;
  }
  const out: ReactiveBinding3D = {
    ...b,
    enabled: b.enabled !== false,
    sourceNode,
    targetName,
    strength,
    smoothing,
  };
  if (objectId) out.objectId = objectId;
  else delete out.objectId;
  if (axis) out.axis = axis;
  else delete out.axis;
  if (invertAxis !== undefined) out.invertAxis = invertAxis;
  else delete out.invertAxis;
  return out;
}

function normalizeReactiveBindings3D(list: ReactiveBinding3D[]): ReactiveBinding3D[] {
  let changed = false;
  const next: ReactiveBinding3D[] = [];
  for (const b of list) {
    if (!b || typeof b !== "object" || typeof b.id !== "string") {
      changed = true;
      continue;
    }
    const nb = normalizeReactiveBinding3D(b);
    if (nb !== b) changed = true;
    next.push(nb);
  }
  return changed ? next : list;
}

/**
 * Single migration point for Scene3DConfig (call it at the borders: project load,
 * .prism import, scene broadcast to desktop windows). Idempotent and cheap: returns
 * the same object when it is already current, so React memoization keeps working.
 */
export function normalizeScene3DConfig(raw: Scene3DConfig | undefined | null): Scene3DConfig {
  const base = defaultScene3D();
  if (!raw || typeof raw !== "object") return base;
  const fps = clampScene3DFps(raw.maxFps);
  const fpsMode: Scene3DFpsMode = raw.fpsMode === "MONITOR" ? "MONITOR" : "CAP";
  const isCurrent = raw.schemaVersion === SCENE3D_SCHEMA_VERSION;
  const hasObjects = Array.isArray(raw.objects) && raw.objects.length > 0;
  if (
    isCurrent &&
    raw.maxFps === fps &&
    raw.fpsMode === fpsMode &&
    hasObjects &&
    isBackgroundMode(raw.backgroundMode) &&
    typeof raw.backgroundColor === "string" &&
    raw.camera &&
    Array.isArray(raw.camera.target) &&
    raw.camera.targetY === raw.camera.target[1] &&
    !hasLegacyScene3DKeys(raw) &&
    raw.objects.every((o, i) => normalizeScene3DObject(o, i, false) === o) &&
    sanitizeScene3DParents(raw.objects) === raw.objects &&
    (raw.bindings === undefined ||
      (Array.isArray(raw.bindings) && normalizeReactiveBindings3D(raw.bindings) === raw.bindings))
  ) {
    return raw;
  }
  // Pre-v3 projects: legacy single-model fields → objects[0], old 0.22 spin → 0, transparent backdrop
  const migrating = !isCurrent;
  const rawObjects = hasObjects ? raw.objects : [legacyPrimaryObject(raw)];
  const linkedObjects = sanitizeScene3DParents(
    rawObjects.map((o, i) => normalizeScene3DObject(o, i, migrating))
  );
  const cam = { ...base.camera, ...(raw.camera ?? {}) };
  const targetY = finiteOr(cam.targetY, 0);
  const target = Array.isArray(cam.target)
    ? finiteTriple(cam.target, [0, targetY, 0])
    : ([0, targetY, 0] as [number, number, number]);
  return {
    ...stripLegacyScene3DKeys({ ...base, ...raw }),
    schemaVersion: SCENE3D_SCHEMA_VERSION,
    objects: linkedObjects,
    backgroundMode: migrating || !isBackgroundMode(raw.backgroundMode) ? "TRANSPARENT" : raw.backgroundMode,
    backgroundColor: typeof raw.backgroundColor === "string" ? raw.backgroundColor : base.backgroundColor,
    maxFps: fps,
    fpsMode,
    renderScale: Math.max(0.5, Math.min(1, finiteOr(raw.renderScale, base.renderScale))),
    camera: { ...cam, target, targetY: target[1] },
    bindings: normalizeReactiveBindings3D(Array.isArray(raw.bindings) ? raw.bindings : base.bindings ?? []),
  };
}

function normalizeLayersScene3D(layers: SceneLayer[]): SceneLayer[] {
  let changed = false;
  const next = layers.map((l) => {
    let out = l;
    if (l.kind === "SCENE_3D") {
      const s3d = normalizeScene3DConfig(l.scene3d);
      if (s3d !== l.scene3d) out = { ...out, scene3d: s3d };
    }
    if (l.children?.length) {
      const kids = normalizeLayersScene3D(l.children);
      if (kids !== l.children) out = { ...out, children: kids };
    }
    if (out !== l) changed = true;
    return out;
  });
  return changed ? next : layers;
}

/** Sala 3D gizmo commit: writes one object's transform (used by the Studio editor) */
export function setScene3DObjectTransform(
  cfg: Scene3DConfig,
  objectId: string,
  position: [number, number, number],
  rotation: [number, number, number],
  scale: [number, number, number]
): Scene3DConfig {
  const objs = normalizeScene3DObjects(cfg);
  const nextObjs = objs.map((o) => (o.id === objectId ? { ...o, position, rotation, scale } : o));
  return { ...cfg, objects: nextObjs };
}

export function duplicateScene3DObject(
  cfg: Scene3DConfig,
  objectId: string
): { config: Scene3DConfig; newId: string | null } {
  const objs = normalizeScene3DObjects(cfg);
  const src = objs.find((o) => o.id === objectId);
  if (!src) return { config: cfg, newId: null };
  const newId = `obj3d-${Math.random().toString(36).slice(2, 9)}`;
  const clone: Scene3DObject = {
    ...(JSON.parse(JSON.stringify(src)) as Scene3DObject),
    id: newId,
    name: `${src.name} (Cópia)`,
    position: [Number(((src.position?.[0] ?? 0) + 0.8).toFixed(2)), src.position?.[1] ?? 0, src.position?.[2] ?? 0],
  };
  const nextObjs = [...objs, clone];
  return { config: { ...cfg, objects: nextObjs }, newId };
}

/** Removes an object (a room always keeps at least one) and its bindings */
export function removeScene3DObject(cfg: Scene3DConfig, objectId: string): Scene3DConfig {
  const objs = normalizeScene3DObjects(cfg);
  if (objs.length <= 1) return cfg;
  const nextObjs = objs.filter((o) => o.id !== objectId);
  if (nextObjs.length === objs.length) return cfg;
  return {
    ...cfg,
    // Children of the removed object become roots
    objects: nextObjs.map((o) => {
      if (o.parentId !== objectId) return o;
      const root = { ...o };
      delete root.parentId;
      return root;
    }),
    bindings: (cfg.bindings ?? []).filter((b) => b.objectId !== objectId),
  };
}

/** Project-level border migration (keeps the same reference when nothing changed) */
export function normalizeProjectScene3D<T extends PrismProject>(project: T): T {
  if (!project || !Array.isArray(project.layers)) return project;
  const layers = normalizeLayersScene3D(project.layers);
  return layers === project.layers ? project : { ...project, layers };
}
