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
import { HUB_MOCK_NOW, HUB_MOCK_WEATHER } from "../engine/hubMock";
import { StemText } from "./StemText";
import { warpGlyphMaps } from "../utils/stemGlyphMaps";
// Type-only: keeps the 3D engine in its own lazy chunk
import type { Scene3DEditorApi, Scene3DGizmoMode, Scene3DStats } from "./Scene3DLayer";

import { MemoLayerNode, RealVideoLayer, Room3DEditBinding, Scene3DSlot, colorWithAlpha, computeMediaDrawRect, ensureCustomFontLoaded, ensureSystemFontLoaded, hexToRgbTuple, isLayerStatic, layersNeedClockTick, sanitizeSvgMarkup, sceneHasAnyGlass, sceneHasVisibleVideo, supportsCorsUrl } from "./UnifiedCompositorParts";
import { snapAxisInto, type SnapAxisBuffer } from "../utils/align";

export type LayerResizePatch = {
  width?: number;
  height?: number;
  fontSize?: number;
  /** `null` volta para largura automática (sem quebra de linha) */
  textMaxWidth?: number | null;
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
  /** Só miniaturas: `full` = vidro/3D/vídeo reais (3 por vez no Hub); `basic` (padrão) = só o básico. */
  thumbnailTier?: "full" | "still";
  /** Miniatura `still`: imagem parada (layerId → URL) no lugar de vídeo/3D vivos. */
  stillFrames?: Readonly<Record<string, string>>;
  /** Miniatura `full`: avisa quando uma camada VIDEO/3D tem quadro para capturar. */
  onThumbFrame?: (layerId: string, el: HTMLCanvasElement | HTMLVideoElement) => void;
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
  /** Alt+arrastar no canvas: clona as camadas no lugar e retorna os novos IDs na mesma ordem. */
  onDuplicateForDrag?: (ids: string[]) => string[];
  onResizeLayer?: (id: string, patch: LayerResizePatch) => void;
  onDragStateChange?: (dragging: boolean) => void;
  /** Menu de botão direito no canvas (um único listener no container raiz). */
  onContextMenuCanvas?: (layerId: string | null, clientX: number, clientY: number) => void;
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
  thumbnailTier = "still",
  stillFrames,
  onThumbFrame,
  interactive = false,
  test3dMode = false,
  selectedLayerId = null,
  selectedLayerIds,
  onSelectLayer,
  onMoveLayer,
  onMoveLayers,
  onDuplicateForDrag,
  onResizeLayer,
  onDragStateChange,
  onContextMenuCanvas,
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
  const [now, setNow] = useState<Date>(() => (isThumbnail ? HUB_MOCK_NOW : new Date()));
  /** Miniatura `still`: tudo parado; vídeo/3D viram imagem em cache; vidro só em CSS (sem WebGL). */
  const lite = isThumbnail && thumbnailTier !== "full";
  const fxWeather = isThumbnail ? HUB_MOCK_WEATHER : undefined;
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

  // Studio 3D Test Mode cursor/click/drag channels (zero React re-renders during live mouse test)
  const previewCursorRef = useRef<[number, number] | null>([0.5, 0.5]);
  const previewTestOrbitRef = useRef<[number, number]>([0, 0]);
  const previewTickSubsRef = useRef<Set<() => void>>(new Set());
  const previewClickSubsRef = useRef<Set<(x: number, y: number) => void>>(new Set());
  const testDragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    lastX: number;
    lastY: number;
    moved: boolean;
  } | null>(null);

  useEffect(() => {
    if (!test3dMode) {
      previewTestOrbitRef.current = [0, 0];
      testDragRef.current = null;
      previewTickSubsRef.current.forEach((cb) => cb());
    }
  }, [test3dMode]);

  const normalizeScene3DCursor = (clientX: number, clientY: number): [number, number] | null => {
    const targetEl = rootRef.current?.querySelector('[data-prism-scene3d="true"]') ?? rootRef.current;
    const rect = targetEl?.getBoundingClientRect();
    if (!rect || rect.width <= 1 || rect.height <= 1) return null;
    return [
      Math.max(0, Math.min(1, (clientX - rect.left) / rect.width)),
      Math.max(0, Math.min(1, (clientY - rect.top) / rect.height)),
    ];
  };

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
    resizeHandle?: "nw" | "ne" | "sw" | "se" | "w" | "e" | "n" | "s";
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
    /** Texto: fontSize inicial (px lógicos), maxWidth inicial (0 = auto) e largura renderizada inicial (px lógicos). */
    startFont?: number;
    startTextMaxW?: number;
    startBoxW?: number;
    startBoxH?: number;
    naturalTextW?: number;
    halfW: number;
    halfH: number;
    snapTargetsX: number[];
    snapTargetsY: number[];
    moved: boolean;
    wasAlreadyInMulti: boolean;
    altDuplicatePending?: boolean;
    altDuplicated?: boolean;
    multiPeers?: Array<{ id: string; startX: number; startY: number }>;
  } | null>(null);
  const snapBufXRef = useRef<SnapAxisBuffer>({ snapped: 0.5, guide: null });
  const snapBufYRef = useRef<SnapAxisBuffer>({ snapped: 0.5, guide: null });
  const dragMoveRafRef = useRef<number | null>(null);
  const pendingDragMoveRef = useRef<{
    clientX: number;
    clientY: number;
    altKey: boolean;
    shiftKey: boolean;
  } | null>(null);

  useEffect(() => {
    return () => {
      if (dragMoveRafRef.current !== null) {
        cancelAnimationFrame(dragMoveRafRef.current);
        dragMoveRafRef.current = null;
      }
      pendingDragMoveRef.current = null;
    };
  }, []);

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
        if (l.kind === "TEXT" && l.text) {
          if (l.text.fontPath) {
            void ensureCustomFontLoaded(l.text.fontFamily, l.text.fontPath).then((loaded) => {
              if (active && loaded) setFontRev((r) => r + 1);
            });
          } else if (l.text.fontFamily) {
            void ensureSystemFontLoaded(l.text.fontFamily, l.text.fontWeight).then((loaded) => {
              if (active && loaded) setFontRev((r) => r + 1);
            });
          }
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

  const flushPendingDragMove = () => {
    const drag = dragRef.current;
    const pending = pendingDragMoveRef.current;
    pendingDragMoveRef.current = null;
    if (!drag || !pending) return;

    if (
      !drag.moved &&
      Math.hypot(pending.clientX - drag.startClientX, pending.clientY - drag.startClientY) >= 3
    ) {
      drag.moved = true;
      // Alt+Arrastar: clona os itens arrastados 1× no início do movimento e passa a mover as cópias.
      if (drag.mode === "MOVE" && drag.altDuplicatePending && !drag.altDuplicated && onDuplicateForDrag) {
        drag.altDuplicated = true;
        // Inclui a própria posição original como alvo de guia inteligente para alinhar a cópia à origem!
        drag.snapTargetsX.push(drag.startX);
        drag.snapTargetsY.push(drag.startY);
        if (drag.halfW > 0.002) {
          drag.snapTargetsX.push(
            Math.max(0, drag.startX - drag.halfW),
            Math.min(1, drag.startX + drag.halfW)
          );
        }
        if (drag.halfH > 0.002) {
          drag.snapTargetsY.push(
            Math.max(0, drag.startY - drag.halfH),
            Math.min(1, drag.startY + drag.halfH)
          );
        }
        const sourceIds =
          drag.multiPeers && drag.multiPeers.length > 1
            ? drag.multiPeers.map((p) => p.id)
            : [drag.id];
        const clonedIds = onDuplicateForDrag(sourceIds);
        if (clonedIds.length === sourceIds.length && clonedIds.length > 0) {
          const primaryIdx = Math.max(0, sourceIds.indexOf(drag.id));
          drag.id = clonedIds[primaryIdx] ?? clonedIds[0];
          if (drag.multiPeers && drag.multiPeers.length > 1) {
            drag.multiPeers = drag.multiPeers.map((p, idx) => ({
              ...p,
              id: clonedIds[idx] ?? p.id,
            }));
          }
        }
      }
    }

    if (drag.mode === "MOVE" && (onMoveLayers || onMoveLayer)) {
      const cw = Math.max(drag.containerW, 1);
      const ch = Math.max(drag.containerH, 1);
      let dx = (pending.clientX - drag.startClientX) / cw;
      let dy = (pending.clientY - drag.startClientY) / ch;
      // Shift durante movimento trava no eixo predominante (horizontal ou vertical).
      if (pending.shiftKey) {
        if (Math.abs(dx) >= Math.abs(dy)) dy = 0;
        else dx = 0;
      }
      const rawX = Math.max(0, Math.min(1, drag.startX + dx));
      const rawY = Math.max(0, Math.min(1, drag.startY + dy));
      const snapX = snapAxisInto(snapBufXRef.current, rawX, drag.halfW, drag.snapTargetsX, 0.012);
      const snapY = snapAxisInto(snapBufYRef.current, rawY, drag.halfH, drag.snapTargetsY, 0.012);
      const gx = drag.isTopLevel ? snapX.guide : null;
      const gy = drag.isTopLevel ? snapY.guide : null;
      // 4.6: pointermove fires constantly; only re-render the tree when a guide actually changes.
      setSnapGuide((prev) => (prev.x === gx && prev.y === gy ? prev : { x: gx, y: gy }));
      const appliedDx = snapX.snapped - drag.startX;
      const appliedDy = snapY.snapped - drag.startY;
      const peers = drag.multiPeers;
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
          drag.id,
          Number(snapX.snapped.toFixed(4)),
          Number(snapY.snapped.toFixed(4))
        );
      }
    } else if (drag.mode === "RESIZE" && onResizeLayer) {
      const handle = drag.resizeHandle ?? "se";
      const dxLog = (pending.clientX - drag.startClientX) / Math.max(scale, 0.05);
      const dyLog = (pending.clientY - drag.startClientY) / Math.max(scale, 0.05);
      const signX = handle.includes("e") ? 1 : handle.includes("w") ? -1 : 0;
      const signY = handle.includes("s") ? 1 : handle.includes("n") ? -1 : 0;

      if (drag.isText) {
        const startFont = drag.startFont ?? drag.startW;
        const startBoxW = Math.max(40, drag.startBoxW ?? 320);
        const startBoxH = Math.max(24, drag.startBoxH ?? startFont * 1.1);
        const startMaxW = drag.startTextMaxW ?? 0;

        if ((handle === "e" || handle === "w") && !pending.shiftKey) {
          // Alças laterais (estilo Canva): mudam APENAS a largura da caixa (quebra de linha), sem alterar a fonte.
          const rawW = Math.max(60, Math.min(2400, Math.round(startBoxW + signX * dxLog * 2)));
          const natW = drag.naturalTextW ?? 0;
          // Se puxou além da largura natural em 1 linha, destrava de volta para auto-width (1 linha limpa).
          if (natW > 0 && rawW >= natW + 12) {
            onResizeLayer(drag.id, { textMaxWidth: null });
          } else {
            onResizeLayer(drag.id, { textMaxWidth: rawW });
          }
        } else {
          // Alças de canto ou Shift: escalam proporcionalmente fonte + largura da caixa mantendo as mesmas quebras!
          const diag = Math.hypot(startBoxW, startBoxH);
          const proj = (signX * dxLog * 2 * startBoxW + signY * dyLog * 2 * startBoxH) / Math.max(diag, 1);
          const ratio = Math.max(0.15, (diag + proj) / Math.max(diag, 1));
          const nextFont = Math.max(10, Math.min(340, Math.round(startFont * ratio)));
          if (startMaxW > 0) {
            const actualRatio = nextFont / Math.max(startFont, 1);
            const nextMaxW = Math.max(60, Math.min(2400, Math.round(startMaxW * actualRatio)));
            onResizeLayer(drag.id, { fontSize: nextFont, textMaxWidth: nextMaxW });
          } else {
            onResizeLayer(drag.id, { fontSize: nextFont });
          }
        }
      } else if ((handle === "e" || handle === "w") && !pending.shiftKey) {
        const nw = Math.max(24, Math.min(2400, Math.round(drag.startW + signX * dxLog * 2)));
        onResizeLayer(drag.id, { width: nw });
      } else if ((handle === "n" || handle === "s") && !pending.shiftKey) {
        const nh = Math.max(20, Math.min(1400, Math.round(drag.startH + signY * dyLog * 2)));
        onResizeLayer(drag.id, { height: nh });
      } else if (pending.altKey && !pending.shiftKey) {
        // Alt no canto = redimensionamento livre nos dois eixos; Shift sempre mantém a proporção!
        const nw = Math.max(24, Math.min(2400, Math.round(drag.startW + signX * dxLog * 2)));
        const nh = Math.max(20, Math.min(1400, Math.round(drag.startH + signY * dyLog * 2)));
        onResizeLayer(drag.id, { width: nw, height: nh });
      } else {
        // Canto ou Shift: mantém a proporção exata da caixa.
        const diag = Math.hypot(drag.startW, drag.startH);
        const proj =
          handle === "e" || handle === "w"
            ? ((drag.startW + signX * dxLog * 2) / Math.max(drag.startW, 1) - 1) * diag
            : handle === "n" || handle === "s"
            ? ((drag.startH + signY * dyLog * 2) / Math.max(drag.startH, 1) - 1) * diag
            : (signX * dxLog * 2 * drag.startW + signY * dyLog * 2 * drag.startH) / Math.max(diag, 1);
        const ratio = Math.max(0.1, (diag + proj) / Math.max(diag, 1));
        const nw = Math.max(24, Math.min(2400, Math.round(drag.startW * ratio)));
        const nh = Math.max(20, Math.min(1400, Math.round(drag.startH * ratio)));
        onResizeLayer(drag.id, { width: nw, height: nh });
      }
    }
  };

  const handlePointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (paused && !interactive) return;
    if (interactive && test3dMode) {
      const norm = normalizeScene3DCursor(e.clientX, e.clientY);
      if (norm) {
        previewCursorRef.current = norm;
      }
      const td = testDragRef.current;
      if (td && td.pointerId === e.pointerId) {
        const dx = e.clientX - td.lastX;
        const dy = e.clientY - td.lastY;
        td.lastX = e.clientX;
        td.lastY = e.clientY;
        if (!td.moved && Math.hypot(e.clientX - td.startX, e.clientY - td.startY) > 4) {
          td.moved = true;
        }
        if (td.moved) {
          const [curAz, curEl] = previewTestOrbitRef.current;
          previewTestOrbitRef.current = [
            curAz - dx * 0.35,
            Math.max(-75, Math.min(75, curEl + dy * 0.25)),
          ];
        }
      }
      previewTickSubsRef.current.forEach((cb) => cb());
      return;
    }
    if (!interactive || !dragRef.current) return;

    // Coalesce high-polling mice (500-1000 Hz) to at most 1 React update per animation frame.
    pendingDragMoveRef.current = {
      clientX: e.clientX,
      clientY: e.clientY,
      altKey: e.altKey,
      shiftKey: e.shiftKey,
    };
    if (dragMoveRafRef.current === null) {
      dragMoveRafRef.current = requestAnimationFrame(() => {
        dragMoveRafRef.current = null;
        flushPendingDragMove();
      });
    }
  };

  const endDrag = () => {
    if (dragMoveRafRef.current !== null) {
      cancelAnimationFrame(dragMoveRafRef.current);
      dragMoveRafRef.current = null;
    }
    if (dragRef.current) {
      // Flush exact final drop coordinates synchronously before committing the drag transaction.
      flushPendingDragMove();
      const finished = dragRef.current;
      dragRef.current = null;
      setSnapGuide((prev) => (prev.x === null && prev.y === null ? prev : { x: null, y: null }));
      onDragStateChange?.(false);
      // Alt+Clique sem arrastar preserva o comportamento de selecionar filho / desmarcar item
      if (finished.mode === "MOVE" && !finished.moved && finished.altDuplicatePending) {
        onSelectLayer?.(finished.id, { altKey: true });
      } else if (finished.mode === "MOVE" && !finished.moved && finished.wasAlreadyInMulti) {
        // Clique simples em item já dentro de multiseleção sem arrastar recolhe para apenas este item
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
      const v = evaluateFormula(candidate.fx.visible, telemetry, project.globals, now, fxWeather).trim().toLowerCase();
      if (v === "0" || v === "false" || v === "") alive = false;
    }
    if (alive) {
      const candOp = candidate.fx?.opacity
        ? Number(evaluateFormula(candidate.fx.opacity, telemetry, project.globals, now, fxWeather).replace(/%/g, "").trim())
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
    if (lite) return { backdrop: null, hasIntermediateHtml: false };

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
          const bgMode = candidate.scene3d?.backgroundMode ?? "TRANSPARENT";
          const underlay =
            bgMode === "TRANSPARENT" ? computeBackdropForTopIndex(i).backdrop ?? undefined : undefined;
          return {
            backdrop: {
              key: `3d|${candidate.id}|${candidate.scene3d?.objects?.[0]?.modelSrc || "default"}|${bgMode}`,
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
              underlay,
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
      const out = evaluateFormula(expr, telemetry, project.globals, now, fxWeather).replace(/%/g, "").trim();
      const num = Number(out);
      return Number.isFinite(num) ? num : fallback;
    };
    const fxStr = (key: string, fallback: string): string => {
      const expr = layer.fx?.[key];
      if (!expr) return fallback;
      const out = evaluateFormula(expr, telemetry, project.globals, now, fxWeather).trim();
      return out || fallback;
    };
    if (layer.fx?.visible) {
      const v = evaluateFormula(layer.fx.visible, telemetry, project.globals, now, fxWeather).trim().toLowerCase();
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
      lite || isTransparentWidget || resolvedBackdrop.hasIntermediateHtml || !resolvedBackdrop.backdrop;
    const glyphUseCssFallback = lite || isTransparentWidget || !resolvedBackdrop.backdrop;

    const flipScale =
      layer.flipX || layer.flipY
        ? ` scale(${layer.flipX ? -1 : 1}, ${layer.flipY ? -1 : 1})`
        : "";

    return (
      <div
        key={layer.id}
        data-layer-id={layer.id}
        data-prism-scene3d={layer.kind === "SCENE_3D" ? "true" : undefined}
        ref={(el) => {
          if (el) {
            layerElsRef.current.set(layer.id, el);
          } else {
            layerElsRef.current.delete(layer.id);
          }
        }}
        onPointerDown={(e) => {
          if (!interactive || test3dMode || roomActive || e.button !== 0) return;
          const isCtrlSelect = e.ctrlKey || e.metaKey;
          // Allow direct child selection with Alt+Click or when already selected
          if (!isTopLevel && !selectedSet.has(layer.id) && !e.altKey) return;
          e.stopPropagation();

          // Camada bloqueada continua sólida para o clique (seleciona, nunca deixa o clique atravessar
          // para o item de baixo), mas não inicia arraste.
          if (layer.locked) {
            if (isCtrlSelect || e.altKey || !selectedSet.has(layer.id)) {
              onSelectLayer?.(layer.id, {
                ctrlKey: isCtrlSelect,
                altKey: e.altKey,
              });
            }
            return;
          }

          if (isCtrlSelect) {
            onSelectLayer?.(layer.id, {
              ctrlKey: true,
              altKey: e.altKey,
            });
            return;
          }

          const altDuplicatePending = Boolean(e.altKey);
          const wasAlreadyInMulti = !altDuplicatePending && selectedSet.has(layer.id) && selectedSet.size > 1;
          // Preserve multi-selection when starting a drag on an already-selected layer;
          // with Alt held on an unselected item, wait to see if user drags (>=3px = duplicate) or clicks (Alt+Click).
          if (!altDuplicatePending && !selectedSet.has(layer.id)) {
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
            altDuplicatePending,
            altDuplicated: false,
            multiPeers: activePeers.length > 1 ? activePeers : undefined,
          };
          onDragStateChange?.(true);
        }}
        onClick={(e) => {
          if (interactive) {
            if (test3dMode || roomActive) return;
            if (!isTopLevel && !selectedSet.has(layer.id) && !e.altKey) return;
            // A seleção (inclusive de camada bloqueada) já foi tratada no pointerdown; aqui só
            // impede que o clique chegue ao fundo e limpe a seleção.
            e.stopPropagation();
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
            ? `translate(-50%, -50%) translate3d(calc(var(--px, 0) * ${maxPx}px), calc(var(--py, 0) * ${maxPy}px), 0) rotate(${rotation}deg)${flipScale}`
            : `translate(-50%, -50%) rotate(${rotation}deg)${flipScale}`,
          // Never create a CSS Backdrop Root on an ancestor of a backdrop-filter glass surface
          opacity: g.enabled ? undefined : opacity,
          mixBlendMode: g.enabled ? undefined : layer.blendMode,
          willChange: hasParallax && !paused ? "transform" : undefined,
          // Sala 3D: 2D layers stop hit-testing; only the edited 3D canvas opts back in
          // Camada bloqueada no editor ainda recebe o clique (seleciona sem mover): o hit-test segue
          // sempre a ordem visual, nunca atravessa para o item de baixo.
          pointerEvents: roomActive || (interactive && test3dMode) ? "none" : undefined,
          cursor: interactive
            ? test3dMode
              ? "crosshair"
              : layer.locked
              ? "default"
              : isTopLevel || selectedLayerId === layer.id
              ? "grab"
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
        {layer.kind === "SCENE_3D" && lite && stillFrames?.[layer.id] && (
          <img
            src={stillFrames[layer.id]}
            alt=""
            draggable={false}
            style={{ width: `${wPx}px`, height: `${hPx}px`, objectFit: "cover", display: "block", opacity: g.enabled ? opacity : undefined }}
          />
        )}
        {layer.kind === "VIDEO" && layer.video && lite && stillFrames?.[layer.id] && (
          <img
            src={stillFrames[layer.id]}
            alt=""
            draggable={false}
            style={{
              width: `${wPx}px`,
              height: `${hPx}px`,
              objectFit: layer.video.objectFit,
              display: "block",
              borderRadius: `${g.enabled ? plateRadiusPx : layer.video.clipRadius * containerScale}px`,
              opacity: g.enabled ? opacity : undefined,
            }}
          />
        )}
        {layer.kind === "SCENE_3D" && !lite && (
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
                interactive={interactive && !layer.locked}
                testMode={test3dMode}
                cursorNormRef={test3dMode ? previewCursorRef : cursorNormRef}
                testOrbitOffsetRef={test3dMode ? previewTestOrbitRef : undefined}
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
                onThumbFrame={isThumbnail ? onThumbFrame : undefined}
                room3d={roomActive && room3d && room3d.layerId === layer.id ? room3d : null}
              />
            </Suspense>
          </div>
        )}

        {/* VIDEO LAYER */}
        {layer.kind === "VIDEO" && layer.video && !lite && (
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
              eager={isThumbnail}
              onMediaReady={() => {
                setMediaRev((r) => r + 1);
                if (isThumbnail && onThumbFrame) {
                  const el = mediaElementsRef.current.get(layer.id);
                  if (el instanceof HTMLVideoElement) onThumbFrame(layer.id, el);
                }
              }}
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
                  isThumbnail={lite}
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
                isThumbnail={lite}
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
          const resolved = evaluateFormula(layer.text.content, telemetry, project.globals, now, fxWeather);
          const scaledFont = layer.text.fontSize * containerScale;
          const stretchY = Math.max(1, Math.min(2.8, layer.text.stemStretch || 1));
          const safeFamily = (layer.text.fontFamily || "Outfit").replace(/"/g, "");
          const fontStack = `"${safeFamily}", "Outfit", "Inter", "Segoe UI", sans-serif`;
          const textMode = g.textMode ?? "GLYPH";

          const textMaxWLog = layer.text.maxWidth && layer.text.maxWidth > 0 ? layer.text.maxWidth : 0;
          const textMaxWPx = textMaxWLog > 0 ? textMaxWLog * containerScale : 0;

          const baseTextStyle: React.CSSProperties = {
            fontFamily: fontStack,
            fontSize: `${scaledFont}px`,
            fontWeight: layer.text.fontWeight,
            letterSpacing: `${layer.text.letterSpacing}em`,
            textAlign: layer.text.align,
            color: fillRgba,
            lineHeight: 1.06,
            // Sem `maxWidth` explícito = largura automática em 1 linha (respeitando `\n`), sem a
            // quebra artificial em 50% do CSS shrink-to-fit. Com `maxWidth` (alças laterais) = quebra limpa.
            whiteSpace: textMaxWPx > 0 ? "pre-wrap" : "pre",
            width: textMaxWPx > 0 ? `${textMaxWPx}px` : "max-content",
            overflowWrap: "normal",
            fontVariantNumeric: "tabular-nums",
            textShadow:
              glyphAlpha <= 0.002
                ? undefined
                : g.enabled && hasChroma
                ? `-${chromaPx}px 0 8px rgba(255,30,66,${(0.42 * glyphAlpha).toFixed(3)}), ${chromaPx}px 0 8px rgba(0,210,255,${(0.46 * glyphAlpha).toFixed(3)}), 0 2px ${layer.text.shadowBlur}px rgba(0,0,0,${(0.55 * glyphAlpha).toFixed(3)})`
                : layer.text.shadowBlur > 0
                ? `0 2px ${layer.text.shadowBlur}px rgba(0,0,0,${(0.65 * glyphAlpha).toFixed(3)})`
                : undefined,
          };

          if (g.enabled && textMode === "PILL") {
            const padX = Math.max(12 * containerScale, scaledFont * 0.34);
            const padY = Math.max(6 * containerScale, scaledFont * 0.16);
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
                  isThumbnail={lite}
                />
                <div style={{ ...baseTextStyle, position: "relative", opacity }}>{resolved}</div>
              </div>
            );
          }

          if (g.enabled && textMode === "GLYPH") {
            const mapsBase = getGlyphGlassMaps({
              text: resolved,
              fontStack,
              fontSize: scaledFont,
              fontWeight: layer.text.fontWeight,
              letterSpacingEm: layer.text.letterSpacing,
              align: layer.text.align,
              bevelPx,
              fontRev,
              maxWidthPx: textMaxWPx > 0 ? textMaxWPx : undefined,
            });
            // Alongamento por haste: só as partes verticais crescem (mapas deformados, escala vertical = 1).
            const maps = mapsBase && stretchY > 1.01 ? warpGlyphMaps(mapsBase, stretchY, scaledFont) : mapsBase;
            if (maps) {
              return (
                <div
                  style={{
                    position: "relative",
                    width: `${maps.width}px`,
                    height: `${maps.height}px`,
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
                    scaleY={1}
                    glass={g}
                    containerScale={containerScale}
                    opacity={opacity}
                    radiusPx={0}
                    tintRgb={tintRgb}
                    glyphMaps={maps}
                    pigmentRgba={[pigmentRgb[0], pigmentRgb[1], pigmentRgb[2], glyphAlpha]}
                    glyphDropShadowPx={layer.text.shadowBlur}
                    forceCssFallback={glyphUseCssFallback}
                    isThumbnail={lite}
                  />
                </div>
              );
            }
          }

          if (stretchY > 1.01 && !(g.enabled && hasChroma)) {
            return (
              <div style={{ opacity: g.enabled ? opacity : undefined }} className="pointer-events-none">
                <StemText
                  text={resolved}
                  fontStack={fontStack}
                  fontSizePx={scaledFont}
                  fontWeight={layer.text.fontWeight}
                  letterSpacingEm={layer.text.letterSpacing}
                  align={layer.text.align}
                  maxWidthPx={textMaxWPx > 0 ? textMaxWPx : undefined}
                  stretch={stretchY}
                  color={fillRgba}
                  shadowPx={glyphAlpha <= 0.002 ? 0 : layer.text.shadowBlur}
                  shadowAlpha={0.65 * glyphAlpha}
                  fontRev={fontRev}
                  fontPath={layer.text.fontPath}
                />
              </div>
            );
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
                isThumbnail={lite}
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

        {/* Interactive Selection Box + Canva-style Handles (corners = proportional, sides = width/height) */}
        {isSelected && !test3dMode && !roomActive && (
          <div
            className="pointer-events-none absolute inset-0 z-40"
            style={flipScale ? { transform: flipScale } : undefined}
          >
            <div
              className={`pointer-events-none absolute -inset-1.5 rounded-xl border ${
                isPrimarySelected
                  ? "border-white/75"
                  : "border-white/55 border-dashed"
              }`}
            />
            {isPrimarySelected && !layer.locked && onResizeLayer && (() => {
              const startResize = (
                handle: "nw" | "ne" | "sw" | "se" | "w" | "e" | "n" | "s",
                e: React.PointerEvent<HTMLButtonElement>
              ) => {
                e.stopPropagation();
                e.currentTarget.setPointerCapture(e.pointerId);
                const el = layerElsRef.current.get(layer.id);
                const rect = el?.getBoundingClientRect();
                const safeScale = Math.max(containerScale, 0.05);
                const renderedWLog = rect ? rect.width / safeScale : layer.width;
                const renderedHLog = rect ? rect.height / safeScale : layer.height;
                let naturalTextW = 0;
                if (layer.kind === "TEXT" && layer.text && typeof document !== "undefined") {
                  const probe = document.createElement("canvas").getContext("2d");
                  if (probe) {
                    const safeFam = (layer.text.fontFamily || "Outfit").replace(/"/g, "");
                    probe.font = `${layer.text.fontWeight} ${layer.text.fontSize}px "${safeFam}", "Outfit", "Inter", "Segoe UI", sans-serif`;
                    const resolvedTxt = evaluateFormula(layer.text.content, telemetry, project.globals, now, fxWeather);
                    const lsPx = layer.text.letterSpacing * layer.text.fontSize;
                    for (const ln of resolvedTxt.split("\n")) {
                      const chars = Array.from(ln);
                      const w =
                        Math.abs(lsPx) >= 0.25
                          ? chars.reduce((acc, ch, i) => acc + probe.measureText(ch).width + (i < chars.length - 1 ? lsPx : 0), 0)
                          : probe.measureText(ln).width;
                      naturalTextW = Math.max(naturalTextW, Math.ceil(w));
                    }
                  }
                }
                dragRef.current = {
                  mode: "RESIZE",
                  resizeHandle: handle,
                  id: layer.id,
                  isText: layer.kind === "TEXT",
                  isTopLevel,
                  containerW: size.w,
                  containerH: size.h,
                  startClientX: e.clientX,
                  startClientY: e.clientY,
                  startX: layer.x,
                  startY: layer.y,
                  startW: layer.width,
                  startH: layer.height,
                  startFont: layer.text?.fontSize ?? 96,
                  startTextMaxW: layer.text?.maxWidth && layer.text.maxWidth > 0 ? layer.text.maxWidth : 0,
                  startBoxW: layer.kind === "TEXT"
                    ? (layer.text?.maxWidth && layer.text.maxWidth > 0 ? layer.text.maxWidth : renderedWLog)
                    : layer.width,
                  startBoxH: renderedHLog,
                  naturalTextW,
                  halfW: 0,
                  halfH: 0,
                  snapTargetsX: [],
                  snapTargetsY: [],
                  moved: true,
                  wasAlreadyInMulti: false,
                };
                onDragStateChange?.(true);
              };

              const corners: Array<{
                id: "nw" | "ne" | "sw" | "se";
                cls: string;
                cursor: string;
              }> = [
                { id: "nw", cls: "-left-2.5 -top-2.5", cursor: "cursor-nwse-resize" },
                { id: "ne", cls: "-right-2.5 -top-2.5", cursor: "cursor-nesw-resize" },
                { id: "sw", cls: "-left-2.5 -bottom-2.5", cursor: "cursor-nesw-resize" },
                { id: "se", cls: "-right-2.5 -bottom-2.5", cursor: "cursor-nwse-resize" },
              ];
              const showSideHorizontal = layer.kind !== "GROUP";
              const showSideVertical = layer.kind !== "TEXT" && layer.kind !== "GROUP";

              return (
                <>
                  {corners.map((c) => (
                    <button
                      key={c.id}
                      type="button"
                      aria-label="Redimensionar proporcionalmente"
                       title={
                        layer.kind === "TEXT"
                          ? "Arraste o canto para aumentar ou diminuir o texto mantendo a proporção"
                          : "Arraste o canto para escalar proporcionalmente (Shift mantém proporção · Alt = livre)"
                      }
                      onPointerDown={(e) => startResize(c.id, e)}
                      className={`pointer-events-auto absolute ${c.cls} w-3 h-3 rounded-full bg-white border-2 border-black shadow-md ${c.cursor} z-40 hover:scale-125 transition-transform`}
                    />
                  ))}
                  {showSideHorizontal && (
                    <>
                      <button
                        type="button"
                        aria-label="Ajustar largura"
                        title={
                          layer.kind === "TEXT"
                            ? "Arraste para ajustar a largura da caixa (duplo clique = linha única)"
                            : "Arraste para ajustar a largura (Shift mantém proporção)"
                        }
                        onPointerDown={(e) => startResize("w", e)}
                        onDoubleClick={(e) => {
                          if (layer.kind !== "TEXT") return;
                          e.stopPropagation();
                          onResizeLayer(layer.id, { textMaxWidth: null });
                        }}
                        className="pointer-events-auto absolute -left-2.5 top-1/2 -translate-y-1/2 w-1.5 h-6 rounded-full bg-white border border-black/80 shadow-md cursor-ew-resize z-40 hover:scale-y-110 transition-transform"
                      />
                      <button
                        type="button"
                        aria-label="Ajustar largura"
                        title={
                          layer.kind === "TEXT"
                            ? "Arraste para ajustar a largura da caixa (duplo clique = linha única)"
                            : "Arraste para ajustar a largura (Shift mantém proporção)"
                        }
                        onPointerDown={(e) => startResize("e", e)}
                        onDoubleClick={(e) => {
                          if (layer.kind !== "TEXT") return;
                          e.stopPropagation();
                          onResizeLayer(layer.id, { textMaxWidth: null });
                        }}
                        className="pointer-events-auto absolute -right-2.5 top-1/2 -translate-y-1/2 w-1.5 h-6 rounded-full bg-white border border-black/80 shadow-md cursor-ew-resize z-40 hover:scale-y-110 transition-transform"
                      />
                    </>
                  )}
                  {showSideVertical && (
                    <>
                      <button
                        type="button"
                        aria-label="Ajustar altura"
                        title="Arraste para ajustar a altura (Shift mantém proporção)"
                        onPointerDown={(e) => startResize("n", e)}
                        className="pointer-events-auto absolute -top-2.5 left-1/2 -translate-x-1/2 h-1.5 w-6 rounded-full bg-white border border-black/80 shadow-md cursor-ns-resize z-40 hover:scale-x-110 transition-transform"
                      />
                      <button
                        type="button"
                        aria-label="Ajustar altura"
                        title="Arraste para ajustar a altura (Shift mantém proporção)"
                        onPointerDown={(e) => startResize("s", e)}
                        className="pointer-events-auto absolute -bottom-2.5 left-1/2 -translate-x-1/2 h-1.5 w-6 rounded-full bg-white border border-black/80 shadow-md cursor-ns-resize z-40 hover:scale-x-110 transition-transform"
                      />
                    </>
                  )}
                </>
              );
            })()}
          </div>
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
          onDuplicateForDrag,
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
      onPointerDown={(e) => {
        if (interactive && test3dMode && e.button === 0) {
          const norm = normalizeScene3DCursor(e.clientX, e.clientY);
          if (norm) previewCursorRef.current = norm;
          testDragRef.current = {
            pointerId: e.pointerId,
            startX: e.clientX,
            startY: e.clientY,
            lastX: e.clientX,
            lastY: e.clientY,
            moved: false,
          };
          try {
            rootRef.current?.setPointerCapture(e.pointerId);
          } catch {}
          previewTickSubsRef.current.forEach((cb) => cb());
        }
      }}
      onPointerMove={handlePointerMove}
      onPointerUp={(e) => {
        if (interactive && test3dMode) {
          const td = testDragRef.current;
          testDragRef.current = null;
          try {
            if (rootRef.current?.hasPointerCapture(e.pointerId)) {
              rootRef.current.releasePointerCapture(e.pointerId);
            }
          } catch {}
          if (td && td.pointerId === e.pointerId && !td.moved) {
            const norm = normalizeScene3DCursor(e.clientX, e.clientY);
            if (norm) {
              previewCursorRef.current = norm;
              previewClickSubsRef.current.forEach((cb) => cb(norm[0], norm[1]));
            }
          }
          return;
        }
        endDrag();
      }}
      onPointerCancel={(e) => {
        if (interactive && test3dMode) {
          testDragRef.current = null;
          return;
        }
        endDrag();
      }}
      onPointerLeave={() => {
        if (interactive && test3dMode && !testDragRef.current) {
          previewCursorRef.current = [0.5, 0.5];
          previewTickSubsRef.current.forEach((cb) => cb());
        }
      }}
      onContextMenu={(e) => {
        if (!interactive || roomActive || test3dMode || !onContextMenuCanvas) return;
        e.preventDefault();
        e.stopPropagation();
        const targetEl = (e.target as HTMLElement | null)?.closest<HTMLElement>("[data-layer-id]");
        onContextMenuCanvas(targetEl?.dataset.layerId ?? null, e.clientX, e.clientY);
      }}
      onClick={(e) => {
        if (!interactive) return;
        if (roomActive || test3dMode) return;
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
export type { Room3DEditBinding } from "./UnifiedCompositorParts";
