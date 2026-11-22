import React, { Suspense, useCallback, useEffect, useId, useRef, useState } from "react";
import {
  BUILTIN_SVG_ICONS,
  computeLiveMediaProgress,
  defaultScene3D,
  evaluateFormula,
  invokeCmd,
  LiveTelemetry,
  MonitorInfoDto,
  PrismProject,
  ReactiveSignal,
  resolveMediaUrl,
  Scene3DConfig,
  Scene3DInspectInfo,
  SceneLayer,
} from "../types/prism";
import { getGlyphGlassMaps } from "../utils/glassLens";
import { BackdropSourceInfo, getCssBackgroundCanvas } from "../utils/opticalGlassWebGL";
import { OpticalGlassSurface } from "./OpticalGlassSurface";
import { type PowerState, powerCapsFor } from "../engine/powerGovernor";
// Type-only: keeps the 3D engine in its own lazy chunk
import type { Scene3DEditorApi, Scene3DGizmoMode, Scene3DStats } from "./Scene3DLayer";

const LazyScene3DLayer = React.lazy(() => import("./Scene3DLayer"));

/** Studio "Sala 3D" mode: routes the pointer to one 3D layer's editor and mutes 2D hit-testing */
export interface Room3DEditBinding {
  layerId: string;
  selectedObjectId: string | null;
  gizmoMode: Scene3DGizmoMode;
  showGrid: boolean;
  snapEnabled: boolean;
  onSelectObject: (objectId: string | null) => void;
  onCommitObjectTransform: (
    layerId: string,
    objectId: string,
    position: [number, number, number],
    rotationDeg: [number, number, number],
    scale: [number, number, number]
  ) => void;
  editorApiRef: React.MutableRefObject<Scene3DEditorApi | null>;
}

const loadedFontCache = new Set<string>();
/** 2.9: in-flight font loads, so N layers sharing a font trigger ONE download */
const fontLoadsInFlight = new Map<string, Promise<boolean>>();
const sanitizedSvgCache = new Map<string, string>();

/**
 * Strips scriptable/unsafe elements and attributes from user-supplied or imported SVG markup
 * before injecting into the privileged Tauri WebView DOM.
 */
function sanitizeSvgMarkup(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) return "";
  const hit = sanitizedSvgCache.get(trimmed);
  if (hit !== undefined) return hit;

  let safe = "";
  try {
    const parser = new DOMParser();
    const doc = parser.parseFromString(trimmed, "image/svg+xml");
    const root = doc.documentElement;
    if (!root || root.tagName.toLowerCase() !== "svg" || doc.querySelector("parsererror")) {
      safe = "";
    } else {
      const blockedTags = new Set([
        "script",
        "foreignobject",
        "iframe",
        "object",
        "embed",
        "link",
        "meta",
        "audio",
        "video",
        "applet",
        "base",
      ]);
      const cleanNode = (el: Element) => {
        const children = Array.from(el.children);
        for (const child of children) {
          if (blockedTags.has(child.tagName.toLowerCase())) {
            child.remove();
          } else {
            cleanNode(child);
          }
        }
        for (const attr of Array.from(el.attributes)) {
          const name = attr.name.toLowerCase();
          const val = attr.value.trim().toLowerCase().replace(/\s+/g, "");
          if (name.startsWith("on")) {
            el.removeAttribute(attr.name);
            continue;
          }
          if (
            (name === "href" || name === "xlink:href" || name === "src" || name === "action") &&
            (val.startsWith("javascript:") || val.startsWith("vbscript:") || val.startsWith("data:"))
          ) {
            el.removeAttribute(attr.name);
            continue;
          }
          if (name === "style" && (val.includes("expression(") || val.includes("javascript:") || val.includes("url("))) {
            el.removeAttribute(attr.name);
          }
        }
      };
      cleanNode(root);
      safe = new XMLSerializer().serializeToString(root);
    }
  } catch {
    safe = "";
  }

  if (sanitizedSvgCache.size > 64) {
    const oldest = sanitizedSvgCache.keys().next().value;
    if (oldest !== undefined) sanitizedSvgCache.delete(oldest);
  }
  sanitizedSvgCache.set(trimmed, safe);
  return safe;
}

interface SnapAxisResult {
  snapped: number;
  guide: number | null;
}

function snapCoordinate(
  rawCenter: number,
  halfExtent: number,
  targets: number[],
  threshold: number
): SnapAxisResult {
  const probes = [
    { offset: 0, value: rawCenter },
    ...(halfExtent > 0.002
      ? [
          { offset: -halfExtent, value: rawCenter - halfExtent },
          { offset: halfExtent, value: rawCenter + halfExtent },
        ]
      : []),
  ];

  let bestDist = threshold;
  let bestCenter = rawCenter;
  let bestGuide: number | null = null;

  for (const target of targets) {
    for (const p of probes) {
      const dist = Math.abs(p.value - target);
      if (dist < bestDist) {
        bestDist = dist;
        bestCenter = target - p.offset;
        bestGuide = target;
      }
    }
  }

  return {
    snapped: Math.max(0, Math.min(1, bestCenter)),
    guide: bestGuide,
  };
}

function ensureCustomFontLoaded(fontFamily: string, fontPath: string): Promise<boolean> {
  if (!fontFamily || !fontPath) return Promise.resolve(false);
  const key = `${fontFamily}|${fontPath}`;
  if (loadedFontCache.has(key)) return Promise.resolve(false);
  const inFlight = fontLoadsInFlight.get(key);
  if (inFlight) return inFlight;
  const p = (async () => {
    try {
      const url = resolveMediaUrl(fontPath);
      if (!url) return false;
      const safeFamily = fontFamily.replace(/"/g, "");
      const face = new FontFace(safeFamily, `url("${url}")`);
      const loaded = await face.load();
      document.fonts.add(loaded);
      loadedFontCache.add(key);
      return true;
    } catch {
      return false;
    } finally {
      fontLoadsInFlight.delete(key);
    }
  })();
  fontLoadsInFlight.set(key, p);
  return p;
}

function hexToRgbTuple(hex: string): [number, number, number] {
  const clean = (hex || "#FFFFFF").replace("#", "").trim();
  if (clean.length === 3) {
    const r = parseInt(clean[0] + clean[0], 16);
    const g = parseInt(clean[1] + clean[1], 16);
    const b = parseInt(clean[2] + clean[2], 16);
    if (Number.isFinite(r) && Number.isFinite(g) && Number.isFinite(b)) return [r, g, b];
  } else if (clean.length >= 6) {
    const r = parseInt(clean.slice(0, 2), 16);
    const g = parseInt(clean.slice(2, 4), 16);
    const b = parseInt(clean.slice(4, 6), 16);
    if (Number.isFinite(r) && Number.isFinite(g) && Number.isFinite(b)) return [r, g, b];
  }
  return [255, 255, 255];
}

function colorWithAlpha(color: string, alpha?: number): string {
  const a = alpha === undefined ? 1 : Math.max(0, Math.min(1, alpha));
  const raw = (color || "#FFFFFF").trim();
  if (a >= 0.999 && !raw.startsWith("#")) return raw;
  if (raw.startsWith("#")) {
    const [r, g, b] = hexToRgbTuple(raw);
    return `rgba(${r}, ${g}, ${b}, ${a.toFixed(3)})`;
  }
  return raw;
}

function supportsCorsUrl(url: string): boolean {
  if (!url) return false;
  const low = url.toLowerCase();
  return (
    low.startsWith("http://asset.localhost") ||
    low.startsWith("https://asset.localhost") ||
    low.startsWith("asset:") ||
    low.startsWith("blob:") ||
    low.startsWith("data:") ||
    low.startsWith("/")
  );
}

function computeMediaDrawRect(
  boxLeft: number,
  boxTop: number,
  boxW: number,
  boxH: number,
  srcW: number,
  srcH: number,
  objectFit: "cover" | "contain"
): { drawLeft: number; drawTop: number; drawW: number; drawH: number } {
  const safeSrcW = Math.max(1, srcW);
  const safeSrcH = Math.max(1, srcH);
  const safeBoxW = Math.max(1, boxW);
  const safeBoxH = Math.max(1, boxH);
  const mediaAspect = safeSrcW / safeSrcH;
  const boxAspect = safeBoxW / safeBoxH;

  let drawW = safeBoxW;
  let drawH = safeBoxH;
  if (objectFit === "cover") {
    if (mediaAspect > boxAspect) {
      drawH = safeBoxH;
      drawW = safeBoxH * mediaAspect;
    } else {
      drawW = safeBoxW;
      drawH = safeBoxW / mediaAspect;
    }
  } else {
    if (mediaAspect > boxAspect) {
      drawW = safeBoxW;
      drawH = safeBoxW / mediaAspect;
    } else {
      drawH = safeBoxH;
      drawW = safeBoxH * mediaAspect;
    }
  }
  return {
    drawLeft: boxLeft + (safeBoxW - drawW) * 0.5,
    drawTop: boxTop + (safeBoxH - drawH) * 0.5,
    drawW,
    drawH,
  };
}

function sceneHasAnyGlass(layers: SceneLayer[]): boolean {
  for (const l of layers) {
    if (l.hidden) continue;
    if (l.glass?.enabled) return true;
    if (l.children?.length && sceneHasAnyGlass(l.children)) return true;
  }
  return false;
}

function sceneHasVisibleVideo(layers: SceneLayer[]): boolean {
  for (const l of layers) {
    if (l.hidden) continue;
    if (l.kind === "VIDEO" && l.video?.src) return true;
    if (l.children?.length && sceneHasVisibleVideo(l.children)) return true;
  }
  return false;
}

/** True when the layer subtree never reads telemetry, the clock, media state or the backdrop stack. */
function isLayerStatic(layer: SceneLayer): boolean {
  if (layer.glass?.enabled) return false;
  if (layer.fx) {
    for (const k in layer.fx) {
      if (layer.fx[k]?.includes("$")) return false;
    }
  }
  switch (layer.kind) {
    case "IMAGE":
      if (layer.image?.sourceMode === "MEDIA_COVER") return false;
      break;
    case "SVG":
      if (layer.clickAction === "MEDIA_PLAY_PAUSE") return false;
      break;
    case "TEXT":
      if (layer.text?.content?.includes("$")) return false;
      break;
    case "GROUP":
      break;
    default:
      return false;
  }
  if (layer.children?.length) {
    for (const c of layer.children) if (!isLayerStatic(c)) return false;
  }
  return true;
}

interface MemoLayerNodeProps {
  render: () => React.ReactNode;
  deps: readonly unknown[];
}

const MemoLayerNode = React.memo(
  function MemoLayerNode({ render }: MemoLayerNodeProps) {
    return <>{render()}</>;
  },
  (prev, next) =>
    prev.deps.length === next.deps.length && prev.deps.every((d, i) => Object.is(d, next.deps[i]))
);

function layersNeedClockTick(layers: SceneLayer[], mediaPlaying: boolean): boolean {
  for (const l of layers) {
    if (l.hidden) continue;
    if (l.kind === "PROGRESS" && l.progress?.source === "MEDIA" && mediaPlaying) {
      return true;
    }
    if (l.kind === "TEXT" && l.text?.content) {
      if (/df\(/i.test(l.text.content)) return true;
      if (mediaPlaying && /mi\(\s*(pos|pct)\s*\)/i.test(l.text.content)) return true;
    }
    if (l.fx) {
      for (const expr of Object.values(l.fx)) {
        if (expr && (/df\(/i.test(expr) || (mediaPlaying && /mi\(\s*(pos|pct)\s*\)/i.test(expr)))) {
          return true;
        }
      }
    }
    if (l.children?.length && layersNeedClockTick(l.children, mediaPlaying)) {
      return true;
    }
  }
  return false;
}

interface VideoElementProps {
  src: string;
  speed: number;
  muted: boolean;
  volume: number;
  paused: boolean;
  objectFit: "cover" | "contain";
  clipRadius: number;
  onMediaEl?: (el: HTMLVideoElement | null) => void;
  onMediaReady?: () => void;
}

const RealVideoLayer: React.FC<VideoElementProps> = ({
  src,
  speed,
  muted,
  volume,
  paused,
  objectFit,
  clipRadius,
  onMediaEl,
  onMediaReady,
}) => {
  const ref = useRef<HTMLVideoElement | null>(null);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const url = resolveMediaUrl(src);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.playbackRate = Math.max(0.25, Math.min(3, speed || 1));
    el.muted = muted;
    el.volume = muted ? 0 : Math.max(0, Math.min(1, volume));
  }, [speed, muted, volume]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (paused) {
      el.pause();
    } else {
      el.play().catch(() => {});
    }
  }, [paused, url]);

  if (!url) {
    return (
      <div className="w-full h-full flex items-center justify-center bg-[#0D0D0F] border border-dashed border-white/20 text-xs text-[#A1A1AA]">
        Selecione um arquivo .mp4 / .webm no Inspetor
      </div>
    );
  }

  return (
    <video
      ref={(el) => {
        // 2.7: on unmount React passes null — release the decoder/buffers of the previous element
        const prev = ref.current;
        if (!el && prev) {
          prev.pause();
          prev.removeAttribute("src");
          prev.load();
        }
        ref.current = el;
        onMediaEl?.(el);
      }}
      src={url}
      crossOrigin={supportsCorsUrl(url) ? "anonymous" : undefined}
      autoPlay={!paused}
      preload={paused ? "metadata" : "auto"}
      loop
      muted={muted}
      playsInline
      onLoadedData={(e) => {
        if (pausedRef.current) {
          e.currentTarget.pause();
        } else {
          e.currentTarget.play().catch(() => {});
        }
        onMediaReady?.();
      }}
      style={{ objectFit, borderRadius: `${clipRadius}px` }}
      className="w-full h-full pointer-events-none"
    />
  );
};

/**
 * 3.5: per-layer host for the lazy 3D engine. The compositor re-renders on every telemetry (1.5 s)
 * and clock (1 s) tick; this wrapper keeps every callback identity stable (latest handlers live
 * in a ref) so the memoized Scene3DLayer only re-renders when its real inputs change.
 */
interface Scene3DSlotProps {
  layerId: string;
  config: Scene3DConfig | undefined;
  width: number;
  height: number;
  paused: boolean;
  isThumbnail: boolean;
  interactive: boolean;
  testMode: boolean;
  cursorNormRef?: React.RefObject<[number, number] | null>;
  subscribeCursorTick?: (cb: () => void) => () => void;
  subscribeCursorClick?: (cb: (x: number, y: number) => void) => () => void;
  reactiveSignalRef?: React.RefObject<ReactiveSignal>;
  subscribeReactiveTick?: (cb: () => void) => () => void;
  mediaCoverPath: string;
  mediaPlaying: boolean;
  cpuPercent: number;
  powerState: PowerState;
  mediaElementsRef: React.MutableRefObject<
    Map<string, HTMLVideoElement | HTMLImageElement | HTMLCanvasElement>
  >;
  setMediaRev: React.Dispatch<React.SetStateAction<number>>;
  videoFrameTokenRef: React.MutableRefObject<number>;
  frameTickSubsRef: React.MutableRefObject<Set<() => void>>;
  onInspectScene3D?: (layerId: string, info: Scene3DInspectInfo) => void;
  onHoverMesh3D?: (meshName: string | null) => void;
  onScene3DStats?: (layerId: string, stats: Scene3DStats) => void;
  room3d: Room3DEditBinding | null;
}

const Scene3DSlot: React.FC<Scene3DSlotProps> = (p) => {
  const fallbackConfig = React.useMemo(() => defaultScene3D(), []);
  const latest = useRef(p);
  latest.current = p;
  const { layerId } = p;

  const onCanvasEl = useCallback(
    (el: HTMLCanvasElement | null) => {
      const l = latest.current;
      if (el) {
        l.mediaElementsRef.current.set(layerId, el);
        l.setMediaRev((r) => r + 1);
      } else {
        l.mediaElementsRef.current.delete(layerId);
      }
    },
    [layerId]
  );
  const onFrameReady = useCallback(() => {
    const l = latest.current;
    l.videoFrameTokenRef.current += 1;
    l.frameTickSubsRef.current.forEach((cb) => cb());
  }, []);
  const onInspectModel = useCallback(
    (info: Scene3DInspectInfo) => latest.current.onInspectScene3D?.(layerId, info),
    [layerId]
  );
  const onHoverMesh = useCallback(
    (name: string | null) => latest.current.onHoverMesh3D?.(name),
    []
  );
  const hasStats = Boolean(p.onScene3DStats);
  const onStats = useCallback(
    (st: Scene3DStats) => latest.current.onScene3DStats?.(layerId, st),
    [layerId]
  );
  const onCommitObjectTransform = useCallback(
    (
      objectId: string,
      pos: [number, number, number],
      rot: [number, number, number],
      sc: [number, number, number]
    ) => latest.current.room3d?.onCommitObjectTransform(layerId, objectId, pos, rot, sc),
    [layerId]
  );

  // Telemetry only reaches the engine when an enabled binding reads it (3.4/3.5)
  const bindings = p.config?.bindings;
  const needsCpu = React.useMemo(
    () => (bindings ?? []).some((b) => b.enabled && b.source === "CPU_USAGE"),
    [bindings]
  );
  const needsMedia = React.useMemo(
    () => (bindings ?? []).some((b) => b.enabled && b.source === "MEDIA_PLAYING"),
    [bindings]
  );

  const room = p.room3d;
  return (
    <LazyScene3DLayer
      config={p.config ?? fallbackConfig}
      width={p.width}
      height={p.height}
      paused={p.paused}
      isThumbnail={p.isThumbnail}
      interactive={p.interactive}
      testMode={p.testMode}
      cursorNormRef={p.cursorNormRef}
      subscribeCursorTick={p.subscribeCursorTick}
      subscribeCursorClick={p.subscribeCursorClick}
      reactiveSignalRef={p.reactiveSignalRef}
      subscribeReactiveTick={p.subscribeReactiveTick}
      mediaCoverPath={p.mediaCoverPath}
      mediaPlaying={needsMedia ? p.mediaPlaying : false}
      cpuPercent={needsCpu ? p.cpuPercent : 0}
      onCanvasEl={onCanvasEl}
      onFrameReady={onFrameReady}
      onInspectModel={onInspectModel}
      onHoverMesh={onHoverMesh}
      powerState={p.powerState}
      onStats={hasStats ? onStats : undefined}
      {...(room
        ? {
            editMode3d: true,
            selectedObjectId: room.selectedObjectId,
            gizmoMode: room.gizmoMode,
            showGrid: room.showGrid,
            snapEnabled: room.snapEnabled,
            onSelectObject: room.onSelectObject,
            onCommitObjectTransform,
            editorApiRef: room.editorApiRef,
          }
        : null)}
    />
  );
};

export interface UnifiedCompositorProps {
  project: PrismProject;
  telemetry: LiveTelemetry;
  monitorInfo?: MonitorInfoDto | null;
  cursorNormRef?: React.RefObject<[number, number] | null>;
  subscribeCursorTick?: (cb: () => void) => () => void;
  subscribeCursorClick?: (cb: (x: number, y: number) => void) => () => void;
  reactiveSignalRef?: React.RefObject<ReactiveSignal>;
  subscribeReactiveTick?: (cb: () => void) => () => void;
  paused?: boolean;
  isThumbnail?: boolean;
  interactive?: boolean;
  test3dMode?: boolean;
  selectedLayerId?: string | null;
  selectedLayerIds?: string[];
  onSelectLayer?: (
    id: string | null,
    modifiers?: { ctrlKey?: boolean; altKey?: boolean }
  ) => void;
  onMoveLayer?: (id: string, x: number, y: number) => void;
  onMoveLayers?: (moves: Array<{ id: string; x: number; y: number }>) => void;
  onResizeLayer?: (id: string, width: number, height: number, isText: boolean) => void;
  onDragStateChange?: (dragging: boolean) => void;
  onInspectScene3D?: (layerId: string, info: Scene3DInspectInfo) => void;
  onHoverMesh3D?: (meshName: string | null) => void;
  /** Energy governor state forwarded to every 3D layer */
  powerState?: PowerState;
  room3d?: Room3DEditBinding | null;
  onScene3DStats?: (layerId: string, stats: Scene3DStats) => void;
}

export const UnifiedCompositor: React.FC<UnifiedCompositorProps> = ({
  project,
  telemetry,
  monitorInfo = null,
  cursorNormRef,
  subscribeCursorTick,
  subscribeCursorClick,
  reactiveSignalRef,
  subscribeReactiveTick,
  paused = false,
  isThumbnail = false,
  interactive = false,
  test3dMode = false,
  selectedLayerId = null,
  selectedLayerIds,
  onSelectLayer,
  onMoveLayer,
  onMoveLayers,
  onResizeLayer,
  onDragStateChange,
  onInspectScene3D,
  onHoverMesh3D,
  powerState = "NORMAL",
  room3d = null,
  onScene3DStats,
}) => {
  const roomActive = Boolean(interactive && room3d);
  const uid = useId().replace(/:/g, "");
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState<{ w: number; h: number }>(() => {
    if (isThumbnail) return { w: 480, h: 270 };
    if (monitorInfo && monitorInfo.width > 0 && monitorInfo.height > 0) {
      const sf = Math.max(0.5, monitorInfo.scale_factor || 1);
      return { w: monitorInfo.width / sf, h: monitorInfo.height / sf };
    }
    if (typeof window !== "undefined" && window.innerWidth > 0 && window.innerHeight > 0) {
      return { w: window.innerWidth, h: window.innerHeight };
    }
    return { w: 1280, h: 720 };
  });
  const [now, setNow] = useState<Date>(() => new Date());
  const [fontRev, setFontRev] = useState(0);
  const [mediaRev, setMediaRev] = useState(0);
  const [snapGuide, setSnapGuide] = useState<{ x: number | null; y: number | null }>({
    x: null,
    y: null,
  });
  const layerElsRef = useRef<Map<string, HTMLDivElement>>(new Map());
  const mediaElementsRef = useRef<
    Map<string, HTMLVideoElement | HTMLImageElement | HTMLCanvasElement>
  >(new Map());
  const frameTickSubsRef = useRef<Set<() => void>>(new Set());
  const videoFrameTokenRef = useRef(0);
  const rafRef = useRef<number | null>(null);
  const fpsCapRef = useRef<number | null>(null);
  fpsCapRef.current = powerCapsFor(powerState).fpsCap;

  // Studio 3D Test Mode cursor/click channels (zero React re-renders during live mouse test)
  const previewCursorRef = useRef<[number, number] | null>([0.5, 0.5]);
  const previewTickSubsRef = useRef<Set<() => void>>(new Set());
  const previewClickSubsRef = useRef<Set<(x: number, y: number) => void>>(new Set());

  const subscribePreviewTick = useCallback((cb: () => void) => {
    previewTickSubsRef.current.add(cb);
    return () => {
      previewTickSubsRef.current.delete(cb);
    };
  }, []);

  const subscribePreviewClick = useCallback((cb: (x: number, y: number) => void) => {
    previewClickSubsRef.current.add(cb);
    return () => {
      previewClickSubsRef.current.delete(cb);
    };
  }, []);

  const registerFrameTick = useCallback((cb: () => void) => {
    frameTickSubsRef.current.add(cb);
    return () => {
      frameTickSubsRef.current.delete(cb);
    };
  }, []);

  const selectedSet = new Set<string>(
    selectedLayerIds && selectedLayerIds.length > 0
      ? selectedLayerIds
      : selectedLayerId
      ? [selectedLayerId]
      : []
  );

  const dragRef = useRef<{
    mode: "MOVE" | "RESIZE";
    id: string;
    isText: boolean;
    isTopLevel: boolean;
    containerW: number;
    containerH: number;
    startClientX: number;
    startClientY: number;
    startX: number;
    startY: number;
    startW: number;
    startH: number;
    halfW: number;
    halfH: number;
    snapTargetsX: number[];
    snapTargetsY: number[];
    moved: boolean;
    wasAlreadyInMulti: boolean;
    multiPeers?: Array<{ id: string; startX: number; startY: number }>;
  } | null>(null);

  // Measure actual rendered viewport for exact 1:1 proportional scaling
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (r && r.width > 0 && r.height > 0) {
        setSize((prev) =>
          Math.abs(prev.w - r.width) > 1 || Math.abs(prev.h - r.height) > 1
            ? { w: r.width, h: r.height }
            : prev
        );
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Forward desktop cursor ticks directly to WebGL2 glass subscribers (zero React re-renders!)
  useEffect(() => {
    if (paused || isThumbnail || !subscribeCursorTick) return;
    return subscribeCursorTick(() => {
      frameTickSubsRef.current.forEach((cb) => cb());
    });
  }, [paused, isThumbnail, subscribeCursorTick]);

  // Single shared GPU video-to-glass rAF loop: ONLY runs when a visible VIDEO + glass layer exist & not paused (0% idle CPU/GPU)
  const needsVideoGlassLoop =
    !paused && !isThumbnail && sceneHasAnyGlass(project.layers) && sceneHasVisibleVideo(project.layers);

  useEffect(() => {
    if (!needsVideoGlassLoop) {
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
      return;
    }
    let active = true;
    const minIntervalMs = fpsCapRef.current ? 1000 / fpsCapRef.current : 0;
    let lastTickMs = 0;
    const emitTick = () => {
      if (frameTickSubsRef.current.size === 0) return;
      const t = performance.now();
      if (minIntervalMs > 0 && t - lastTickMs < minIntervalMs) return;
      lastTickMs = t;
      videoFrameTokenRef.current += 1;
      frameTickSubsRef.current.forEach((cb) => cb());
    };

    // Watch every mounted <video>; fire only when the decoder presents a new frame.
    const watched = new Map<HTMLVideoElement, number>();
    const watchVideo = (v: HTMLVideoElement) => {
      const onFrame = () => {
        if (!active || !watched.has(v)) return;
        emitTick();
        watched.set(v, v.requestVideoFrameCallback(onFrame));
      };
      watched.set(v, v.requestVideoFrameCallback(onFrame));
    };
    const reconcile = () => {
      const live = new Set<HTMLVideoElement>();
      mediaElementsRef.current.forEach((el) => {
        if (el instanceof HTMLVideoElement && el.isConnected) live.add(el);
      });
      live.forEach((v) => {
        if (!watched.has(v)) watchVideo(v);
      });
      watched.forEach((handle, v) => {
        if (!live.has(v)) {
          v.cancelVideoFrameCallback(handle);
          watched.delete(v);
        }
      });
    };

    const supportsVfc =
      typeof HTMLVideoElement !== "undefined" && "requestVideoFrameCallback" in HTMLVideoElement.prototype;

    const rafStep = () => {
      if (!active) return;
      emitTick();
      rafRef.current = requestAnimationFrame(rafStep);
    };
    if (!supportsVfc) {
      rafRef.current = requestAnimationFrame(rafStep);
      return () => {
        active = false;
        if (rafRef.current !== null) {
          cancelAnimationFrame(rafRef.current);
          rafRef.current = null;
        }
      };
    }

    reconcile();
    // Videos mount/unmount without touching this effect: cheap re-scan twice a second.
    const watchdog = window.setInterval(reconcile, 500);
    return () => {
      active = false;
      window.clearInterval(watchdog);
      watched.forEach((handle, v) => v.cancelVideoFrameCallback(handle));
      watched.clear();
    };
  }, [needsVideoGlassLoop]);

  // Live 1Hz tick ONLY when scene uses clock formulas ($df) or active media timeline progress
  const needsClock = layersNeedClockTick(project.layers, Boolean(telemetry.media.playing));
  useEffect(() => {
    if (paused || isThumbnail || !needsClock) return;
    setNow(new Date());
    const id = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(id);
  }, [paused, isThumbnail, needsClock]);

  // Bump fontRev ONCE when initial webfonts settle (never on every layer drag/edit!)
  useEffect(() => {
    let active = true;
    if (typeof document !== "undefined" && document.fonts?.ready) {
      void document.fonts.ready.then(() => {
        if (active) setFontRev((r) => r + 1);
      });
    }
    return () => {
      active = false;
    };
  }, []);

  // Preload any custom .ttf/.otf fonts referenced by text layers and trigger redraw ONLY if a new font loads
  useEffect(() => {
    let active = true;
    const walk = (layers: SceneLayer[]) => {
      for (const l of layers) {
        if (l.kind === "TEXT" && l.text?.fontPath) {
          void ensureCustomFontLoaded(l.text.fontFamily, l.text.fontPath).then((loaded) => {
            if (active && loaded) setFontRev((r) => r + 1);
          });
        }
        if (l.children?.length) walk(l.children);
      }
    };
    walk(project.layers);
    return () => {
      active = false;
    };
  }, [project.layers]);

  // Sync initial viewport when physical monitor metrics change (e.g. WM_DISPLAYCHANGE / DPI change)
  useEffect(() => {
    if (isThumbnail || interactive || !monitorInfo) return;
    const el = rootRef.current;
    if (el && el.clientWidth > 0 && el.clientHeight > 0) {
      setSize({ w: el.clientWidth, h: el.clientHeight });
      return;
    }
    const sf = Math.max(0.5, monitorInfo.scale_factor || 1);
    setSize({ w: monitorInfo.width / sf, h: monitorInfo.height / sf });
  }, [monitorInfo, isThumbnail, interactive]);

  // Dynamic logical basis adapted to monitor aspect ratio (supports 16:9, 21:9 ultrawide, 4K, 9:16 portrait)
  const isPortraitViewport = !isThumbnail && project.packageType !== "WIDGET" && size.h > size.w * 1.05;
  const aspect = size.w / Math.max(size.h, 1);
  const logicalW =
    project.packageType === "WIDGET"
      ? project.widget.width
      : isPortraitViewport
      ? 1080
      : Math.max(960, Math.round(1080 * aspect));
  const logicalH =
    project.packageType === "WIDGET"
      ? project.widget.height
      : isPortraitViewport
      ? Math.max(960, Math.round(1080 / Math.max(aspect, 0.25)))
      : 1080;
  const scale = Math.min(size.w / Math.max(logicalW, 1), size.h / Math.max(logicalH, 1));

  const handlePointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (paused && !interactive) return;
    if (interactive && test3dMode) {
      const rect = rootRef.current?.getBoundingClientRect();
      if (rect && rect.width > 1 && rect.height > 1) {
        const nx = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
        const ny = Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height));
        previewCursorRef.current = [nx, ny];
        previewTickSubsRef.current.forEach((cb) => cb());
      }
      return;
    }
    if (!interactive || !dragRef.current) return;

    if (
      !dragRef.current.moved &&
      Math.hypot(e.clientX - dragRef.current.startClientX, e.clientY - dragRef.current.startClientY) >= 3
    ) {
      dragRef.current.moved = true;
    }

    if (dragRef.current.mode === "MOVE" && (onMoveLayers || onMoveLayer)) {
      const cw = Math.max(dragRef.current.containerW, 1);
      const ch = Math.max(dragRef.current.containerH, 1);
      const dx = (e.clientX - dragRef.current.startClientX) / cw;
      const dy = (e.clientY - dragRef.current.startClientY) / ch;
      const rawX = Math.max(0, Math.min(1, dragRef.current.startX + dx));
      const rawY = Math.max(0, Math.min(1, dragRef.current.startY + dy));
      const threshold = e.altKey ? 0 : 0.012;
      const snapX = snapCoordinate(
        rawX,
        dragRef.current.halfW,
        dragRef.current.snapTargetsX,
        threshold
      );
      const snapY = snapCoordinate(
        rawY,
        dragRef.current.halfH,
        dragRef.current.snapTargetsY,
        threshold
      );
      const gx = dragRef.current.isTopLevel ? snapX.guide : null;
      const gy = dragRef.current.isTopLevel ? snapY.guide : null;
      // 4.6: pointermove fires constantly; only re-render the tree when a guide actually changes.
      setSnapGuide((prev) => (prev.x === gx && prev.y === gy ? prev : { x: gx, y: gy }));
      const appliedDx = snapX.snapped - dragRef.current.startX;
      const appliedDy = snapY.snapped - dragRef.current.startY;
      const peers = dragRef.current.multiPeers;
      if (peers && peers.length > 1 && onMoveLayers) {
        onMoveLayers(
          peers.map((p) => ({
            id: p.id,
            x: Math.max(0, Math.min(1, Number((p.startX + appliedDx).toFixed(4)))),
            y: Math.max(0, Math.min(1, Number((p.startY + appliedDy).toFixed(4)))),
          }))
        );
      } else if (onMoveLayer) {
        onMoveLayer(
          dragRef.current.id,
          Number(snapX.snapped.toFixed(4)),
          Number(snapY.snapped.toFixed(4))
        );
      }
    } else if (dragRef.current.mode === "RESIZE" && onResizeLayer) {
      const dxLog = (e.clientX - dragRef.current.startClientX) / Math.max(scale, 0.05);
      const dyLog = (e.clientY - dragRef.current.startClientY) / Math.max(scale, 0.05);
      if (dragRef.current.isText) {
        const nextFont = Math.max(12, Math.min(340, Math.round(dragRef.current.startW + dxLog)));
        onResizeLayer(dragRef.current.id, nextFont, nextFont, true);
      } else {
        const nw = Math.max(24, Math.min(2400, Math.round(dragRef.current.startW + dxLog * 2)));
        const nh = Math.max(20, Math.min(1400, Math.round(dragRef.current.startH + dyLog * 2)));
        onResizeLayer(dragRef.current.id, nw, nh, false);
      }
    }
  };

  const endDrag = () => {
    if (dragRef.current) {
      const finished = dragRef.current;
      dragRef.current = null;
      setSnapGuide((prev) => (prev.x === null && prev.y === null ? prev : { x: null, y: null }));
      onDragStateChange?.(false);
      // Clicking an item inside a multi-selection without dragging collapses selection to just that item
      if (finished.mode === "MOVE" && !finished.moved && finished.wasAlreadyInMulti) {
        onSelectLayer?.(finished.id);
      }
    }
  };

  const hasFullscreenBackdropLayer = project.layers.some(
    (l) =>
      !l.hidden &&
      (l.kind === "VIDEO" || l.kind === "SCENE_3D" || (l.kind === "IMAGE" && l.width >= 1600))
  );
  const isTransparentWidget =
    project.packageType === "WIDGET" && project.backgroundColor === "transparent";
  const effectiveBackgroundCss = isTransparentWidget
    ? "transparent"
    : hasFullscreenBackdropLayer
    ? project.backgroundColor || "#060608"
    : project.backgroundColor && project.backgroundColor !== "#000000"
    ? project.backgroundColor
    : "radial-gradient(circle at 50% 18%, #181920 0%, #09090C 62%, #050507 100%)";

  const isLayerFullBleed = (layer: SceneLayer, isTopLevel: boolean): boolean =>
    project.packageType === "WALLPAPER" &&
    isTopLevel &&
    Math.abs(layer.x - 0.5) < 0.02 &&
    Math.abs(layer.y - 0.5) < 0.02 &&
    layer.width >= 1600 &&
    layer.height >= 900 &&
    (layer.kind === "VIDEO" ||
      layer.kind === "IMAGE" ||
      layer.kind === "SCENE_3D");

  // 4.5: per-render memo. Glass layers sharing a topIndex reuse one result, and each candidate's
  // visible/opacity formulas are evaluated at most once per render.
  type BackdropResolution = { backdrop: BackdropSourceInfo | null; hasIntermediateHtml: boolean };
  const backdropByTopIndex = new Map<number, BackdropResolution>();
  const candidateAliveByIndex = new Map<number, boolean>();

  const isBackdropCandidateAlive = (i: number): boolean => {
    const cached = candidateAliveByIndex.get(i);
    if (cached !== undefined) return cached;
    const candidate = project.layers[i];
    let alive = !candidate.hidden;
    if (alive && candidate.fx?.visible) {
      const v = evaluateFormula(candidate.fx.visible, telemetry, project.globals, now).trim().toLowerCase();
      if (v === "0" || v === "false" || v === "") alive = false;
    }
    if (alive) {
      const candOp = candidate.fx?.opacity
        ? Number(evaluateFormula(candidate.fx.opacity, telemetry, project.globals, now).replace(/%/g, "").trim())
        : candidate.opacity;
      if ((Number.isFinite(candOp) ? candOp : candidate.opacity) <= 0.001) alive = false;
    }
    candidateAliveByIndex.set(i, alive);
    return alive;
  };

  const resolveBackdropForTopIndex = (topIndex: number): BackdropResolution => {
    const hit = backdropByTopIndex.get(topIndex);
    if (hit) return hit;
    const res = computeBackdropForTopIndex(topIndex);
    backdropByTopIndex.set(topIndex, res);
    return res;
  };

  const computeBackdropForTopIndex = (topIndex: number): BackdropResolution => {
    if (isThumbnail) return { backdrop: null, hasIntermediateHtml: false };

    let hasIntermediateHtml = false;
    for (let i = topIndex - 1; i >= 0; i--) {
      const candidate = project.layers[i];
      if (!isBackdropCandidateAlive(i)) continue;

      if (candidate.kind === "SCENE_3D") {
        const el = mediaElementsRef.current.get(candidate.id) as HTMLCanvasElement | undefined;
        const full = isLayerFullBleed(candidate, true);
        const bw = full ? size.w : candidate.width * scale;
        const bh = full ? size.h : candidate.height * scale;
        const coversScene = full || (bw >= size.w * 0.6 && bh >= size.h * 0.6);
        if (coversScene && el && el.width > 1 && el.height > 1) {
          const boxLeft = full ? 0 : candidate.x * size.w - bw * 0.5;
          const boxTop = full ? 0 : candidate.y * size.h - bh * 0.5;
          return {
            backdrop: {
              key: `3d|${candidate.id}|${candidate.scene3d?.objects?.[0]?.modelSrc || "default"}`,
              kind: "canvas",
              live: true,
              element: el,
              intrinsicW: el.width,
              intrinsicH: el.height,
              drawLeft: boxLeft,
              drawTop: boxTop,
              drawW: Math.max(1, bw),
              drawH: Math.max(1, bh),
              frameToken: videoFrameTokenRef.current,
            },
            hasIntermediateHtml,
          };
        }
        hasIntermediateHtml = true;
      } else if (candidate.kind === "VIDEO" && candidate.video?.src) {
        const el = mediaElementsRef.current.get(candidate.id) as HTMLVideoElement | undefined;
        const full = isLayerFullBleed(candidate, true);
        const bw = full ? size.w : candidate.width * scale;
        const bh = full ? size.h : candidate.height * scale;
        const coversScene = full || (bw >= size.w * 0.6 && bh >= size.h * 0.6);
        if (coversScene && el && el.readyState >= 2 && el.videoWidth > 0 && el.videoHeight > 0) {
          const boxLeft = full ? 0 : candidate.x * size.w - bw * 0.5;
          const boxTop = full ? 0 : candidate.y * size.h - bh * 0.5;
          const rect = computeMediaDrawRect(
            boxLeft,
            boxTop,
            bw,
            bh,
            el.videoWidth,
            el.videoHeight,
            candidate.video.objectFit
          );
          return {
            backdrop: {
              key: `vid|${candidate.id}|${candidate.video.src}`,
              kind: "video",
              element: el,
              intrinsicW: el.videoWidth,
              intrinsicH: el.videoHeight,
              ...rect,
              frameToken: videoFrameTokenRef.current,
            },
            hasIntermediateHtml,
          };
        }
        hasIntermediateHtml = true;
      } else if (candidate.kind === "IMAGE" && candidate.image) {
        const isCover = candidate.image.sourceMode === "MEDIA_COVER";
        const rawSrc = isCover ? telemetry.media.cover_path || "" : candidate.image.src;
        const el = mediaElementsRef.current.get(candidate.id) as HTMLImageElement | undefined;
        const full = isLayerFullBleed(candidate, true);
        const bw = full ? size.w : candidate.width * scale;
        const bh = full ? size.h : candidate.height * scale;
        const coversScene = full || (bw >= size.w * 0.6 && bh >= size.h * 0.6);
        if (coversScene && rawSrc && el && el.complete && el.naturalWidth > 0 && el.naturalHeight > 0) {
          const boxLeft = full ? 0 : candidate.x * size.w - bw * 0.5;
          const boxTop = full ? 0 : candidate.y * size.h - bh * 0.5;
          const rect = computeMediaDrawRect(
            boxLeft,
            boxTop,
            bw,
            bh,
            el.naturalWidth,
            el.naturalHeight,
            candidate.image.objectFit
          );
          return {
            backdrop: {
              key: `img|${candidate.id}|${rawSrc}`,
              kind: "image",
              element: el,
              intrinsicW: el.naturalWidth,
              intrinsicH: el.naturalHeight,
              ...rect,
              frameToken: 0,
            },
            hasIntermediateHtml,
          };
        }
        hasIntermediateHtml = true;
      } else {
        hasIntermediateHtml = true;
      }
    }

    if (!isTransparentWidget) {
      const bgCanvas = getCssBackgroundCanvas(effectiveBackgroundCss);
      if (bgCanvas) {
        return {
          backdrop: {
            key: `bg|${effectiveBackgroundCss}`,
            kind: "canvas",
            element: bgCanvas,
            intrinsicW: bgCanvas.width,
            intrinsicH: bgCanvas.height,
            drawLeft: 0,
            drawTop: 0,
            drawW: Math.max(1, size.w),
            drawH: Math.max(1, size.h),
            frameToken: 0,
          },
          hasIntermediateHtml,
        };
      }
    }

    return { backdrop: null, hasIntermediateHtml };
  };

  const buildSnapTargets = (
    siblings: SceneLayer[],
    selfId: string,
    containerW: number,
    containerH: number
  ): { x: number[]; y: number[] } => {
    const xs = [0, 0.5, 1];
    const ys = [0, 0.5, 1];
    for (const sib of siblings) {
      if (sib.id === selfId || sib.hidden) continue;
      const el = layerElsRef.current.get(sib.id);
      const halfW = el ? el.offsetWidth / Math.max(containerW, 1) / 2 : 0;
      const halfH = el ? el.offsetHeight / Math.max(containerH, 1) / 2 : 0;
      xs.push(sib.x);
      ys.push(sib.y);
      if (halfW > 0.002) {
        xs.push(Math.max(0, sib.x - halfW), Math.min(1, sib.x + halfW));
      }
      if (halfH > 0.002) {
        ys.push(Math.max(0, sib.y - halfH), Math.min(1, sib.y + halfH));
      }
    }
    return { x: xs, y: ys };
  };

  const renderLayerInner = (
    layer: SceneLayer,
    isTopLevel: boolean,
    containerScale: number,
    siblings: SceneLayer[],
    topIndex: number,
    parentBox: { left: number; top: number; width: number; height: number }
  ): React.ReactNode => {
    if (layer.hidden) return null;

    // Evaluate per-property fx formulas live
    const fxVal = (key: string, fallback: number): number => {
      const expr = layer.fx?.[key];
      if (!expr) return fallback;
      const out = evaluateFormula(expr, telemetry, project.globals, now).replace(/%/g, "").trim();
      const num = Number(out);
      return Number.isFinite(num) ? num : fallback;
    };
    const fxStr = (key: string, fallback: string): string => {
      const expr = layer.fx?.[key];
      if (!expr) return fallback;
      const out = evaluateFormula(expr, telemetry, project.globals, now).trim();
      return out || fallback;
    };
    if (layer.fx?.visible) {
      const v = evaluateFormula(layer.fx.visible, telemetry, project.globals, now).trim().toLowerCase();
      if (v === "0" || v === "false" || v === "") return null;
    }

    const opacity = Math.max(0, Math.min(1, fxVal("opacity", layer.opacity)));
    if (opacity <= 0.001) return null;
    const rotation = fxVal("rotation", layer.rotation);
    const maxPxNum = 36 * layer.parallaxX * containerScale;
    const maxPyNum = 26 * layer.parallaxY * containerScale;
    const maxPx = maxPxNum.toFixed(2);
    const maxPy = maxPyNum.toFixed(2);
    const isFullBleedBackdrop = isLayerFullBleed(layer, isTopLevel);
    const wPx = isFullBleedBackdrop ? size.w : layer.width * containerScale;
    const hPx = isFullBleedBackdrop ? size.h : layer.height * containerScale;
    const isPrimarySelected = interactive && selectedLayerId === layer.id;
    const isSelected = interactive && selectedSet.has(layer.id);

    // Parallax stays OFF inside the Studio editor so items never dodge the cursor while positioning
    const hasParallax = !isThumbnail && !interactive && (layer.parallaxX !== 0 || layer.parallaxY !== 0);
    const layerCenterX = parentBox.left + layer.x * parentBox.width;
    const layerCenterY = parentBox.top + layer.y * parentBox.height;
    const parallaxMaxTuple: [number, number] | undefined = hasParallax ? [maxPxNum, maxPyNum] : undefined;

    const g = layer.glass;
    const safeId = layer.id.replace(/[^a-zA-Z0-9_-]/g, "_");
    const rgbFilterId = `prism-rgb-${uid}-${safeId}`;
    const chromaPx = Math.max(0, Math.min(6, g.chromaticAberration));
    const hasChroma = chromaPx > 0.05;
    const bevelPx = Math.max(4, Math.min(80, g.bevelWidth ?? 22)) * containerScale;
    const tintRgb = hexToRgbTuple(g.tintColor);

    const plateRadiusPx =
      layer.kind === "IMAGE" && layer.image
        ? layer.image.borderRadius * containerScale
        : g.borderRadius * containerScale;

    const resolvedBackdrop = g.enabled
      ? resolveBackdropForTopIndex(topIndex)
      : { backdrop: null, hasIntermediateHtml: false };
    const plateUseCssFallback =
      isThumbnail || isTransparentWidget || resolvedBackdrop.hasIntermediateHtml || !resolvedBackdrop.backdrop;
    const glyphUseCssFallback = isThumbnail || isTransparentWidget || !resolvedBackdrop.backdrop;

    return (
      <div
        key={layer.id}
        ref={(el) => {
          if (el) {
            layerElsRef.current.set(layer.id, el);
          } else {
            layerElsRef.current.delete(layer.id);
          }
        }}
        onPointerDown={(e) => {
          if (!interactive || test3dMode || roomActive || layer.locked) return;
          const isModSelect = e.ctrlKey || e.metaKey || e.altKey;
          // Allow direct child selection with Alt+Click or when already selected
          if (!isTopLevel && !selectedSet.has(layer.id) && !e.altKey) return;
          e.stopPropagation();

          if (isModSelect) {
            onSelectLayer?.(layer.id, {
              ctrlKey: e.ctrlKey || e.metaKey,
              altKey: e.altKey,
            });
            return;
          }

          const wasAlreadyInMulti = selectedSet.has(layer.id) && selectedSet.size > 1;
          // Preserve multi-selection when starting a drag on an already-selected layer
          if (!selectedSet.has(layer.id)) {
            onSelectLayer?.(layer.id);
          }

          const parentEl = (e.currentTarget.offsetParent as HTMLElement | null) ?? rootRef.current;
          const parentRect = parentEl?.getBoundingClientRect();
          const cw = parentRect?.width || size.w || 1;
          const ch = parentRect?.height || size.h || 1;
          const selfRect = e.currentTarget.getBoundingClientRect();
          const halfW = selfRect.width / cw / 2;
          const halfH = selfRect.height / ch / 2;

          const activePeers = selectedSet.has(layer.id)
            ? siblings
                .filter((s) => !s.locked && !s.hidden && selectedSet.has(s.id))
                .map((s) => ({ id: s.id, startX: s.x, startY: s.y }))
            : [{ id: layer.id, startX: layer.x, startY: layer.y }];
          const peerIds = new Set(activePeers.map((p) => p.id));
          const nonDraggedSiblings = siblings.filter((s) => !peerIds.has(s.id));
          const targets = buildSnapTargets(nonDraggedSiblings, layer.id, cw, ch);

          e.currentTarget.setPointerCapture(e.pointerId);
          dragRef.current = {
            mode: "MOVE",
            id: layer.id,
            isText: layer.kind === "TEXT",
            isTopLevel,
            containerW: cw,
            containerH: ch,
            startClientX: e.clientX,
            startClientY: e.clientY,
            startX: layer.x,
            startY: layer.y,
            startW: layer.kind === "TEXT" ? layer.text?.fontSize ?? 96 : layer.width,
            startH: layer.kind === "TEXT" ? layer.text?.fontSize ?? 96 : layer.height,
            halfW,
            halfH,
            snapTargetsX: targets.x,
            snapTargetsY: targets.y,
            moved: false,
            wasAlreadyInMulti,
            multiPeers: activePeers.length > 1 ? activePeers : undefined,
          };
          onDragStateChange?.(true);
        }}
        onClick={(e) => {
          if (interactive) {
            if (test3dMode || roomActive) return;
            if (!isTopLevel && !selectedSet.has(layer.id) && !e.altKey) return;
            e.stopPropagation();
            // On locked layers (where pointerDown returned early), handle selection here including modifiers
            if (layer.locked) {
              onSelectLayer?.(layer.id, {
                ctrlKey: e.ctrlKey || e.metaKey,
                altKey: e.altKey,
              });
            }
            return;
          }
          if (!isThumbnail && layer.clickAction && layer.clickAction !== "NONE") {
            e.stopPropagation();
            const action =
              layer.clickAction === "MEDIA_PLAY_PAUSE"
                ? "play_pause"
                : layer.clickAction === "MEDIA_NEXT"
                ? "next"
                : "prev";
            void invokeCmd("media_control", { action }).catch(() => {});
          }
        }}
        onDoubleClick={(e) => {
          if (!interactive || isTopLevel || roomActive) return;
          e.stopPropagation();
          onSelectLayer?.(layer.id);
        }}
        style={{
          position: "absolute",
          left: `${(layer.x * 100).toFixed(3)}%`,
          top: `${(layer.y * 100).toFixed(3)}%`,
          transform: hasParallax
            ? `translate(-50%, -50%) translate3d(calc(var(--px, 0) * ${maxPx}px), calc(var(--py, 0) * ${maxPy}px), 0) rotate(${rotation}deg)`
            : `translate(-50%, -50%) rotate(${rotation}deg)`,
          // Never create a CSS Backdrop Root on an ancestor of a backdrop-filter glass surface
          opacity: g.enabled ? undefined : opacity,
          mixBlendMode: g.enabled ? undefined : layer.blendMode,
          willChange: hasParallax && !paused ? "transform" : undefined,
          // Sala 3D: 2D layers stop hit-testing; only the edited 3D canvas opts back in
          pointerEvents: roomActive ? "none" : undefined,
          cursor: interactive
            ? isTopLevel || selectedLayerId === layer.id
              ? layer.locked
                ? "not-allowed"
                : "grab"
              : "default"
            : !isThumbnail && layer.clickAction && layer.clickAction !== "NONE"
            ? "pointer"
            : "default",
        }}
      >
        {/* Lightweight RGB Chromatic Filter only when aberration > 0 on SVG */}
        {g.enabled && hasChroma && !isThumbnail && layer.kind === "SVG" && (
          <svg className="absolute w-0 h-0 pointer-events-none" aria-hidden="true">
            <defs>
              <filter id={rgbFilterId} x="-25%" y="-25%" width="150%" height="150%">
                <feOffset in="SourceGraphic" dx={-chromaPx} dy={0} result="rShift" />
                <feColorMatrix
                  in="rShift"
                  type="matrix"
                  values="1 0 0 0 0  0 0.08 0 0 0  0 0.16 0 0 0  0 0 0 0.72 0"
                  result="rOut"
                />
                <feOffset in="SourceGraphic" dx={chromaPx} dy={0} result="bShift" />
                <feColorMatrix
                  in="bShift"
                  type="matrix"
                  values="0 0 0 0 0  0 0.78 0 0 0  0 0 1 0 0  0 0 0 0.76 0"
                  result="bOut"
                />
                <feBlend in="rOut" in2="bOut" mode="screen" result="rgbRim" />
                <feBlend in="rgbRim" in2="SourceGraphic" mode="screen" />
              </filter>
            </defs>
          </svg>
        )}

        {/* 3D INTERACTIVE SCENE LAYER (glTF / GLB + Draco/Meshopt/KTX2 + Zero-Copy Glass Backdrop) */}
        {layer.kind === "SCENE_3D" && (
          <div
            style={{
              width: `${wPx}px`,
              height: `${hPx}px`,
              position: "relative",
              overflow: "hidden",
              opacity: g.enabled ? opacity : undefined,
            }}
          >
            <Suspense
              fallback={
                <div className="w-full h-full flex items-center justify-center bg-[#07080b] text-xs text-[#A1A1AA]">
                  Inicializando motor 3D…
                </div>
              }
            >
              <Scene3DSlot
                layerId={layer.id}
                config={layer.scene3d}
                width={wPx}
                height={hPx}
                paused={paused}
                isThumbnail={isThumbnail}
                interactive={interactive}
                testMode={test3dMode}
                cursorNormRef={test3dMode ? previewCursorRef : cursorNormRef}
                subscribeCursorTick={test3dMode ? subscribePreviewTick : subscribeCursorTick}
                subscribeCursorClick={test3dMode ? subscribePreviewClick : subscribeCursorClick}
                reactiveSignalRef={reactiveSignalRef}
                subscribeReactiveTick={subscribeReactiveTick}
                mediaCoverPath={telemetry.media.cover_path || ""}
                mediaPlaying={Boolean(telemetry.media.playing)}
                cpuPercent={telemetry.cpu_percent || 0}
                powerState={powerState}
                mediaElementsRef={mediaElementsRef}
                setMediaRev={setMediaRev}
                videoFrameTokenRef={videoFrameTokenRef}
                frameTickSubsRef={frameTickSubsRef}
                onInspectScene3D={onInspectScene3D}
                onHoverMesh3D={onHoverMesh3D}
                onScene3DStats={onScene3DStats}
                room3d={roomActive && room3d && room3d.layerId === layer.id ? room3d : null}
              />
            </Suspense>
          </div>
        )}

        {/* VIDEO LAYER */}
        {layer.kind === "VIDEO" && layer.video && (
          <div
            style={{
              width: `${wPx}px`,
              height: `${hPx}px`,
              position: "relative",
              overflow: "hidden",
              borderRadius: `${g.enabled ? plateRadiusPx : layer.video.clipRadius * containerScale}px`,
              opacity: g.enabled ? opacity : undefined,
            }}
          >
            <RealVideoLayer
              src={layer.video.src}
              speed={layer.video.speed}
              muted={layer.video.muted}
              volume={layer.video.volume}
              paused={paused || isThumbnail}
              objectFit={layer.video.objectFit}
              clipRadius={layer.video.clipRadius * containerScale}
              onMediaEl={(el) => {
                if (el) mediaElementsRef.current.set(layer.id, el);
                else mediaElementsRef.current.delete(layer.id);
              }}
              onMediaReady={() => setMediaRev((r) => r + 1)}
            />
          </div>
        )}

        {/* IMAGE LAYER (PNG with real alpha transparency OR live Media Album Cover) */}
        {layer.kind === "IMAGE" && layer.image && (() => {
          const isCover = layer.image.sourceMode === "MEDIA_COVER";
          const rawSrc = isCover ? telemetry.media.cover_path || "" : layer.image.src;
          const imgUrl = rawSrc ? resolveMediaUrl(rawSrc) : "";
          return (
            <div
              style={{
                width: `${wPx}px`,
                height: `${hPx}px`,
                position: "relative",
                borderRadius: `${layer.image.borderRadius * containerScale}px`,
                overflow: "hidden",
              }}
            >
              {g.enabled && (
                <OpticalGlassSurface
                  mode="plate"
                  backdrop={resolvedBackdrop.backdrop}
                  videoFrameTokenRef={videoFrameTokenRef}
                  registerFrameTick={registerFrameTick}
                  cursorNormRef={cursorNormRef}
                  parallaxMaxPx={parallaxMaxTuple}
                  mediaRev={mediaRev}
                  viewportW={size.w}
                  viewportH={size.h}
                  centerX={layerCenterX}
                  centerY={layerCenterY}
                  width={wPx}
                  height={hPx}
                  rotationDeg={rotation}
                  scaleY={1}
                  glass={g}
                  containerScale={containerScale}
                  opacity={opacity}
                  radiusPx={plateRadiusPx}
                  tintRgb={tintRgb}
                  forceCssFallback={plateUseCssFallback}
                  isThumbnail={isThumbnail}
                />
              )}
              {imgUrl ? (
                <img
                  ref={(el) => {
                    if (el) mediaElementsRef.current.set(layer.id, el);
                    else mediaElementsRef.current.delete(layer.id);
                  }}
                  src={imgUrl}
                  crossOrigin={supportsCorsUrl(imgUrl) ? "anonymous" : undefined}
                  alt={layer.name}
                  draggable={false}
                  onLoad={() => setMediaRev((r) => r + 1)}
                  style={{
                    objectFit: layer.image.objectFit,
                    opacity: g.enabled ? opacity : undefined,
                  }}
                  className="relative w-full h-full pointer-events-none"
                />
              ) : (
                <div
                  style={{ opacity: g.enabled ? opacity : undefined }}
                  className="relative w-full h-full flex items-center justify-center bg-white/[0.06] border border-white/10 text-[11px] text-[#A1A1AA]"
                >
                  {isCover ? (
                    <span
                      className="w-7 h-7 text-white/40 [&>svg]:w-full [&>svg]:h-full"
                      dangerouslySetInnerHTML={{ __html: BUILTIN_SVG_ICONS.music }}
                    />
                  ) : (
                    "Imagem PNG / WebP"
                  )}
                </div>
              )}
            </div>
          );
        })()}

        {/* SVG & VECTOR ICON / MEDIA BUTTON LAYER */}
        {layer.kind === "SVG" && layer.svg && (() => {
          const effectiveIcon =
            layer.clickAction === "MEDIA_PLAY_PAUSE" &&
            (layer.svg.iconName === "play" || layer.svg.iconName === "pause")
              ? telemetry.media.playing
                ? "pause"
                : "play"
              : layer.svg.iconName;
          const customClean = layer.svg.customSvg.trim()
            ? sanitizeSvgMarkup(layer.svg.customSvg)
            : "";
          const sw = Math.max(0.6, Math.min(4.5, layer.svg.strokeWidth || 1.6));
          const svgMarkup = customClean
            ? customClean
            : (BUILTIN_SVG_ICONS[effectiveIcon] || BUILTIN_SVG_ICONS.prism).replace(
                /stroke-width="[^"]*"/g,
                `stroke-width="${sw}"`
              );
          const svgColor = colorWithAlpha(fxStr("color", layer.svg.color), layer.svg.colorAlpha);
          const isInteractiveBtn =
            !interactive && !isThumbnail && layer.clickAction && layer.clickAction !== "NONE";
          return (
            <div
              style={{
                width: `${wPx}px`,
                height: `${hPx}px`,
                color: svgColor,
                opacity: g.enabled ? opacity : undefined,
                filter: g.enabled && hasChroma ? `url(#${rgbFilterId})` : undefined,
              }}
              className={`flex items-center justify-center [&>svg]:w-full [&>svg]:h-full ${
                isInteractiveBtn
                  ? "transition-transform duration-150 hover:scale-110 active:scale-95"
                  : "pointer-events-none"
              }`}
              dangerouslySetInnerHTML={{ __html: svgMarkup }}
            />
          );
        })()}

        {/* PROGRESS BAR LAYER (Media Timeline, CPU, RAM, Battery) */}
        {layer.kind === "PROGRESS" && layer.progress && (() => {
          const src = layer.progress.source;
          let ratio = 0;
          if (src === "MEDIA") {
            const p = computeLiveMediaProgress(telemetry.media, now.getTime());
            ratio = p.durationSecs > 0 ? p.ratio : isThumbnail || interactive ? 0.38 : 0;
          } else if (src === "CPU") {
            ratio = Math.max(0, Math.min(1, telemetry.cpu_percent / 100));
          } else if (src === "RAM") {
            ratio = Math.max(0, Math.min(1, telemetry.ram_percent / 100));
          } else if (src === "BATTERY") {
            ratio = Math.max(0, Math.min(1, telemetry.battery_percent / 100));
          }
          const trackBg = colorWithAlpha(layer.progress.trackColor, layer.progress.trackAlpha);
          const fillBg = colorWithAlpha(layer.progress.fillColor, layer.progress.fillAlpha);
          const radius = `${layer.progress.borderRadius * containerScale}px`;
          return (
            <div
              style={{
                width: `${wPx}px`,
                height: `${Math.max(2, hPx)}px`,
                opacity: g.enabled ? opacity : undefined,
                background: trackBg,
                borderRadius: radius,
                overflow: "hidden",
              }}
              className="pointer-events-none relative"
            >
              <div
                style={{
                  width: `${(ratio * 100).toFixed(2)}%`,
                  height: "100%",
                  background: fillBg,
                  borderRadius: radius,
                  transition: "width 320ms linear",
                }}
              />
            </div>
          );
        })()}

        {/* GLASS / OPTICAL CRYSTAL PLATE */}
        {layer.kind === "GLASS" && (
          <div
            style={{
              width: `${wPx}px`,
              height: `${hPx}px`,
              position: "relative",
              borderRadius: `${plateRadiusPx}px`,
            }}
          >
            {g.enabled && (
              <OpticalGlassSurface
                mode="plate"
                backdrop={resolvedBackdrop.backdrop}
                videoFrameTokenRef={videoFrameTokenRef}
                registerFrameTick={registerFrameTick}
                cursorNormRef={cursorNormRef}
                parallaxMaxPx={parallaxMaxTuple}
                mediaRev={mediaRev}
                viewportW={size.w}
                viewportH={size.h}
                centerX={layerCenterX}
                centerY={layerCenterY}
                width={wPx}
                height={hPx}
                rotationDeg={rotation}
                scaleY={1}
                glass={g}
                containerScale={containerScale}
                opacity={opacity}
                radiusPx={plateRadiusPx}
                tintRgb={tintRgb}
                forceCssFallback={plateUseCssFallback}
                isThumbnail={isThumbnail}
              />
            )}
          </div>
        )}

        {/* TEXT / CLOCK / LIVE FX LAYER (Real Color Alpha + Real WebGL2 Optical Glyph/Pill Glass) */}
        {layer.kind === "TEXT" && layer.text && (() => {
          const rawColor = fxStr("color", layer.text.color);
          const pigmentRgb = hexToRgbTuple(rawColor);
          const glyphAlpha = layer.text.colorAlpha === undefined ? 1 : Math.max(0, Math.min(1, layer.text.colorAlpha));
          const fillRgba = colorWithAlpha(rawColor, glyphAlpha);
          const resolved = evaluateFormula(layer.text.content, telemetry, project.globals, now);
          const scaledFont = layer.text.fontSize * containerScale;
          const stretchY = Math.max(1, Math.min(2.8, layer.text.stemStretch || 1));
          const safeFamily = (layer.text.fontFamily || "Outfit").replace(/"/g, "");
          const fontStack = `"${safeFamily}", "Outfit", "Inter", "Segoe UI", sans-serif`;
          const textMode = g.textMode ?? "GLYPH";

          const baseTextStyle: React.CSSProperties = {
            fontFamily: fontStack,
            fontSize: `${scaledFont}px`,
            fontWeight: layer.text.fontWeight,
            letterSpacing: `${layer.text.letterSpacing}em`,
            textAlign: layer.text.align,
            color: fillRgba,
            lineHeight: 1.06,
            whiteSpace: "pre-wrap",
            fontVariantNumeric: "tabular-nums",
            textShadow:
              g.enabled && hasChroma
                ? `-${chromaPx}px 0 8px rgba(255,30,66,0.42), ${chromaPx}px 0 8px rgba(0,210,255,0.46), 0 2px ${layer.text.shadowBlur}px rgba(0,0,0,0.55)`
                : layer.text.shadowBlur > 0
                ? `0 2px ${layer.text.shadowBlur}px rgba(0,0,0,0.65)`
                : undefined,
          };

          if (g.enabled && textMode === "PILL") {
            const padX = Math.max(12, scaledFont * 0.34);
            const padY = Math.max(6, scaledFont * 0.16);
            return (
              <div
                style={{
                  position: "relative",
                  padding: `${padY}px ${padX}px`,
                  borderRadius: `${plateRadiusPx}px`,
                  transform: stretchY > 1.01 ? `scaleY(${stretchY.toFixed(3)})` : undefined,
                  transformOrigin: "center center",
                }}
                className="pointer-events-none"
              >
                <OpticalGlassSurface
                  mode="plate"
                  backdrop={resolvedBackdrop.backdrop}
                  videoFrameTokenRef={videoFrameTokenRef}
                  registerFrameTick={registerFrameTick}
                  cursorNormRef={cursorNormRef}
                  parallaxMaxPx={parallaxMaxTuple}
                  mediaRev={mediaRev}
                  viewportW={size.w}
                  viewportH={size.h}
                  centerX={layerCenterX}
                  centerY={layerCenterY}
                  width={0}
                  height={0}
                  rotationDeg={rotation}
                  scaleY={stretchY}
                  glass={g}
                  containerScale={containerScale}
                  opacity={opacity}
                  radiusPx={plateRadiusPx}
                  tintRgb={tintRgb}
                  forceCssFallback={plateUseCssFallback}
                  isThumbnail={isThumbnail}
                />
                <div style={{ ...baseTextStyle, position: "relative", opacity }}>{resolved}</div>
              </div>
            );
          }

          if (g.enabled && textMode === "GLYPH") {
            const maps = getGlyphGlassMaps({
              text: resolved,
              fontStack,
              fontSize: scaledFont,
              fontWeight: layer.text.fontWeight,
              letterSpacingEm: layer.text.letterSpacing,
              align: layer.text.align,
              bevelPx,
              fontRev,
            });
            if (maps) {
              return (
                <div
                  style={{
                    position: "relative",
                    width: `${maps.width}px`,
                    height: `${maps.height}px`,
                    transform: stretchY > 1.01 ? `scaleY(${stretchY.toFixed(3)})` : undefined,
                    transformOrigin: "center center",
                  }}
                  className="pointer-events-none"
                >
                  {/* Real GPU WebGL2 Optical Glass inside anti-aliased 3D glyph strokes (zero PNG encoding!) */}
                  <OpticalGlassSurface
                    mode="glyph"
                    backdrop={resolvedBackdrop.backdrop}
                    videoFrameTokenRef={videoFrameTokenRef}
                    registerFrameTick={registerFrameTick}
                    cursorNormRef={cursorNormRef}
                    parallaxMaxPx={parallaxMaxTuple}
                    mediaRev={mediaRev}
                    viewportW={size.w}
                    viewportH={size.h}
                    centerX={layerCenterX}
                    centerY={layerCenterY}
                    width={maps.width}
                    height={maps.height}
                    rotationDeg={rotation}
                    scaleY={stretchY}
                    glass={g}
                    containerScale={containerScale}
                    opacity={opacity}
                    radiusPx={0}
                    tintRgb={tintRgb}
                    glyphMaps={maps}
                    pigmentRgba={[pigmentRgb[0], pigmentRgb[1], pigmentRgb[2], glyphAlpha]}
                    glyphDropShadowPx={layer.text.shadowBlur}
                    forceCssFallback={glyphUseCssFallback}
                    isThumbnail={isThumbnail}
                  />
                </div>
              );
            }
          }

          return (
            <div
              style={{
                ...baseTextStyle,
                opacity: g.enabled ? opacity : undefined,
                transform: stretchY > 1.01 ? `scaleY(${stretchY.toFixed(3)})` : undefined,
                transformOrigin: "center center",
              }}
              className="pointer-events-none"
            >
              {resolved}
            </div>
          );
        })()}

        {/* GROUP / IMPORTED WIDGET LAYER */}
        {layer.kind === "GROUP" && (
          <div
            style={{
              width: `${wPx}px`,
              height: `${hPx}px`,
              position: "relative",
              borderRadius: `${plateRadiusPx}px`,
            }}
          >
            {g.enabled && (
              <OpticalGlassSurface
                mode="plate"
                backdrop={resolvedBackdrop.backdrop}
                videoFrameTokenRef={videoFrameTokenRef}
                registerFrameTick={registerFrameTick}
                cursorNormRef={cursorNormRef}
                parallaxMaxPx={parallaxMaxTuple}
                mediaRev={mediaRev}
                viewportW={size.w}
                viewportH={size.h}
                centerX={layerCenterX}
                centerY={layerCenterY}
                width={wPx}
                height={hPx}
                rotationDeg={rotation}
                scaleY={1}
                glass={g}
                containerScale={containerScale}
                opacity={opacity}
                radiusPx={plateRadiusPx}
                tintRgb={tintRgb}
                forceCssFallback={plateUseCssFallback}
                isThumbnail={isThumbnail}
              />
            )}
            <div
              className="absolute inset-0"
              style={g.enabled && opacity < 0.999 ? { opacity } : undefined}
            >
              {(layer.children ?? []).map((child) =>
                renderLayer(child, false, containerScale, layer.children ?? [], topIndex, {
                  left: layerCenterX - wPx * 0.5,
                  top: layerCenterY - hPx * 0.5,
                  width: wPx,
                  height: hPx,
                })
              )}
            </div>
          </div>
        )}

        {/* Interactive Selection Box + Corner Resize Handle (pure absolute, zero layout shift) */}
        {isSelected && !test3dMode && !roomActive && (
          <>
            <div
              className={`pointer-events-none absolute -inset-1.5 rounded-xl border ${
                isPrimarySelected
                  ? "border-white/75"
                  : "border-white/55 border-dashed"
              }`}
            />
            {isPrimarySelected && !layer.locked && onResizeLayer && (
              <button
                type="button"
                aria-label="Redimensionar"
                onPointerDown={(e) => {
                  e.stopPropagation();
                  e.currentTarget.setPointerCapture(e.pointerId);
                  dragRef.current = {
                    mode: "RESIZE",
                    id: layer.id,
                    isText: layer.kind === "TEXT",
                    isTopLevel,
                    containerW: size.w,
                    containerH: size.h,
                    startClientX: e.clientX,
                    startClientY: e.clientY,
                    startX: layer.x,
                    startY: layer.y,
                    startW: layer.kind === "TEXT" ? layer.text?.fontSize ?? 96 : layer.width,
                    startH: layer.kind === "TEXT" ? layer.text?.fontSize ?? 96 : layer.height,
                    halfW: 0,
                    halfH: 0,
                    snapTargetsX: [],
                    snapTargetsY: [],
                    moved: true,
                    wasAlreadyInMulti: false,
                  };
                  onDragStateChange?.(true);
                }}
                className="absolute -right-2 -bottom-2 w-3.5 h-3.5 rounded-full bg-white border-2 border-black shadow-md cursor-nwse-resize z-40"
              />
            )}
          </>
        )}
      </div>
    );
  };

  // 4.6: layers that read neither telemetry nor the clock are memoized, so telemetry (1.5 s), clock (1 s)
  // and snap-guide updates no longer rebuild their subtrees. Anything the layer's handlers/render close over
  // (except telemetry/now/project, unused by static layers) is listed in `deps`.
  const selectionKey = selectedSet.size > 0 ? Array.from(selectedSet).join(",") : "";
  const renderLayer = (
    layer: SceneLayer,
    isTopLevel: boolean,
    containerScale: number,
    siblings: SceneLayer[],
    topIndex: number,
    parentBox: { left: number; top: number; width: number; height: number }
  ): React.ReactNode => {
    if (layer.hidden) return null;
    const render = () => renderLayerInner(layer, isTopLevel, containerScale, siblings, topIndex, parentBox);
    if (!isLayerStatic(layer)) return render();
    return (
      <MemoLayerNode
        key={layer.id}
        render={render}
        deps={[
          layer,
          siblings,
          isTopLevel,
          containerScale,
          topIndex,
          parentBox.left,
          parentBox.top,
          parentBox.width,
          parentBox.height,
          size.w,
          size.h,
          scale,
          uid,
          project.packageType,
          interactive,
          isThumbnail,
          test3dMode,
          roomActive,
          paused,
          powerState,
          fontRev,
          mediaRev,
          selectionKey,
          selectedLayerId,
          onSelectLayer,
          onResizeLayer,
          onDragStateChange,
          onInspectScene3D,
          onHoverMesh3D,
          onScene3DStats,
          room3d,
        ]}
      />
    );
  };

  return (
    <div
      ref={rootRef}
      onPointerMove={handlePointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onPointerLeave={() => {
        if (interactive && test3dMode) {
          previewCursorRef.current = [0.5, 0.5];
          previewTickSubsRef.current.forEach((cb) => cb());
        }
      }}
      onClick={(e) => {
        if (!interactive) return;
        if (roomActive) return;
        if (test3dMode) {
          const rect = rootRef.current?.getBoundingClientRect();
          if (rect && rect.width > 1 && rect.height > 1) {
            const nx = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
            const ny = Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height));
            previewCursorRef.current = [nx, ny];
            previewClickSubsRef.current.forEach((cb) => cb(nx, ny));
          }
          return;
        }
        onSelectLayer?.(null);
      }}
      style={{
        background: effectiveBackgroundCss,
        animationPlayState: paused ? "paused" : "running",
        contain: !interactive ? "layout style paint" : undefined,
        cursor: interactive && test3dMode ? "crosshair" : undefined,
      }}
      className="relative w-full h-full overflow-hidden select-none"
    >
      {/* Ordered Layer Stack (zIndex = array order) */}
      {project.layers.map((layer, idx) =>
        renderLayer(layer, true, scale, project.layers, idx, {
          left: 0,
          top: 0,
          width: size.w,
          height: size.h,
        })
      )}

      {/* Optional Global Dim Overlay */}
      {project.dim > 0 && !isTransparentWidget && (
        <div
          className="pointer-events-none absolute inset-0 bg-black"
          style={{ opacity: Math.min(0.85, project.dim) }}
        />
      )}

      {/* Smart Magnetic Alignment Guides (Minimalist Monochrome Hairlines) */}
      {snapGuide.x !== null && (
        <div
          style={{ left: `${(snapGuide.x * 100).toFixed(3)}%` }}
          className={`pointer-events-none absolute top-0 bottom-0 w-px z-50 transition-opacity ${
            Math.abs(snapGuide.x - 0.5) < 0.001
              ? "bg-white/80 shadow-[0_0_6px_rgba(255,255,255,0.35)]"
              : "bg-white/45"
          }`}
        />
      )}
      {snapGuide.y !== null && (
        <div
          style={{ top: `${(snapGuide.y * 100).toFixed(3)}%` }}
          className={`pointer-events-none absolute left-0 right-0 h-px z-50 transition-opacity ${
            Math.abs(snapGuide.y - 0.5) < 0.001
              ? "bg-white/80 shadow-[0_0_6px_rgba(255,255,255,0.35)]"
              : "bg-white/45"
          }`}
        />
      )}
    </div>
  );
};
