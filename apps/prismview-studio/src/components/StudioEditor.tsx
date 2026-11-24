import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { COLLAPSE, POP, ROW } from "./motion/Presence";
import { AnimatePresence, motion } from "framer-motion";
import {
  ArrowDown,
  ArrowLeft,
  ArrowUp,
  Bone,
  Box,
  Camera,
  ChevronDown,
  ChevronRight,
  Copy,
  Download,
  Eye,
  EyeOff,
  Film,
  Focus,
  Folder,
  FolderPlus,
  Gauge,
  Grid3x3,
  GripVertical,
  Image as ImageIcon,
  BookOpen,
  Layers,
  Lock,
  Magnet,
  MousePointerClick,
  Move,
  PanelRightClose,
  PanelRightOpen,
  Redo2,
  Rotate3d,
  Scaling,
  Shapes,
  Sparkles,
  Trash2,
  Type,
  Undo2,
  Ungroup,
  Unlock,
  Upload,
  X,
} from "lucide-react";
import {
  createBindingForPart,
  createLayer,
  createScene3DObject,
  DEFAULT_REACTIVE_SIGNAL,
  defaultScene3D,
  duplicateScene3DObject,
  removeScene3DObject,
  setScene3DObjectTransform,
  invokeCmd,
  isTauriRuntime,
  LayerKind,
  LiveTelemetry,
  normalizeScene3DObjects,
  PickedFileDto,
  PrismProject,
  ReactiveSignal,
  Scene3DInspectInfo,
  Scene3DPickedPart,
  SceneLayer,
  ImportConflictDto,
  isImportConflict,
} from "../types/prism";
import { useHistory } from "../hooks/useHistory";
import { type RoomShortcuts, useKeyboardShortcuts } from "../hooks/useKeyboardShortcuts";
import { friendlyError } from "../utils/friendlyError";
import { readPanelOpen, stageRightInset, writePanelOpen } from "../utils/panelSplit";
import { StudioSidePanel } from "./studio/StudioSidePanel";
import { collectOpenAppReview } from "../utils/importReview";
import { SaveStatus, SaveStatusChip } from "./SaveStatusChip";
import { nativeCompatIssues } from "../utils/nativeCompat";
import {
  cloneLayerWithFreshIds,
  collectAllLayerIds,
  countAllLayers,
  duplicateLayersByIds,
  findLayerById,
  findLayerLocation,
  groupLayersByIds,
  isDescendantOf,
  LayerDropPlacement,
  moveLayersByDrop,
  moveLayersZByIds,
  pruneNestedSelection,
  removeLayersByIds,
  ungroupLayerById,
  updateLayerById,
  updateLayersByIds,
} from "../utils/layerTree";
import { FxExpressionPopover } from "./FxExpressionPopover";
import { Room3DEditBinding, UnifiedCompositor } from "./UnifiedCompositor";
import type {
  Scene3DEditMode,
  Scene3DEditorApi,
  Scene3DGizmoMode,
  Scene3DStats,
} from "./Scene3DLayer";
import { POWER_PREFERENCE_LABELS, usePowerState } from "../engine/powerGovernor";
import {
  createGroupChildLayer,
  FxTargetRequest,
  InspectorMode,
  StudioInspector,
} from "./StudioInspector";

/** Tipos de camada que a barra do painel cria (ordem de exibição). */
const ADD_LAYER_BUTTONS: { kind: LayerKind; label: string; Icon: React.ComponentType<{ className?: string }> }[] = [
  { kind: "VIDEO", label: "vídeo", Icon: Film },
  { kind: "IMAGE", label: "imagem PNG", Icon: ImageIcon },
  { kind: "SVG", label: "SVG / ícone", Icon: Shapes },
  { kind: "TEXT", label: "texto / ƒx", Icon: Type },
  { kind: "GLASS", label: "vidro", Icon: Sparkles },
  { kind: "PROGRESS", label: "barra", Icon: Layers },
  { kind: "SCENE_3D", label: "cena 3D", Icon: Box },
  { kind: "GROUP", label: "grupo", Icon: FolderPlus },
];

interface StudioEditorProps {
  project: PrismProject;
  availableWidgets: PrismProject[];
  telemetry: LiveTelemetry;
  /** Real target desktop monitor aspect ratio (defaults to 16/9) for 1:1 preview framing. */
  monitorAspect?: number;
  onChangeProject: (next: PrismProject) => void;
  /** Live transient update path (sliders/canvas drag): skips Hub re-renders and deep 3D normalization. */
  onChangeProjectLive?: (next: PrismProject) => void;
  /** Signals active slider/canvas drag transactions so autosave & desktop sync wait for commit. */
  onTxStateChange?: (active: boolean) => void;
  onFlushSave?: () => void;
  /** Estado do salvamento (chip no cabeçalho). */
  saveStatus?: SaveStatus;
  /** Ctrl+S / "Tentar de novo": grava o pendente e o que falhou. */
  onSaveNow?: () => void;
  onBack: () => void;
  /** Abre a Ajuda (documentação do app e das fórmulas ƒx). */
  onOpenHelp?: () => void;
  onApply: () => void;
  onExport: () => void;
  /** Aplicar/fixar em andamento: trava o botão contra clique duplo. */
  applying?: boolean;
  /** Exportação em andamento. */
  exporting?: boolean;
}

interface TreeDragState {
  dragIds: string[];
  label: string;
  hasNestedSource: boolean;
  startX: number;
  startY: number;
  clientX: number;
  clientY: number;
  active: boolean;
  targetId: string | null;
  placement: LayerDropPlacement | null;
}

/** Uniformly scales a layer and all descendants (sizes, fonts, radii, glass px values). x/y are normalized, so they follow the box. */
function scaleLayerTree(layer: SceneLayer, f: number): SceneLayer {
  if (!Number.isFinite(f) || f <= 0 || Math.abs(f - 1) < 1e-6) return layer;
  const r = (n: number) => Number((n * f).toFixed(3));
  const o = (n: number | undefined) => (n === undefined ? undefined : r(n));
  const next: SceneLayer = {
    ...layer,
    width: r(layer.width),
    height: r(layer.height),
    glass: {
      ...layer.glass,
      blurPx: r(layer.glass.blurPx),
      borderRadius: r(layer.glass.borderRadius),
      shadowBlur: r(layer.glass.shadowBlur),
      bevelWidth: o(layer.glass.bevelWidth),
    },
  };
  if (layer.text) {
    next.text = {
      ...layer.text,
      fontSize: r(layer.text.fontSize),
      // `letterSpacing` é em `em` (já proporcional ao `fontSize`), não deve ser multiplicado por `f`.
      letterSpacing: layer.text.letterSpacing,
      shadowBlur: r(layer.text.shadowBlur),
      maxWidth: layer.text.maxWidth && layer.text.maxWidth > 0 ? Math.max(60, Math.round(layer.text.maxWidth * f)) : undefined,
    };
  }
  if (layer.image) next.image = { ...layer.image, borderRadius: r(layer.image.borderRadius) };
  if (layer.video) next.video = { ...layer.video, clipRadius: r(layer.video.clipRadius) };
  if (layer.progress) next.progress = { ...layer.progress, borderRadius: r(layer.progress.borderRadius) };
  if (layer.children) next.children = layer.children.map((c) => scaleLayerTree(c, f));
  return next;
}

export const StudioEditor: React.FC<StudioEditorProps> = ({
  project,
  availableWidgets,
  telemetry,
  monitorAspect = 16 / 9,
  onChangeProject,
  onChangeProjectLive,
  onTxStateChange,
  onFlushSave,
  saveStatus,
  onSaveNow,
  onBack,
  onOpenHelp,
  onApply,
  onExport,
  applying = false,
  exporting = false,
}) => {
  const [selectedIds, setSelectedIds] = useState<string[]>(() =>
    project.layers.length > 0 ? [project.layers[project.layers.length - 1].id] : []
  );
  const [expandedGroups, setExpandedGroups] = useState<Record<string, boolean>>({});
  const [systemFonts, setSystemFonts] = useState<string[]>([
    "Outfit",
    "Inter",
    "JetBrains Mono",
    "Segoe UI",
  ]);
  const [inspectorMode, setInspectorMode] = useState<InspectorMode>("LAYER");
  const [widgetPickerOpen, setWidgetPickerOpen] = useState(false);
  // Painel único (propriedades + camadas): aberto/recolhido lembrado entre sessões.
  const [panelOpen, setPanelOpen] = useState<boolean>(() =>
    readPanelOpen(typeof window === "undefined" ? null : window.localStorage)
  );
  const togglePanel = useCallback(() => {
    setPanelOpen((open) => {
      const next = !open;
      writePanelOpen(typeof window === "undefined" ? null : window.localStorage, next);
      return next;
    });
  }, []);
  const [fxTarget, setFxTarget] = useState<FxTargetRequest | null>(null);
  const [statusHint, setStatusHint] = useState<string | null>(null);
  const [treeDrag, setTreeDrag] = useState<TreeDragState | null>(null);
  const [scene3dInspectMap, setScene3dInspectMap] = useState<Record<string, Scene3DInspectInfo>>({});
  const [selected3dObjectId, setSelected3dObjectId] = useState<string | null>(null);
  const selected3dObjectIdRef = useRef<string | null>(null);
  selected3dObjectIdRef.current = selected3dObjectId;
  const [hoveredMesh3d, setHoveredMesh3d] = useState<string | null>(null);
  // LAYOUT = 2D layout · ROOM = Sala 3D (free camera + gizmos, sensors off) · TEST = live interactions (wallpaper camera locked)
  const [scene3dEditMode, setScene3dEditMode] = useState<Scene3DEditMode>("LAYOUT");
  const test3dMode = scene3dEditMode === "TEST";
  const room3dMode = scene3dEditMode === "ROOM";
  const [gizmoMode, setGizmoMode] = useState<Scene3DGizmoMode>("translate");
  // Editor-only toggles (local state, never persisted in the project)
  const [showGrid3d, setShowGrid3d] = useState(true);
  const [showSkeleton3d, setShowSkeleton3d] = useState(false);
  const [snap3d, setSnap3d] = useState(false);
  const editorApiRef = useRef<Scene3DEditorApi | null>(null);
  // Bumped when the Sala 3D editor API appears or a model (re)loads → outliner refetches its tree
  const [outlinerRev, setOutlinerRev] = useState(0);
  const bumpOutlinerRev = useCallback(() => setOutlinerRev((r) => r + 1), []);
  const [pickedPart3d, setPickedPart3d] = useState<Scene3DPickedPart | null>(null);
  const [selectedBindingId3d, setSelectedBindingId3d] = useState<string | null>(null);
  // Performance HUD (Ctrl+Shift+P)
  const [hudOpen, setHudOpen] = useState(false);
  const [hudStats, setHudStats] = useState<Record<string, Scene3DStats>>({});
  const power = usePowerState(telemetry);

  const hasScene3D = useMemo(() => {
    const walk = (list: SceneLayer[]): boolean =>
      list.some((l) => l.kind === "SCENE_3D" || (l.children ? walk(l.children) : false));
    return walk(project.layers);
  }, [project.layers]);

  // Live Studio WASAPI Audio & Anonymous Keyboard reactive channel (strictly on demand)
  const studioReactiveRef = useRef<ReactiveSignal>(DEFAULT_REACTIVE_SIGNAL);
  const studioReactiveSubsRef = useRef<Set<() => void>>(new Set());
  const subscribeStudioReactiveTick = useCallback((cb: () => void) => {
    studioReactiveSubsRef.current.add(cb);
    return () => {
      studioReactiveSubsRef.current.delete(cb);
    };
  }, []);

  const reactiveNeeds = useMemo(() => {
    let audio = false;
    let keyboard = false;
    const walk = (list: SceneLayer[]) => {
      for (const l of list) {
        if (l.hidden) continue;
        if (l.kind === "SCENE_3D" && l.scene3d?.bindings) {
          for (const b of l.scene3d.bindings) {
            if (!b.enabled) continue;
            if (b.source === "AUDIO_BASS" || b.source === "AUDIO_MID" || b.source === "AUDIO_TREBLE") {
              audio = true;
            } else if (b.source === "KEY_PULSE") {
              keyboard = true;
            }
          }
        }
        if (l.children?.length) walk(l.children);
      }
    };
    // Sala 3D edits the room, not the reactions: sensors stay off until LAYOUT/TEST
    if (!room3dMode) walk(project.layers);
    return { audio, keyboard };
  }, [project.layers, room3dMode]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    void invokeCmd("set_studio_reactive_sensors", {
      audio: reactiveNeeds.audio,
      keyboard: reactiveNeeds.keyboard,
    }).catch(() => {});
  }, [reactiveNeeds.audio, reactiveNeeds.keyboard]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unsub: (() => void) | undefined;
    void import("@tauri-apps/api/event")
      .then(async ({ listen }) => {
        const fn = await listen<ReactiveSignal>("prism://reactive", (ev) => {
          if (disposed) return;
          studioReactiveRef.current = ev.payload;
          studioReactiveSubsRef.current.forEach((cb) => cb());
        });
        if (disposed) fn();
        else unsub = fn;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unsub?.();
      void invokeCmd("set_studio_reactive_sensors", { audio: false, keyboard: false }).catch(
        () => {}
      );
    };
  }, []);

  useEffect(() => {
    if (!hasScene3D && scene3dEditMode !== "LAYOUT") {
      setScene3dEditMode("LAYOUT");
      setHoveredMesh3d(null);
    }
  }, [hasScene3D, scene3dEditMode]);

  const clipboardRef = useRef<SceneLayer[]>([]);
  const hintTimerRef = useRef<number | null>(null);
  const treeDragRef = useRef<TreeDragState | null>(null);
  treeDragRef.current = treeDrag;
  const suppressRowClickRef = useRef(false);

  const flashHint = useCallback((msg: string) => {
    setStatusHint(msg);
    if (hintTimerRef.current !== null) {
      window.clearTimeout(hintTimerRef.current);
    }
    hintTimerRef.current = window.setTimeout(() => {
      hintTimerRef.current = null;
      setStatusHint(null);
    }, 2200);
  }, []);

  useEffect(() => {
    return () => {
      if (hintTimerRef.current !== null) {
        window.clearTimeout(hintTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    void invokeCmd<string[]>("list_system_fonts")
      .then((fonts) => {
        if (Array.isArray(fonts) && fonts.length > 0) {
          setSystemFonts(fonts);
        }
      })
      .catch(() => {});
  }, []);

  const projectRef = useRef(project);
  projectRef.current = project;
  const blobUrlsRef = useRef<string[]>([]);

  useEffect(() => {
    return () => {
      blobUrlsRef.current.forEach((u) => URL.revokeObjectURL(u));
      blobUrlsRef.current = [];
    };
  }, []);

  /**
   * Revokes a previous `blob:` URL once a layer's source is replaced (audit 2.6).
   * Called BEFORE the patch is applied, so the old URL still appears once in the project
   * for the layer being replaced; if it appears more than once (duplicated layer) it is kept alive.
   */
  const releaseBlobUrl = (prev: string | undefined | null) => {
    if (!prev || !prev.startsWith("blob:")) return;
    try {
      if (JSON.stringify(projectRef.current).split(prev).length - 1 > 1) return;
    } catch {
      return;
    }
    URL.revokeObjectURL(prev);
    blobUrlsRef.current = blobUrlsRef.current.filter((u) => u !== prev);
  };

  const prevModelSrcOf = (layer: SceneLayer): string => {
    if (!layer.scene3d) return "";
    const objs = normalizeScene3DObjects(layer.scene3d);
    const obj = objs.find((o) => o.id === selected3dObjectIdRef.current) ?? objs[0];
    return obj?.modelSrc ?? "";
  };

  const history = useHistory<PrismProject>({ limit: 50, resetKey: project.id });

  const canvasLogicalW = project.packageType === "WIDGET" ? Math.max(160, project.widget.width) : 1920;
  const canvasLogicalH = project.packageType === "WIDGET" ? Math.max(100, project.widget.height) : 1080;

  // Measure the center stage so the wallpaper/widget frame ALWAYS fits 100% inside both width & height
  // (zero vertical clipping on 1536×864 / 125% DPI or short windows, matching exact desktop monitor aspect).
  const stageRef = useRef<HTMLDivElement | null>(null);
  const [stageBounds, setStageBounds] = useState<{ w: number; h: number }>({ w: 960, h: 540 });
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const update = () => {
      const w = Math.max(160, el.clientWidth);
      const h = Math.max(120, el.clientHeight);
      setStageBounds((prev) =>
        Math.abs(prev.w - w) > 1 || Math.abs(prev.h - h) > 1 ? { w, h } : prev
      );
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const stageFrameStyle = useMemo<React.CSSProperties>(() => {
    const availW = Math.max(160, stageBounds.w - 24);
    const availH = Math.max(120, stageBounds.h - 16);
    if (project.packageType === "WIDGET") {
      const ratio = canvasLogicalW / Math.max(1, canvasLogicalH);
      const maxW = Math.min(680, canvasLogicalW, availW);
      let w = maxW;
      let h = w / ratio;
      if (h > availH) {
        h = availH;
        w = h * ratio;
      }
      return {
        width: `${Math.round(w)}px`,
        height: `${Math.round(h)}px`,
        backgroundColor: "#0e1015",
        backgroundImage:
          "radial-gradient(circle at 28% 24%, rgba(255,255,255,0.16) 0%, transparent 45%), radial-gradient(circle at 76% 78%, rgba(255,255,255,0.10) 0%, transparent 48%), linear-gradient(rgba(255,255,255,0.06) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.06) 1px, transparent 1px)",
        backgroundSize: "100% 100%, 100% 100%, 24px 24px, 24px 24px",
      };
    }
    const aspect = Number.isFinite(monitorAspect) && monitorAspect > 0.2 ? monitorAspect : 16 / 9;
    let w = Math.min(1024, availW);
    let h = w / aspect;
    if (h > availH) {
      h = availH;
      w = h * aspect;
    }
    return {
      width: `${Math.round(w)}px`,
      height: `${Math.round(h)}px`,
    };
  }, [stageBounds.w, stageBounds.h, project.packageType, canvasLogicalW, canvasLogicalH, monitorAspect]);

  // Keep only IDs that still exist in project.layers (respects explicit empty selection)
  const validSelectedIds = useMemo(
    () => selectedIds.filter((id) => findLayerById(project.layers, id) !== null),
    [project.layers, selectedIds]
  );
  const selectedIdSet = useMemo(() => new Set(validSelectedIds), [validSelectedIds]);
  const primarySelectedId = validSelectedIds[validSelectedIds.length - 1] ?? null;

  const selectedLocation = useMemo(
    () => (primarySelectedId ? findLayerLocation(project.layers, primarySelectedId) : null),
    [project.layers, primarySelectedId]
  );
  const selectedLayer = selectedLocation?.layer ?? null;
  // O que o desktop (motor nativo) não desenha: painel no Inspetor + badge no "Aplicar".
  const compatIssues = useMemo(() => nativeCompatIssues(project), [project]);
  const selectedParent = selectedLocation?.parent ?? null;
  const selectedLayerIdRef = useRef<string | null>(null);
  selectedLayerIdRef.current = selectedLayer?.id ?? null;
  const selectedIdsRef = useRef<string[]>([]);
  selectedIdsRef.current = validSelectedIds;
  const totalLayersCount = useMemo(() => countAllLayers(project.layers), [project.layers]);

  const emitLiveProject = onChangeProjectLive ?? onChangeProject;

  useEffect(() => {
    return () => {
      onTxStateChange?.(false);
    };
  }, [onTxStateChange]);

  const applyDiscreteChange = useCallback(
    (next: PrismProject) => {
      history.pushSnapshot(projectRef.current);
      onChangeProject(next);
    },
    [history, onChangeProject]
  );

  const applyCoalescedChange = useCallback(
    (next: PrismProject) => {
      history.recordCoalesced(projectRef.current, 350);
      emitLiveProject(next);
    },
    [history, emitLiveProject]
  );

  const patchLayerRaw = useCallback(
    (id: string, fn: (l: SceneLayer) => SceneLayer) => {
      const cur = projectRef.current;
      emitLiveProject({
        ...cur,
        layers: updateLayerById(cur.layers, id, fn),
      });
    },
    [emitLiveProject]
  );

  const patchMultiplePositionsRaw = useCallback(
    (moves: Array<{ id: string; x: number; y: number }>) => {
      if (moves.length === 0) return;
      const map = new Map(moves.map((m) => [m.id, m]));
      const cur = projectRef.current;
      emitLiveProject({
        ...cur,
        layers: updateLayersByIds(cur.layers, new Set(map.keys()), (l) => {
          const target = map.get(l.id);
          return target ? { ...l, x: target.x, y: target.y } : l;
        }),
      });
    },
    [emitLiveProject]
  );

  const patchLayer = useCallback(
    (
      id: string,
      fn: (l: SceneLayer) => SceneLayer,
      mode: "raw" | "discrete" | "coalesced" = "raw"
    ) => {
      if (mode === "discrete") {
        history.pushSnapshot(projectRef.current);
        const cur = projectRef.current;
        onChangeProject({
          ...cur,
          layers: updateLayerById(cur.layers, id, fn),
        });
        return;
      }
      if (mode === "coalesced") {
        history.recordCoalesced(projectRef.current, 350);
      }
      patchLayerRaw(id, fn);
    },
    [history, onChangeProject, patchLayerRaw]
  );

  const handleSelectLayer = useCallback(
    (
      id: string | null,
      modifiers?: { ctrlKey?: boolean; altKey?: boolean }
    ) => {
      if (id === null) {
        setSelectedIds([]);
        return;
      }

      setInspectorMode("LAYER");

      // Alt+Click: remove this item from selection if already selected, or select child directly
      if (modifiers?.altKey) {
        setSelectedIds((prev) =>
          prev.includes(id) ? prev.filter((item) => item !== id) : [id]
        );
        return;
      }

      // Ctrl/Cmd+Click: ONLY modifier for multi-selection (toggle item in/out of selection set)
      if (modifiers?.ctrlKey) {
        setSelectedIds((prev) => {
          const valid = prev.filter((item) => findLayerById(projectRef.current.layers, item) !== null);
          return valid.includes(id) ? valid.filter((item) => item !== id) : [...valid, id];
        });
        return;
      }

      setSelectedIds([id]);
    },
    []
  );

  const handleUndo = useCallback(() => {
    const prev = history.undo(projectRef.current);
    if (prev) {
      onChangeProject(prev);
      onFlushSave?.();
      flashHint("Desfeito (Ctrl+Z)");
    }
  }, [flashHint, history, onChangeProject, onFlushSave]);

  const handleRedo = useCallback(() => {
    const next = history.redo(projectRef.current);
    if (next) {
      onChangeProject(next);
      onFlushSave?.();
      flashHint("Refeito (Ctrl+Y)");
    }
  }, [flashHint, history, onChangeProject, onFlushSave]);

  // ── Sala 3D ─────────────────────────────────────────────────────────────────────
  const room3dLayer = useMemo<SceneLayer | null>(() => {
    if (selectedLayer?.kind === "SCENE_3D") return selectedLayer;
    const walk = (list: SceneLayer[]): SceneLayer | null => {
      for (const l of list) {
        if (l.kind === "SCENE_3D" && !l.hidden) return l;
        if (l.children?.length) {
          const hit = walk(l.children);
          if (hit) return hit;
        }
      }
      return null;
    };
    return walk(project.layers);
  }, [project.layers, selectedLayer]);
  const room3dLayerIdRef = useRef<string | null>(null);
  room3dLayerIdRef.current = room3dLayer?.id ?? null;

  const setViewMode = useCallback(
    (mode: Scene3DEditMode) => {
      setScene3dEditMode(mode);
      if (mode !== "TEST") setHoveredMesh3d(null);
      if (mode === "ROOM" && room3dLayerIdRef.current) {
        const layerId = room3dLayerIdRef.current;
        setSelectedIds([layerId]);
        setInspectorMode("LAYER");
        const layer = findLayerById(projectRef.current.layers, layerId);
        const objs = layer?.scene3d ? normalizeScene3DObjects(layer.scene3d) : [];
        if (!objs.some((o) => o.id === selected3dObjectIdRef.current)) {
          setSelected3dObjectId(objs[0]?.id ?? null);
        }
      }
    },
    []
  );

  const commitRoomTransform = useCallback(
    (
      layerId: string,
      objectId: string,
      position: [number, number, number],
      rotation: [number, number, number],
      scale: [number, number, number]
    ) => {
      const layer = findLayerById(projectRef.current.layers, layerId);
      if (!layer?.scene3d) return;
      const cur = normalizeScene3DObjects(layer.scene3d).find((o) => o.id === objectId);
      if (!cur) return;
      const same = (a: number[], b: number[]) => a.every((v, i) => Math.abs(v - (b[i] ?? 0)) < 1e-4);
      // A click on the gizmo without moving must not create an empty undo step
      if (same(position, cur.position) && same(rotation, cur.rotation) && same(scale, cur.scale)) {
        return;
      }
      patchLayer(
        layerId,
        (l) => ({
          ...l,
          scene3d: setScene3DObjectTransform(l.scene3d ?? defaultScene3D(), objectId, position, rotation, scale),
        }),
        "discrete"
      );
      onFlushSave?.();
    },
    [onFlushSave, patchLayer]
  );

  const saveWallpaperCamera = useCallback(() => {
    const layerId = room3dLayerIdRef.current;
    const cap = editorApiRef.current?.getCameraState();
    if (!layerId || !cap) return;
    patchLayer(
      layerId,
      (l) => {
        const s3d = l.scene3d ?? defaultScene3D();
        return {
          ...l,
          scene3d: {
            ...s3d,
            camera: {
              ...s3d.camera,
              target: cap.target,
              targetY: cap.target[1],
              distance: cap.distance,
              azimuthDeg: cap.azimuthDeg,
              elevationDeg: cap.elevationDeg,
              fov: cap.fov,
            },
          },
        };
      },
      "discrete"
    );
    onFlushSave?.();
    flashHint("Câmera do Wallpaper salva");
  }, [flashHint, onFlushSave, patchLayer]);

  const duplicateRoomObject = useCallback(() => {
    const layerId = room3dLayerIdRef.current;
    const objId = selected3dObjectIdRef.current;
    if (!layerId || !objId) return;
    let createdId: string | null = null;
    patchLayer(
      layerId,
      (l) => {
        const res = duplicateScene3DObject(l.scene3d ?? defaultScene3D(), objId);
        createdId = res.newId;
        return { ...l, scene3d: res.config };
      },
      "discrete"
    );
    if (createdId) {
      setSelected3dObjectId(createdId);
      flashHint("Objeto 3D duplicado (Ctrl+D)");
    }
  }, [flashHint, patchLayer]);

  const removeRoomObject = useCallback(() => {
    const layerId = room3dLayerIdRef.current;
    const objId = selected3dObjectIdRef.current;
    if (!layerId || !objId) return;
    const layer = findLayerById(projectRef.current.layers, layerId);
    const objs = layer?.scene3d ? normalizeScene3DObjects(layer.scene3d) : [];
    if (objs.length <= 1) {
      flashHint("A sala precisa de ao menos 1 objeto");
      return;
    }
    patchLayer(
      layerId,
      (l) => ({ ...l, scene3d: removeScene3DObject(l.scene3d ?? defaultScene3D(), objId) }),
      "discrete"
    );
    setSelected3dObjectId(objs.find((o) => o.id !== objId)?.id ?? null);
    flashHint("Objeto 3D removido");
  }, [flashHint, patchLayer]);

  // Sala 3D key map, handed to useKeyboardShortcuts only while mode === ROOM (undefined otherwise,
  // so G/R/S/F/Del/Ctrl+D never fire in LAYOUT/TEST). While RMB is held the editor camera consumes
  // W/A/S/D/Q/E first (capture phase).
  const roomShortcuts = useMemo<RoomShortcuts | undefined>(
    () =>
      room3dMode
        ? {
            onGizmo: setGizmoMode,
            onFocus: () => editorApiRef.current?.focusSelected(),
            onDuplicate: duplicateRoomObject,
            onDelete: removeRoomObject,
            onExit: () => setViewMode("LAYOUT"),
          }
        : undefined,
    [duplicateRoomObject, removeRoomObject, room3dMode, setViewMode]
  );

  // Ctrl+ recolhe/mostra o painel de propriedades e camadas (funciona também com foco em campos).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && (e.key === "\\" || e.code === "Backslash")) {
        e.preventDefault();
        togglePanel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [togglePanel]);

  // Performance HUD toggle (works in every Studio mode)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "p") {
        e.preventDefault();
        setHudOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const handleScene3DStats = useCallback((layerId: string, st: Scene3DStats) => {
    setHudStats((prev) => ({ ...prev, [layerId]: st }));
  }, []);

  useEffect(() => {
    if (!room3dMode) setPickedPart3d(null);
  }, [room3dMode]);

  useEffect(() => {
    if (pickedPart3d && (!selected3dObjectId || pickedPart3d.objectId !== selected3dObjectId)) {
      setPickedPart3d(null);
    }
  }, [pickedPart3d, selected3dObjectId]);

  const interactWithPickedPart = useCallback(
    (part: Scene3DPickedPart) => {
      const layerId = room3dLayerIdRef.current ?? selectedLayerIdRef.current;
      if (!layerId) return;
      const binding = createBindingForPart(part);
      patchLayer(
        layerId,
        (l) => {
          const s3d = l.scene3d ?? defaultScene3D();
          return {
            ...l,
            scene3d: {
              ...s3d,
              bindings: [...(s3d.bindings ?? []), binding],
            },
          };
        },
        "discrete"
      );
      setSelectedIds([layerId]);
      setSelected3dObjectId(part.objectId);
      setInspectorMode("LAYER");
      setSelectedBindingId3d(binding.id);
    },
    [patchLayer]
  );

  const room3dBinding: Room3DEditBinding | null =
    room3dMode && room3dLayer
      ? {
          layerId: room3dLayer.id,
          selectedObjectId: selected3dObjectId,
          gizmoMode,
          showGrid: showGrid3d,
          snapEnabled: snap3d,
          showSkeleton: showSkeleton3d,
          onEditorSync: bumpOutlinerRev,
          onSelectObject: setSelected3dObjectId,
          onPickPart: setPickedPart3d,
          onCommitObjectTransform: commitRoomTransform,
          editorApiRef,
        }
      : null;

  const addNewLayer = (kind: LayerKind) => {
    const created = createLayer(kind);
    const nextLayers =
      kind === "VIDEO" || kind === "SCENE_3D"
        ? [created, ...project.layers]
        : [...project.layers, created];
    applyDiscreteChange({ ...project, layers: nextLayers });
    if (kind === "GROUP") {
      setExpandedGroups((prev) => ({ ...prev, [created.id]: true }));
    }
    setSelectedIds([created.id]);
    setInspectorMode("LAYER");
  };

  const addChildToGroup = (groupId: string, kind: LayerKind) => {
    const child = createGroupChildLayer(kind);
    patchLayer(
      groupId,
      (g) => ({
        ...g,
        children: [...(g.children ?? []), child],
      }),
      "discrete"
    );
    setExpandedGroups((prev) => ({ ...prev, [groupId]: true }));
    setSelectedIds([child.id]);
    setInspectorMode("LAYER");
  };

  const importWidgetAsGroup = (widget: PrismProject) => {
    const cur = projectRef.current;
    const clonedChildren = widget.layers.map((c) => cloneLayerWithFreshIds(c, false));
    const group: SceneLayer = {
      ...createLayer("GROUP", `Widget · ${widget.title}`),
      width: widget.widget.width,
      height: widget.widget.height,
      children: clonedChildren,
    };
    applyDiscreteChange({ ...cur, layers: [...cur.layers, group] });
    setExpandedGroups((prev) => ({ ...prev, [group.id]: true }));
    setSelectedIds([group.id]);
    setInspectorMode("LAYER");
    setWidgetPickerOpen(false);
  };

  const importWidgetFromDisk = async () => {
    try {
      const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", { kind: "prism" });
      if (!picked) return;
      // Mesmo id já na biblioteca: importa como cópia (nunca sobrescreve sem perguntar).
      const first = await invokeCmd<PrismProject | ImportConflictDto>("import_prism_archive", {
        archivePath: picked.path,
      });
      const imported = isImportConflict(first)
        ? await invokeCmd<PrismProject>("import_prism_archive", { archivePath: picked.path, replace: false })
        : first;
      if (imported?.layers) {
        importWidgetAsGroup(imported);
        if (collectOpenAppReview(imported.layers).length > 0) {
          flashHint('Ações "Abrir app" do pacote vieram desativadas: revise o caminho no Inspetor.');
        }
      }
    } catch (err) {
      flashHint(friendlyError(err, "Não foi possível importar o widget."));
    }
  };

  const moveZ = useCallback(
    (ids: string[], delta: number) => {
      if (ids.length === 0) return;
      const cur = projectRef.current;
      const nextLayers = moveLayersZByIds(cur.layers, new Set(ids), delta);
      if (nextLayers !== cur.layers) {
        applyDiscreteChange({ ...cur, layers: nextLayers });
      }
    },
    [applyDiscreteChange]
  );

  const duplicateLayers = useCallback(
    (ids: string[]) => {
      if (ids.length === 0) return;
      const cur = projectRef.current;
      const { layers, newIds } = duplicateLayersByIds(cur.layers, ids);
      if (newIds.length > 0) {
        applyDiscreteChange({ ...cur, layers });
        setSelectedIds(newIds);
        flashHint(
          newIds.length === 1 ? "Camada duplicada (Ctrl+D)" : `${newIds.length} camadas duplicadas`
        );
      }
    },
    [applyDiscreteChange, flashHint]
  );

  const deleteLayers = useCallback(
    (ids: string[]) => {
      const cur = projectRef.current;
      const deletable = pruneNestedSelection(cur.layers, ids).filter((id) => {
        const layer = findLayerById(cur.layers, id);
        return layer && !layer.locked;
      });
      if (deletable.length === 0) return;
      const firstLoc = findLayerLocation(cur.layers, deletable[0]);
      const next = removeLayersByIds(cur.layers, new Set(deletable));
      if (next === cur.layers) return;
      applyDiscreteChange({ ...cur, layers: next });
      const remainingSelected = selectedIdsRef.current.filter(
        (id) => !deletable.includes(id) && findLayerById(next, id) !== null
      );
      if (remainingSelected.length > 0) {
        setSelectedIds(remainingSelected);
      } else if (firstLoc?.parent && findLayerById(next, firstLoc.parent.id)) {
        setSelectedIds([firstLoc.parent.id]);
      } else if (next.length > 0) {
        setSelectedIds([next[next.length - 1].id]);
      } else {
        setSelectedIds([]);
      }
      flashHint(
        deletable.length === 1 ? "Camada removida" : `${deletable.length} camadas removidas`
      );
    },
    [applyDiscreteChange, flashHint]
  );

  const handleCopy = useCallback(() => {
    const cur = projectRef.current;
    const topIds = pruneNestedSelection(cur.layers, selectedIdsRef.current);
    const copied: SceneLayer[] = [];
    for (const id of topIds) {
      const layer = findLayerById(cur.layers, id);
      if (layer) copied.push(JSON.parse(JSON.stringify(layer)) as SceneLayer);
    }
    if (copied.length === 0) return;
    clipboardRef.current = copied;
    flashHint(copied.length === 1 ? "1 camada copiada (Ctrl+C)" : `${copied.length} camadas copiadas`);
  }, [flashHint]);

  const handleCut = useCallback(() => {
    handleCopy();
    deleteLayers(selectedIdsRef.current);
  }, [deleteLayers, handleCopy]);

  const handlePaste = useCallback(() => {
    const items = clipboardRef.current;
    if (items.length === 0) return;
    const cur = projectRef.current;
    const pasted = items.map((item) => {
      const fresh = cloneLayerWithFreshIds(item, true);
      return {
        ...fresh,
        x: Math.max(0, Math.min(1, Number((fresh.x + 0.018).toFixed(4)))),
        y: Math.max(0, Math.min(1, Number((fresh.y + 0.018).toFixed(4)))),
      };
    });
    // If primary selected layer is an expanded GROUP, paste inside it; otherwise paste into same parent or root
    const primaryLoc = findLayerLocation(cur.layers, selectedLayerIdRef.current);
    let nextLayers: SceneLayer[];
    if (primaryLoc?.layer.kind === "GROUP" && expandedGroups[primaryLoc.layer.id]) {
      nextLayers = updateLayerById(cur.layers, primaryLoc.layer.id, (g) => ({
        ...g,
        children: [...(g.children ?? []), ...pasted],
      }));
    } else if (primaryLoc?.parent) {
      nextLayers = updateLayerById(cur.layers, primaryLoc.parent.id, (g) => ({
        ...g,
        children: [...(g.children ?? []), ...pasted],
      }));
    } else {
      nextLayers = [...cur.layers, ...pasted];
    }
    applyDiscreteChange({ ...cur, layers: nextLayers });
    const newIds = pasted.map((p) => p.id);
    setSelectedIds(newIds);
    flashHint(newIds.length === 1 ? "Camada colada (Ctrl+V)" : `${newIds.length} camadas coladas`);
  }, [applyDiscreteChange, expandedGroups, flashHint]);

  const handleGroupSelected = useCallback(() => {
    const cur = projectRef.current;
    const ids = selectedIdsRef.current;
    if (ids.length === 0) return;
    const w = cur.packageType === "WIDGET" ? Math.max(160, cur.widget.width) : 1920;
    const h = cur.packageType === "WIDGET" ? Math.max(100, cur.widget.height) : 1080;
    const { layers, groupId } = groupLayersByIds(cur.layers, ids, w, h);
    if (!groupId) return;
    applyDiscreteChange({ ...cur, layers });
    setExpandedGroups((prev) => ({ ...prev, [groupId]: true }));
    setSelectedIds([groupId]);
    flashHint("Agrupado (Ctrl+G)");
  }, [applyDiscreteChange, flashHint]);

  const handleUngroupSelected = useCallback(() => {
    const cur = projectRef.current;
    const targetId = selectedLayerIdRef.current;
    if (!targetId) return;
    const target = findLayerById(cur.layers, targetId);
    if (!target || target.kind !== "GROUP") return;
    const w = cur.packageType === "WIDGET" ? Math.max(160, cur.widget.width) : 1920;
    const h = cur.packageType === "WIDGET" ? Math.max(100, cur.widget.height) : 1080;
    const { layers, childIds } = ungroupLayerById(cur.layers, targetId, w, h);
    applyDiscreteChange({ ...cur, layers });
    setSelectedIds(childIds);
    flashHint("Grupo desfeito (Ctrl+Shift+G)");
  }, [applyDiscreteChange, flashHint]);

  const handleToggleHiddenSelected = useCallback(() => {
    const cur = projectRef.current;
    const ids = selectedIdsRef.current;
    if (ids.length === 0) return;
    const anyVisible = ids.some((id) => !findLayerById(cur.layers, id)?.hidden);
    applyDiscreteChange({
      ...cur,
      layers: updateLayersByIds(cur.layers, new Set(ids), (l) => ({ ...l, hidden: anyVisible })),
    });
    flashHint(anyVisible ? "Camadas ocultadas" : "Camadas exibidas");
  }, [applyDiscreteChange, flashHint]);

  const handleToggleLockSelected = useCallback(() => {
    const cur = projectRef.current;
    const ids = selectedIdsRef.current;
    if (ids.length === 0) return;
    const anyUnlocked = ids.some((id) => !findLayerById(cur.layers, id)?.locked);
    applyDiscreteChange({
      ...cur,
      layers: updateLayersByIds(cur.layers, new Set(ids), (l) => ({ ...l, locked: anyUnlocked })),
    });
    flashHint(anyUnlocked ? "Camadas travadas (Ctrl+L)" : "Camadas destravadas");
  }, [applyDiscreteChange, flashHint]);

  const nudgeSelectedLayers = useCallback(
    (dxPx: number, dyPx: number) => {
      const cur = projectRef.current;
      const ids = pruneNestedSelection(cur.layers, selectedIdsRef.current);
      if (ids.length === 0) return;
      const moves: Array<{ id: string; x: number; y: number }> = [];
      for (const id of ids) {
        const loc = findLayerLocation(cur.layers, id);
        if (!loc || loc.layer.locked) continue;
        const refW = loc.parent
          ? Math.max(loc.parent.width, 1)
          : cur.packageType === "WIDGET"
          ? Math.max(cur.widget.width, 1)
          : 1920;
        const refH = loc.parent
          ? Math.max(loc.parent.height, 1)
          : cur.packageType === "WIDGET"
          ? Math.max(cur.widget.height, 1)
          : 1080;
        const nx = Math.max(0, Math.min(1, Number((loc.layer.x + dxPx / refW).toFixed(4))));
        const ny = Math.max(0, Math.min(1, Number((loc.layer.y + dyPx / refH).toFixed(4))));
        moves.push({ id: loc.layer.id, x: nx, y: ny });
      }
      if (moves.length === 0) return;
      history.recordCoalesced(cur, 350);
      patchMultiplePositionsRaw(moves);
    },
    [history, patchMultiplePositionsRaw]
  );

  useKeyboardShortcuts({
    enabled: !fxTarget,
    room: roomShortcuts,
    onUndo: handleUndo,
    onRedo: handleRedo,
    onSave: onSaveNow,
    onDelete: () => deleteLayers(selectedIdsRef.current),
    onNudge: nudgeSelectedLayers,
    onCopy: handleCopy,
    onCut: handleCut,
    onPaste: handlePaste,
    onDuplicate: () => duplicateLayers(selectedIdsRef.current),
    onSelectAll: () => {
      const all = collectAllLayerIds(projectRef.current.layers);
      setSelectedIds(all);
      flashHint(`${all.length} camadas selecionadas (Ctrl+A)`);
    },
    onDeselect: () => setSelectedIds([]),
    onGroup: handleGroupSelected,
    onUngroup: handleUngroupSelected,
    onBringForward: () => moveZ(selectedIdsRef.current, 1),
    onSendBackward: () => moveZ(selectedIdsRef.current, -1),
    onToggleHidden: handleToggleHiddenSelected,
    onToggleLock: handleToggleLockSelected,
  });

  // Pointer-based Drag-and-Drop for Layers panel (100% immune to Tauri WebView2 native dragDrop hooks)
  const isTreeDragging = treeDrag !== null;
  useEffect(() => {
    if (!isTreeDragging) return;

    const handleMove = (e: PointerEvent) => {
      const cur = treeDragRef.current;
      if (!cur) return;
      const dist = Math.hypot(e.clientX - cur.startX, e.clientY - cur.startY);
      const active = cur.active || dist >= 5;
      if (!active) return;

      let targetId: string | null = null;
      let placement: LayerDropPlacement | null = null;

      const el = document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null;
      const rootZone = el?.closest<HTMLElement>("[data-layer-root-drop]");
      if (rootZone) {
        placement = rootZone.dataset.layerRootDrop === "top" ? "root-top" : "root-bottom";
      } else {
        const rowEl = el?.closest<HTMLElement>("[data-layer-drop-id]");
        if (rowEl) {
          const candId = rowEl.dataset.layerDropId ?? null;
          const candIsGroup = rowEl.dataset.layerIsGroup === "1";
          if (
            candId &&
            !cur.dragIds.includes(candId) &&
            !cur.dragIds.some((dId) => isDescendantOf(projectRef.current.layers, dId, candId))
          ) {
            const rect = rowEl.getBoundingClientRect();
            const ratio = (e.clientY - rect.top) / Math.max(1, rect.height);
            targetId = candId;
            if (candIsGroup) {
              if (ratio < 0.28) placement = "above";
              else if (ratio > 0.72) placement = "below";
              else placement = "inside";
            } else {
              placement = ratio < 0.5 ? "above" : "below";
            }
          }
        }
      }

      setTreeDrag({
        ...cur,
        active: true,
        clientX: e.clientX,
        clientY: e.clientY,
        targetId,
        placement,
      });
    };

    const handleUp = () => {
      const cur = treeDragRef.current;
      setTreeDrag(null);
      if (!cur || !cur.active) return;
      suppressRowClickRef.current = true;
      window.setTimeout(() => {
        suppressRowClickRef.current = false;
      }, 60);

      if (!cur.placement) return;
      const proj = projectRef.current;
      const w = proj.packageType === "WIDGET" ? Math.max(160, proj.widget.width) : 1920;
      const h = proj.packageType === "WIDGET" ? Math.max(100, proj.widget.height) : 1080;
      const nextLayers = moveLayersByDrop(
        proj.layers,
        cur.dragIds,
        cur.targetId,
        cur.placement,
        w,
        h
      );
      if (nextLayers !== proj.layers) {
        applyDiscreteChange({ ...proj, layers: nextLayers });
        if (cur.placement === "inside" && cur.targetId) {
          setExpandedGroups((prev) => ({ ...prev, [cur.targetId!]: true }));
          flashHint("Aninhado dentro do grupo");
        } else if (cur.placement === "root-top" || cur.placement === "root-bottom") {
          flashHint("Movido para fora do grupo");
        } else {
          flashHint("Ordem das camadas atualizada");
        }
      }
    };

    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [applyDiscreteChange, flashHint, isTreeDragging]);

  const startRowPointerDrag = (e: React.PointerEvent, layer: SceneLayer) => {
    if (e.button !== 0 || e.ctrlKey || e.metaKey || e.altKey) return;
    const dragIds = selectedIdSet.has(layer.id) ? [...validSelectedIds] : [layer.id];
    const hasNestedSource = dragIds.some((id) => Boolean(findLayerLocation(project.layers, id)?.parent));
    setTreeDrag({
      dragIds,
      label: dragIds.length === 1 ? layer.name : `${dragIds.length} camadas`,
      hasNestedSource,
      startX: e.clientX,
      startY: e.clientY,
      clientX: e.clientX,
      clientY: e.clientY,
      active: false,
      targetId: null,
      placement: null,
    });
  };

  const pickFileForLayer = async (kind: "video" | "image" | "svg" | "font" | "model3d") => {
    const targetId = selectedLayerIdRef.current;
    if (!targetId) return;

    if (isTauriRuntime()) {
      try {
        const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", { kind });
        if (!picked) return;
        const freshLoc = findLayerLocation(projectRef.current.layers, targetId);
        if (!freshLoc) return;
        const target = freshLoc.layer;
        if (kind === "video" && target.video) {
          releaseBlobUrl(target.video.src);
          patchLayer(targetId, (l) => (l.video ? { ...l, video: { ...l.video, src: picked.path } } : l), "discrete");
        } else if (kind === "image" && target.image) {
          releaseBlobUrl(target.image.src);
          patchLayer(targetId, (l) => (l.image ? { ...l, image: { ...l.image, src: picked.path } } : l), "discrete");
        } else if (kind === "svg" && target.svg) {
          patchLayer(
            targetId,
            (l) => (l.svg ? { ...l, svg: { ...l.svg, customSvg: picked.text_content } } : l),
            "discrete"
          );
        } else if (kind === "font" && target.text) {
          const family = picked.file_name.replace(/\.(ttf|otf|woff2?)$/i, "");
          patchLayer(
            targetId,
            (l) => (l.text ? { ...l, text: { ...l.text, fontFamily: family, fontPath: picked.path } } : l),
            "discrete"
          );
        } else if (kind === "model3d" && target.kind === "SCENE_3D") {
          const objTargetId = selected3dObjectIdRef.current;
          const cleanName = picked.file_name.replace(/\.[^.]+$/, "") || "Modelo .glb";
          releaseBlobUrl(prevModelSrcOf(target));
          patchLayer(
            targetId,
            (l) => {
              const prev = l.scene3d ?? defaultScene3D(picked.path);
              const objs = normalizeScene3DObjects(prev);
              const activeIdx = objs.findIndex((o) => o.id === objTargetId);
              const idxToReplace = activeIdx >= 0 ? activeIdx : 0;
              const curObj = objs[idxToReplace];
              const replacingDefault =
                objs.length === 1 && (!curObj || curObj.kind === "SHOWCASE" || !curObj.modelSrc);
              const nextObjs = objs.map((o, i) =>
                i === idxToReplace
                  ? {
                      ...o,
                      kind: "GLB" as const,
                      name: o.kind === "SHOWCASE" || !o.modelSrc ? cleanName : o.name,
                      modelSrc: picked.path,
                      showcaseVariant: undefined,
                      autoRotateSpeed:
                        o.kind === "SHOWCASE" || !o.modelSrc ? 0 : o.autoRotateSpeed,
                      playback: { ...o.playback, clipName: "" },
                    }
                  : o
              );
              return {
                ...l,
                scene3d: {
                  ...prev,
                  objects: nextObjs,
                  camera: replacingDefault
                    ? { ...prev.camera, azimuthDeg: 0, elevationDeg: 8 }
                    : prev.camera,
                },
              };
            },
            "discrete"
          );
          flashHint(`Modelo 3D carregado: ${picked.file_name}`);
        }
      } catch (err) {
        flashHint(friendlyError(err, "Não foi possível carregar o arquivo escolhido."));
      }
      return;
    }

    // Browser-only dev fallback (no native path available). `blob:` URLs here are TEMPORARY previews:
    // they die with the page and are never readable from the wallpaper WebView. The real app
    // (isTauriRuntime) always goes through the native picker above and stores a real file path.
    const input = document.createElement("input");
    input.type = "file";
    input.accept =
      kind === "video"
        ? "video/mp4,video/webm"
        : kind === "image"
        ? "image/png,image/jpeg,image/webp"
        : kind === "svg"
        ? ".svg"
        : kind === "model3d"
        ? ".glb,.gltf"
        : ".ttf,.otf";
    input.onchange = async () => {
      const f = input.files?.[0];
      if (!f) return;
      const freshLoc = findLayerLocation(projectRef.current.layers, targetId);
      if (!freshLoc) return;
      const target = freshLoc.layer;
      if (kind === "video" && target.video) {
        const url = URL.createObjectURL(f);
        releaseBlobUrl(target.video.src);
        blobUrlsRef.current.push(url);
        patchLayer(targetId, (l) => (l.video ? { ...l, video: { ...l.video, src: url } } : l), "discrete");
      } else if (kind === "image" && target.image) {
        const url = URL.createObjectURL(f);
        releaseBlobUrl(target.image.src);
        blobUrlsRef.current.push(url);
        patchLayer(targetId, (l) => (l.image ? { ...l, image: { ...l.image, src: url } } : l), "discrete");
      } else if (kind === "svg" && target.svg) {
        const txt = await f.text();
        patchLayer(targetId, (l) => (l.svg ? { ...l, svg: { ...l.svg, customSvg: txt } } : l), "discrete");
      } else if (kind === "font" && target.text) {
        const family = f.name.replace(/\.(ttf|otf|woff2?)$/i, "");
        const buf = await f.arrayBuffer();
        const face = new FontFace(family, buf);
        await face.load();
        document.fonts.add(face);
        patchLayer(
          targetId,
          (l) => (l.text ? { ...l, text: { ...l.text, fontFamily: family, fontPath: "" } } : l),
          "discrete"
        );
      } else if (kind === "model3d" && target.kind === "SCENE_3D") {
        const url = URL.createObjectURL(f);
        releaseBlobUrl(prevModelSrcOf(target));
        blobUrlsRef.current.push(url);
        const objTargetId = selected3dObjectIdRef.current;
        const cleanName = f.name.replace(/\.[^.]+$/, "") || "Modelo .glb";
        patchLayer(
          targetId,
          (l) => {
            const prev = l.scene3d ?? defaultScene3D(url);
            const objs = normalizeScene3DObjects(prev);
            const activeIdx = objs.findIndex((o) => o.id === objTargetId);
            const idxToReplace = activeIdx >= 0 ? activeIdx : 0;
            const curObj = objs[idxToReplace];
            const replacingDefault =
              objs.length === 1 && (!curObj || curObj.kind === "SHOWCASE" || !curObj.modelSrc);
            const nextObjs = objs.map((o, i) =>
              i === idxToReplace
                ? {
                    ...o,
                    kind: "GLB" as const,
                    name: o.kind === "SHOWCASE" || !o.modelSrc ? cleanName : o.name,
                    modelSrc: url,
                    showcaseVariant: undefined,
                    autoRotateSpeed:
                      o.kind === "SHOWCASE" || !o.modelSrc ? 0 : o.autoRotateSpeed,
                    playback: { ...o.playback, clipName: "" },
                  }
                : o
            );
            return {
              ...l,
              scene3d: {
                ...prev,
                objects: nextObjs,
                camera: replacingDefault
                  ? { ...prev.camera, azimuthDeg: 0, elevationDeg: 8 }
                  : prev.camera,
              },
            };
          },
          "discrete"
        );
      }
    };
    input.click();
  };

  const addGlbObjectTo3DLayer = async () => {
    const targetId = selectedLayerIdRef.current;
    if (!targetId) return;

    const appendModel = (srcPath: string, fileName: string) => {
      const cleanName = fileName.replace(/\.[^.]+$/, "") || "Objeto .glb";
      let createdId = "";
      patchLayer(
        targetId,
        (l) => {
          const prev = l.scene3d ?? defaultScene3D();
          const objs = normalizeScene3DObjects(prev);
          const nextObj = createScene3DObject("GLB", {
            name: cleanName,
            modelSrc: srcPath,
            position: [Number((objs.length * 1.6).toFixed(2)), 0, 0],
            autoRotateSpeed: 0,
          });
          createdId = nextObj.id;
          const nextObjs = [...objs, nextObj];
          return {
            ...l,
            scene3d: {
              ...prev,
              objects: nextObjs,
            },
          };
        },
        "discrete"
      );
      if (createdId) setSelected3dObjectId(createdId);
      flashHint(`Adicionado à sala 3D: ${cleanName}`);
    };

    if (isTauriRuntime()) {
      try {
        const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", {
          kind: "model3d",
        });
        if (!picked) return;
        appendModel(picked.path, picked.file_name);
      } catch (err) {
        flashHint(friendlyError(err, "Não foi possível adicionar o modelo."));
      }
      return;
    }

    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".glb,.gltf";
    input.onchange = () => {
      const f = input.files?.[0];
      if (!f) return;
      const url = URL.createObjectURL(f);
      blobUrlsRef.current.push(url);
      appendModel(url, f.name);
    };
    input.click();
  };

  const toggleGroupExpanded = (groupId: string) => {
    setExpandedGroups((prev) => ({ ...prev, [groupId]: !prev[groupId] }));
  };

  const renderLayerTreeRow = (
    layer: SceneLayer,
    realIdx: number,
    depth: number
  ): React.ReactNode => {
    const isSelected = selectedIdSet.has(layer.id);
    const isPrimary = primarySelectedId === layer.id;
    const isGroup = layer.kind === "GROUP";
    const children = layer.children ?? [];
    const expanded = Boolean(expandedGroups[layer.id]);

    const isBeingDragged = Boolean(treeDrag?.active && treeDrag.dragIds.includes(layer.id));
    const isDropTarget = Boolean(treeDrag?.active && treeDrag.targetId === layer.id);
    const dropAbove = isDropTarget && treeDrag?.placement === "above";
    const dropBelow = isDropTarget && treeDrag?.placement === "below";
    const dropInside = isDropTarget && treeDrag?.placement === "inside";

    return (
      <motion.div key={layer.id} layout="position" {...ROW} className="space-y-1">
        <div
          data-layer-drop-id={layer.id}
          data-layer-is-group={isGroup ? "1" : "0"}
          onPointerDown={(e) => startRowPointerDrag(e, layer)}
          onClick={(e) => {
            if (suppressRowClickRef.current) return;
            handleSelectLayer(layer.id, {
              ctrlKey: e.ctrlKey || e.metaKey,
              altKey: e.altKey,
            });
          }}
          style={{ marginLeft: depth > 0 ? `${depth * 14}px` : undefined }}
          className={`group relative rounded-2xl px-2 py-2 flex items-center justify-between gap-1 cursor-grab active:cursor-grabbing border transition-colors select-none ${
            isBeingDragged
              ? "opacity-40 border-white/25 bg-white/5"
              : dropInside
              ? "bg-white/20 border-white text-white shadow-[0_0_0_1px_rgba(255,255,255,0.65)]"
              : isPrimary
              ? "bg-white/15 border-white/75 text-white shadow-[0_0_14px_rgba(255,255,255,0.16)]"
              : isSelected
              ? "bg-white/10 border-white/55 text-white shadow-[0_0_10px_rgba(255,255,255,0.12)]"
              : "bg-white/[0.03] border-transparent text-[#A1A1AA] hover:bg-white/[0.07] hover:text-white"
          }`}
        >
          {/* Visual Drop Indicators (Above / Below) */}
          {dropAbove && (
            <div className="pointer-events-none absolute -top-1 left-2 right-2 h-0.5 rounded-full bg-white shadow-[0_0_8px_rgba(255,255,255,0.85)]" />
          )}
          {dropBelow && (
            <div className="pointer-events-none absolute -bottom-1 left-2 right-2 h-0.5 rounded-full bg-white shadow-[0_0_8px_rgba(255,255,255,0.85)]" />
          )}

          <div className="flex items-center gap-1 min-w-0">
            <GripVertical className="w-3 h-3 text-white/25 shrink-0" />
            {isGroup ? (
              <button
                type="button"
                onPointerDown={(e) => e.stopPropagation()}
                onClick={(e) => {
                  e.stopPropagation();
                  toggleGroupExpanded(layer.id);
                }}
                title={expanded ? "Recolher grupo" : "Expandir subcamadas"}
                className="p-0.5 rounded hover:bg-white/10 text-[#A1A1AA] hover:text-white shrink-0"
              >
                {expanded ? (
                  <ChevronDown className="w-3.5 h-3.5" />
                ) : (
                  <ChevronRight className="w-3.5 h-3.5" />
                )}
              </button>
            ) : null}
            <div className="min-w-0">
              <span className="block text-xs font-medium truncate">{layer.name}</span>
              <span className="block text-[10px] text-[#71717A]">
                {dropInside
                  ? "Soltar dentro do grupo"
                  : `Z${realIdx} · ${layer.kind}${isGroup ? ` (${children.length})` : ""}`}
              </span>
            </div>
          </div>

          <div
            className="flex items-center gap-0.5 shrink-0"
            onPointerDown={(e) => e.stopPropagation()}
            onClick={(e) => e.stopPropagation()}
          >
            {/* Ordem, duplicar e excluir só aparecem no hover, no foco ou na linha selecionada;
                ocultar e travar são estado da camada e ficam sempre à vista. */}
            <div
              className={`flex items-center gap-0.5 transition-opacity focus-within:opacity-100 ${
                isSelected ? "opacity-95" : "opacity-0 group-hover:opacity-100"
              }`}
            >
              <button
                type="button"
                aria-label={`Trazer ${layer.name} para frente (Ctrl+])`}
                onClick={() => moveZ([layer.id], 1)}
                title="Trazer para frente (Ctrl+])"
                className="p-1 hover:text-white"
              >
                <ArrowUp className="w-3 h-3" />
              </button>
              <button
                type="button"
                aria-label={`Enviar ${layer.name} para trás (Ctrl+[)`}
                onClick={() => moveZ([layer.id], -1)}
                title="Enviar para trás (Ctrl+[)"
                className="p-1 hover:text-white"
              >
                <ArrowDown className="w-3 h-3" />
              </button>
              <button
                type="button"
                aria-label={`Duplicar ${layer.name} (Ctrl+D)`}
                onClick={() => duplicateLayers([layer.id])}
                title="Duplicar (Ctrl+D)"
                className="p-1 hover:text-white"
              >
                <Copy className="w-3 h-3" />
              </button>
              <button
                type="button"
                aria-label={`Excluir ${layer.name} (Del)`}
                onClick={() => deleteLayers([layer.id])}
                title="Excluir (Del)"
                className="p-1 hover:text-white"
              >
                <Trash2 className="w-3 h-3" />
              </button>
            </div>
            <button
              type="button"
              aria-label={layer.hidden ? `Mostrar ${layer.name} (Ctrl+Shift+H)` : `Ocultar ${layer.name} (Ctrl+Shift+H)`}
              aria-pressed={Boolean(layer.hidden)}
              onClick={() => patchLayer(layer.id, (l) => ({ ...l, hidden: !l.hidden }), "discrete")}
              title="Mostrar / Ocultar (Ctrl+Shift+H)"
              className={`p-1 hover:text-white ${layer.hidden ? "text-white" : ""}`}
            >
              {layer.hidden ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3" />}
            </button>
            <button
              type="button"
              aria-label={layer.locked ? `Destravar ${layer.name} (Ctrl+L)` : `Travar ${layer.name} (Ctrl+L)`}
              aria-pressed={Boolean(layer.locked)}
              onClick={() => patchLayer(layer.id, (l) => ({ ...l, locked: !l.locked }), "discrete")}
              title="Travar / Destravar (Ctrl+L)"
              className={`p-1 hover:text-white ${layer.locked ? "text-white" : ""}`}
            >
              {layer.locked ? <Lock className="w-3 h-3" /> : <Unlock className="w-3 h-3" />}
            </button>
          </div>
        </div>
        <AnimatePresence initial={false}>{isGroup && expanded && children.length > 0 && (
          <motion.div {...COLLAPSE} className="space-y-1 border-l border-white/15 ml-2.5 pl-1">
            <AnimatePresence initial={false}>
              {[...children]
                .reverse()
                .map((child, childRevIdx) =>
                  renderLayerTreeRow(child, children.length - 1 - childRevIdx, depth + 1)
                )}
            </AnimatePresence>
          </motion.div>
        )}</AnimatePresence>
      </motion.div>
    );
  };

  // Metade de baixo do painel único: criar, agrupar e reordenar camadas.
  const layersPanel = (
  <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="px-3 pt-2.5 pb-2 border-b border-white/10 space-y-2">
        <div
          className="flex items-center justify-between"
          title="Arraste as linhas para mudar a ordem (Z) ou soltar dentro de um grupo"
        >
          <span className="text-[11px] font-semibold uppercase tracking-wider text-[#A1A1AA] flex items-center gap-1.5">
            <Layers className="w-3.5 h-3.5 text-white" />
            Camadas ({totalLayersCount})
          </span>
          <span className="text-[10px] text-[#71717A]">Arraste para reordenar</span>
        </div>
        {/* Criar camada: uma fileira de ícones (nome no tooltip e no leitor de tela) */}
        <div role="toolbar" aria-label="Adicionar camada" className="flex items-center gap-1">
          {ADD_LAYER_BUTTONS.map(({ kind, label, Icon }) => (
            <button
              key={kind}
              type="button"
              onClick={() => addNewLayer(kind)}
              aria-label={`Adicionar ${label}`}
              title={`Adicionar ${label}`}
              className="prism-pill-idle rounded-lg w-8 h-8 flex items-center justify-center"
            >
              <Icon className="w-3.5 h-3.5" />
            </button>
          ))}
          {project.packageType === "WALLPAPER" && (
            <button
              type="button"
              onClick={() => setWidgetPickerOpen((v) => !v)}
              aria-label="Importar widget (.widget.prism)"
              aria-expanded={widgetPickerOpen}
              title="Importar widget (.widget.prism) como grupo editável"
              className={`rounded-lg w-8 h-8 flex items-center justify-center ${widgetPickerOpen ? "prism-pill-active" : "prism-pill-idle"}`}
            >
              <Upload className="w-3.5 h-3.5" />
            </button>
          )}
        </div>
      </div>

      {/* Multi-Selection / Group Quick Action Bar */}
      {(validSelectedIds.length > 1 || selectedLayer?.kind === "GROUP") && (
        <div className="px-3 py-2 border-b border-white/10 bg-white/[0.04] flex items-center justify-between gap-1.5">
          <span className="text-[10px] font-medium text-white/85 truncate">
            {validSelectedIds.length > 1
              ? `${validSelectedIds.length} selecionadas`
              : "Grupo selecionado"}
          </span>
          <div className="flex items-center gap-1 shrink-0">
            {validSelectedIds.length > 1 && (
              <button
                type="button"
                onClick={handleGroupSelected}
                title="Agrupar selecionadas (Ctrl+G)"
                className="px-2 py-1 rounded-lg bg-white/10 hover:bg-white hover:text-black text-[10px] text-white transition flex items-center gap-1"
              >
                <FolderPlus className="w-3 h-3" /> Agrupar
              </button>
            )}
            {selectedLayer?.kind === "GROUP" && (
              <button
                type="button"
                onClick={handleUngroupSelected}
                title="Desagrupar (Ctrl+Shift+G)"
                className="px-2 py-1 rounded-lg bg-white/10 hover:bg-white hover:text-black text-[10px] text-white transition flex items-center gap-1"
              >
                <Ungroup className="w-3 h-3" /> Desagrupar
              </button>
            )}
          </div>
        </div>
      )}

      {/* Widget Importer Drawer inside Left Tree */}
      <AnimatePresence initial={false}>{widgetPickerOpen && (
        <motion.div {...COLLAPSE}><div className="p-3 border-b border-white/10 bg-black/50 space-y-2">
          <span className="block text-[10px] uppercase tracking-wider text-[#A1A1AA]">
            Inserir como Grupo Editável
          </span>
          {availableWidgets.map((w) => (
            <button
              key={w.id}
              onClick={() => importWidgetAsGroup(w)}
              className="w-full text-left px-3 py-2 rounded-xl bg-white/5 hover:bg-white hover:text-black text-[11px] transition truncate"
            >
              + {w.title} ({w.layers.length} camadas)
            </button>
          ))}
          <button
            onClick={importWidgetFromDisk}
            className="w-full px-3 py-1.5 rounded-xl border border-white/15 text-[11px] text-[#A1A1AA] hover:text-white flex items-center justify-center gap-1.5"
          >
            <Upload className="w-3 h-3" /> Escolher .widget.prism do Disco
          </button>
        </div></motion.div>
      )}</AnimatePresence>

      {/* Ordered Stack (Front-most on top) */}
      <div className="flex-1 overflow-y-auto p-2.5 space-y-1.5">
        {treeDrag?.active && treeDrag.hasNestedSource && (
          <div
            data-layer-root-drop="top"
            className={`rounded-xl px-2.5 py-1.5 text-center text-[10px] border border-dashed transition ${
              treeDrag.placement === "root-top"
                ? "border-white bg-white/20 text-white"
                : "border-white/25 text-[#A1A1AA]"
            }`}
          >
            Soltar na Raiz (Topo / Frente)
          </div>
        )}

        <AnimatePresence initial={false}>
          {[...project.layers]
            .reverse()
            .map((layer, revIdx) =>
              renderLayerTreeRow(layer, project.layers.length - 1 - revIdx, 0)
            )}
        </AnimatePresence>

        {treeDrag?.active && treeDrag.hasNestedSource && (
          <div
            data-layer-root-drop="bottom"
            className={`rounded-xl px-2.5 py-1.5 text-center text-[10px] border border-dashed transition ${
              treeDrag.placement === "root-bottom"
                ? "border-white bg-white/20 text-white"
                : "border-white/25 text-[#A1A1AA]"
            }`}
          >
            Soltar na Raiz (Fundo / Atrás)
          </div>
        )}
      </div>
  </div>
  );

  return (
    <div className="relative w-full h-full bg-pureBlack overflow-hidden">
      {/* Top Floating Bar (Stationary during drag) */}
      <header className="fixed top-4 left-4 right-4 z-30 flex items-center justify-between gap-3">
        <div className="frosted-chrome rounded-full px-2 py-1.5 flex items-center gap-2">
          <button
            type="button"
            aria-label="Voltar ao Hub"
            title="Voltar ao Hub"
            onClick={onBack}
            className="w-8 h-8 rounded-full bg-white/10 hover:bg-white/15 flex items-center justify-center text-white transition"
          >
            <ArrowLeft className="w-4 h-4" />
          </button>
          <input
            aria-label="Nome do projeto"
            value={project.title}
            onChange={(e) => applyCoalescedChange({ ...project, title: e.target.value })}
            className="bg-transparent text-xs font-semibold text-white outline-none px-1.5 w-48"
          />
          {saveStatus && onSaveNow && <SaveStatusChip status={saveStatus} onSaveNow={onSaveNow} />}
          <span className="px-2.5 py-1 rounded-full bg-white/10 text-[10px] uppercase tracking-wider text-[#A1A1AA]">
            {project.packageType === "WIDGET" ? ".widget.prism" : ".prism"}
          </span>
          <span className="h-4 w-px bg-white/15 mx-0.5" />
          <button aria-label="Desfazer (Ctrl+Z)"
            type="button"
            onClick={handleUndo}
            disabled={!history.canUndo}
            title="Desfazer (Ctrl+Z)"
            className={`w-7 h-7 rounded-full flex items-center justify-center transition ${
              history.canUndo
                ? "text-white hover:bg-white/15"
                : "text-[#52525B] cursor-not-allowed"
            }`}
          >
            <Undo2 className="w-3.5 h-3.5" />
          </button>
          <button aria-label="Refazer (Ctrl+Y)"
            type="button"
            onClick={handleRedo}
            disabled={!history.canRedo}
            title="Refazer (Ctrl+Y)"
            className={`w-7 h-7 rounded-full flex items-center justify-center transition ${
              history.canRedo
                ? "text-white hover:bg-white/15"
                : "text-[#52525B] cursor-not-allowed"
            }`}
          >
            <Redo2 className="w-3.5 h-3.5" />
          </button>
          {onOpenHelp && (
            // Uma entrada só para ajuda e atalhos: a tabela de atalhos vive na Ajuda (seção "Atalhos").
            <button aria-label="Ajuda, fórmulas e atalhos (F1)"
              type="button"
              onClick={onOpenHelp}
              title="Ajuda, fórmulas e atalhos (F1)"
              className="w-7 h-7 rounded-full flex items-center justify-center transition text-[#A1A1AA] hover:text-white hover:bg-white/15"
            >
              <BookOpen className="w-3.5 h-3.5" />
            </button>
          )}
        </div>

        <div className="flex items-center gap-2">
          <AnimatePresence initial={false}>{statusHint && (
            <motion.span {...POP} className="frosted-chrome rounded-full px-3 py-1.5 text-[11px] text-white/90">
              {statusHint}
            </motion.span>
          )}</AnimatePresence>
          {power.state !== "NORMAL" && (
            <span
              title={`Energia: ${POWER_PREFERENCE_LABELS[power.state]} (preferência: ${POWER_PREFERENCE_LABELS[power.preference]})`}
              className="frosted-chrome rounded-full px-3 py-1.5 text-[11px] text-amber-100/90 border border-amber-300/25"
            >
              Energia · {POWER_PREFERENCE_LABELS[power.state]}
            </span>
          )}
          {hasScene3D && (selectedLayer?.kind === "SCENE_3D" || scene3dEditMode !== "LAYOUT") && (
            <div className="frosted-chrome rounded-full p-1 flex items-center gap-0.5">
              {(
                [
                  { id: "LAYOUT" as const, label: "Layout 2D", icon: Layers, title: "Editar camadas 2D" },
                  {
                    id: "ROOM" as const,
                    label: "Sala 3D",
                    icon: Box,
                    title: "Navegar pela sala, mover objetos com gizmos e salvar a câmera do wallpaper (sensores desligados)",
                  },
                  {
                    id: "TEST" as const,
                    label: "Testar Interações",
                    icon: MousePointerClick,
                    title: "Câmera do wallpaper travada; mouse, clique, áudio e teclado ao vivo",
                  },
                ]
              ).map((m) => {
                const active = scene3dEditMode === m.id;
                const Icon = m.icon;
                return (
                  <button aria-label={m.title}
                    key={m.id}
                    type="button"
                    onClick={() => setViewMode(m.id)}
                    title={m.title}
                    className={`rounded-full px-3 py-1.5 text-xs transition flex items-center gap-1.5 ${
                      active ? "prism-pill-active" : "text-[#A1A1AA] hover:text-white"
                    }`}
                  >
                    <Icon className="w-3.5 h-3.5" />
                    {m.label}
                  </button>
                );
              })}
            </div>
          )}
          <button
            type="button"
            onClick={togglePanel}
            aria-label={panelOpen ? "Recolher o painel de propriedades e camadas (Ctrl+\\)" : "Mostrar o painel de propriedades e camadas (Ctrl+\\)"}
            aria-pressed={panelOpen}
            title={panelOpen ? "Recolher painel (Ctrl+\\) — canvas em tela cheia" : "Mostrar painel (Ctrl+\\)"}
            className="frosted-chrome rounded-full w-9 h-9 flex items-center justify-center text-[#A1A1AA] hover:text-white transition"
          >
            {panelOpen ? <PanelRightClose className="w-4 h-4" /> : <PanelRightOpen className="w-4 h-4" />}
          </button>
          <button
            onClick={onExport}
            disabled={exporting}
            aria-busy={exporting}
            className="frosted-chrome rounded-full px-4 py-2 text-xs text-[#A1A1AA] hover:text-white transition flex items-center gap-1.5 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:text-[#A1A1AA]"
          >
            <Download className={`w-3.5 h-3.5 ${exporting ? "animate-pulse" : ""}`} />
            {exporting ? "Exportando…" : "Exportar"}
          </button>
          <button
            type="button"
            onClick={onApply}
            disabled={applying}
            aria-busy={applying}
            title={
              compatIssues.length > 0
                ? `${compatIssues.length} item(ns) não aparecem no desktop (veja o Inspetor)`
                : undefined
            }
            className="relative prismatic-cta px-6 py-2 text-xs flex items-center gap-2 disabled:opacity-60 disabled:cursor-not-allowed"
          >
            {compatIssues.length > 0 && (
              <span
                className="absolute -top-1.5 -right-1.5 min-w-5 h-5 px-1 rounded-full bg-amber-400 text-black text-[10px] font-semibold flex items-center justify-center"
                aria-label={`${compatIssues.length} aviso(s) de compatibilidade`}
              >
                {compatIssues.length}
              </span>
            )}
            <Sparkles className={`w-3.5 h-3.5 ${applying ? "animate-spin" : ""}`} />
            {applying
              ? project.packageType === "WIDGET"
                ? "Fixando…"
                : "Aplicando…"
              : project.packageType === "WIDGET"
                ? "Fixar Widget na Tela"
                : "Aplicar no Desktop"}
          </button>
        </div>
      </header>


      {/* Floating Drag Ghost Pill for Layer Tree Reordering */}
      {treeDrag?.active && (
        <div
          style={{
            left: `${treeDrag.clientX + 14}px`,
            top: `${treeDrag.clientY + 12}px`,
          }}
          className="pointer-events-none fixed z-[90] frosted-chrome rounded-full px-3 py-1.5 text-[11px] text-white border border-white/35 shadow-2xl flex items-center gap-1.5"
        >
          <Layers className="w-3 h-3" />
          <span>{treeDrag.label}</span>
        </div>
      )}

      {/* CENTER: 1:1 Unified GPU Compositor Canvas (Always 100% visible, never clipped vertically) */}
      <div
        className="absolute inset-0 pt-20 pb-6 pl-4"
        style={{ paddingRight: stageRightInset(panelOpen) }}
      >
        <div ref={stageRef} className="w-full h-full flex items-center justify-center">
          <div
            className="relative overflow-hidden rounded-[22px] border border-white/15 shadow-[0_28px_88px_rgba(0,0,0,0.92)] shrink-0"
            style={stageFrameStyle}
          >
          <UnifiedCompositor
            project={project}
            telemetry={telemetry}
            interactive
            test3dMode={test3dMode}
            reactiveSignalRef={studioReactiveRef}
            subscribeReactiveTick={subscribeStudioReactiveTick}
            selectedLayerId={selectedLayer?.id ?? null}
            selectedLayerIds={validSelectedIds}
            onSelectLayer={(id, modifiers) => handleSelectLayer(id, modifiers)}
            onMoveLayer={(id, x, y) => patchLayerRaw(id, (l) => ({ ...l, x, y }))}
            onMoveLayers={patchMultiplePositionsRaw}
            onResizeLayer={(id, patch) =>
              patchLayerRaw(id, (l) => {
                if (l.kind === "TEXT" && l.text) {
                  const nextText = { ...l.text };
                  if (patch.fontSize !== undefined) {
                    nextText.fontSize = Math.max(10, Math.min(340, patch.fontSize));
                  }
                  if (patch.textMaxWidth === null) {
                    delete nextText.maxWidth;
                  } else if (patch.textMaxWidth !== undefined) {
                    nextText.maxWidth = Math.max(60, Math.min(2400, patch.textMaxWidth));
                  }
                  return { ...l, text: nextText };
                }
                if (l.kind === "GROUP" && l.width > 0 && patch.width !== undefined) {
                  return scaleLayerTree(l, patch.width / l.width);
                }
                return {
                  ...l,
                  width: patch.width ?? l.width,
                  height: patch.height ?? l.height,
                };
              })
            }
            onDragStateChange={(dragging) => {
              if (dragging) {
                onTxStateChange?.(true);
                history.beginTransaction(projectRef.current);
              } else {
                history.commitTransaction(projectRef.current);
                onTxStateChange?.(false);
                onFlushSave?.();
              }
            }}
            onInspectScene3D={(layerId, info) => {
              setScene3dInspectMap((prev) => ({ ...prev, [layerId]: info }));
            }}
            onHoverMesh3D={setHoveredMesh3d}
            powerState={power.state}
            room3d={room3dBinding}
            onScene3DStats={hudOpen ? handleScene3DStats : undefined}
          />
          {room3dMode && room3dLayer && (
            <div className="absolute bottom-3 left-1/2 -translate-x-1/2 z-40 frosted-chrome rounded-full px-1.5 py-1 flex items-center gap-0.5 text-[11px] text-white border border-white/20 shadow-2xl">
              {(
                [
                  { mode: "translate" as const, label: "Mover", key: "G", icon: Move },
                  { mode: "rotate" as const, label: "Girar", key: "R", icon: Rotate3d },
                  { mode: "scale" as const, label: "Escalar", key: "S", icon: Scaling },
                ]
              ).map((g) => {
                const Icon = g.icon;
                return (
                  <button aria-label={`${g.label} (${g.key})`}
                    key={g.mode}
                    type="button"
                    onClick={() => setGizmoMode(g.mode)}
                    title={`${g.label} (${g.key})`}
                    className={`rounded-full px-2.5 py-1 flex items-center gap-1 transition ${
                      gizmoMode === g.mode ? "bg-white text-black" : "text-[#D4D4D8] hover:bg-white/15"
                    }`}
                  >
                    <Icon className="w-3.5 h-3.5" />
                    {g.label}
                  </button>
                );
              })}
              <span className="h-4 w-px bg-white/15 mx-1" />
              <button aria-label="Mostrar / ocultar grade"
                type="button"
                onClick={() => setShowGrid3d((v) => !v)}
                title="Mostrar / ocultar grade"
                className={`rounded-full p-1.5 transition ${showGrid3d ? "bg-white/20" : "hover:bg-white/15"}`}
              >
                <Grid3x3 className="w-3.5 h-3.5" />
              </button>
              <button aria-label="Snap (0,25 · 15° · 0,1×)"
                type="button"
                onClick={() => setSnap3d((v) => !v)}
                title="Snap (0,25 · 15° · 0,1×)"
                className={`rounded-full p-1.5 transition ${snap3d ? "bg-white/20" : "hover:bg-white/15"}`}
              >
                <Magnet className="w-3.5 h-3.5" />
              </button>
              <button aria-label="Focar objeto selecionado (F)"
                type="button"
                onClick={() => editorApiRef.current?.focusSelected()}
                title="Focar objeto selecionado (F)"
                className="rounded-full p-1.5 hover:bg-white/15 transition"
              >
                <Focus className="w-3.5 h-3.5" />
              </button>
              <button
                type="button"
                onClick={() => void addGlbObjectTo3DLayer()}
                title="Adicionar modelo .glb à sala"
                className="rounded-full px-2.5 py-1 hover:bg-white/15 transition"
              >
                + .glb
              </button>
              <button aria-label="Duplicar objeto (Ctrl+D)"
                type="button"
                onClick={duplicateRoomObject}
                disabled={!selected3dObjectId}
                title="Duplicar objeto (Ctrl+D)"
                className="rounded-full p-1.5 hover:bg-white/15 transition disabled:opacity-40"
              >
                <Copy className="w-3.5 h-3.5" />
              </button>
              <button aria-label="Excluir objeto (Del)"
                type="button"
                onClick={removeRoomObject}
                disabled={!selected3dObjectId}
                title="Excluir objeto (Del)"
                className="rounded-full p-1.5 hover:bg-white/15 transition disabled:opacity-40"
              >
                <Trash2 className="w-3.5 h-3.5" />
              </button>
              <button aria-label="Mostrar / ocultar esqueleto do objeto selecionado"
                type="button"
                onClick={() => setShowSkeleton3d((v) => !v)}
                title="Mostrar / ocultar esqueleto do objeto selecionado"
                className={`rounded-full p-1.5 transition ${showSkeleton3d ? "bg-white/20" : "hover:bg-white/15"}`}
              >
                <Bone className="w-3.5 h-3.5" />
              </button>
              <span className="h-4 w-px bg-white/15 mx-1" />
              <button
                type="button"
                onClick={saveWallpaperCamera}
                title="Grava o enquadramento atual (posição, alvo e FOV) como câmera do wallpaper"
                className="rounded-full px-3 py-1 bg-white text-black flex items-center gap-1.5 font-medium"
              >
                <Camera className="w-3.5 h-3.5" />
                Definir Câmera do Wallpaper
              </button>
            </div>
          )}
          {room3dMode && (
            <div className="pointer-events-none absolute top-3 left-3 z-40 prism-glass-pop !rounded-xl px-2.5 py-1.5 text-[10px] text-[#D4D4D8] leading-relaxed">
              Botão dir. + WASD/QE: voar · Meio/Alt+arrastar: pan · Alt+Clique: peça · Scroll: zoom · Clique: selecionar
            </div>
          )}
          {room3dMode && room3dLayer && pickedPart3d && (
            <div className="absolute bottom-16 left-1/2 -translate-x-1/2 z-40 frosted-chrome rounded-full pl-3 pr-1.5 py-1 flex items-center gap-2 text-[11px] text-white border border-amber-300/40 shadow-2xl">
              {pickedPart3d.type === "bone" ? (
                <Bone className="w-3.5 h-3.5 text-amber-300 shrink-0" />
              ) : pickedPart3d.type === "mesh" ? (
                <Box className="w-3.5 h-3.5 text-amber-200 shrink-0" />
              ) : (
                <Folder className="w-3.5 h-3.5 text-amber-200 shrink-0" />
              )}
              <span className="text-[10px] text-[#A1A1AA]">
                {pickedPart3d.type === "bone"
                  ? "Osso"
                  : pickedPart3d.type === "mesh"
                  ? "Malha"
                  : "Grupo"}
              </span>
              <span className="font-mono-fx max-w-[180px] truncate" title={pickedPart3d.name}>
                {pickedPart3d.name}
              </span>
              <button
                type="button"
                onClick={() => interactWithPickedPart(pickedPart3d)}
                className="rounded-full px-2.5 py-1 bg-amber-300 text-black font-medium text-[10px] hover:bg-amber-200 transition shrink-0"
              >
                + Interagir com esta parte
              </button>
              <button aria-label="Limpar seleção da parte"
                type="button"
                title="Limpar seleção da parte"
                onClick={() => {
                  editorApiRef.current?.highlightNode(pickedPart3d.objectId, null);
                  setPickedPart3d(null);
                }}
                className="rounded-full p-1 text-[#A1A1AA] hover:text-white hover:bg-white/10 transition"
              >
                <X className="w-3 h-3" />
              </button>
            </div>
          )}
          <AnimatePresence initial={false}>{hudOpen && (
            <motion.div {...POP} className="pointer-events-none absolute top-3 right-3 z-40 prism-glass-pop !rounded-xl px-3 py-2 text-[10px] font-mono-fx text-white/90 space-y-0.5 min-w-[180px]">
              <div className="flex items-center gap-1.5 text-[11px] font-semibold">
                <Gauge className="w-3.5 h-3.5" /> HUD de desempenho
              </div>
              <div>Energia: {POWER_PREFERENCE_LABELS[power.state]}</div>
              {Object.keys(hudStats).length === 0 ? (
                <div className="text-[#A1A1AA]">Sem camadas 3D ativas</div>
              ) : (
                Object.entries(hudStats).map(([id, st]) => (
                  <div key={id} className="pt-1 border-t border-white/10">
                    <div>
                      {st.sleeping ? "Em repouso (0 FPS)" : `${st.fps} FPS`} · {st.frameMs} ms CPU
                    </div>
                    <div>
                      {st.drawCalls} draws · {(st.triangles / 1000).toFixed(1)}k tris
                    </div>
                    <div>
                      {st.geometries} geo · {st.textures} tex · DPR {st.pixelRatio}
                    </div>
                  </div>
                ))
              )}
            </motion.div>
          )}</AnimatePresence>
          {test3dMode && (
            <div className="pointer-events-none absolute bottom-3 left-1/2 -translate-x-1/2 z-40 frosted-chrome rounded-full px-3.5 py-1.5 text-[11px] text-white flex items-center gap-2 border border-white/25">
              <span className="w-2 h-2 rounded-full bg-white animate-pulse" />
              <span>Teste 3D · Mova e clique no modelo</span>
              {hoveredMesh3d && (
                <span className="font-mono-fx text-[10px] px-2 py-0.5 rounded-full bg-white/15 text-white">
                  Malha: {hoveredMesh3d}
                </span>
              )}
            </div>
          )}
          </div>
        </div>
      </div>

      {/* PAINEL ÚNICO (direita): propriedades em cima, camadas embaixo, divisória arrastável.
          Recolhido (Ctrl+\) o canvas ocupa a largura toda; o painel nem fica montado. */}
      {panelOpen && (
      <StudioSidePanel
        layers={layersPanel}
        properties={
      <StudioInspector
        compatIssues={compatIssues}
        project={project}
        mode={inspectorMode}
        onModeChange={setInspectorMode}
        selectedLayer={selectedLayer}
        selectedParent={selectedParent}
        scene3dInfo={selectedLayer ? scene3dInspectMap[selectedLayer.id] ?? null : null}
        selected3dObjectId={selected3dObjectId}
        onSelect3dObjectId={setSelected3dObjectId}
        room3dMode={room3dMode}
        editorApiRef={editorApiRef}
        outlinerRev={outlinerRev}
        pickedPart3d={pickedPart3d}
        onPickPart3d={setPickedPart3d}
        onInteractPickedPart={interactWithPickedPart}
        selectedBindingId={selectedBindingId3d}
        onSelectBindingId={setSelectedBindingId3d}
        onAdd3dGlbObject={() => void addGlbObjectTo3DLayer()}
        test3dMode={test3dMode}
        onToggleTest3d={() => setViewMode(test3dMode ? "LAYOUT" : "TEST")}
        systemFonts={systemFonts}
        onSelectLayer={(id) => handleSelectLayer(id)}
        onPatchLayer={patchLayer}
        onAddChildToGroup={addChildToGroup}
        onApplyDiscreteProject={applyDiscreteChange}
        onApplyCoalescedProject={applyCoalescedChange}
        onChangeProjectRaw={emitLiveProject}
        onPickFile={pickFileForLayer}
        onPickDynamicTexImage={async (objectId: string, texId: string) => {
          const targetId = selectedLayerIdRef.current;
          if (!targetId) return;
          if (isTauriRuntime()) {
            try {
              const picked = await invokeCmd<PickedFileDto | null>("pick_local_file", {
                kind: "image",
              });
              if (!picked) return;
              patchLayer(
                targetId,
                (l) => {
                  const s3d = l.scene3d ?? defaultScene3D();
                  const objs = normalizeScene3DObjects(s3d);
                  const nextObjs = objs.map((obj) =>
                    obj.id === objectId
                      ? {
                          ...obj,
                          dynamicTextures: (obj.dynamicTextures ?? []).map((dt) =>
                            dt.id === texId ? { ...dt, customSrc: picked.path } : dt
                          ),
                        }
                      : obj
                  );
                  return {
                    ...l,
                    scene3d: {
                      ...s3d,
                      objects: nextObjs,
                    },
                  };
                },
                "discrete"
              );
            } catch (err) {
              flashHint(friendlyError(err, "Não foi possível carregar a imagem."));
            }
          }
        }}
        onOpenFx={setFxTarget}
        onBeginTx={() => {
          onTxStateChange?.(true);
          history.beginTransaction(projectRef.current);
        }}
        onCommitTx={() => {
          history.commitTransaction(projectRef.current);
          onTxStateChange?.(false);
          onFlushSave?.();
        }}
        onRecordStep={() => history.pushSnapshot(projectRef.current)}
        onFlushSave={onFlushSave}
      />
        }
      />
      )}

      <AnimatePresence>
        {fxTarget && (
          <FxExpressionPopover
            key={`${selectedLayer?.id ?? "none"}-${fxTarget.label}`}
            label={fxTarget.label}
            initialExpr={fxTarget.initialExpr}
            fallbackDisplay={fxTarget.fallback}
            telemetry={telemetry}
            globals={project.globals}
            onApply={fxTarget.onApply}
            onClear={fxTarget.onClear}
            onClose={() => setFxTarget(null)}
          />
        )}
      </AnimatePresence>
    </div>
  );
};
