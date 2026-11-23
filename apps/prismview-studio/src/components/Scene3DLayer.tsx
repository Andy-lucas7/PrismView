import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  ACESFilmicToneMapping,
  AmbientLight,
  AnimationAction,
  AnimationClip,
  AnimationMixer,
  Box3,
  Color,
  DirectionalLight,
  Euler,
  Group,
  LoopOnce,
  LoopRepeat,
  Object3D,
  PerspectiveCamera,
  PMREMGenerator,
  PointLight,
  Quaternion,
  Raycaster,
  Scene,
  SRGBColorSpace,
  Texture,
  TextureLoader,
  Vector2,
  Vector3,
  WebGLRenderer,
  WebGLRenderTarget,
} from "three";
import {
  getSharedGltfLoader,
  releaseSharedLoaders,
  retainSharedLoaders,
} from "../engine/scene3d/loaders";
import {
  clampScene3DFps,
  DEFAULT_REACTIVE_SIGNAL,
  normalizeScene3DObjects,
  ReactiveBinding3D,
  ReactiveSignal,
  resolveMediaUrl,
  Scene3DAxis,
  Scene3DConfig,
  Scene3DEnvPreset,
  Scene3DInspectInfo,
  Scene3DObject,
  Scene3DObjectInspectInfo,
  Scene3DPickedPart,
} from "../types/prism";
import {
  applyConfigCamera,
  applyDayNightLighting,
  applySceneBackground,
  buildProceduralEnvironment,
  computeConfigCameraView,
  ENV_SPECS,
  sampleDayNightBackground,
} from "../engine/scene3d/environment";
import { buildShowcaseSculpture, type DefaultSculptureRefs } from "../engine/scene3d/sculptures";
import {
  buildModelIndex,
  disposeObject3D,
  restoreBaseMaps,
  type EmissiveMatEntry,
  matchesMeshTarget,
  type ModelSceneIndex,
  nodeUnitScale,
  resolvePickedPartNode,
} from "../engine/scene3d/modelIndex";
import { type PowerState, powerCapsFor } from "../engine/powerGovernor";
import type { Scene3DObjectTree } from "../engine/scene3d/modelTree";
import type {
  Scene3DCameraCapture,
  Scene3DEditorSession,
  Scene3DGizmoMode,
} from "../utils/scene3dEditorTools";

export type { Scene3DCameraCapture, Scene3DGizmoMode };

/** Studio view mode for SCENE_3D layers: 2D layout, Sala 3D room editor, or live interaction test */
export type Scene3DEditMode = "LAYOUT" | "ROOM" | "TEST";

/** Imperative handle the Studio uses while the Sala 3D editor is mounted */
export interface Scene3DEditorApi {
  /** Current free-camera state in orbit form (null until the editor chunk is loaded) */
  getCameraState: () => Scene3DCameraCapture | null;
  /** Eases the free camera to an orbit state (`fov` comes from the config, not from here) */
  setCameraState: (state: Scene3DCameraCapture, animate?: boolean) => void;
  /** Frames one Sala 3D object; false if it has no pivot/bounds yet */
  focusOn: (objectId: string) => boolean;
  /** @deprecated alias of `getCameraState` */
  captureCameraState: () => Scene3DCameraCapture | null;
  focusSelected: () => void;
  resetView: () => void;
  /** Outliner tree (bones / meshes / groups, morphs, materials) of an object's loaded model; cached per model */
  getObjectTree: (objectId: string) => Scene3DObjectTree | null;
  /** Highlights one outliner part in the viewport (null clears) */
  highlightNode: (objectId: string, nodeId: number | null) => void;
}

/** Lightweight performance HUD sample (emitted at most ~2×/s, Studio only) */
export interface Scene3DStats {
  fps: number;
  frameMs: number;
  drawCalls: number;
  triangles: number;
  geometries: number;
  textures: number;
  pixelRatio: number;
  powerState: PowerState;
  sleeping: boolean;
}

export interface Scene3DLayerProps {
  config: Scene3DConfig;
  width: number;
  height: number;
  paused: boolean;
  isThumbnail?: boolean;
  interactive?: boolean;
  /** When true inside Studio, enables live mouse orbit, Bone Look-At, hover & click raycast testing */
  testMode?: boolean;
  cursorNormRef?: React.RefObject<[number, number] | null>;
  subscribeCursorTick?: (cb: () => void) => () => void;
  subscribeCursorClick?: (cb: (x: number, y: number) => void) => () => void;
  reactiveSignalRef?: React.RefObject<ReactiveSignal>;
  subscribeReactiveTick?: (cb: () => void) => () => void;
  mediaCoverPath?: string;
  mediaPlaying?: boolean;
  cpuPercent?: number;
  onCanvasEl?: (el: HTMLCanvasElement | null) => void;
  onFrameReady?: () => void;
  onInspectModel?: (info: Scene3DInspectInfo) => void;
  onHoverMesh?: (meshName: string | null) => void;
  /** Energy governor state (Normal / Eco / Ultra / Frozen) */
  powerState?: PowerState;
  /** Studio-only Sala 3D editor (free camera, gizmos, grid). Never mounted on Desktop. */
  editMode3d?: boolean;
  selectedObjectId?: string | null;
  gizmoMode?: Scene3DGizmoMode;
  showGrid?: boolean;
  snapEnabled?: boolean;
  /** Sala 3D: draws the selected object's SkeletonHelper (editor chunk only; removed with dispose on exit) */
  showSkeleton?: boolean;
  /** Sala 3D: fires when the editor API becomes available and after every model (re)load, so the outliner can refetch */
  onEditorSync?: () => void;
  onSelectObject?: (objectId: string | null) => void;
  /** Sala 3D: fires on Alt+Click in the viewport with the resolved bone/part (or null on empty space) */
  onPickPart?: (part: Scene3DPickedPart | null) => void;
  onCommitObjectTransform?: (
    objectId: string,
    position: [number, number, number],
    rotationDeg: [number, number, number],
    scale: [number, number, number]
  ) => void;
  editorApiRef?: React.MutableRefObject<Scene3DEditorApi | null>;
  onStats?: (stats: Scene3DStats) => void;
}

const AXIS_VECTORS: Record<Scene3DAxis, Vector3> = {
  X: new Vector3(1, 0, 0),
  Y: new Vector3(0, 1, 0),
  Z: new Vector3(0, 0, 1),
};

interface LoadedObjectRuntime {
  id: string;
  sourceKey: string;
  /** Key (`kind|modelSrc|variant`) of the load currently in flight; "" when idle (AUDITORIA 1.6). */
  loadKey: string;
  loadToken: number;
  pivot: Group;
  modelRoot: Object3D | null;
  modelIndex: ModelSceneIndex | null;
  defaultRefs: DefaultSculptureRefs | null;
  mixer: AnimationMixer | null;
  clips: AnimationClip[];
  baseAction: AnimationAction | null;
  overrideAction: AnimationAction | null;
  overrideClipName: string | null;
  overrideHoldHoverId: string | null;
  activeClipKey: string;
  spinAngle: number;
  error: string | null;
}

/**
 * Binding target resolved by `objectId` + `targetName` against the owning object's index.
 * Rebuilt only when the config or a model index changes (`targetsRev`), never per frame.
 * `rt === null` ⇒ owner object not loaded yet: the binding is skipped (never falls onto another object).
 */
interface ResolvedBindingTarget {
  rt: LoadedObjectRuntime | null;
  node: Object3D | null;
  mats: readonly EmissiveMatEntry[];
}

const EMPTY_MATS: readonly EmissiveMatEntry[] = [];
const UNRESOLVED_TARGET: ResolvedBindingTarget = { rt: null, node: null, mats: EMPTY_MATS };

/** Owner of a binding: its `objectId` when that object exists, otherwise the first object (≤v2 compat). */
function bindingOwnerId(objs: readonly Scene3DObject[], objectId: string | undefined): string | null {
  if (objectId) {
    for (const o of objs) if (o.id === objectId) return objectId;
  }
  return objs[0]?.id ?? null;
}

function dayNightHourFor(cfg: Scene3DConfig): number {
  if (typeof cfg.dayNightHourOverride === "number" && Number.isFinite(cfg.dayNightHourOverride)) {
    return cfg.dayNightHourOverride;
  }
  const d = new Date();
  return d.getHours() + d.getMinutes() / 60;
}

/** (Re)applies the canvas background for `cfg` (mode/colour/day-night). Never called per frame. */
function syncSceneBackground(scene: Scene, cfg: Scene3DConfig): void {
  applySceneBackground(scene, cfg, cfg.autoDayNight ? sampleDayNightBackground(dayNightHourFor(cfg)) : undefined);
}

function buildAggregatedInspectInfo(
  objectsMap: Map<string, LoadedObjectRuntime>,
  orderedObjs: Scene3DObject[]
): Scene3DInspectInfo {
  const byObject: Record<string, Scene3DObjectInspectInfo> = {};
  let meshCount = 0;
  let triangleCount = 0;
  let textureCount = 0;
  let maxTextureSize = 0;
  let estimatedVramMb = 0;
  let hasExternal = false;
  let primaryInfo: Scene3DObjectInspectInfo | null = null;
  let firstError: string | null = null;

  for (const obj of orderedObjs) {
    const rt = objectsMap.get(obj.id);
    if (!rt?.modelIndex) continue;
    const objInfo: Scene3DObjectInspectInfo = {
      ...rt.modelIndex.info,
      objectId: obj.id,
      error: rt.error ?? rt.modelIndex.info.error ?? null,
    };
    byObject[obj.id] = objInfo;
    if (!primaryInfo) primaryInfo = objInfo;
    if (!firstError && objInfo.error) firstError = objInfo.error;
    meshCount += objInfo.meshCount;
    triangleCount += objInfo.triangleCount;
    textureCount += objInfo.textureCount;
    if (objInfo.maxTextureSize > maxTextureSize) maxTextureSize = objInfo.maxTextureSize;
    estimatedVramMb += objInfo.estimatedVramMb;
    if (!objInfo.isDefaultSculpture) hasExternal = true;
  }

  estimatedVramMb = Math.round(estimatedVramMb * 10) / 10;

  const warnings: string[] = [];
  if (hasExternal) {
    if (triangleCount > 80_000) {
      warnings.push(
        `Geometria densa na sala (${(triangleCount / 1000).toFixed(0)}k tris) — ideal ≤ 60k tris ou Decimate no Blender.`
      );
    }
    if (maxTextureSize > 2048) {
      warnings.push(
        `Textura ${maxTextureSize}px detectada — reduza para ≤ 2048px (WebP/KTX2) para economizar VRAM.`
      );
    }
    if (textureCount > 12) {
      warnings.push(
        `${textureCount} texturas na sala — combine materiais em Atlas para reduzir draw calls.`
      );
    }
    if (estimatedVramMb > 160) {
      warnings.push(
        `VRAM estimada ~${Math.round(estimatedVramMb)} MB — recomendado ativar Modo Leve (70% · 30 FPS).`
      );
    }
  }

  return {
    clipNames: primaryInfo?.clipNames ?? [],
    boneNames: primaryInfo?.boneNames ?? [],
    meshNames: primaryInfo?.meshNames ?? [],
    materialNames: primaryInfo?.materialNames ?? [],
    morphNames: primaryInfo?.morphNames ?? [],
    meshCount,
    triangleCount,
    textureCount,
    maxTextureSize,
    estimatedVramMb,
    warnings,
    isDefaultSculpture: primaryInfo?.isDefaultSculpture ?? true,
    error: firstError,
    byObject,
  };
}

interface BindingRuntimeState {
  yaw: number;
  pitch: number;
  spin: number;
  weight: number;
  pulse: number;
  toggled: boolean;
  wasHovered: boolean;
}

const DEEP_SLEEP_MS = 60_000;

// ── 2.1 Hub thumbnails: ONE live engine at a time, then a static image ─────────────────────────
// Each thumbnail used to keep a full WebGL renderer (+PMREM, model, textures) alive forever.
// Now thumbnails take a turn on a serial queue, render once, snapshot to a blob URL, release the
// GPU context, and display an <img>. Snapshots are cached by config hash + size.
const THUMB_CACHE_MAX = 64;
const thumbCache = new Map<string, string>();
let thumbQueueTail: Promise<void> = Promise.resolve();

function acquireThumbSlot(): { granted: Promise<void>; release: () => void } {
  let release!: () => void;
  const mine = new Promise<void>((res) => {
    release = res;
  });
  const granted = thumbQueueTail;
  thumbQueueTail = thumbQueueTail.then(() => mine);
  return { granted, release };
}

function hashString(str: string): string {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (let i = 0; i < str.length; i++) {
    const c = str.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 2654435761);
    h2 = Math.imul(h2 ^ c, 1597334677);
  }
  return `${(h1 >>> 0).toString(36)}${(h2 >>> 0).toString(36)}`;
}

function storeThumb(key: string, url: string) {
  const old = thumbCache.get(key);
  if (old) URL.revokeObjectURL(old);
  thumbCache.delete(key);
  thumbCache.set(key, url);
  if (thumbCache.size > THUMB_CACHE_MAX) {
    const oldest = thumbCache.keys().next().value;
    if (oldest !== undefined) {
      const u = thumbCache.get(oldest);
      if (u) URL.revokeObjectURL(u);
      thumbCache.delete(oldest);
    }
  }
}

const Scene3DLayerImpl: React.FC<Scene3DLayerProps> = ({
  config,
  width,
  height,
  paused,
  isThumbnail = false,
  interactive = false,
  testMode = false,
  cursorNormRef,
  subscribeCursorTick,
  subscribeCursorClick,
  reactiveSignalRef,
  subscribeReactiveTick,
  mediaCoverPath = "",
  mediaPlaying = false,
  cpuPercent = 0,
  onCanvasEl,
  onFrameReady,
  onInspectModel,
  onHoverMesh,
  powerState = "NORMAL",
  editMode3d = false,
  selectedObjectId = null,
  gizmoMode = "translate",
  showGrid = true,
  snapEnabled = false,
  showSkeleton = false,
  onEditorSync,
  onSelectObject,
  onPickPart,
  onCommitObjectTransform,
  editorApiRef,
  onStats,
}) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [modelLoadRev, setModelLoadRev] = useState(0);
  // 2.3 deep sleep: hidden for > DEEP_SLEEP_MS → engine (renderer, models, textures, env) is disposed.
  // Only `config` is kept; waking remounts a fresh canvas (a lost GL context can't be reused).
  const [sleeping, setSleeping] = useState(false);
  const [canvasGen, setCanvasGen] = useState(0);
  // 2.1 thumbnails: static snapshot + serial slot for the one-off live render
  const thumbKey = useMemo(
    () => (isThumbnail ? `${hashString(JSON.stringify(config))}|${Math.round(width)}x${Math.round(height)}` : ""),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [isThumbnail, config, Math.round(width), Math.round(height)]
  );
  const [thumbUrl, setThumbUrl] = useState<string | null>(() => (thumbKey ? thumbCache.get(thumbKey) ?? null : null));
  const [thumbSlot, setThumbSlot] = useState(false);
  useEffect(() => {
    if (!isThumbnail) {
      setThumbUrl(null);
      setThumbSlot(false);
      return;
    }
    const hit = thumbKey ? thumbCache.get(thumbKey) ?? null : null;
    setThumbUrl(hit);
    setThumbSlot(false);
    if (!hit) setCanvasGen((g) => g + 1);
  }, [isThumbnail, thumbKey]);
  useEffect(() => {
    if (!isThumbnail || thumbUrl) return;
    let cancelled = false;
    const slot = acquireThumbSlot();
    void slot.granted.then(() => {
      if (cancelled) slot.release();
      else {
        thumbReleaseRef.current = slot.release;
        setCanvasGen((g) => g + 1); // fresh canvas + re-run engine-dependent effects
        setThumbSlot(true);
      }
    });
    return () => {
      cancelled = true;
      thumbReleaseRef.current?.();
      thumbReleaseRef.current = null;
      // if the slot was never granted, release it as soon as it is (handled above)
    };
  }, [isThumbnail, thumbUrl, thumbKey]);
  const thumbReleaseRef = useRef<(() => void) | null>(null);
  const engineOff = sleeping || (isThumbnail && (thumbUrl !== null || !thumbSlot));
  const sleepingRef = useRef(engineOff);
  sleepingRef.current = engineOff;
  useEffect(() => {
    if (isThumbnail) return;
    if (paused) {
      const t = window.setTimeout(() => setSleeping(true), DEEP_SLEEP_MS);
      return () => window.clearTimeout(t);
    }
    if (sleepingRef.current) {
      setCanvasGen((g) => g + 1);
      setSleeping(false);
    }
  }, [paused, isThumbnail]);
  const configRef = useRef(config);
  configRef.current = config;
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const isThumbRef = useRef(isThumbnail);
  isThumbRef.current = isThumbnail;
  const interactiveRef = useRef(interactive);
  interactiveRef.current = interactive;
  const testModeRef = useRef(testMode);
  testModeRef.current = testMode;
  const mediaPlayingRef = useRef(mediaPlaying);
  mediaPlayingRef.current = mediaPlaying;
  const cpuPercentRef = useRef(cpuPercent);
  cpuPercentRef.current = cpuPercent;
  const onCanvasElRef = useRef(onCanvasEl);
  onCanvasElRef.current = onCanvasEl;
  const onFrameReadyRef = useRef(onFrameReady);
  onFrameReadyRef.current = onFrameReady;
  const onInspectRef = useRef(onInspectModel);
  onInspectRef.current = onInspectModel;
  const onHoverMeshRef = useRef(onHoverMesh);
  onHoverMeshRef.current = onHoverMesh;
  const powerCaps = powerCapsFor(powerState);
  const powerCapsRef = useRef(powerCaps);
  powerCapsRef.current = powerCaps;
  const onStatsRef = useRef(onStats);
  onStatsRef.current = onStats;
  const onSelectObjectRef = useRef(onSelectObject);
  onSelectObjectRef.current = onSelectObject;
  const onPickPartRef = useRef(onPickPart);
  onPickPartRef.current = onPickPart;
  const onEditorSyncRef = useRef(onEditorSync);
  onEditorSyncRef.current = onEditorSync;
  const onCommitTransformRef = useRef(onCommitObjectTransform);
  onCommitTransformRef.current = onCommitObjectTransform;
  // The editor only ever exists inside the interactive Studio preview
  const editorWanted = Boolean(editMode3d && interactive && !isThumbnail);

  const engineRef = useRef<{
    renderer: WebGLRenderer;
    scene: Scene;
    camera: PerspectiveCamera;
    pmrem: PMREMGenerator;
    envTarget: WebGLRenderTarget | null;
    envPresetLoaded: Scene3DEnvPreset | null;
    ambientLight: AmbientLight;
    keyLight: DirectionalLight;
    rimLight: PointLight;
    stagePivot: Group;
    objectsMap: Map<string, LoadedObjectRuntime>;
    texLoader: TextureLoader;
    dynamicTexCache: Map<string, Texture>;
    raycaster: Raycaster;
    rayNdc: Vector2;
    lastRayX: number;
    lastRayY: number;
    lastRaySpin: number;
    hoveredObject: Object3D | null;
    hoveredObjectId: string | null;
    lastHoveredName: string | null;
    bindingStates: Map<string, BindingRuntimeState>;
    /** Bumped whenever an object's model index appears/changes/disappears: re-resolves binding targets */
    targetsRev: number;
    appliedNodeQuats: Map<Object3D, Quaternion>;
    /** NODE_MOVE_OFFSET deltas, undone before mixer.update() exactly like appliedNodeQuats */
    appliedNodePosOffsets: Map<Object3D, Vector3>;
    rafId: number | null;
    /** setTimeout used to sleep between frames when the FPS cap is below the display rate */
    timerId: number | null;
    editor: Scene3DEditorSession | null;
    statsFrames: number;
    statsSinceMs: number;
    statsWorkMs: number;
    statsWasSleeping: boolean;
    cancelFrames: () => void;
    pick: (nx: number, ny: number) => string | null;
    pickPart: (nx: number, ny: number) => { objectId: string; node: Object3D } | null;
    lastRenderMs: number;
    lastFrameMs: number;
    curOrbitAz: number;
    curOrbitEl: number;
    dirty: boolean;
    contextLost: boolean;
    wake: () => void;
    handleCursorClick: (nx: number, ny: number) => void;
  } | null>(null);

  // Initialize WebGL2 renderer & scene ONCE per mounted layer
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || engineOff) return;
    onCanvasElRef.current?.(canvas);

    // 3.6: MSAA only when the effective pixel ratio is low; decided at boot (needs a new
    // context to change). 3.3: the buffer is only preserved for thumbnail snapshots (toBlob);
    // the glass backdrop reads the canvas synchronously inside onFrameReady.
    const bootCaps = powerCapsRef.current;
    const bootDpr = isThumbRef.current
      ? 1
      : Math.min(window.devicePixelRatio || 1, bootCaps.dprCap) *
        Math.max(0.5, Math.min(bootCaps.renderScaleCap, configRef.current.renderScale || 0.85));
    const wantMsaa =
      isThumbRef.current ||
      (bootCaps.state !== "ECO" && bootCaps.state !== "ULTRA" && bootDpr < 1.25);
    let renderer: WebGLRenderer;
    try {
      renderer = new WebGLRenderer({
        canvas,
        antialias: wantMsaa,
        alpha: true,
        premultipliedAlpha: true,
        powerPreference: "high-performance",
        preserveDrawingBuffer: isThumbRef.current,
      });
      renderer.setClearColor(0x000000, 0);
    } catch {
      onInspectRef.current?.({
        clipNames: [],
        boneNames: [],
        meshNames: [],
        materialNames: [],
        morphNames: [],
        meshCount: 0,
        triangleCount: 0,
        textureCount: 0,
        maxTextureSize: 0,
        estimatedVramMb: 0,
        warnings: [],
        isDefaultSculpture: true,
        error: "WebGL2 indisponível neste dispositivo",
      });
      return;
    }

    renderer.outputColorSpace = SRGBColorSpace;
    renderer.toneMapping = ACESFilmicToneMapping;
    renderer.toneMappingExposure = configRef.current.exposure || 1.05;

    const scene = new Scene();
    const camera = new PerspectiveCamera(
      configRef.current.camera.fov || 42,
      Math.max(1, width) / Math.max(1, height),
      0.08,
      120
    );

    const pmrem = new PMREMGenerator(renderer);
    pmrem.compileEquirectangularShader();

    const ambientLight = new AmbientLight("#ffffff", 0.4);
    const keyLight = new DirectionalLight("#ffffff", 2.2);
    const rimLight = new PointLight("#c8d4ea", 1.4, 24);
    scene.add(ambientLight, keyLight, rimLight);

    const stagePivot = new Group();
    scene.add(stagePivot);

    // 2.2: loaders are a lazy module singleton (see engine/scene3d/loaders.ts)
    retainSharedLoaders();

    const tmpEuler = new Euler(0, 0, 0, "YXZ");
    const tmpQuat = new Quaternion();
    const tmpInvQuat = new Quaternion();
    const tmpGlowColor = new Color("#dce6ff");
    const rayBox = new Box3();
    const rayHitPoint = new Vector3();
    const rayHitResult: {
      object: Object3D;
      objectId: string;
      face: { a: number; b: number; c: number } | null;
      point: Vector3;
    } = { object: stagePivot, objectId: "", face: null, point: rayHitPoint };

    // ── 3.1/3.2 per-frame reuse: nothing below is allocated inside renderFrame ──
    const DEG2RAD = Math.PI / 180;
    const EMPTY_BINDINGS: ReactiveBinding3D[] = [];
    const nodeRotAccum = new Map<Object3D, Quaternion>();
    const nodePosAccum = new Map<Object3D, Vector3>();
    const nodeScaleAccum = new Map<Object3D, number>();
    const matGlowAccum = new Map<EmissiveMatEntry, number>();
    const morphAccumPerRt = new Map<LoadedObjectRuntime, Map<string, number>>();
    const quatPool: Quaternion[] = [];
    const vecPool: Vector3[] = [];
    let quatPoolUsed = 0;
    let vecPoolUsed = 0;
    let curDt = 0;
    let anyBindingMoving = false;
    // Config-derived data, rebuilt only when the config object (or thumbnail flag) changes
    let cachedCfg: Scene3DConfig | null = null;
    let cachedThumb = false;
    let cachedObjs: Scene3DObject[] = [];
    let cachedCursorBindings: ReactiveBinding3D[] = EMPTY_BINDINGS;
    let cachedSignalBindings: ReactiveBinding3D[] = EMPTY_BINDINGS;
    let cachedHasHoverBinding = false;
    // Targets resolved by objectId + targetName (parallel to the cached binding lists)
    let cachedTargetsRev = -1;
    let cachedSignalTargets: ResolvedBindingTarget[] = [];
    let cachedCursorTargets: ResolvedBindingTarget[] = [];
    // Last-applied camera / exposure state
    let viewCfg: Scene3DConfig | null = null;
    let viewAz = NaN;
    let viewEl = NaN;
    let viewWasEditor = false;
    let lastExposure = NaN;
    let lastEnvIntensity = NaN;
    // Day/night: re-evaluated on config change or every 30 s
    let dnCfg: Scene3DConfig | null = null;
    let dnLastMs = -Infinity;

    const addNodeRotation = (node: Object3D, q: Quaternion) => {
      const acc = nodeRotAccum.get(node);
      if (acc) acc.multiply(q);
      else {
        const slot = quatPool[quatPoolUsed] ?? (quatPool[quatPoolUsed] = new Quaternion());
        quatPoolUsed++;
        nodeRotAccum.set(node, slot.copy(q));
      }
    };
    const addAxisRotation = (node: Object3D, axis: Scene3DAxis, angleRad: number) => {
      if (Math.abs(angleRad) < 1e-6) return;
      tmpQuat.setFromAxisAngle(AXIS_VECTORS[axis] ?? AXIS_VECTORS.Y, angleRad);
      addNodeRotation(node, tmpQuat);
    };
    const addAxisOffset = (
      idx: ModelSceneIndex,
      node: Object3D,
      axis: Scene3DAxis,
      roomUnits: number
    ) => {
      if (Math.abs(roomUnits) < 1e-6) return;
      let acc = nodePosAccum.get(node);
      if (!acc) {
        acc = vecPool[vecPoolUsed] ?? (vecPool[vecPoolUsed] = new Vector3());
        vecPoolUsed++;
        acc.set(0, 0, 0);
        nodePosAccum.set(node, acc);
      }
      acc.addScaledVector(AXIS_VECTORS[axis] ?? AXIS_VECTORS.Y, roomUnits * nodeUnitScale(idx, node));
    };
    const axisOf = (b: ReactiveBinding3D): Scene3DAxis => b.axis ?? "Y";
    const signOf = (b: ReactiveBinding3D) => (b.invertAxis ? -1 : 1);

    let disposed = false;

    const playOverrideClip = (
      rt: LoadedObjectRuntime,
      clipName: string,
      loop: boolean,
      holdHoverId: string | null
    ) => {
      if (!rt.mixer || rt.clips.length === 0) return;
      const clip = rt.clips.find((c) => c.name === clipName) ?? rt.clips[0];
      if (!clip) return;
      if (rt.overrideClipName === clip.name && rt.overrideAction?.isRunning()) {
        rt.overrideHoldHoverId = holdHoverId;
        return;
      }
      const nextAction = rt.mixer.clipAction(clip);
      nextAction.reset();
      nextAction.setLoop(loop ? LoopRepeat : LoopOnce, loop ? Infinity : 1);
      nextAction.clampWhenFinished = !loop;
      nextAction.enabled = true;
      nextAction.setEffectiveWeight(1);

      const prev = rt.overrideAction ?? rt.baseAction;
      if (prev && prev !== nextAction) {
        nextAction.play();
        prev.crossFadeTo(nextAction, 0.22, false);
      } else {
        nextAction.play();
      }
      rt.overrideAction = nextAction;
      rt.overrideClipName = clip.name;
      rt.overrideHoldHoverId = holdHoverId;
    };

    const returnToBaseClip = (rt: LoadedObjectRuntime) => {
      if (!rt.overrideAction) return;
      const prevOverride = rt.overrideAction;
      rt.overrideAction = null;
      rt.overrideClipName = null;
      rt.overrideHoldHoverId = null;

      if (rt.baseAction && rt.baseAction !== prevOverride) {
        rt.baseAction.enabled = true;
        rt.baseAction.setEffectiveWeight(1);
        rt.baseAction.reset().play();
        prevOverride.crossFadeTo(rt.baseAction, 0.28, false);
      } else {
        prevOverride.fadeOut(0.24);
      }
    };

    const applyScalarTarget = (
      b: ReactiveBinding3D,
      target: ResolvedBindingTarget,
      st: BindingRuntimeState,
      rawVal: number,
      bAlpha: number
    ) => {
      const targetRt = target.rt;
      const idx = targetRt?.modelIndex;
      if (!targetRt || !idx) return;
      const node = target.node;
      if (b.targetType === "PLAY_CLIP") {
        const active = rawVal >= 0.35;
        if (active && !st.wasHovered) {
          playOverrideClip(targetRt, b.targetName, true, b.id);
        } else if (!active && st.wasHovered && targetRt.overrideHoldHoverId === b.id) {
          returnToBaseClip(targetRt);
        }
        st.wasHovered = active;
        return;
      }
      const prevWeight = st.weight;
      st.weight += (rawVal - st.weight) * bAlpha;
      // Snap the spring when it is visually settled: the last frame then applies the
      // exact target (no residual offset left on the rig) and a steady signal such as
      // "music playing" or a flat CPU load lets the loop sleep instead of re-rendering.
      if (Math.abs(rawVal - st.weight) < 0.002) st.weight = rawVal;
      if (
        st.weight !== prevWeight ||
        (b.targetType === "NODE_ROTATE_SPIN" && Math.abs(st.weight) > 1e-4)
      ) {
        anyBindingMoving = true;
      }
      if (b.targetType === "NODE_SCALE_PULSE") {
        if (node) {
          nodeScaleAccum.set(node, (nodeScaleAccum.get(node) ?? 0) + st.weight * (b.strength || 0.16));
        }
      } else if (b.targetType === "EMISSIVE_GLOW") {
        for (const m of target.mats) {
          matGlowAccum.set(m, (matGlowAccum.get(m) ?? 0) + st.weight * (b.strength || 1.4));
        }
      } else if (b.targetType === "MORPH_WEIGHT" && b.targetName) {
        let mMap = morphAccumPerRt.get(targetRt);
        if (!mMap) {
          mMap = new Map<string, number>();
          morphAccumPerRt.set(targetRt, mMap);
        }
        mMap.set(
          b.targetName,
          Math.max(mMap.get(b.targetName) ?? 0, st.weight * Math.max(0, Math.min(1, b.strength || 1)))
        );
      } else if (b.targetType === "NODE_ROTATE_SPIN") {
        st.spin = (st.spin + st.weight * (b.strength || 2.0) * curDt) % (Math.PI * 2);
        if (node) addAxisRotation(node, axisOf(b), st.spin * signOf(b));
      } else if (b.targetType === "NODE_ROTATE_SWING") {
        if (node) {
          addAxisRotation(node, axisOf(b), st.weight * (b.strength || 25) * DEG2RAD * signOf(b));
        }
      } else if (b.targetType === "NODE_MOVE_OFFSET") {
        if (node) {
          addAxisOffset(idx, node, axisOf(b), st.weight * (b.strength || 0.15) * signOf(b));
        }
      }
    };

    const resolveBindingTarget = (
      eng: NonNullable<typeof engineRef.current>,
      objs: Scene3DObject[],
      b: ReactiveBinding3D
    ): ResolvedBindingTarget => {
      const ownerId = bindingOwnerId(objs, b.objectId);
      const rt = ownerId ? eng.objectsMap.get(ownerId) ?? null : null;
      const idx = rt?.modelIndex;
      if (!rt || !idx) return UNRESOLVED_TARGET;
      return {
        rt,
        node: idx.nodesByName.get(b.targetName || "__ROOT__") ?? null,
        mats:
          !b.targetName || b.targetName === "*"
            ? idx.emissiveMats
            : idx.emissiveByName.get(b.targetName) ?? EMPTY_MATS,
      };
    };

    /** Rebuilds binding lists on config change and re-resolves targets on load/remove (`targetsRev`). */
    const ensureBindingCache = (eng: NonNullable<typeof engineRef.current>, cfg: Scene3DConfig) => {
      if (cfg !== cachedCfg || cachedThumb !== isThumbRef.current) {
        cachedCfg = cfg;
        cachedThumb = isThumbRef.current;
        cachedObjs = normalizeScene3DObjects(cfg);
        const enabled = cachedThumb ? EMPTY_BINDINGS : (cfg.bindings ?? []).filter((b) => b.enabled);
        cachedCursorBindings = enabled.filter(
          (b) => b.source === "CURSOR_LOOK" || b.source === "HOVER_MESH" || b.source === "CLICK_MESH"
        );
        cachedSignalBindings = enabled.filter(
          (b) =>
            b.source === "AUDIO_BASS" ||
            b.source === "AUDIO_MID" ||
            b.source === "AUDIO_TREBLE" ||
            b.source === "KEY_PULSE" ||
            b.source === "MEDIA_PLAYING" ||
            b.source === "CPU_USAGE"
        );
        cachedHasHoverBinding = cachedCursorBindings.some((b) => b.source === "HOVER_MESH");
        morphAccumPerRt.clear();
        cachedTargetsRev = -1;
      }
      if (cachedTargetsRev !== eng.targetsRev) {
        cachedTargetsRev = eng.targetsRev;
        cachedSignalTargets = cachedSignalBindings.map((b) => resolveBindingTarget(eng, cachedObjs, b));
        cachedCursorTargets = cachedCursorBindings.map((b) => resolveBindingTarget(eng, cachedObjs, b));
        // Runtimes may have been replaced/removed: drop their morph accumulators
        morphAccumPerRt.clear();
      }
    };

    const getBindingState = (
      eng: NonNullable<typeof engineRef.current>,
      id: string
    ): BindingRuntimeState => {
      let st = eng.bindingStates.get(id);
      if (!st) {
        st = { yaw: 0, pitch: 0, spin: 0, weight: 0, pulse: 0, toggled: false, wasHovered: false };
        eng.bindingStates.set(id, st);
      }
      return st;
    };

    const performRaycastAt = (
      eng: NonNullable<typeof engineRef.current>,
      nx: number,
      ny: number
    ): {
      object: Object3D;
      objectId: string;
      face: { a: number; b: number; c: number } | null;
      point: Vector3;
    } | null => {
      if (eng.objectsMap.size === 0) return null;
      eng.camera.updateMatrixWorld();
      eng.rayNdc.set(nx * 2 - 1, -(ny * 2 - 1));
      eng.raycaster.setFromCamera(eng.rayNdc, eng.camera);
      let bestDist = Infinity;
      let bestHit: Object3D | null = null;
      let bestFace: { a: number; b: number; c: number } | null = null;
      let bestObjId = "";
      eng.objectsMap.forEach((rt, objId) => {
        if (!rt.pivot.visible || !rt.modelIndex || rt.modelIndex.meshes.length === 0) return;
        // 3.7: cheap broad-phase — skip per-triangle tests when the ray misses the (padded) bounds
        rayBox.setFromObject(rt.pivot);
        if (!rayBox.isEmpty()) {
          const pad =
            0.1 *
            Math.max(
              rayBox.max.x - rayBox.min.x,
              rayBox.max.y - rayBox.min.y,
              rayBox.max.z - rayBox.min.z
            );
          rayBox.expandByScalar(pad);
          if (!eng.raycaster.ray.intersectsBox(rayBox)) return;
        }
        const hits = eng.raycaster.intersectObjects(rt.modelIndex.meshes, false);
        if (hits.length > 0 && hits[0].distance < bestDist) {
          bestDist = hits[0].distance;
          bestHit = hits[0].object;
          bestFace = hits[0].face ?? null;
          rayHitPoint.copy(hits[0].point);
          bestObjId = objId;
        }
      });
      if (!bestHit) return null;
      rayHitResult.object = bestHit;
      rayHitResult.objectId = bestObjId;
      rayHitResult.face = bestFace;
      return rayHitResult;
    };

    const handleCursorClick = (nx: number, ny: number) => {
      const eng = engineRef.current;
      if (!eng || disposed || eng.contextLost || isThumbRef.current) return;
      if (interactiveRef.current && !testModeRef.current) return;
      if (!powerCapsRef.current.allowCursorBindings) return;
      ensureBindingCache(eng, configRef.current);
      let hasClick = false;
      for (const b of cachedCursorBindings) {
        if (b.source === "CLICK_MESH") {
          hasClick = true;
          break;
        }
      }
      if (!hasClick) return;

      const hit = performRaycastAt(eng, nx, ny);
      if (!hit) return;

      let triggered = false;
      for (let i = 0; i < cachedCursorBindings.length; i++) {
        const b = cachedCursorBindings[i];
        if (b.source !== "CLICK_MESH") continue;
        const targetRt = cachedCursorTargets[i]?.rt;
        // The click must land on the binding's own object (objectId, or objects[0] when unset)
        if (!targetRt || hit.objectId !== targetRt.id) continue;
        if (!matchesMeshTarget(hit.object, b.sourceNode, targetRt.modelRoot)) continue;
        const st = getBindingState(eng, b.id);
        triggered = true;
        if (b.targetType === "PLAY_CLIP") {
          playOverrideClip(targetRt, b.targetName, false, null);
        } else if (
          b.targetType === "NODE_SCALE_PULSE" ||
          b.targetType === "NODE_ROTATE_SPIN" ||
          b.targetType === "NODE_ROTATE_SWING" ||
          b.targetType === "NODE_MOVE_OFFSET"
        ) {
          st.pulse = 1.0;
        } else if (b.targetType === "EMISSIVE_GLOW" || b.targetType === "MORPH_WEIGHT") {
          st.toggled = !st.toggled;
        }
      }
      if (triggered) {
        eng.wake();
      }
    };

    // ── Frame scheduling ────────────────────────────────────────────────────────
    // Below the display refresh rate we really sleep (setTimeout) instead of spinning
    // requestAnimationFrame every vsync just to skip it: at 5–30 FPS that removes most
    // CPU wake-ups, which is where laptop battery actually goes.
    const requestFrame = (eng: NonNullable<typeof engineRef.current>) => {
      if (eng.rafId === null) eng.rafId = requestAnimationFrame(renderFrame);
    };
    const requestFrameAfter = (eng: NonNullable<typeof engineRef.current>, delayMs: number) => {
      if (delayMs <= 4) {
        requestFrame(eng);
        return;
      }
      if (eng.timerId !== null || eng.rafId !== null) return;
      eng.timerId = window.setTimeout(() => {
        const cur = engineRef.current;
        if (!cur || cur !== eng || disposed) return;
        cur.timerId = null;
        requestFrame(cur);
      }, Math.max(0, delayMs - 2));
    };
    const cancelFrames = () => {
      const eng = engineRef.current;
      if (!eng) return;
      if (eng.rafId !== null) {
        cancelAnimationFrame(eng.rafId);
        eng.rafId = null;
      }
      if (eng.timerId !== null) {
        window.clearTimeout(eng.timerId);
        eng.timerId = null;
      }
    };

    /** 0 = no cap (follow the monitor). */
    const frameIntervalMs = (eng: NonNullable<typeof engineRef.current>, cfg: Scene3DConfig) => {
      if (isThumbRef.current) return 0;
      const caps = powerCapsRef.current;
      if (eng.editor) {
        // Sala 3D editor: ignores the scene's maxFps but honours the energy cap
        // (floored at 15 fps so FROZEN/ULTRA stay navigable while editing).
        return caps.fpsCap !== null ? 1000 / Math.max(15, caps.fpsCap) : 0;
      }
      let fps = cfg.fpsMode === "MONITOR" ? Infinity : clampScene3DFps(cfg.maxFps);
      if (caps.fpsCap !== null) fps = Math.min(fps, caps.fpsCap);
      return Number.isFinite(fps) ? 1000 / fps : 0;
    };

    const emitStats = (
      eng: NonNullable<typeof engineRef.current>,
      nowMs: number,
      sleeping: boolean,
      stateLabel: PowerState
    ) => {
      const cb = onStatsRef.current;
      if (!cb || isThumbRef.current) return;
      const span = nowMs - eng.statsSinceMs;
      if (span < 500 && (!sleeping || eng.statsWasSleeping)) return;
      eng.statsWasSleeping = sleeping;
      const info = renderer.info;
      cb({
        fps: sleeping ? 0 : Math.round((eng.statsFrames * 1000) / Math.max(1, span)),
        frameMs:
          eng.statsFrames > 0 ? Math.round((eng.statsWorkMs / eng.statsFrames) * 100) / 100 : 0,
        drawCalls: info.render.calls,
        triangles: info.render.triangles,
        geometries: info.memory.geometries,
        textures: info.memory.textures,
        pixelRatio: Math.round(renderer.getPixelRatio() * 100) / 100,
        powerState: stateLabel,
        sleeping,
      });
      eng.statsFrames = 0;
      eng.statsWorkMs = 0;
      eng.statsSinceMs = nowMs;
    };

    const renderFrame = (nowMs: number) => {
      const eng = engineRef.current;
      if (!eng || disposed || eng.contextLost) return;
      eng.rafId = null;

      if (pausedRef.current && !isThumbRef.current) {
        return;
      }

      const cfg = configRef.current;
      const editor = eng.editor;
      // Thumbnails and the Studio editor always render at full quality
      const caps = isThumbRef.current || editor ? powerCapsFor("NORMAL") : powerCapsRef.current;
      const minInterval = frameIntervalMs(eng, cfg);
      const elapsedSinceDraw = nowMs - eng.lastRenderMs;
      if (minInterval > 0 && elapsedSinceDraw < minInterval - 1.2) {
        // Changes ("dirty") are drawn at the next allowed slot, never above the cap
        requestFrameAfter(eng, minInterval - elapsedSinceDraw);
        return;
      }

      const workStartMs = performance.now();
      const maxDt = Math.min(0.5, Math.max(0.05, (minInterval / 1000) * 1.5));
      const dt = Math.min(maxDt, Math.max(0.001, (nowMs - eng.lastFrameMs) / 1000));
      eng.lastFrameMs = nowMs;
      // Keep the cadence phase so caps that are not divisors of the refresh rate
      // (e.g. 45 FPS on 60 Hz) average out correctly instead of collapsing to 30.
      const overshoot = elapsedSinceDraw - minInterval;
      eng.lastRenderMs =
        minInterval > 0 && overshoot >= 0 && overshoot < minInterval ? nowMs - overshoot : nowMs;
      eng.dirty = false;

      // 1. Undo previously applied bone/part deltas BEFORE AnimationMixer runs so both
      //    animated rigs and static nodes stay 100% drift-free (rotations AND offsets).
      if (eng.appliedNodeQuats.size > 0) {
        eng.appliedNodeQuats.forEach((prevQ, node) => {
          tmpInvQuat.copy(prevQ).invert();
          node.quaternion.multiply(tmpInvQuat);
        });
        eng.appliedNodeQuats.clear();
      }
      if (eng.appliedNodePosOffsets.size > 0) {
        eng.appliedNodePosOffsets.forEach((off, node) => {
          node.position.sub(off);
        });
        eng.appliedNodePosOffsets.clear();
      }

      // 2. Per-object AnimationMixer & Transforms (Room Multi-Object System)
      quatPoolUsed = 0;
      vecPoolUsed = 0;
      curDt = dt;
      ensureBindingCache(eng, cfg);
      const orderedObjs = cachedObjs;
      let anyBasePlaying = false;
      let anyOverridePlaying = false;
      let anyAutoRotate = false;
      let spinKeySum = 0;
      const studioFraming = interactiveRef.current && !testModeRef.current;

      for (const obj of orderedObjs) {
        const rt = eng.objectsMap.get(obj.id);
        if (!rt) continue;

        // While the Sala 3D gizmo drags this object it owns the pivot transform
        const editorOwnsTransform = editor ? editor.isDragging(obj.id) : false;

        rt.pivot.visible = obj.visible !== false;
        if (!editorOwnsTransform) {
          rt.pivot.position.set(
            obj.position?.[0] ?? 0,
            obj.position?.[1] ?? 0,
            obj.position?.[2] ?? 0
          );
          rt.pivot.scale.set(
            Math.max(0.05, obj.scale?.[0] ?? 1),
            Math.max(0.05, obj.scale?.[1] ?? 1),
            Math.max(0.05, obj.scale?.[2] ?? 1)
          );
        }

        const overrideRunning = Boolean(rt.overrideAction && rt.overrideAction.isRunning());
        const basePlaying =
          Boolean(rt.mixer && rt.baseAction && obj.playback.clipName !== "__NONE__") &&
          Math.abs(obj.playback.speed) > 1e-4;
        if (overrideRunning && !caps.frozen) anyOverridePlaying = true;
        if (basePlaying && !caps.frozen) anyBasePlaying = true;

        if (rt.mixer && (basePlaying || overrideRunning)) {
          // Frozen: update(0) still poses the rig on the current clip time
          rt.mixer.update(caps.frozen ? 0 : dt * Math.max(0.05, obj.playback.speed || 1));
          if (rt.overrideAction && !rt.overrideHoldHoverId && !rt.overrideAction.isRunning()) {
            returnToBaseClip(rt);
          }
        }

        // Keep spinAngle at exact 0 while framing in Studio (unless Testar 3D is active)
        const spinWanted =
          Math.abs(obj.autoRotateSpeed) > 1e-4 && !isThumbRef.current && !studioFraming;
        if (spinWanted && caps.allowAutoRotate) {
          rt.spinAngle += obj.autoRotateSpeed * dt;
          anyAutoRotate = true;
        } else if (!spinWanted) {
          rt.spinAngle = 0;
        }
        // (spinWanted but power saver active: hold the current angle, no snap back)
        spinKeySum += rt.spinAngle;

        if (!editorOwnsTransform) {
          rt.pivot.rotation.order = "YXZ";
          rt.pivot.rotation.set(
            (obj.rotation?.[0] ?? 0) * DEG2RAD,
            (obj.rotation?.[1] ?? 0) * DEG2RAD + rt.spinAngle,
            (obj.rotation?.[2] ?? 0) * DEG2RAD
          );
        }

        if (rt.defaultRefs) {
          rt.defaultRefs.tick(rt.spinAngle);
        }
      }

      // 4. Live cursor input (active on Desktop OR inside Studio when "Testar 3D" is ON)
      const cursorEnabled =
        !isThumbRef.current && !editor && (!interactiveRef.current || testModeRef.current);
      const rawCursor = cursorEnabled ? cursorNormRef?.current ?? null : null;
      const cx = rawCursor ? (rawCursor[0] - 0.5) * 2 : 0; // -1..+1
      const cy = rawCursor ? (rawCursor[1] - 0.5) * 2 : 0; // -1..+1

      let orbitSettled = true;
      const nextFov = Math.max(20, Math.min(85, cfg.camera.fov || 42));
      let projDirty = false;
      if (camera.fov !== nextFov) {
        camera.fov = nextFov;
        projDirty = true;
      }
      if (editor) {
        // Sala 3D: the free-fly editor camera owns position & orientation
        editor.update(dt);
        viewWasEditor = true;
      } else {
        const useCursorOrbit =
          cursorEnabled &&
          caps.allowCursorOrbit &&
          (Math.abs(cfg.camera.orbitParallaxX) > 1e-3 || Math.abs(cfg.camera.orbitParallaxY) > 1e-3);
        const targetAz = useCursorOrbit ? cx * cfg.camera.orbitParallaxX : 0;
        const targetEl = useCursorOrbit ? -cy * cfg.camera.orbitParallaxY : 0;
        const damp = Math.max(0.02, Math.min(0.4, cfg.camera.damping || 0.08));
        const alpha = 1 - Math.exp(-damp * 60 * dt);
        eng.curOrbitAz += (targetAz - eng.curOrbitAz) * alpha;
        eng.curOrbitEl += (targetEl - eng.curOrbitEl) * alpha;

        orbitSettled =
          Math.abs(targetAz - eng.curOrbitAz) < 0.01 && Math.abs(targetEl - eng.curOrbitEl) < 0.01;

        if (
          viewWasEditor ||
          viewCfg !== cfg ||
          viewAz !== eng.curOrbitAz ||
          viewEl !== eng.curOrbitEl
        ) {
          viewWasEditor = false;
          viewCfg = cfg;
          viewAz = eng.curOrbitAz;
          viewEl = eng.curOrbitEl;
          applyConfigCamera(cfg, eng.curOrbitAz, eng.curOrbitEl, camera);
        }
      }
      if (projDirty) camera.updateProjectionMatrix();

      // 5. Reactive Bindings:
      //    - Cursor bindings (CURSOR_LOOK / HOVER_MESH / CLICK_MESH) run on Desktop OR Studio Test Mode
      //    - Signal bindings (AUDIO_* / KEY_PULSE / MEDIA_PLAYING / CPU_USAGE) run live everywhere (even Studio!)
      //    - The energy governor can switch either group off (Ultra / Frozen)
      const cursorBindings =
        cursorEnabled && caps.allowCursorBindings ? cachedCursorBindings : EMPTY_BINDINGS;
      // Sala 3D (editor mounted) edits the room: sensor-driven bindings stay off
      const signalBindings =
        caps.allowSignalBindings && !editor ? cachedSignalBindings : EMPTY_BINDINGS;
      const needHoverRay =
        cursorEnabled &&
        caps.allowCursorBindings &&
        (testModeRef.current || cachedHasHoverBinding);

      if (needHoverRay && rawCursor && eng.objectsMap.size > 0) {
        const rx = Math.round(rawCursor[0] * 1000);
        const ry = Math.round(rawCursor[1] * 1000);
        const rs = Math.round(spinKeySum * 100);
        if (rx !== eng.lastRayX || ry !== eng.lastRayY || rs !== eng.lastRaySpin) {
          eng.lastRayX = rx;
          eng.lastRayY = ry;
          eng.lastRaySpin = rs;
          const rayHit = performRaycastAt(eng, rawCursor[0], rawCursor[1]);
          eng.hoveredObject = rayHit?.object ?? null;
          eng.hoveredObjectId = rayHit?.objectId ?? null;
          const nextName = eng.hoveredObject?.name || null;
          if (nextName !== eng.lastHoveredName) {
            eng.lastHoveredName = nextName;
            onHoverMeshRef.current?.(nextName);
          }
        }
      } else if (!needHoverRay && eng.hoveredObject) {
        eng.hoveredObject = null;
        eng.hoveredObjectId = null;
        if (eng.lastHoveredName !== null) {
          eng.lastHoveredName = null;
          onHoverMeshRef.current?.(null);
        }
      }

      // Automatic Day/Night Solar Lighting Cycle
      if (cfg.autoDayNight) {
        if (dnCfg !== cfg || nowMs - dnLastMs >= 29_000) {
          dnCfg = cfg;
          dnLastMs = nowMs;
          applyDayNightLighting(dayNightHourFor(cfg), eng.scene, eng.ambientLight, eng.keyLight, eng.rimLight, cfg);
        }
      } else {
        dnCfg = null;
      }

      anyBindingMoving = false;
      nodeRotAccum.clear();
      nodePosAccum.clear();
      nodeScaleAccum.clear();
      matGlowAccum.clear();
      morphAccumPerRt.forEach((m) => m.clear());
      const sig = reactiveSignalRef?.current ?? DEFAULT_REACTIVE_SIGNAL;
      if (eng.objectsMap.size > 0) {
        for (let i = 0; i < signalBindings.length; i++) {
          const b = signalBindings[i];
          const target = cachedSignalTargets[i];
          if (!target?.rt) continue;
          const st = getBindingState(eng, b.id);
          const bAlpha = 1 - Math.exp(-Math.max(0.02, Math.min(0.45, b.smoothing || 0.18)) * 60 * dt);
          const rawVal =
            b.source === "AUDIO_BASS"
              ? sig.bass
              : b.source === "AUDIO_MID"
              ? sig.mid
              : b.source === "AUDIO_TREBLE"
              ? sig.treble
              : b.source === "KEY_PULSE"
              ? sig.key_pulse
              : b.source === "MEDIA_PLAYING"
              ? mediaPlayingRef.current
                ? 1
                : 0
              : Math.max(0, Math.min(1, (cpuPercentRef.current || 0) / 100));
          applyScalarTarget(b, target, st, rawVal, bAlpha);
        }

        for (let i = 0; i < cursorBindings.length; i++) {
          const b = cursorBindings[i];
          const target = cachedCursorTargets[i];
          const targetRt = target?.rt;
          const idx = targetRt?.modelIndex;
          if (!target || !targetRt || !idx) continue;
          const node = target.node;
          const st = getBindingState(eng, b.id);
          const bAlpha = 1 - Math.exp(-Math.max(0.02, Math.min(0.45, b.smoothing || 0.12)) * 60 * dt);

          if (b.source === "CURSOR_LOOK" && b.targetType === "NODE_LOOK_AT") {
            const maxRad = (Math.max(0, Math.min(75, b.strength || 20)) * Math.PI) / 180;
            const tYaw = (b.invertX ? -cx : cx) * maxRad;
            const tPitch = (b.invertY ? -cy : cy) * maxRad * 0.72;
            const prevYaw = st.yaw;
            const prevPitch = st.pitch;
            st.yaw += (tYaw - st.yaw) * bAlpha;
            st.pitch += (tPitch - st.pitch) * bAlpha;
            if (Math.abs(tYaw - st.yaw) < 0.001) st.yaw = tYaw;
            if (Math.abs(tPitch - st.pitch) < 0.001) st.pitch = tPitch;
            if (st.yaw !== prevYaw || st.pitch !== prevPitch) {
              anyBindingMoving = true;
            }
            if (node && (Math.abs(st.yaw) > 1e-6 || Math.abs(st.pitch) > 1e-6)) {
              tmpEuler.set(st.pitch, st.yaw, 0, "YXZ");
              tmpQuat.setFromEuler(tmpEuler);
              addNodeRotation(node, tmpQuat);
            }
            continue;
          }

          if (b.source === "HOVER_MESH") {
            // Hover counts only on the binding's own object (objectId, or objects[0] when unset)
            const isHovered =
              eng.hoveredObjectId === targetRt.id &&
              matchesMeshTarget(eng.hoveredObject, b.sourceNode, targetRt.modelRoot);
            applyScalarTarget(b, target, st, isHovered ? 1 : 0, bAlpha);
            continue;
          }

          if (b.source === "CLICK_MESH") {
            if (
              b.targetType === "NODE_SCALE_PULSE" ||
              b.targetType === "NODE_ROTATE_SPIN" ||
              b.targetType === "NODE_ROTATE_SWING" ||
              b.targetType === "NODE_MOVE_OFFSET"
            ) {
              if (st.pulse > 0.002) {
                st.pulse *= Math.exp(-Math.max(2, (b.smoothing || 0.15) * 45) * dt);
                anyBindingMoving = true;
              } else {
                st.pulse = 0;
              }
              if (node && st.pulse > 0.001) {
                if (b.targetType === "NODE_SCALE_PULSE") {
                  nodeScaleAccum.set(node, (nodeScaleAccum.get(node) ?? 0) + st.pulse * (b.strength || 0.16));
                } else if (b.targetType === "NODE_ROTATE_SPIN") {
                  st.spin = (st.spin + st.pulse * (b.strength || 4.0) * dt) % (Math.PI * 2);
                  addAxisRotation(node, axisOf(b), st.spin * signOf(b));
                } else if (b.targetType === "NODE_ROTATE_SWING") {
                  addAxisRotation(node, axisOf(b), st.pulse * (b.strength || 25) * DEG2RAD * signOf(b));
                } else {
                  addAxisOffset(idx, node, axisOf(b), st.pulse * (b.strength || 0.15) * signOf(b));
                }
              }
            } else if (b.targetType === "EMISSIVE_GLOW" || b.targetType === "MORPH_WEIGHT") {
              applyScalarTarget(b, target, st, st.toggled ? 1 : 0, bAlpha);
            }
          }
        }

        // Apply look-at / swing / spin rotations and move offsets AFTER the mixer
        nodeRotAccum.forEach((q, node) => {
          node.quaternion.multiply(q);
          eng.appliedNodeQuats.set(node, q);
        });
        nodePosAccum.forEach((off, node) => {
          node.position.add(off);
          eng.appliedNodePosOffsets.set(node, off);
        });

        // Apply base + delta scales, emissive glow & morphs cleanly across every runtime object
        eng.objectsMap.forEach((rt) => {
          const idx = rt.modelIndex;
          if (!idx) return;
          idx.baseScales.forEach((base, node) => {
            const delta = nodeScaleAccum.get(node) ?? 0;
            const mult = Math.max(0.2, 1 + delta);
            node.scale.set(base.x * mult, base.y * mult, base.z * mult);
          });

          for (const entry of idx.emissiveMats) {
            const addGlow = matGlowAccum.get(entry) ?? 0;
            if (entry.hasDynamicEmissiveMap) {
              entry.mat.emissive.setHex(0xffffff);
              entry.mat.emissiveIntensity = Math.max(0.95, entry.baseIntensity) + addGlow;
            } else if (addGlow > 0.002) {
              if (!entry.hadEmissiveColor) {
                entry.mat.emissive.copy(tmpGlowColor);
              }
              entry.mat.emissiveIntensity = entry.baseIntensity + addGlow;
            } else {
              if (!entry.hadEmissiveColor) {
                entry.mat.emissive.setHex(entry.baseColorHex);
              }
              entry.mat.emissiveIntensity = entry.baseIntensity;
            }
          }

          const morphAccum = morphAccumPerRt.get(rt);
          if (morphAccum) {
            idx.morphTargets.forEach((targets, mName) => {
              if (morphAccum.has(mName)) {
                const w = morphAccum.get(mName) ?? 0;
                for (const { mesh, index } of targets) {
                  if (mesh.morphTargetInfluences) {
                    mesh.morphTargetInfluences[index] = w;
                  }
                }
              }
            });
          }
        });
      }

      const nextExposure = Math.max(0.2, Math.min(3, cfg.exposure || 1.05));
      if (nextExposure !== lastExposure) {
        lastExposure = nextExposure;
        renderer.toneMappingExposure = nextExposure;
      }
      const nextEnvInt = Math.max(0, Math.min(3, cfg.envIntensity ?? 1.2));
      if (nextEnvInt !== lastEnvIntensity) {
        lastEnvIntensity = nextEnvInt;
        scene.environmentIntensity = nextEnvInt;
      }

      renderer.render(scene, camera);
      onFrameReadyRef.current?.();
      eng.statsFrames += 1;
      eng.statsWorkMs += performance.now() - workStartMs;

      // Demand-driven sleep: stop scheduling completely (0% GPU) when nothing moves
      if (isThumbRef.current) return;
      const keepGoing =
        !caps.frozen &&
        (anyBasePlaying ||
          anyOverridePlaying ||
          anyAutoRotate ||
          !orbitSettled ||
          anyBindingMoving ||
          (editor ? editor.needsFrames() : false));
      const hudPowerState = powerCapsRef.current.state;
      if (keepGoing) {
        emitStats(eng, nowMs, false, hudPowerState);
        requestFrameAfter(eng, eng.lastRenderMs + minInterval - performance.now());
      } else {
        emitStats(eng, nowMs, true, hudPowerState);
      }
    };

    const wake = () => {
      const eng = engineRef.current;
      if (!eng || disposed || eng.contextLost) return;
      eng.dirty = true;
      if (pausedRef.current && !isThumbRef.current) return;
      if (eng.rafId === null && eng.timerId === null) {
        eng.lastFrameMs = performance.now();
        requestFrame(eng);
      }
    };

    engineRef.current = {
      renderer,
      scene,
      camera,
      pmrem,
      envTarget: null,
      envPresetLoaded: null,
      ambientLight,
      keyLight,
      rimLight,
      stagePivot,
      objectsMap: new Map(),
      texLoader: new TextureLoader(),
      dynamicTexCache: new Map(),
      raycaster: new Raycaster(),
      rayNdc: new Vector2(),
      lastRayX: NaN,
      lastRayY: NaN,
      lastRaySpin: NaN,
      hoveredObject: null,
      hoveredObjectId: null,
      lastHoveredName: null,
      bindingStates: new Map(),
      targetsRev: 0,
      appliedNodeQuats: new Map(),
      appliedNodePosOffsets: new Map(),
      rafId: null,
      timerId: null,
      editor: null,
      statsFrames: 0,
      statsSinceMs: performance.now(),
      statsWorkMs: 0,
      statsWasSleeping: false,
      cancelFrames,
      pick: (nx, ny) => {
        const eng = engineRef.current;
        if (!eng) return null;
        return performRaycastAt(eng, nx, ny)?.objectId ?? null;
      },
      pickPart: (nx, ny) => {
        const eng = engineRef.current;
        if (!eng) return null;
        const hit = performRaycastAt(eng, nx, ny);
        if (!hit) return null;
        const rt = eng.objectsMap.get(hit.objectId);
        if (!rt?.modelIndex) return null;
        const node = resolvePickedPartNode(rt.modelIndex, hit.object, hit.face, hit.point);
        return { objectId: hit.objectId, node };
      },
      lastRenderMs: 0,
      lastFrameMs: performance.now(),
      curOrbitAz: 0,
      curOrbitEl: 0,
      dirty: true,
      contextLost: false,
      wake,
      handleCursorClick,
    };

    const onContextLost = (ev: Event) => {
      ev.preventDefault();
      if (engineRef.current) {
        engineRef.current.contextLost = true;
        cancelFrames();
      }
    };
    const onContextRestored = () => {
      const eng = engineRef.current;
      if (!eng) return;
      eng.contextLost = false;

      // The PMREM render target has no CPU copy: after a context loss it is black/empty.
      // Force a rebuild of the procedural environment with the current preset.
      try {
        eng.envTarget?.dispose();
        eng.envTarget = null;
        eng.envPresetLoaded = null;
        const cfg = configRef.current;
        const preset = cfg.envPreset || "STUDIO_DARK";
        const target = buildProceduralEnvironment(eng.pmrem, preset);
        eng.envTarget = target;
        eng.envPresetLoaded = preset;
        eng.scene.environment = target.texture;
      } catch {
        // Leave envPresetLoaded = null so the env effect retries on the next preset/config change.
        eng.scene.environment = null;
      }
      // Background mode survives the context loss (clear colour/alpha are re-applied by three on restore).
      eng.renderer.setClearColor(0x000000, 0);
      syncSceneBackground(eng.scene, configRef.current);

      // Cached dynamic textures (album cover / custom image) must be re-uploaded to the new context.
      eng.dynamicTexCache.forEach((tex) => {
        tex.needsUpdate = true;
      });
      eng.wake();
    };
    canvas.addEventListener("webglcontextlost", onContextLost);
    canvas.addEventListener("webglcontextrestored", onContextRestored);

    return () => {
      disposed = true;
      canvas.removeEventListener("webglcontextlost", onContextLost);
      canvas.removeEventListener("webglcontextrestored", onContextRestored);
      onCanvasElRef.current?.(null);
      const eng = engineRef.current;
      engineRef.current = null;
      if (eng) {
        if (eng.rafId !== null) cancelAnimationFrame(eng.rafId);
        if (eng.timerId !== null) window.clearTimeout(eng.timerId);
        eng.editor?.dispose();
        eng.editor = null;
        const sharedTextures = new Set(eng.dynamicTexCache.values());
        eng.objectsMap.forEach((rt) => {
          rt.loadToken++;
          rt.loadKey = "";
          rt.mixer?.stopAllAction();
          if (rt.mixer) rt.mixer.uncacheRoot(rt.mixer.getRoot());
          rt.mixer = null;
          restoreBaseMaps(rt.modelIndex);
          if (rt.modelRoot) {
            rt.pivot.remove(rt.modelRoot);
            disposeObject3D(rt.modelRoot, sharedTextures);
            rt.modelRoot = null;
          }
          rt.pivot.parent?.remove(rt.pivot);
          // per-object loader reference (taken when the runtime was created)
          releaseSharedLoaders();
        });
        eng.objectsMap.clear();
        eng.dynamicTexCache.forEach((tex) => tex.dispose());
        eng.dynamicTexCache.clear();
        eng.envTarget?.dispose();
        eng.pmrem.dispose();
        releaseSharedLoaders();
        eng.renderer.dispose();
        // 2.3: deep sleep → really free the GPU context (a disposed renderer keeps it alive)
        if (sleepingRef.current) eng.renderer.forceContextLoss();
      }
    };
  }, [engineOff, canvasGen]);

  // Resize WebGL buffer with dynamic renderScale
  useEffect(() => {
    const eng = engineRef.current;
    if (!eng) return;
    const safeW = Math.max(16, Math.round(width));
    const safeH = Math.max(16, Math.round(height));
    // Editor honours the energy DPR caps; FROZEN would leave the Sala 3D unusable
    const caps = powerCapsFor(editorWanted && powerState === "FROZEN" ? "NORMAL" : powerState);
    const dpr = isThumbnail
      ? 1
      : Math.min(window.devicePixelRatio || 1, caps.dprCap) *
        Math.max(0.5, Math.min(caps.renderScaleCap, config.renderScale || 0.85));
    eng.renderer.setPixelRatio(dpr);
    eng.renderer.setSize(safeW, safeH, false);
    eng.camera.aspect = safeW / safeH;
    eng.camera.updateProjectionMatrix();
    eng.wake();
  }, [width, height, config.renderScale, isThumbnail, powerState, editorWanted, canvasGen]);

  // Rebuild lightweight procedural IBL environment & background mode
  useEffect(() => {
    const eng = engineRef.current;
    if (!eng) return;
    const cfg = configRef.current;
    const preset = cfg.envPreset || "STUDIO_DARK";
    if (eng.envPresetLoaded === preset && eng.envTarget) {
      if (!cfg.autoDayNight) {
        const spec = ENV_SPECS[preset] ?? ENV_SPECS.STUDIO_DARK;
        eng.ambientLight.color.set(spec.ambientHex);
        eng.ambientLight.intensity = spec.ambientIntensity;
        eng.keyLight.color.set(spec.keyHex);
        eng.keyLight.intensity = spec.keyIntensity;
        eng.keyLight.position.set(spec.keyPos[0], spec.keyPos[1], spec.keyPos[2]);
        eng.rimLight.color.set(spec.rimHex);
        eng.rimLight.intensity = spec.rimIntensity;
        eng.rimLight.position.set(spec.rimPos[0], spec.rimPos[1], spec.rimPos[2]);
      }
      syncSceneBackground(eng.scene, cfg);
      eng.wake();
      return;
    }

    eng.envTarget?.dispose();
    const spec = ENV_SPECS[preset] ?? ENV_SPECS.STUDIO_DARK;
    const target = buildProceduralEnvironment(eng.pmrem, preset);
    eng.envTarget = target;
    eng.envPresetLoaded = preset;

    syncSceneBackground(eng.scene, cfg);
    eng.scene.environment = target.texture;
    eng.ambientLight.color.set(spec.ambientHex);
    eng.ambientLight.intensity = spec.ambientIntensity;
    eng.keyLight.color.set(spec.keyHex);
    eng.keyLight.intensity = spec.keyIntensity;
    eng.keyLight.position.set(spec.keyPos[0], spec.keyPos[1], spec.keyPos[2]);
    eng.rimLight.color.set(spec.rimHex);
    eng.rimLight.intensity = spec.rimIntensity;
    eng.rimLight.position.set(spec.rimPos[0], spec.rimPos[1], spec.rimPos[2]);
    eng.wake();
  }, [config.envPreset, config.autoDayNight, config.backgroundMode, config.backgroundColor, canvasGen]);

  // 3.5: derived keys are memoized on the config object (stable between telemetry ticks)
  const normalizedObjects = useMemo(() => normalizeScene3DObjects(config), [config]);
  const objectsSourceKey = useMemo(
    () =>
      normalizedObjects
        .map((o) => `${o.id}|${o.kind}|${o.modelSrc}|${o.showcaseVariant ?? ""}`)
        .join(";;"),
    [normalizedObjects]
  );

  // Diff-based multi-object loader (loads/disposes only changed objects, zero flicker for siblings)
  useEffect(() => {
    const eng = engineRef.current;
    if (!eng) return;

    const orderedObjs = normalizeScene3DObjects(configRef.current);
    const keepIds = new Set(orderedObjs.map((o) => o.id));

    /** Drops applied rotation AND move-offset deltas of nodes under `pivot` (model being disposed) */
    const purgeObjectQuats = (pivot: Group) => {
      const toDelete: Object3D[] = [];
      const collect = (_: unknown, node: Object3D) => {
        let cur: Object3D | null = node;
        while (cur) {
          if (cur === pivot) {
            toDelete.push(node);
            break;
          }
          cur = cur.parent;
        }
      };
      eng.appliedNodeQuats.forEach(collect);
      eng.appliedNodePosOffsets.forEach(collect);
      for (const n of toDelete) {
        eng.appliedNodeQuats.delete(n);
        eng.appliedNodePosOffsets.delete(n);
      }
    };

    const clearRuntimeModel = (rt: LoadedObjectRuntime) => {
      // 1.4: hand the original maps back to the materials BEFORE disposing, so a dynamic texture
      // (album cover) sitting in `map` doesn't hide the glTF's own texture from disposeObject3D.
      restoreBaseMaps(rt.modelIndex);
      rt.mixer?.stopAllAction();
      // 2.8: drop the mixer's cached actions/bindings for this root
      if (rt.mixer) rt.mixer.uncacheRoot(rt.mixer.getRoot());
      rt.mixer = null;
      rt.baseAction = null;
      rt.overrideAction = null;
      rt.overrideClipName = null;
      rt.overrideHoldHoverId = null;
      rt.activeClipKey = "";
      rt.clips = [];
      rt.defaultRefs = null;
      if (rt.modelIndex) eng.targetsRev++;
      rt.modelIndex = null;
      rt.spinAngle = 0;
      rt.error = null;
      rt.pivot.rotation.set(0, 0, 0);
      purgeObjectQuats(rt.pivot);
      if (eng.hoveredObjectId === rt.id) {
        eng.hoveredObject = null;
        eng.hoveredObjectId = null;
        if (eng.lastHoveredName !== null) {
          eng.lastHoveredName = null;
          onHoverMeshRef.current?.(null);
        }
      }
      if (rt.modelRoot) {
        rt.pivot.remove(rt.modelRoot);
        // dynamicTexCache textures are shared between objects/models: never dispose them here.
        disposeObject3D(rt.modelRoot, new Set(eng.dynamicTexCache.values()));
        rt.modelRoot = null;
      }
    };

    let syncChanged = false;

    // Dispose removed objects (in-flight loads are invalidated by the token bump and disposed on arrival)
    Array.from(eng.objectsMap.entries()).forEach(([id, rt]) => {
      if (!keepIds.has(id)) {
        rt.loadToken++;
        rt.loadKey = "";
        clearRuntimeModel(rt);
        // Reparent any remaining child pivots back to stagePivot before removing this pivot
        while (rt.pivot.children.length > 0) {
          eng.stagePivot.add(rt.pivot.children[0]);
        }
        rt.pivot.parent?.remove(rt.pivot);
        eng.objectsMap.delete(id);
        releaseSharedLoaders();
        syncChanged = true;
      }
    });

    const emitInspect = () => {
      const curObjs = normalizeScene3DObjects(configRef.current);
      onInspectRef.current?.(buildAggregatedInspectInfo(eng.objectsMap, curObjs));
    };

    for (const obj of orderedObjs) {
      let rt = eng.objectsMap.get(obj.id);
      if (!rt) {
        const pivot = new Group();
        pivot.name = `ObjPivot_${obj.id}`;
        eng.stagePivot.add(pivot);
        rt = {
          id: obj.id,
          sourceKey: "",
          loadKey: "",
          loadToken: 0,
          pivot,
          modelRoot: null,
          modelIndex: null,
          defaultRefs: null,
          mixer: null,
          clips: [],
          baseAction: null,
          overrideAction: null,
          overrideClipName: null,
          overrideHoldHoverId: null,
          activeClipKey: "",
          spinAngle: 0,
          error: null,
        };
        eng.objectsMap.set(obj.id, rt);
        // one shared-loader reference per live object runtime (released on removal / engine dispose)
        retainSharedLoaders();
        syncChanged = true;
      }

      const nextKey = `${obj.kind}|${obj.modelSrc}|${obj.showcaseVariant ?? ""}`;
      if (rt.loadKey === nextKey) {
        continue;
      }
      if (rt.sourceKey === nextKey && rt.modelRoot) {
        continue;
      }

      clearRuntimeModel(rt);
      const rawSrc = obj.kind === "GLB" ? (obj.modelSrc || "").trim() : "";

      const mountProcedural = (errMsg: string | null) => {
        clearRuntimeModel(rt!);
        const sculp = buildShowcaseSculpture(obj.showcaseVariant ?? "MONOLITH");
        rt!.pivot.add(sculp.group);
        sculp.group.updateMatrixWorld(true);
        rt!.modelRoot = sculp.group;
        rt!.defaultRefs = sculp;
        rt!.modelIndex = buildModelIndex(obj.id, sculp.group, [], true);
        eng.targetsRev++;
        rt!.error = errMsg;
        rt!.sourceKey = nextKey;
        rt!.loadKey = "";
        syncChanged = true;
      };

      if (!rawSrc) {
        mountProcedural(null);
        continue;
      }

      const url = resolveMediaUrl(rawSrc);
      if (!url) {
        mountProcedural("Caminho do modelo inválido");
        continue;
      }

      const myToken = ++rt.loadToken;
      rt.sourceKey = "";
      rt.loadKey = nextKey;
      syncChanged = true; // previous model was cleared: refresh inspect/stats while the new one loads
      const capturedRt = rt;

      // 1.6: this load is intentionally NOT cancelled when the effect re-runs. `loadingKey` marks it
      // as in flight, so a re-run with the same key skips it (no second download / parse). It is
      // invalidated only by a newer load (`++loadToken`), by object removal, or by engine disposal.
      getSharedGltfLoader(eng.renderer).load(
        url,
        (gltf) => {
          if (
            engineRef.current !== eng ||
            capturedRt.loadToken !== myToken ||
            eng.objectsMap.get(obj.id) !== capturedRt
          ) {
            disposeObject3D(gltf.scene);
            return;
          }
          clearRuntimeModel(capturedRt);

          const holder = new Group();
          holder.name = `ModelRoot_${obj.id}`;
          const root = gltf.scene;
          root.updateMatrixWorld(true);
          const box = new Box3().setFromObject(root);
          const boxSize = new Vector3();
          const boxCenter = new Vector3();
          box.getSize(boxSize);
          box.getCenter(boxCenter);
          const maxDim = Math.max(boxSize.x, boxSize.y, boxSize.z);
          if (Number.isFinite(maxDim) && maxDim > 1e-4) {
            const normScale = 2.4 / maxDim;
            root.scale.multiplyScalar(normScale);
            root.position.sub(boxCenter.multiplyScalar(normScale));
          }

          holder.add(root);
          capturedRt.pivot.add(holder);
          // Refresh world matrices after normalization so nodeUnitScale() reads the true scale on frame 1
          holder.updateMatrixWorld(true);
          capturedRt.modelRoot = holder;
          capturedRt.clips = gltf.animations ?? [];
          if (capturedRt.clips.length > 0) {
            capturedRt.mixer = new AnimationMixer(root);
          }

          capturedRt.modelIndex = buildModelIndex(obj.id, holder, capturedRt.clips, false);
          eng.targetsRev++;
          capturedRt.sourceKey = nextKey;
          capturedRt.loadKey = "";

          const latestObj =
            normalizeScene3DObjects(configRef.current).find((x) => x.id === obj.id) ?? obj;
          const desired = latestObj.playback.clipName;
          if (capturedRt.mixer && capturedRt.clips.length > 0 && desired !== "__NONE__") {
            const chosen = capturedRt.clips.find((c) => c.name === desired) ?? capturedRt.clips[0];
            const action = capturedRt.mixer.clipAction(chosen);
            action.setLoop(latestObj.playback.loop ? LoopRepeat : LoopOnce, Infinity);
            action.clampWhenFinished = !latestObj.playback.loop;
            action.reset().play();
            capturedRt.baseAction = action;
            capturedRt.activeClipKey = `${chosen.name}|${latestObj.playback.loop}`;
          }

          emitInspect();
          setModelLoadRev((r) => r + 1);
          eng.wake();
        },
        undefined,
        (err) => {
          if (
            engineRef.current !== eng ||
            capturedRt.loadToken !== myToken ||
            eng.objectsMap.get(obj.id) !== capturedRt
          ) {
            return;
          }
          const msg = err instanceof Error ? err.message : "Falha ao carregar modelo .glb";
          clearRuntimeModel(capturedRt);
          const sculp = buildShowcaseSculpture("MONOLITH");
          capturedRt.pivot.add(sculp.group);
          capturedRt.modelRoot = sculp.group;
          capturedRt.defaultRefs = sculp;
          capturedRt.modelIndex = buildModelIndex(obj.id, sculp.group, [], true);
          eng.targetsRev++;
          capturedRt.error = msg;
          capturedRt.sourceKey = nextKey;
          capturedRt.loadKey = "";
          emitInspect();
          setModelLoadRev((r) => r + 1);
          eng.wake();
        }
      );
    }

    if (syncChanged) {
      emitInspect();
      setModelLoadRev((r) => r + 1);
      eng.wake();
    }

  }, [objectsSourceKey, canvasGen]);

  const playbackSyncKey = useMemo(
    () =>
      normalizedObjects
        .map((o) => `${o.id}:${o.playback.clipName}:${o.playback.loop}:${o.playback.speed}`)
        .join("|"),
    [normalizedObjects]
  );

  // Sync per-object base Animation Clip selection / loop mode without reloading models
  useEffect(() => {
    const eng = engineRef.current;
    if (!eng) return;
    const orderedObjs = normalizeScene3DObjects(configRef.current);
    for (const obj of orderedObjs) {
      const rt = eng.objectsMap.get(obj.id);
      if (!rt || !rt.mixer || rt.clips.length === 0) continue;
      const desired = obj.playback.clipName;
      if (desired === "__NONE__") {
        rt.baseAction?.stop();
        rt.baseAction = null;
        rt.activeClipKey = "__NONE__";
        continue;
      }
      const chosen = rt.clips.find((c) => c.name === desired) ?? rt.clips[0];
      const nextKey = `${chosen.name}|${obj.playback.loop}`;
      if (rt.activeClipKey !== nextKey) {
        rt.baseAction?.stop();
        const action = rt.mixer.clipAction(chosen);
        action.setLoop(obj.playback.loop ? LoopRepeat : LoopOnce, Infinity);
        action.clampWhenFinished = !obj.playback.loop;
        action.reset().play();
        rt.baseAction = action;
        rt.activeClipKey = nextKey;
      }
    }
    eng.wake();
  }, [playbackSyncKey, modelLoadRev, canvasGen]);

  const objectsTransformKey = useMemo(
    () =>
      normalizedObjects
        .map(
          (o) =>
            `${o.id}:${o.parentId ?? ""}:${o.visible}:${o.autoRotateSpeed}:${o.position.join(",")}:${o.rotation.join(",")}:${o.scale.join(",")}`
        )
        .join("|"),
    [normalizedObjects]
  );

  const objectsParentKey = useMemo(
    () => normalizedObjects.map((o) => `${o.id}>${o.parentId ?? ""}`).join("|"),
    [normalizedObjects]
  );

  // Sync object parentId hierarchy (mirrors Rust SceneGraph; cyclic/missing parentId falls back to stagePivot)
  useEffect(() => {
    const eng = engineRef.current;
    if (!eng) return;
    const orderedObjs = normalizeScene3DObjects(configRef.current);
    const parentById = new Map<string, string | undefined>();
    for (const o of orderedObjs) parentById.set(o.id, o.parentId);
    const hasCycle = (id: string): boolean => {
      let cur = parentById.get(id);
      for (let steps = 0; cur && steps <= orderedObjs.length; steps++) {
        if (cur === id) return true;
        cur = parentById.get(cur);
      }
      return false;
    };
    for (const obj of orderedObjs) {
      const rt = eng.objectsMap.get(obj.id);
      if (!rt) continue;
      const pId = obj.parentId;
      const parentRt = pId && pId !== obj.id && !hasCycle(obj.id) ? eng.objectsMap.get(pId) : undefined;
      const desiredParent: Object3D = parentRt ? parentRt.pivot : eng.stagePivot;
      if (rt.pivot.parent !== desiredParent) {
        desiredParent.add(rt.pivot);
      }
    }
    eng.wake();
  }, [objectsParentKey, objectsSourceKey, canvasGen]);
  // 3.4: telemetry only wakes the renderer when an enabled binding actually reads it
  const usesCpuBinding = useMemo(
    () => (config.bindings ?? []).some((b) => b.enabled && b.source === "CPU_USAGE"),
    [config.bindings]
  );
  const usesMediaBinding = useMemo(
    () => (config.bindings ?? []).some((b) => b.enabled && b.source === "MEDIA_PLAYING"),
    [config.bindings]
  );

  // Wake renderer whenever visual/camera/bindings/testMode/transforms or pause state change
  useEffect(() => {
    const eng = engineRef.current;
    if (!eng) return;
    if (paused && !isThumbnail) {
      eng.cancelFrames();
      return;
    }
    eng.wake();
  }, [
    paused,
    isThumbnail,
    testMode,
    powerState,
    config.envIntensity,
    config.exposure,
    config.maxFps,
    config.fpsMode,
    config.backgroundMode,
    config.backgroundColor,
    config.camera.fov,
    config.camera.distance,
    config.camera.azimuthDeg,
    config.camera.elevationDeg,
    config.camera.targetY,
    config.camera.target?.[0],
    config.camera.target?.[1],
    config.camera.target?.[2],
    config.camera.orbitParallaxX,
    config.camera.orbitParallaxY,
    config.camera.damping,
    config.bindings,
    config.autoDayNight,
    config.dayNightHourOverride,
    objectsTransformKey,
    usesMediaBinding ? mediaPlaying : false,
    usesCpuBinding ? cpuPercent : 0,
  ]);

  // Periodic 30s solar update when autoDayNight tracks real system clock
  useEffect(() => {
    if (paused || isThumbnail || !config.autoDayNight || typeof config.dayNightHourOverride === "number") {
      return;
    }
    const id = window.setInterval(() => {
      engineRef.current?.wake();
    }, powerCaps.dayNightIntervalMs);
    return () => window.clearInterval(id);
  }, [paused, isThumbnail, config.autoDayNight, config.dayNightHourOverride, powerCaps.dayNightIntervalMs]);

  // Dynamic 3D Textures per object (Live Media Album Cover or Custom Image onto .glb materials)
  const dynamicTexKey = useMemo(
    () => JSON.stringify(normalizedObjects.map((o) => ({ id: o.id, dt: o.dynamicTextures ?? [] }))),
    [normalizedObjects]
  );
  useEffect(() => {
    const eng = engineRef.current;
    if (!eng) return;
    let cancelled = false;

    const orderedObjs = normalizeScene3DObjects(configRef.current);

    // Reset all materials to their original base maps first
    eng.objectsMap.forEach((rt) => {
      const idx = rt.modelIndex;
      if (!idx) return;
      for (const entry of idx.emissiveMats) {
        let changed = false;
        if (entry.mat.map !== entry.baseMap) {
          entry.mat.map = entry.baseMap;
          changed = true;
        }
        if (entry.mat.emissiveMap !== entry.baseEmissiveMap) {
          entry.mat.emissiveMap = entry.baseEmissiveMap;
          changed = true;
        }
        entry.hasDynamicEmissiveMap = false;
        if (changed) entry.mat.needsUpdate = true;
      }
    });

    const activeUrls = new Set<string>();
    const pendingLoads = new Map<
      string,
      { rt: LoadedObjectRuntime; matName: string; asEmissive: boolean }[]
    >();

    const applyTexToTargetRt = (
      rt: LoadedObjectRuntime,
      matName: string,
      tex: Texture,
      asEmissive: boolean
    ) => {
      const idx = rt.modelIndex;
      if (!idx) return;
      const targets =
        !matName || matName === "*"
          ? idx.emissiveMats
          : idx.emissiveByName.get(matName) ?? [];
      for (const entry of targets) {
        entry.mat.map = tex;
        if (asEmissive) {
          entry.mat.emissiveMap = tex;
          entry.hasDynamicEmissiveMap = true;
        }
        entry.mat.needsUpdate = true;
      }
      eng.wake();
    };

    for (const obj of orderedObjs) {
      const rt = eng.objectsMap.get(obj.id);
      if (!rt?.modelIndex) continue;
      const rules = obj.dynamicTextures ?? [];
      const isProcedural = Boolean(rt.modelIndex.info.isDefaultSculpture);

      for (const rule of rules) {
        const rawSrc = rule.source === "MEDIA_COVER" ? mediaCoverPath : rule.customSrc ?? "";
        const url = rawSrc ? resolveMediaUrl(rawSrc) : "";
        if (!url) continue;
        const cacheKey = `${isProcedural ? "proc" : "gltf"}|${url}`;
        activeUrls.add(cacheKey);

        const cached = eng.dynamicTexCache.get(cacheKey);
        const waiting = pendingLoads.get(cacheKey);
        if (cached) {
          applyTexToTargetRt(rt, rule.materialName, cached, Boolean(rule.emissiveMap));
        } else if (waiting) {
          // Same texture already loading for another object/rule: share it (one GPU texture per URL)
          waiting.push({ rt, matName: rule.materialName, asEmissive: Boolean(rule.emissiveMap) });
        } else {
          const targets = [{ rt, matName: rule.materialName, asEmissive: Boolean(rule.emissiveMap) }];
          pendingLoads.set(cacheKey, targets);
          eng.texLoader.load(
            url,
            (loaded) => {
              if (cancelled || engineRef.current !== eng) {
                loaded.dispose();
                return;
              }
              loaded.colorSpace = SRGBColorSpace;
              loaded.flipY = isProcedural;
              loaded.needsUpdate = true;
              eng.dynamicTexCache.set(cacheKey, loaded);
              for (const t of targets) {
                // Object removed or model swapped while loading: skip it (the effect re-runs on reload)
                if (eng.objectsMap.get(t.rt.id) !== t.rt) continue;
                applyTexToTargetRt(t.rt, t.matName, loaded, t.asEmissive);
              }
            },
            undefined,
            () => {
              /* ignore transient missing cover */
            }
          );
        }
      }
    }

    // Evict & dispose stale textures (e.g. previous song's album cover) to keep VRAM constant
    eng.dynamicTexCache.forEach((tex, key) => {
      if (!activeUrls.has(key)) {
        tex.dispose();
        eng.dynamicTexCache.delete(key);
      }
    });

    eng.wake();
    return () => {
      cancelled = true;
    };
  }, [dynamicTexKey, mediaCoverPath, objectsSourceKey, modelLoadRev, canvasGen]);

  // Wake on live WASAPI audio / keyboard reactive sensor ticks (zero React re-renders!)
  useEffect(() => {
    if (paused || isThumbnail || !subscribeReactiveTick) return;
    if (!powerCaps.allowSignalBindings) return;
    return subscribeReactiveTick(() => {
      engineRef.current?.wake();
    });
  }, [paused, isThumbnail, subscribeReactiveTick, powerCaps.allowSignalBindings]);

  // Wake on cursor move events (Desktop OR Studio Test Mode)
  const cursorWakeAllowed = powerCaps.allowCursorBindings || powerCaps.allowCursorOrbit;
  useEffect(() => {
    if (paused || isThumbnail || !subscribeCursorTick) return;
    if (interactive && !testMode) return;
    if (!cursorWakeAllowed) return;
    return subscribeCursorTick(() => {
      engineRef.current?.wake();
    });
  }, [paused, isThumbnail, interactive, testMode, subscribeCursorTick, cursorWakeAllowed]);

  // Listen to cursor clicks for 3D raycast click triggers (Desktop OR Studio Test Mode)
  useEffect(() => {
    if (paused || isThumbnail || !subscribeCursorClick) return;
    if (interactive && !testMode) return;
    if (!powerCaps.allowCursorBindings) return;
    return subscribeCursorClick((nx, ny) => {
      engineRef.current?.handleCursorClick(nx, ny);
    });
  }, [paused, isThumbnail, interactive, testMode, subscribeCursorClick, powerCaps.allowCursorBindings]);

  // 2.1: once every object finished loading, snapshot the canvas, free the GPU, show an <img>
  useEffect(() => {
    if (!isThumbnail || !thumbSlot || thumbUrl) return;
    const eng = engineRef.current;
    const canvas = canvasRef.current;
    if (!eng || !canvas) return;
    let cancelled = false;
    const startedAt = performance.now();
    let timer = 0;
    const finish = (url: string | null) => {
      if (cancelled) return;
      if (url) {
        storeThumb(thumbKey, url);
        setThumbUrl(url);
      }
      thumbReleaseRef.current?.();
      thumbReleaseRef.current = null;
    };
    const snapshot = () => {
      eng.wake();
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          if (cancelled) return;
          canvas.toBlob((blob) => finish(blob ? URL.createObjectURL(blob) : null), "image/png");
        })
      );
    };
    const poll = () => {
      if (cancelled) return;
      const objs = Array.from(eng.objectsMap.values());
      const ready = objs.length > 0 && objs.every((o) => o.modelRoot || o.error);
      if (ready || performance.now() - startedAt > 8000) snapshot();
      else timer = window.setTimeout(poll, 150);
    };
    timer = window.setTimeout(poll, 150);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [isThumbnail, thumbSlot, thumbUrl, thumbKey, canvasGen, modelLoadRev]);

  // ── Sala 3D editor (Studio only, lazy chunk: zero bytes/CPU on Desktop) ────────
  const [editorRev, setEditorRev] = useState(0);
  useEffect(() => {
    const eng = engineRef.current;
    const canvas = canvasRef.current;
    if (!eng || !canvas || !editorWanted) return;
    let cancelled = false;
    let session: Scene3DEditorSession | null = null;

    void import("../utils/scene3dEditorTools").then((mod) => {
      const cur = engineRef.current;
      if (cancelled || !cur || cur !== eng) return;
      session = mod.createScene3DEditorSession({
        canvas,
        camera: cur.camera,
        scene: cur.scene,
        initialView: computeConfigCameraView(configRef.current, 0, 0),
        getPivot: (id) => engineRef.current?.objectsMap.get(id)?.pivot ?? null,
        getModelIndex: (id) => engineRef.current?.objectsMap.get(id)?.modelIndex ?? null,
        pickObjectAt: (nx, ny) => engineRef.current?.pick(nx, ny) ?? null,
        pickPartAt: (nx, ny) => engineRef.current?.pickPart(nx, ny) ?? null,
        wake: () => engineRef.current?.wake(),
        onSelectObject: (id) => onSelectObjectRef.current?.(id),
        onPickPart: (part) => onPickPartRef.current?.(part),
        onCommitTransform: (id, p, r, sc) => onCommitTransformRef.current?.(id, p, r, sc),
      });
      cur.editor = session;
      if (editorApiRef) {
        editorApiRef.current = {
          getCameraState: () => session?.captureCameraState() ?? null,
          setCameraState: (state, animate) => session?.setCameraState(state, animate),
          focusOn: (objectId) => session?.focusOn(objectId) ?? false,
          captureCameraState: () => session?.captureCameraState() ?? null,
          focusSelected: () => session?.focusSelected(),
          resetView: () => session?.resetView(computeConfigCameraView(configRef.current, 0, 0)),
          getObjectTree: (objectId) => session?.getObjectTree(objectId) ?? null,
          highlightNode: (objectId, nodeId) => session?.highlightNode(objectId, nodeId),
        };
      }
      setEditorRev((r) => r + 1);
      onEditorSyncRef.current?.();
      cur.wake();
    });

    return () => {
      cancelled = true;
      const cur = engineRef.current;
      if (session) {
        if (cur?.editor === session) cur.editor = null;
        session.dispose();
      }
      if (editorApiRef) editorApiRef.current = null;
      cur?.wake();
    };
  }, [editorWanted, canvasGen]);

  useEffect(() => {
    const ed = engineRef.current?.editor;
    if (!ed) return;
    ed.setSelected(selectedObjectId ?? null);
    ed.setMode(gizmoMode);
    ed.setGrid(showGrid);
    ed.setSnap(snapEnabled);
    ed.setSkeleton(showSkeleton);
    engineRef.current?.wake();
  }, [editorRev, modelLoadRev, selectedObjectId, gizmoMode, showGrid, snapEnabled, showSkeleton]);

  // A (re)loaded model has a new index/tree: let the outliner refetch
  useEffect(() => {
    if (editorRev > 0 && modelLoadRev > 0) onEditorSyncRef.current?.();
  }, [editorRev, modelLoadRev]);

  if (isThumbnail && thumbUrl) {
    return (
      <img
        src={thumbUrl}
        alt=""
        draggable={false}
        style={{ width: `${Math.max(1, width)}px`, height: `${Math.max(1, height)}px` }}
        className="block w-full h-full object-cover pointer-events-none"
      />
    );
  }

  return (
    <canvas
      key={canvasGen}
      ref={canvasRef}
      style={{
        width: `${Math.max(1, width)}px`,
        height: `${Math.max(1, height)}px`,
        pointerEvents: editorWanted ? "auto" : undefined,
        touchAction: editorWanted ? "none" : undefined,
        cursor: editorWanted ? "default" : undefined,
      }}
      className={`block w-full h-full ${editorWanted ? "" : "pointer-events-none"}`}
    />
  );
};

// 3.5: parent re-renders every telemetry/clock tick; props are shallow-compared
export const Scene3DLayer = React.memo(Scene3DLayerImpl);
export default Scene3DLayer;
