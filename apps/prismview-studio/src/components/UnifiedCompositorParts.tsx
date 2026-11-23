// Peças do UnifiedCompositor (PLANO_CORRECAO 5.1): SVG saneado, fontes, cores/mídia, vídeo real e slot 3D lazy.
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
  Scene3DPickedPart,
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
  showSkeleton: boolean;
  onEditorSync: () => void;
  onSelectObject: (objectId: string | null) => void;
  onPickPart?: (part: Scene3DPickedPart | null) => void;
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
export function sanitizeSvgMarkup(raw: string): string {
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

export function snapCoordinate(
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

export function ensureCustomFontLoaded(fontFamily: string, fontPath: string): Promise<boolean> {
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

export function hexToRgbTuple(hex: string): [number, number, number] {
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

export function colorWithAlpha(color: string, alpha?: number): string {
  const a = alpha === undefined ? 1 : Math.max(0, Math.min(1, alpha));
  const raw = (color || "#FFFFFF").trim();
  if (a >= 0.999 && !raw.startsWith("#")) return raw;
  if (raw.startsWith("#")) {
    const [r, g, b] = hexToRgbTuple(raw);
    return `rgba(${r}, ${g}, ${b}, ${a.toFixed(3)})`;
  }
  return raw;
}

export function supportsCorsUrl(url: string): boolean {
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

export function computeMediaDrawRect(
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

export function sceneHasAnyGlass(layers: SceneLayer[]): boolean {
  for (const l of layers) {
    if (l.hidden) continue;
    if (l.glass?.enabled) return true;
    if (l.children?.length && sceneHasAnyGlass(l.children)) return true;
  }
  return false;
}

export function sceneHasVisibleVideo(layers: SceneLayer[]): boolean {
  for (const l of layers) {
    if (l.hidden) continue;
    if (l.kind === "VIDEO" && l.video?.src) return true;
    if (l.children?.length && sceneHasVisibleVideo(l.children)) return true;
  }
  return false;
}

/** True when the layer subtree never reads telemetry, the clock, media state or the backdrop stack. */
export function isLayerStatic(layer: SceneLayer): boolean {
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

export const MemoLayerNode = React.memo(
  function MemoLayerNode({ render }: MemoLayerNodeProps) {
    return <>{render()}</>;
  },
  (prev, next) =>
    prev.deps.length === next.deps.length && prev.deps.every((d, i) => Object.is(d, next.deps[i]))
);

export function layersNeedClockTick(layers: SceneLayer[], mediaPlaying: boolean): boolean {
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

export const RealVideoLayer: React.FC<VideoElementProps> = ({
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

export const Scene3DSlot: React.FC<Scene3DSlotProps> = (p) => {
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
            showSkeleton: room.showSkeleton,
            onEditorSync: room.onEditorSync,
            onSelectObject: room.onSelectObject,
            onPickPart: room.onPickPart,
            onCommitObjectTransform,
            editorApiRef: room.editorApiRef,
          }
        : null)}
    />
  );
};
