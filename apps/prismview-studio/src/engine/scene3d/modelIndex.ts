import {
  AnimationClip,
  BufferGeometry,
  Material,
  Mesh,
  MeshPhysicalMaterial,
  MeshStandardMaterial,
  Object3D,
  Texture,
  Vector3,
} from "three";
import type { Scene3DAutoSuggestion, Scene3DAxis, Scene3DObjectInspectInfo } from "../../types/prism";

// Model indexing (bones, meshes, materials, morphs, VRAM audit), GPU disposal and
// raycast target matching for loaded .glb / procedural objects.

/**
 * Puts the model's ORIGINAL `map` / `emissiveMap` back on every indexed material.
 * Dynamic textures (album cover, custom image) replace them at runtime; without this the original
 * textures are no longer reachable from the material and `disposeObject3D` would leak them in VRAM.
 */
export function restoreBaseMaps(index: ModelSceneIndex | null | undefined): void {
  if (!index) return;
  for (const entry of index.emissiveMats) {
    if (entry.mat.map !== entry.baseMap) entry.mat.map = entry.baseMap;
    if (entry.mat.emissiveMap !== entry.baseEmissiveMap) entry.mat.emissiveMap = entry.baseEmissiveMap;
    entry.hasDynamicEmissiveMap = false;
  }
}

/**
 * Disposes geometries, materials and textures under `root`.
 * `skipTextures` = textures owned by someone else (e.g. the shared dynamicTexCache): never disposed here.
 */
export function disposeObject3D(root: Object3D, skipTextures?: ReadonlySet<Texture>): void {
  const geometries = new Set<BufferGeometry>();
  const materials = new Set<Material>();
  const textures = new Set<Texture>();
  const skeletons = new Set<{ dispose: () => void }>();

  root.traverse((obj) => {
    const mesh = obj as Mesh;
    if (mesh.geometry) geometries.add(mesh.geometry);
    // SkinnedMesh: the skeleton owns a bone DataTexture (GPU) when bone textures are used
    const skeleton = (obj as Object3D & { skeleton?: { dispose: () => void } }).skeleton;
    if (skeleton) skeletons.add(skeleton);
    const matOrArr = mesh.material as Material | Material[] | undefined;
    if (matOrArr) {
      const list = Array.isArray(matOrArr) ? matOrArr : [matOrArr];
      for (const m of list) {
        materials.add(m);
        for (const val of Object.values(m)) {
          if (val && typeof val === "object" && (val as Texture).isTexture) {
            if (!skipTextures?.has(val as Texture)) textures.add(val as Texture);
          }
        }
      }
    }
  });

  textures.forEach((t) => t.dispose());
  materials.forEach((m) => m.dispose());
  geometries.forEach((g) => g.dispose());
  skeletons.forEach((s) => s.dispose());
}

export interface EmissiveMatEntry {
  mat: MeshStandardMaterial | MeshPhysicalMaterial;
  baseIntensity: number;
  baseColorHex: number;
  hadEmissiveColor: boolean;
  baseMap: Texture | null;
  baseEmissiveMap: Texture | null;
  hasDynamicEmissiveMap: boolean;
}

export interface ModelSceneIndex {
  meshes: Mesh[];
  nodesByName: Map<string, Object3D>;
  baseScales: Map<Object3D, Vector3>;
  emissiveMats: EmissiveMatEntry[];
  emissiveByName: Map<string, EmissiveMatEntry[]>;
  morphTargets: Map<string, Array<{ mesh: Mesh; index: number }>>;
  /** Lazy cache: room-units → node-parent-local units (NODE_MOVE_OFFSET) */
  unitScale: Map<Object3D, number>;
  holder: Object3D;
  info: Scene3DObjectInspectInfo;
}

export function buildModelIndex(
  objectId: string,
  holder: Object3D,
  clips: AnimationClip[],
  isDefaultSculpture: boolean
): ModelSceneIndex {
  const meshes: Mesh[] = [];
  const nodesByName = new Map<string, Object3D>();
  const baseScales = new Map<Object3D, Vector3>();
  const seenMats = new Set<Material>();
  const seenGeos = new Set<BufferGeometry>();
  const seenTextures = new Set<Texture>();
  const emissiveMats: EmissiveMatEntry[] = [];
  const emissiveByName = new Map<string, EmissiveMatEntry[]>();
  const morphTargets = new Map<string, Array<{ mesh: Mesh; index: number }>>();

  let triangleCount = 0;
  let geoBytes = 0;
  const boneSet = new Set<string>();
  const meshNameSet = new Set<string>();
  const matNameSet = new Set<string>();

  nodesByName.set("__ROOT__", holder);
  baseScales.set(holder, holder.scale.clone());

  holder.traverse((obj) => {
    if (obj !== holder && obj.name && !nodesByName.has(obj.name)) {
      nodesByName.set(obj.name, obj);
    }
    baseScales.set(obj, obj.scale.clone());

    if ((obj as Object3D & { isBone?: boolean }).isBone && obj.name) {
      boneSet.add(obj.name);
    }
    const mesh = obj as Mesh;
    if (mesh.isMesh && mesh.geometry) {
      meshes.push(mesh);
      if (mesh.name) meshNameSet.add(mesh.name);
      const geo = mesh.geometry;
      if (geo.index) {
        triangleCount += Math.floor(geo.index.count / 3);
      } else {
        const pos = geo.getAttribute("position");
        if (pos) triangleCount += Math.floor(pos.count / 3);
      }
      if (!seenGeos.has(geo)) {
        seenGeos.add(geo);
        if (geo.index?.array) geoBytes += geo.index.array.byteLength;
        for (const attr of Object.values(geo.attributes)) {
          if (attr?.array) geoBytes += attr.array.byteLength;
        }
      }

      if (mesh.morphTargetDictionary && mesh.morphTargetInfluences) {
        for (const [mName, idx] of Object.entries(mesh.morphTargetDictionary)) {
          const arr = morphTargets.get(mName) ?? [];
          arr.push({ mesh, index: idx });
          morphTargets.set(mName, arr);
        }
      }

      const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
      for (const m of mats) {
        if (!m) continue;
        if (m.name) matNameSet.add(m.name);
        if (!seenMats.has(m)) {
          seenMats.add(m);
          for (const val of Object.values(m)) {
            if (val && typeof val === "object" && (val as Texture).isTexture) {
              seenTextures.add(val as Texture);
            }
          }
          if (
            (m as MeshStandardMaterial).isMeshStandardMaterial ||
            (m as MeshPhysicalMaterial).isMeshPhysicalMaterial
          ) {
            const sm = m as MeshStandardMaterial;
            const hex = sm.emissive ? sm.emissive.getHex() : 0;
            const entry: EmissiveMatEntry = {
              mat: sm,
              baseIntensity: sm.emissiveIntensity ?? 1,
              baseColorHex: hex,
              hadEmissiveColor: hex !== 0,
              baseMap: sm.map ?? null,
              baseEmissiveMap: sm.emissiveMap ?? null,
              hasDynamicEmissiveMap: false,
            };
            emissiveMats.push(entry);
            if (sm.name) {
              const list = emissiveByName.get(sm.name) ?? [];
              list.push(entry);
              emissiveByName.set(sm.name, list);
            }
          }
        }
      }
    }
  });

  let maxTextureSize = 0;
  let texBytes = 0;
  seenTextures.forEach((tex) => {
    const img = tex.image as
      | { width?: number; height?: number; naturalWidth?: number; naturalHeight?: number }
      | undefined;
    const w = img?.width || img?.naturalWidth || 0;
    const h = img?.height || img?.naturalHeight || 0;
    const dim = Math.max(w, h);
    if (dim > maxTextureSize) maxTextureSize = dim;
    if (w > 0 && h > 0) {
      // Uncompressed GPU RGBA8 + mipmaps (~1.33×); compressed KTX2 uses less but conservative audit protects VRAM
      texBytes += w * h * 4 * 1.33;
    }
  });
  const textureCount = seenTextures.size;
  const estimatedVramMb = Math.round(((geoBytes + texBytes) / (1024 * 1024)) * 10) / 10;

  const suggestions = detectModelSuggestions(nodesByName, boneSet, meshes, emissiveMats, morphTargets);

  const info: Scene3DObjectInspectInfo = {
    objectId,
    clipNames: clips.map((c, idx) => c.name || `Clip ${idx + 1}`),
    boneNames: Array.from(boneSet),
    meshNames: Array.from(meshNameSet),
    materialNames: Array.from(matNameSet),
    morphNames: Array.from(morphTargets.keys()),
    meshCount: meshes.length,
    triangleCount,
    textureCount,
    maxTextureSize,
    estimatedVramMb,
    isDefaultSculpture,
    error: null,
    suggestions,
  };

  return {
    meshes,
    nodesByName,
    baseScales,
    emissiveMats,
    emissiveByName,
    morphTargets,
    unitScale: new Map(),
    holder,
    info,
  };
}

const unitTmpA = new Vector3();
const unitTmpB = new Vector3();

/**
 * How many node-parent-local units equal one normalized "room unit" (the model is
 * normalized to 2.4 units inside its holder). Lets NODE_MOVE_OFFSET strength mean
 * the same distance on a 1:1 rig and on an armature exported at 0.01 scale.
 */
export function nodeUnitScale(idx: ModelSceneIndex, node: Object3D): number {
  const cached = idx.unitScale.get(node);
  if (cached !== undefined) return cached;
  let unit = 1;
  const parent = node.parent;
  if (node !== idx.holder && parent) {
    parent.getWorldScale(unitTmpA);
    idx.holder.getWorldScale(unitTmpB);
    const p = Math.max(Math.abs(unitTmpA.x), Math.abs(unitTmpA.y), Math.abs(unitTmpA.z));
    const h = Math.max(Math.abs(unitTmpB.x), Math.abs(unitTmpB.y), Math.abs(unitTmpB.z));
    if (p > 1e-8 && h > 1e-8) unit = h / p;
  }
  idx.unitScale.set(node, unit);
  return unit;
}

export function matchesMeshTarget(hitObj: Object3D | null, sourceNode: string, root: Object3D | null): boolean {
  if (!hitObj) return false;
  if (!sourceNode || sourceNode === "*") return true;
  let cur: Object3D | null = hitObj;
  while (cur && cur !== root) {
    if (cur.name === sourceNode) return true;
    cur = cur.parent;
  }
  return false;
}

// ── Auto-detection of semantic parts (runs ONCE on model load, never per frame) ──

function splitNameTokens(raw: string): string[] {
  return raw
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .toLowerCase()
    .split(/[^a-z0-9\u00c0-\u024f]+/)
    .filter(Boolean);
}

const END_EFFECTOR_RE = /(?:^|[_\-\s.])(end|nub|tip|target|ik|pole|helper|collider)(?:$|[_\-\s.])/i;
const SCENE_WRAPPER_RE = /^(scene|rootnode|auxscene|sketchfab_model|sketchfab_scene|modelroot_.+|objpivot_.+)$/i;
const GENERIC_MESH_RE = /^(mesh|primitive|object|node|geometry|polysurface|pcube|psphere)[_.\-\s]?\d*$/i;

function matchHead(raw: string, tokens: string[]): boolean {
  if (tokens.includes("head") || tokens.includes("cabeca") || tokens.includes("cabeça")) return true;
  return /head(?!phone|set|light|er)/i.test(raw);
}

function matchNeck(raw: string, tokens: string[]): boolean {
  return tokens.includes("neck") || tokens.includes("pescoco") || tokens.includes("pescoço") || /neck/i.test(raw);
}

function matchEye(raw: string, tokens: string[]): boolean {
  if (/eyebrow|eyelid|eyelash|eyeshadow|eyewear/i.test(raw)) return false;
  if (
    tokens.includes("eye") ||
    tokens.includes("eyes") ||
    tokens.includes("eyeball") ||
    tokens.includes("olho") ||
    tokens.includes("olhos")
  ) {
    return true;
  }
  return /(?:^|left|right)eyes?(?:$|\d)/i.test(raw);
}

function matchJaw(raw: string, tokens: string[]): boolean {
  if (
    tokens.includes("jaw") ||
    tokens.includes("jaws") ||
    tokens.includes("mandible") ||
    tokens.includes("mandibula") ||
    tokens.includes("mandíbula")
  ) {
    return true;
  }
  return /jaw/i.test(raw);
}

function matchJawMorph(raw: string, tokens: string[]): boolean {
  return matchJaw(raw, tokens) || /mouth.?open|viseme.?aa/i.test(raw);
}

function matchWing(raw: string, tokens: string[]): boolean {
  if (tokens.includes("swing") || /swing/i.test(raw)) return false;
  if (tokens.includes("wing") || tokens.includes("wings") || tokens.includes("asa") || tokens.includes("asas")) {
    return true;
  }
  return /wing/i.test(raw);
}

function matchFan(_raw: string, tokens: string[]): boolean {
  return (
    tokens.includes("fan") ||
    tokens.includes("fans") ||
    tokens.includes("ventoinha") ||
    tokens.includes("rotor") ||
    tokens.includes("propeller") ||
    tokens.includes("helice") ||
    tokens.includes("hélice")
  );
}

function matchWheel(raw: string, tokens: string[]): boolean {
  return (
    tokens.includes("wheel") ||
    tokens.includes("wheels") ||
    tokens.includes("roda") ||
    tokens.includes("rodas") ||
    tokens.includes("turntable") ||
    tokens.includes("vinyl") ||
    /wheel/i.test(raw)
  );
}

function matchSpeaker(raw: string, tokens: string[]): boolean {
  return (
    tokens.includes("speaker") ||
    tokens.includes("speakers") ||
    tokens.includes("woofer") ||
    tokens.includes("subwoofer") ||
    tokens.includes("tweeter") ||
    /speaker|woofer/i.test(raw)
  );
}

function matchScreen(raw: string, tokens: string[]): boolean {
  return (
    tokens.includes("screen") ||
    tokens.includes("screens") ||
    tokens.includes("display") ||
    tokens.includes("monitor") ||
    tokens.includes("tela") ||
    tokens.includes("lcd") ||
    tokens.includes("oled") ||
    /screen/i.test(raw)
  );
}

interface NodeCandidate {
  name: string;
  tokens: string[];
  isBone: boolean;
  isMesh: boolean;
}

function pickBestNode(
  candidates: readonly NodeCandidate[],
  matcher: (raw: string, tokens: string[]) => boolean,
  preferBone: boolean
): NodeCandidate | null {
  let best: NodeCandidate | null = null;
  let bestScore = -Infinity;
  for (const c of candidates) {
    if (!matcher(c.name, c.tokens)) continue;
    let score = 0;
    if (preferBone) score += c.isBone ? 24 : c.isMesh ? 6 : 12;
    else score += c.isBone ? 14 : c.isMesh ? 10 : 12;
    if (END_EFFECTOR_RE.test(c.name)) score -= 30;
    score -= c.name.length * 0.1;
    if (score > bestScore) {
      bestScore = score;
      best = c;
    }
  }
  return best;
}

function detectModelSuggestions(
  nodesByName: Map<string, Object3D>,
  boneSet: Set<string>,
  meshes: Mesh[],
  emissiveMats: EmissiveMatEntry[],
  morphTargets: Map<string, Array<{ mesh: Mesh; index: number }>>
): Scene3DAutoSuggestion[] {
  const candidates: NodeCandidate[] = [];
  nodesByName.forEach((obj, name) => {
    if (name === "__ROOT__" || SCENE_WRAPPER_RE.test(name)) return;
    candidates.push({
      name,
      tokens: splitNameTokens(name),
      isBone: boneSet.has(name) || Boolean((obj as Object3D & { isBone?: boolean }).isBone),
      isMesh: Boolean((obj as Mesh).isMesh),
    });
  });

  const out: Scene3DAutoSuggestion[] = [];

  const head = pickBestNode(candidates, matchHead, true);
  if (head) {
    out.push({
      id: `sug-head-${head.name}`,
      role: "HEAD",
      kind: "BINDING",
      label: "Cabeça → Mouse",
      detail: `Olhar p/ Cursor em "${head.name}" (24° · suav. 0.12)`,
      targetName: head.name,
      binding: {
        enabled: true,
        source: "CURSOR_LOOK",
        sourceNode: "*",
        targetType: "NODE_LOOK_AT",
        targetName: head.name,
        strength: 24,
        smoothing: 0.12,
        invertX: false,
        invertY: false,
      },
    });
  }

  const neck = pickBestNode(candidates, matchNeck, true);
  if (neck) {
    out.push({
      id: `sug-neck-${neck.name}`,
      role: "NECK",
      kind: "BINDING",
      label: "Pescoço → Mouse",
      detail: `Olhar p/ Cursor em "${neck.name}" (15° · suav. 0.10)`,
      targetName: neck.name,
      binding: {
        enabled: true,
        source: "CURSOR_LOOK",
        sourceNode: "*",
        targetType: "NODE_LOOK_AT",
        targetName: neck.name,
        strength: 15,
        smoothing: 0.1,
        invertX: false,
        invertY: false,
      },
    });
  }

  const eye = pickBestNode(candidates, matchEye, true);
  if (eye) {
    out.push({
      id: `sug-eye-${eye.name}`,
      role: "EYE",
      kind: "BINDING",
      label: "Olho → Mouse",
      detail: `Olhar p/ Cursor em "${eye.name}" (18° · suav. 0.18)`,
      targetName: eye.name,
      binding: {
        enabled: true,
        source: "CURSOR_LOOK",
        sourceNode: "*",
        targetType: "NODE_LOOK_AT",
        targetName: eye.name,
        strength: 18,
        smoothing: 0.18,
        invertX: false,
        invertY: false,
      },
    });
  }

  // Jaw: prefer Morph Target on AUDIO_MID, else bone/node NODE_ROTATE_SWING on AUDIO_MID
  let bestJawMorph: string | null = null;
  let bestJawMorphScore = -Infinity;
  for (const mName of morphTargets.keys()) {
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
    if (jaw) {
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
  if (wing) {
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
  if (fan) {
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
  if (wheel) {
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
  if (speaker) {
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

  // Screen: prefer matching material name, else first standard/physical material on a mesh/group named Screen
  let screenMatName: string | null = null;
  let screenMatchLabel = "";
  for (const entry of emissiveMats) {
    const mName = entry.mat.name;
    if (mName && matchScreen(mName, splitNameTokens(mName))) {
      screenMatName = mName;
      screenMatchLabel = mName;
      break;
    }
  }
  if (!screenMatName) {
    for (const mesh of meshes) {
      const mHit =
        (mesh.name && matchScreen(mesh.name, splitNameTokens(mesh.name))) ||
        (mesh.parent?.name &&
          !SCENE_WRAPPER_RE.test(mesh.parent.name) &&
          matchScreen(mesh.parent.name, splitNameTokens(mesh.parent.name)));
      if (!mHit) continue;
      const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
      const firstNamed = mats.find((m) => Boolean(m?.name))?.name;
      screenMatName = firstNamed || "*";
      screenMatchLabel = firstNamed || mesh.name || mesh.parent?.name || "Tela";
      break;
    }
  }
  if (screenMatName) {
    out.push({
      id: `sug-screen-${screenMatName}`,
      role: "SCREEN",
      kind: "DYNAMIC_TEXTURE",
      label: "Tela → Capa Música",
      detail: `Aplicar capa da música ao vivo no material "${screenMatchLabel}"`,
      targetName: screenMatName,
      dynamicTexture: {
        materialName: screenMatName,
        source: "MEDIA_COVER",
        emissiveMap: true,
      },
    });
  }

  return out;
}

// ── Alt+Click part picking (Sala 3D ROOM mode) ───────────────────────────────────

const pickBonePos = new Vector3();

type SkinAttrLike = {
  getX: (i: number) => number;
  getY: (i: number) => number;
  getZ: (i: number) => number;
  getW: (i: number) => number;
};

type SkinnedMeshLike = Mesh & {
  isSkinnedMesh?: boolean;
  skeleton?: { bones: Object3D[] };
};

function isPrimitiveSplitOfParent(meshName: string, parentName: string): boolean {
  if (!meshName) return true;
  if (GENERIC_MESH_RE.test(meshName)) return true;
  if (!parentName) return false;
  if (!meshName.startsWith(parentName)) return false;
  const suffix = meshName.slice(parentName.length);
  return /^[_.\-]\d+$/.test(suffix);
}

/**
 * Given a raycast hit on a mesh, resolves the most useful named node under `idx.holder`:
 * - For SkinnedMesh: dominant weighted bone of the hit triangle (fallback: nearest named bone).
 * - For rigid meshes: climbs out of unnamed / generic / multi-material-split primitives to the
 *   enclosing named Group, otherwise keeps the named Mesh itself.
 */
export function resolvePickedPartNode(
  idx: ModelSceneIndex,
  hitObj: Object3D,
  face: { a: number; b: number; c: number } | null | undefined,
  hitPoint: Vector3
): Object3D {
  const skinned = hitObj as SkinnedMeshLike;
  if (skinned.isSkinnedMesh && skinned.skeleton && skinned.skeleton.bones.length > 0) {
    const bones = skinned.skeleton.bones;
    const geo = skinned.geometry;
    const skinIndex = geo?.getAttribute("skinIndex") as SkinAttrLike | undefined;
    const skinWeight = geo?.getAttribute("skinWeight") as SkinAttrLike | undefined;

    if (face && skinIndex && skinWeight) {
      const weights = new Map<number, number>();
      const verts = [face.a, face.b, face.c];
      for (let i = 0; i < 3; i++) {
        const v = verts[i];
        const b0 = Math.round(skinIndex.getX(v));
        const b1 = Math.round(skinIndex.getY(v));
        const b2 = Math.round(skinIndex.getZ(v));
        const b3 = Math.round(skinIndex.getW(v));
        const w0 = skinWeight.getX(v);
        const w1 = skinWeight.getY(v);
        const w2 = skinWeight.getZ(v);
        const w3 = skinWeight.getW(v);
        if (w0 > 0) weights.set(b0, (weights.get(b0) ?? 0) + w0);
        if (w1 > 0) weights.set(b1, (weights.get(b1) ?? 0) + w1);
        if (w2 > 0) weights.set(b2, (weights.get(b2) ?? 0) + w2);
        if (w3 > 0) weights.set(b3, (weights.get(b3) ?? 0) + w3);
      }
      let bestBoneIdx = -1;
      let bestWeight = 0;
      for (const [bIdx, w] of weights) {
        if (w > bestWeight && bIdx >= 0 && bIdx < bones.length) {
          bestWeight = w;
          bestBoneIdx = bIdx;
        }
      }
      if (bestBoneIdx >= 0) {
        let cur: Object3D | null = bones[bestBoneIdx];
        while (cur && cur !== idx.holder && !cur.name) {
          cur = cur.parent;
        }
        if (cur && cur !== idx.holder && cur.name) return cur;
      }
    }

    // Fallback: closest named bone to the hit world point
    let nearestBone: Object3D | null = null;
    let nearestDistSq = Infinity;
    for (const bone of bones) {
      if (!bone.name) continue;
      bone.getWorldPosition(pickBonePos);
      const dSq = pickBonePos.distanceToSquared(hitPoint);
      if (dSq < nearestDistSq) {
        nearestDistSq = dSq;
        nearestBone = bone;
      }
    }
    if (nearestBone) return nearestBone;
  }

  // Rigid mesh: climb to parent group if mesh is unnamed / generic / multi-material split
  const parent = hitObj.parent;
  if (
    parent &&
    parent !== idx.holder &&
    parent.name &&
    !SCENE_WRAPPER_RE.test(parent.name) &&
    isPrimitiveSplitOfParent(hitObj.name, parent.name)
  ) {
    return parent;
  }

  if (hitObj.name && !SCENE_WRAPPER_RE.test(hitObj.name)) {
    return hitObj;
  }

  // Climb to first named ancestor below holder (preferring non-wrapper names)
  let fallbackNamed: Object3D | null = null;
  let cur: Object3D | null = hitObj.parent;
  while (cur && cur !== idx.holder) {
    if (cur.name) {
      if (!SCENE_WRAPPER_RE.test(cur.name)) return cur;
      if (!fallbackNamed) fallbackNamed = cur;
    }
    cur = cur.parent;
  }

  return fallbackNamed ?? hitObj;
}
