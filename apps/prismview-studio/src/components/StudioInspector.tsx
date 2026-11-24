import { NativeCompatPanel } from "./NativeCompatPanel";
import { Select, ColorInput } from "./ui/Controls";
import { AnimatePresence, motion } from "framer-motion";
import { COLLAPSE, TAB_LITE } from "./motion/Presence";
import type { NativeCompatIssue } from "../utils/nativeCompat";
import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  AlignCenter,
  AlignCenterHorizontal,
  AlignCenterVertical,
  AlignEndHorizontal,
  AlignEndVertical,
  AlignHorizontalSpaceAround,
  AlignLeft,
  AlignRight,
  AlignStartHorizontal,
  AlignStartVertical,
  AlignVerticalSpaceAround,
  Copy,
  Crosshair,
  Eye,
  EyeOff,
  FlipHorizontal2,
  FlipVertical2,
  Lock,
  Palette,
  Plus,
  RotateCcw,
  RotateCw,
  Search,
  SlidersHorizontal,
  Sparkles,
  Trash2,
  Unlock,
  Upload,
  Variable,
} from "lucide-react";
import {
  BlendModeCss,
  BUILTIN_SVG_ICONS,
  ClickAction,
  clampScene3DFps,
  createBindingForTargetType,
  createLayer,
  createScene3DObject,
  defaultScene3D,
  duplicateScene3DObject,
  GLASS_STYLE_PRESETS,
  GlobalVar,
  LayerKind,
  normalizeReactiveBinding3D,
  normalizeScene3DObjects,
  PrismProject,
  ReactiveBinding3D,
  removeScene3DObject,
  SCENE3D_ACTION_LIST,
  SCENE3D_ACTION_META,
  SCENE3D_BINDING_SMOOTHING_MAX,
  SCENE3D_BINDING_SMOOTHING_MIN,
  Scene3DAutoSuggestion,
  Scene3DAxis,
  Scene3DBackgroundMode,
  Scene3DBindingSource,
  Scene3DBindingTarget,
  Scene3DBindingTargetKind,
  Scene3DConfig,
  Scene3DDynamicTexture,
  Scene3DEnvPreset,
  Scene3DInspectInfo,
  Scene3DObject,
  Scene3DObjectInspectInfo,
  Scene3DPickedPart,
  Scene3DShowcaseVariant,
  SCENE3D_FPS_MAX,
  SCENE3D_FPS_MIN,
  SCENE3D_FPS_STEP,
  SceneLayer,
  validateReactiveBinding3D,
} from "../types/prism";
import { FxButton } from "./FxExpressionPopover";
import { Scene3DOutliner } from "./Scene3DOutliner";
import { Scene3DQuickActions } from "./Scene3DQuickActions";
import type { Scene3DEditorApi } from "./Scene3DLayer";
import type { Scene3DObjectTree } from "../engine/scene3d/modelTree";

import { BindingTargetOption, BindingTargetPicker, InspectorSlider, Vec3Field } from "./InspectorControls";
import {
  formatMultiSelectionSummary,
  LAYER_KIND_LABELS,
  mergeProps,
  transformTextCase,
} from "../utils/multiSelect";
import type { AlignMode, DistributeAxis, FlipAxis, MatchSizeMode } from "../utils/align";

function toHexColorInput(raw?: string, fallback = "#FFFFFF"): string {
  const s = (raw || "").trim();
  if (/^#[0-9a-f]{6}$/i.test(s)) return s;
  if (/^#[0-9a-f]{3}$/i.test(s)) {
    return `#${s[1]}${s[1]}${s[2]}${s[2]}${s[3]}${s[3]}`;
  }
  return fallback;
}

const BLEND_MODES: BlendModeCss[] = [
  "normal",
  "screen",
  "multiply",
  "overlay",
  "lighten",
  "darken",
  "soft-light",
  "difference",
  "plus-lighter",
];

const BG_PRESETS: Array<{ label: string; value: string; swatch: string }> = [
  {
    label: "Obsidian",
    value: "radial-gradient(circle at 50% 18%, #1A1B21 0%, #09090C 62%, #050507 100%)",
    swatch: "radial-gradient(circle at 50% 20%, #262830, #08080a)",
  },
  {
    label: "Grafite",
    value: "linear-gradient(145deg, #181920 0%, #0B0C10 55%, #060608 100%)",
    swatch: "linear-gradient(145deg, #24262f, #09090c)",
  },
  {
    label: "OLED Puro",
    value: "#060608",
    swatch: "#060608",
  },
  {
    label: "Slate",
    value: "radial-gradient(circle at 50% 0%, #1F242D 0%, #0D1015 60%, #07080B 100%)",
    swatch: "radial-gradient(circle at 50% 0%, #2b323e, #0a0c10)",
  },
  {
    label: "Carvão",
    value: "linear-gradient(180deg, #1B1A18 0%, #0E0D0C 65%, #070706 100%)",
    swatch: "linear-gradient(180deg, #282623, #0b0a09)",
  },
  {
    label: "Névoa",
    value: "radial-gradient(circle at 50% 35%, #26272E 0%, #111216 65%, #08090B 100%)",
    swatch: "radial-gradient(circle at 50% 35%, #343640, #0c0d10)",
  },
];

export type InspectorMode = "LAYER" | "SCENE" | "GLOBALS";
type LayerTab = "CONTENT" | "STYLE" | "TRANSFORM";

export interface FxTargetRequest {
  label: string;
  initialExpr: string;
  fallback: string;
  onApply: (expr: string) => void;
  onClear: () => void;
}

export interface StudioInspectorProps {
  project: PrismProject;
  /** O que o desktop não desenha (`nativeCompatIssues`), mostrado no topo. */
  compatIssues?: NativeCompatIssue[];
  mode: InspectorMode;
  onModeChange: (mode: InspectorMode) => void;
  selectedLayer: SceneLayer | null;
  selectedLayers?: SceneLayer[];
  selectedParent: SceneLayer | null;
  scene3dInfo?: Scene3DInspectInfo | null;
  selected3dObjectId?: string | null;
  onSelect3dObjectId?: (id: string) => void;
  /** Sala 3D (ROOM) active: enables the bone/part outliner */
  room3dMode?: boolean;
  editorApiRef?: React.MutableRefObject<Scene3DEditorApi | null>;
  /** Bumped when the editor API appears or a model (re)loads */
  outlinerRev?: number;
  pickedPart3d?: Scene3DPickedPart | null;
  onPickPart3d?: (part: Scene3DPickedPart | null) => void;
  onInteractPickedPart?: (part: Scene3DPickedPart) => void;
  selectedBindingId?: string | null;
  onSelectBindingId?: (id: string | null) => void;
  onAdd3dGlbObject?: () => void;
  test3dMode?: boolean;
  onToggleTest3d?: () => void;
  systemFonts: string[];
  onSelectLayer: (id: string) => void;
  onPatchLayer: (
    id: string,
    fn: (l: SceneLayer) => SceneLayer,
    commit?: "raw" | "discrete" | "coalesced"
  ) => void;
  onPatchSelectedLayers?: (
    fn: (l: SceneLayer) => SceneLayer,
    commit?: "raw" | "discrete" | "coalesced"
  ) => void;
  onAlignSelected?: (mode: AlignMode, reference: "screen" | "selection") => void;
  onCenterSelected?: () => void;
  onDistributeSelected?: (axis: DistributeAxis) => void;
  onMatchSizeSelected?: (mode: MatchSizeMode) => void;
  onFlipSelected?: (axis: FlipAxis) => void;
  onRotate90Selected?: (dir: 1 | -1) => void;
  onAddChildToGroup: (groupId: string, kind: LayerKind) => void;
  onApplyDiscreteProject: (next: PrismProject) => void;
  onApplyCoalescedProject: (next: PrismProject) => void;
  onChangeProjectRaw: (next: PrismProject) => void;
  onPickFile: (kind: "video" | "image" | "svg" | "font" | "model3d") => void;
  onPickDynamicTexImage?: (objectId: string, texId: string) => void;
  onOpenFx: (target: FxTargetRequest) => void;
  onBeginTx: () => void;
  onCommitTx: () => void;
  onRecordStep: () => void;
  onFlushSave?: () => void;
}

export const StudioInspector: React.FC<StudioInspectorProps> = ({
  compatIssues = [],
  project,
  mode,
  onModeChange,
  selectedLayer,
  selectedLayers,
  selectedParent,
  scene3dInfo = null,
  selected3dObjectId = null,
  onSelect3dObjectId,
  room3dMode = false,
  editorApiRef,
  outlinerRev = 0,
  pickedPart3d = null,
  onPickPart3d,
  onInteractPickedPart,
  selectedBindingId = null,
  onSelectBindingId,
  onAdd3dGlbObject,
  test3dMode = false,
  onToggleTest3d,
  systemFonts,
  onSelectLayer,
  onPatchLayer,
  onPatchSelectedLayers,
  onAlignSelected,
  onCenterSelected,
  onDistributeSelected,
  onMatchSizeSelected,
  onFlipSelected,
  onRotate90Selected,
  onAddChildToGroup,
  onApplyDiscreteProject,
  onApplyCoalescedProject,
  onChangeProjectRaw,
  onPickFile,
  onPickDynamicTexImage,
  onOpenFx,
  onBeginTx,
  onCommitTx,
  onRecordStep,
  onFlushSave,
}) => {
  const [layerTab, setLayerTab] = useState<LayerTab>("CONTENT");
  const [showOptics, setShowOptics] = useState(false);
  const [selectedDynTexId, setSelectedDynTexId] = useState<string | null>(null);
  const bindingsSectionRef = useRef<HTMLDivElement | null>(null);
  const dynTexSectionRef = useRef<HTMLDivElement | null>(null);
  // Modo Pro do inspetor 3D: preferência em localStorage; abre sozinho ao focar binding/textura ou na Sala 3D
  const [show3dPro, setShow3dProState] = useState<boolean>(() => {
    try {
      return localStorage.getItem("prismview.inspector3dPro.v1") === "1";
    } catch {
      return false;
    }
  });
  const setShow3dPro = (v: boolean) => {
    setShow3dProState(v);
    try {
      localStorage.setItem("prismview.inspector3dPro.v1", v ? "1" : "0");
    } catch {
      /* storage indisponível */
    }
  };
  useEffect(() => {
    if (selectedBindingId || selectedDynTexId || room3dMode) setShow3dProState(true);
  }, [selectedBindingId, selectedDynTexId, room3dMode]);

  useEffect(() => {
    if (!selectedBindingId) return;
    setLayerTab("CONTENT");
    const raf = requestAnimationFrame(() => {
      const container = bindingsSectionRef.current;
      if (!container) return;
      const cards = container.querySelectorAll<HTMLElement>("[data-binding-id]");
      for (const el of cards) {
        if (el.dataset.bindingId === selectedBindingId) {
          el.scrollIntoView({ behavior: "smooth", block: "nearest" });
          break;
        }
      }
    });
    return () => cancelAnimationFrame(raf);
  }, [selectedBindingId]);

  useEffect(() => {
    if (!selectedDynTexId) return;
    setLayerTab("CONTENT");
    const raf = requestAnimationFrame(() => {
      const container = dynTexSectionRef.current;
      if (!container) return;
      const cards = container.querySelectorAll<HTMLElement>("[data-dyntex-id]");
      for (const el of cards) {
        if (el.dataset.dyntexId === selectedDynTexId) {
          el.scrollIntoView({ behavior: "smooth", block: "nearest" });
          break;
        }
      }
    });
    return () => cancelAnimationFrame(raf);
  }, [selectedDynTexId]);

  const sliderHandlers = {
    onBeginTx,
    onCommitTx,
    onRecordStep,
    onFlushSave,
  };

  const effectiveLayers = useMemo<SceneLayer[]>(
    () =>
      selectedLayers && selectedLayers.length > 0
        ? selectedLayers
        : selectedLayer
        ? [selectedLayer]
        : [],
    [selectedLayers, selectedLayer]
  );
  const isMulti = effectiveLayers.length >= 2;
  const merged = useMemo(() => mergeProps(effectiveLayers), [effectiveLayers]);

  const patchTarget = (
    fn: (l: SceneLayer) => SceneLayer,
    commit: "raw" | "discrete" | "coalesced" = "raw"
  ) => {
    if (isMulti && onPatchSelectedLayers) {
      onPatchSelectedLayers(fn, commit);
      return;
    }
    if (selectedLayer) {
      onPatchLayer(selectedLayer.id, fn, commit);
    }
  };

  const renderAlignAndTransformQuickBar = (count: number) => {
    const ref: "screen" | "selection" = count >= 2 ? "selection" : "screen";
    return (
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <span className="text-[11px] text-[#A1A1AA]">
            {count >= 2 ? "Alinhar entre Selecionados" : "Alinhar à Tela"}
          </span>
          {count >= 2 && onCenterSelected && (
            <button
              type="button"
              onClick={onCenterSelected}
              className="text-[10px] text-[#A1A1AA] hover:text-white underline-offset-2 hover:underline"
            >
              Centralizar na tela
            </button>
          )}
        </div>
        <div className="grid grid-cols-7 gap-1">
          {(
            [
              { m: "left" as const, t: "Alinhar à esquerda", Icon: AlignStartVertical },
              { m: "centerX" as const, t: "Centralizar na horizontal", Icon: AlignCenterVertical },
              { m: "right" as const, t: "Alinhar à direita", Icon: AlignEndVertical },
              { m: "top" as const, t: "Alinhar ao topo", Icon: AlignStartHorizontal },
              { m: "centerY" as const, t: "Centralizar na vertical", Icon: AlignCenterHorizontal },
              { m: "bottom" as const, t: "Alinhar à base", Icon: AlignEndHorizontal },
            ] as const
          ).map(({ m, t, Icon }) => (
            <button
              key={m}
              type="button"
              title={t}
              aria-label={t}
              onClick={() =>
                onAlignSelected
                  ? onAlignSelected(m, ref)
                  : patchTarget(
                      (l) => ({
                        ...l,
                        x: m === "left" ? 0.12 : m === "centerX" ? 0.5 : m === "right" ? 0.88 : l.x,
                        y: m === "top" ? 0.14 : m === "centerY" ? 0.5 : m === "bottom" ? 0.86 : l.y,
                      }),
                      "discrete"
                    )
              }
              className="prism-pill-idle grid h-7 place-items-center rounded-lg"
            >
              <Icon className="w-3.5 h-3.5" />
            </button>
          ))}
          <button
            type="button"
            title="Centralizar na tela"
            aria-label="Centralizar na tela"
            onClick={() =>
              onCenterSelected
                ? onCenterSelected()
                : patchTarget((l) => ({ ...l, x: 0.5, y: 0.5 }), "discrete")
            }
            className="prism-pill-idle grid h-7 place-items-center rounded-lg"
          >
            <Crosshair className="w-3.5 h-3.5" />
          </button>
        </div>

        <div className="grid grid-cols-4 gap-1">
          <button
            type="button"
            title="Espelhar horizontalmente"
            onClick={() =>
              onFlipSelected
                ? onFlipSelected("horizontal")
                : patchTarget((l) => ({ ...l, flipX: !l.flipX }), "discrete")
            }
            className={`py-1.5 rounded-lg text-[10px] flex items-center justify-center gap-1 ${
              !merged.common.flipX.mixed && merged.common.flipX.value
                ? "prism-pill-active"
                : "prism-pill-idle"
            }`}
          >
            <FlipHorizontal2 className="w-3 h-3" /> Esp. H
          </button>
          <button
            type="button"
            title="Espelhar verticalmente"
            onClick={() =>
              onFlipSelected
                ? onFlipSelected("vertical")
                : patchTarget((l) => ({ ...l, flipY: !l.flipY }), "discrete")
            }
            className={`py-1.5 rounded-lg text-[10px] flex items-center justify-center gap-1 ${
              !merged.common.flipY.mixed && merged.common.flipY.value
                ? "prism-pill-active"
                : "prism-pill-idle"
            }`}
          >
            <FlipVertical2 className="w-3 h-3" /> Esp. V
          </button>
          <button
            type="button"
            title="Girar -90°"
            onClick={() =>
              onRotate90Selected
                ? onRotate90Selected(-1)
                : patchTarget((l) => ({ ...l, rotation: l.rotation - 90 }), "discrete")
            }
            className="prism-pill-idle py-1.5 rounded-lg text-[10px] flex items-center justify-center gap-1"
          >
            <RotateCcw className="w-3 h-3" /> -90°
          </button>
          <button
            type="button"
            title="Girar +90°"
            onClick={() =>
              onRotate90Selected
                ? onRotate90Selected(1)
                : patchTarget((l) => ({ ...l, rotation: l.rotation + 90 }), "discrete")
            }
            className="prism-pill-idle py-1.5 rounded-lg text-[10px] flex items-center justify-center gap-1"
          >
            <RotateCw className="w-3 h-3" /> +90°
          </button>
        </div>

        {count >= 2 && (
          <div className="grid grid-cols-2 gap-1">
            {onMatchSizeSelected && (
              <>
                <button
                  type="button"
                  onClick={() => onMatchSizeSelected("width")}
                  className="prism-pill-idle py-1.5 rounded-lg text-[10px]"
                >
                  Igualar Largura
                </button>
                <button
                  type="button"
                  onClick={() => onMatchSizeSelected("height")}
                  className="prism-pill-idle py-1.5 rounded-lg text-[10px]"
                >
                  Igualar Altura
                </button>
              </>
            )}
            {count >= 3 && onDistributeSelected && (
              <>
                <button
                  type="button"
                  onClick={() => onDistributeSelected("horizontal")}
                  className="prism-pill-idle py-1.5 rounded-lg text-[10px] flex items-center justify-center gap-1"
                >
                  <AlignHorizontalSpaceAround className="w-3 h-3" /> Distribuir H
                </button>
                <button
                  type="button"
                  onClick={() => onDistributeSelected("vertical")}
                  className="prism-pill-idle py-1.5 rounded-lg text-[10px] flex items-center justify-center gap-1"
                >
                  <AlignVerticalSpaceAround className="w-3 h-3" /> Distribuir V
                </button>
              </>
            )}
          </div>
        )}
      </div>
    );
  };

  const renderSceneTab = () => {
    const isWidget = project.packageType === "WIDGET";
    const isTransparent = project.backgroundColor === "transparent";
    const solidHex = /^#[0-9a-f]{6}$/i.test(project.backgroundColor)
      ? project.backgroundColor
      : "#09090B";

    return (
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {isWidget ? (
          <>
            <div className="space-y-2">
              <span className="block text-[11px] font-medium uppercase tracking-wider text-[#A1A1AA]">
                Dimensões do Widget
              </span>
              <div className="grid grid-cols-4 gap-1.5">
                {[
                  { l: "2×2", w: 300, h: 300 },
                  { l: "4×2", w: 480, h: 220 },
                  { l: "4×4", w: 480, h: 480 },
                  { l: "6×2", w: 640, h: 220 },
                ].map((p) => {
                  const active = project.widget.width === p.w && project.widget.height === p.h;
                  return (
                    <button
                      key={p.l}
                      type="button"
                      onClick={() =>
                        onApplyDiscreteProject({
                          ...project,
                          widget: { ...project.widget, width: p.w, height: p.h },
                        })
                      }
                      className={`py-1.5 rounded-xl text-[11px] ${
                        active ? "prism-pill-active" : "prism-pill-idle"
                      }`}
                    >
                      {p.l}
                    </button>
                  );
                })}
              </div>
              <InspectorSlider
                label="Largura"
                value={project.widget.width}
                min={160}
                max={1400}
                step={8}
                unit="px"
                {...sliderHandlers}
                onChange={(w) =>
                  onChangeProjectRaw({
                    ...project,
                    widget: { ...project.widget, width: w },
                  })
                }
              />
              <InspectorSlider
                label="Altura"
                value={project.widget.height}
                min={100}
                max={1000}
                step={8}
                unit="px"
                {...sliderHandlers}
                onChange={(h) =>
                  onChangeProjectRaw({
                    ...project,
                    widget: { ...project.widget, height: h },
                  })
                }
              />
            </div>

            <div className="space-y-2.5 pt-3 border-t border-white/10">
              <label className="flex items-center justify-between frosted-chrome-card px-3 py-2 text-xs">
                <span>Fundo Transparente</span>
                <input
                  type="checkbox"
                  checked={isTransparent}
                  onChange={(e) =>
                    onApplyDiscreteProject({
                      ...project,
                      backgroundColor: e.target.checked ? "transparent" : "#0D0D10",
                    })
                  }
                />
              </label>
              {!isTransparent && (
                <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                  <span className="text-xs text-[#A1A1AA]">Cor do Fundo</span>
                  <ColorInput aria-label="Cor do fundo"
                    
                    value={solidHex}
                    onChange={(e) =>
                      onApplyCoalescedProject({
                        ...project,
                        backgroundColor: e.target.value,
                      })
                    }
                  />
                </div>
              )}
            </div>
          </>
        ) : (
          <>
            <div className="space-y-2.5">
              <span className="block text-[11px] font-medium uppercase tracking-wider text-[#A1A1AA]">
                Superfície de Fundo
              </span>
              <div className="grid grid-cols-3 gap-2">
                {BG_PRESETS.map((preset) => {
                  const active = project.backgroundColor === preset.value;
                  return (
                    <button
                      key={preset.label}
                      type="button"
                      onClick={() =>
                        onApplyDiscreteProject({
                          ...project,
                          backgroundColor: preset.value,
                        })
                      }
                      className={`rounded-xl p-1.5 border text-left transition ${
                        active
                          ? "border-white/75 bg-white/10"
                          : "border-white/10 bg-white/[0.02] hover:border-white/25"
                      }`}
                    >
                      <div
                        className="h-7 rounded-lg border border-white/10 mb-1"
                        style={{ background: preset.swatch }}
                      />
                      <span className="block text-[10px] text-white/90 truncate">{preset.label}</span>
                    </button>
                  );
                })}
              </div>
              <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                <span className="text-xs text-[#A1A1AA]">Cor Sólida Personalizada</span>
                <ColorInput aria-label="Cor sólida personalizada"
                  
                  value={solidHex}
                  onChange={(e) =>
                    onApplyCoalescedProject({
                      ...project,
                      backgroundColor: e.target.value,
                    })
                  }
                />
              </div>
            </div>

            <div className="space-y-2.5 pt-3 border-t border-white/10">
              <InspectorSlider
                label="Escurecimento (Dim)"
                value={project.dim}
                min={0}
                max={0.85}
                step={0.02}
                {...sliderHandlers}
                onChange={(dim) => onChangeProjectRaw({ ...project, dim })}
              />
            </div>
          </>
        )}
      </div>
    );
  };

  const renderGlobalsTab = () => (
    <div className="flex-1 overflow-y-auto p-4 space-y-3">
      <div className="flex items-center justify-between">
        <span className="text-[11px] font-medium uppercase tracking-wider text-[#A1A1AA]">
          Variáveis ($gv)
        </span>
        <button
          type="button"
          onClick={() =>
            onApplyDiscreteProject({
              ...project,
              globals: [
                ...project.globals,
                {
                  name: `g${project.globals.length + 1}`,
                  kind: "SWITCH",
                  value: "1",
                  label: `Variável ${project.globals.length + 1}`,
                  exposed: true,
                },
              ],
            })
          }
          className="prism-pill-active px-3 py-1 rounded-full text-[11px] flex items-center gap-1"
        >
          <Plus className="w-3 h-3" /> Nova
        </button>
      </div>

      {project.globals.length === 0 ? (
        <div className="frosted-chrome-card p-5 text-center text-xs text-[#A1A1AA]">
          Crie variáveis <code className="font-mono-fx text-white">$gv(nome)$</code> para controlar
          opacidade, cores ou visibilidade direto pelo Hub.
        </div>
      ) : (
        project.globals.map((g, i) => {
          const updateGlobal = (patch: Partial<GlobalVar>, discrete = false) => {
            const next = [...project.globals];
            next[i] = { ...g, ...patch };
            if (discrete) {
              onApplyDiscreteProject({ ...project, globals: next });
            } else {
              onApplyCoalescedProject({ ...project, globals: next });
            }
          };
          return (
            <div key={i} className="frosted-chrome-card p-3 space-y-2">
              <div className="flex items-center gap-1.5">
                <input aria-label="nome"
                  value={g.name}
                  placeholder="nome"
                  onChange={(e) => updateGlobal({ name: e.target.value })}
                  className="flex-1 bg-black/60 border border-white/10 rounded-lg px-2 py-1 font-mono-fx text-xs text-white outline-none"
                />
                <Select aria-label="Tipo da variável"
                  value={g.kind}
                  onChange={(e) =>
                    updateGlobal({ kind: e.target.value as GlobalVar["kind"] }, true)
                  }
                  className="bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                >
                  <option value="SWITCH">Switch</option>
                  <option value="NUMBER">Número</option>
                  <option value="TEXT">Texto</option>
                  <option value="COLOR">Cor</option>
                </Select>
                <button
                  type="button"
                  aria-label={`Remover variável ${g.name || i + 1}`}
                  title="Remover variável"
                  onClick={() =>
                    onApplyDiscreteProject({
                      ...project,
                      globals: project.globals.filter((_, idx) => idx !== i),
                    })
                  }
                  className="p-1 text-[#A1A1AA] hover:text-white"
                >
                  <Trash2 className="w-3.5 h-3.5" />
                </button>
              </div>
              <input aria-label="Rótulo no Hub"
                value={g.label}
                placeholder="Rótulo no Hub"
                onChange={(e) => updateGlobal({ label: e.target.value })}
                className="w-full bg-black/60 border border-white/10 rounded-lg px-2 py-1 text-xs text-[#E4E4E7] outline-none"
              />
              <div className="flex items-center justify-between gap-2">
                {g.kind === "COLOR" ? (
                  <ColorInput aria-label="Valor da variável"
                    
                    value={/^#[0-9a-f]{6}$/i.test(g.value) ? g.value : "#ffffff"}
                    onChange={(e) => updateGlobal({ value: e.target.value })}
                  />
                ) : (
                  <input aria-label="Valor"
                    value={g.value}
                    placeholder="Valor"
                    onChange={(e) => updateGlobal({ value: e.target.value })}
                    className="flex-1 bg-black/60 border border-white/10 rounded-lg px-2 py-1 font-mono-fx text-xs text-white outline-none"
                  />
                )}
                <label className="flex items-center gap-1.5 text-[11px] text-[#A1A1AA] shrink-0">
                  <input
                    type="checkbox"
                    checked={g.exposed}
                    onChange={(e) => updateGlobal({ exposed: e.target.checked }, true)}
                  />
                  Hub
                </label>
              </div>
            </div>
          );
        })
      )}
    </div>
  );

  const renderLayerTab = () => {
    if (!selectedLayer || effectiveLayers.length === 0) {
      return (
        <div className="flex-1 flex flex-col items-center justify-center p-6 text-center text-xs text-[#A1A1AA] space-y-2">
          <p>Selecione uma camada na lista abaixo ou clique no canvas.</p>
          <button
            type="button"
            onClick={() => onModeChange("SCENE")}
            className="prism-pill-idle px-3.5 py-1.5 rounded-full text-[11px]"
          >
            Configurar Fundo / Cena
          </button>
        </div>
      );
    }

    // Multiseleção de tipos mistos: exibe apenas as propriedades comuns a todos os itens selecionados.
    if (isMulti && merged.sameKind === null) {
      const anyNonText = effectiveLayers.some((l) => l.kind !== "TEXT");
      return (
        <div className="flex-1 flex flex-col min-h-0">
          <div className="px-4 pt-3 pb-2.5 border-b border-white/10 space-y-2">
            <div className="flex items-center justify-between gap-2">
              <span className="text-[10px] font-mono-fx uppercase tracking-widest text-[#A1A1AA]">
                {formatMultiSelectionSummary(merged)}
              </span>
              <div className="flex items-center gap-1">
                <button
                  type="button"
                  title="Mostrar / Ocultar selecionadas (Ctrl+Shift+H)"
                  onClick={() =>
                    patchTarget(
                      (l) => ({ ...l, hidden: !effectiveLayers.every((it) => it.hidden) }),
                      "discrete"
                    )
                  }
                  className="p-1 rounded-lg bg-white/5 text-[#A1A1AA] hover:text-white"
                >
                  {effectiveLayers.every((it) => it.hidden) ? (
                    <EyeOff className="w-3.5 h-3.5 text-white" />
                  ) : (
                    <Eye className="w-3.5 h-3.5" />
                  )}
                </button>
                <button
                  type="button"
                  title="Travar / Destravar selecionadas (Ctrl+L)"
                  onClick={() =>
                    patchTarget(
                      (l) => ({ ...l, locked: !effectiveLayers.every((it) => it.locked) }),
                      "discrete"
                    )
                  }
                  className="p-1 rounded-lg bg-white/5 text-[#A1A1AA] hover:text-white"
                >
                  {effectiveLayers.every((it) => it.locked) ? (
                    <Lock className="w-3.5 h-3.5 text-white" />
                  ) : (
                    <Unlock className="w-3.5 h-3.5" />
                  )}
                </button>
              </div>
            </div>
            <div className="flex flex-wrap gap-1">
              {merged.kinds.map((k) => (
                <span
                  key={k}
                  className="px-2 py-0.5 rounded-full bg-white/[0.06] border border-white/10 text-[10px] text-[#A1A1AA]"
                >
                  {LAYER_KIND_LABELS[k]}
                </span>
              ))}
            </div>
          </div>

          <div className="flex-1 overflow-y-auto p-4 space-y-3.5">
            {renderAlignAndTransformQuickBar(merged.count)}

            <div className="space-y-2.5 pt-2.5 border-t border-white/10">
              <span className="block text-[10px] uppercase tracking-wider text-[#71717A]">
                Propriedades Comuns
              </span>
              <InspectorSlider
                label="Opacidade"
                value={merged.common.opacity.value}
                mixed={merged.common.opacity.mixed}
                min={0}
                max={1}
                step={0.02}
                {...sliderHandlers}
                onChange={(v) => patchTarget((l) => ({ ...l, opacity: v }))}
              />

              <label className="block text-xs text-[#A1A1AA]">
                Blend Mode
                <Select
                  value={merged.common.blendMode.value}
                  mixed={merged.common.blendMode.mixed}
                  onChange={(e) =>
                    patchTarget(
                      (l) => ({ ...l, blendMode: e.target.value as BlendModeCss }),
                      "discrete"
                    )
                  }
                  className="mt-1 w-full bg-black border border-white/15 rounded-xl px-2.5 py-1.5 text-xs text-white outline-none"
                >
                  {BLEND_MODES.map((bm) => (
                    <option key={bm} value={bm}>
                      {bm}
                    </option>
                  ))}
                </Select>
              </label>

              <InspectorSlider
                label="Posição X (Relativa)"
                value={Number((merged.common.x.value * 100).toFixed(1))}
                mixed={merged.common.x.mixed}
                min={0}
                max={100}
                step={0.5}
                unit="%"
                {...sliderHandlers}
                onChange={(v) => {
                  const delta = v / 100 - merged.common.x.value;
                  patchTarget((l) => ({
                    ...l,
                    x: Math.max(0, Math.min(1, Number((l.x + delta).toFixed(4)))),
                  }));
                }}
              />
              <InspectorSlider
                label="Posição Y (Relativa)"
                value={Number((merged.common.y.value * 100).toFixed(1))}
                mixed={merged.common.y.mixed}
                min={0}
                max={100}
                step={0.5}
                unit="%"
                {...sliderHandlers}
                onChange={(v) => {
                  const delta = v / 100 - merged.common.y.value;
                  patchTarget((l) => ({
                    ...l,
                    y: Math.max(0, Math.min(1, Number((l.y + delta).toFixed(4)))),
                  }));
                }}
              />

              {anyNonText && (
                <>
                  <InspectorSlider
                    label="Largura"
                    value={merged.common.width.value}
                    mixed={merged.common.width.mixed}
                    min={8}
                    max={2200}
                    step={4}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) => (l.kind === "TEXT" ? l : { ...l, width: v }))
                    }
                  />
                  <InspectorSlider
                    label="Altura"
                    value={merged.common.height.value}
                    mixed={merged.common.height.mixed}
                    min={8}
                    max={1400}
                    step={4}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) => (l.kind === "TEXT" ? l : { ...l, height: v }))
                    }
                  />
                </>
              )}

              <InspectorSlider
                label="Rotação"
                value={merged.common.rotation.value}
                mixed={merged.common.rotation.mixed}
                min={-180}
                max={180}
                step={1}
                unit="°"
                {...sliderHandlers}
                onChange={(v) => patchTarget((l) => ({ ...l, rotation: v }))}
              />

              <InspectorSlider
                label="Parallax X"
                value={merged.common.parallaxX.value}
                mixed={merged.common.parallaxX.mixed}
                min={-1.2}
                max={1.2}
                step={0.05}
                {...sliderHandlers}
                onChange={(v) => patchTarget((l) => ({ ...l, parallaxX: v }))}
              />
              <InspectorSlider
                label="Parallax Y"
                value={merged.common.parallaxY.value}
                mixed={merged.common.parallaxY.mixed}
                min={-1.2}
                max={1.2}
                step={0.05}
                {...sliderHandlers}
                onChange={(v) => patchTarget((l) => ({ ...l, parallaxY: v }))}
              />
            </div>

            {merged.allSupportFullGlass && (
              <div className="space-y-2.5 pt-2.5 border-t border-white/10">
                <label className="flex items-center justify-between frosted-chrome-card px-3 py-2 text-xs font-medium text-white">
                  <span>
                    Vidro Óptico{" "}
                    {merged.common.glass.enabled.mixed && (
                      <span className="italic text-[#A1A1AA] font-normal">(misto)</span>
                    )}
                  </span>
                  <input
                    type="checkbox"
                    checked={!merged.common.glass.enabled.mixed && merged.common.glass.enabled.value}
                    onChange={(e) => {
                      const checked = e.target.checked;
                      patchTarget(
                        (l) => ({ ...l, glass: { ...l.glass, enabled: checked } }),
                        "discrete"
                      );
                    }}
                  />
                </label>
                {effectiveLayers.some((l) => l.glass.enabled) && (
                  <>
                    <InspectorSlider
                      label="Desfoque do Vidro"
                      value={merged.common.glass.blurPx.value}
                      mixed={merged.common.glass.blurPx.mixed}
                      min={0}
                      max={80}
                      step={1}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({ ...l, glass: { ...l.glass, blurPx: v } }))
                      }
                    />
                    <InspectorSlider
                      label="Refração Óptica"
                      value={merged.common.glass.refractionScale.value}
                      mixed={merged.common.glass.refractionScale.mixed}
                      min={0}
                      max={60}
                      step={1}
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({ ...l, glass: { ...l.glass, refractionScale: v } }))
                      }
                    />
                  </>
                )}
              </div>
            )}
          </div>
        </div>
      );
    }

    return (
      <div className="flex-1 flex flex-col min-h-0">
        {/* Selected Layer Header */}
        <div className="px-4 pt-3 pb-2.5 border-b border-white/10 space-y-2">
          <div className="flex items-center justify-between gap-2">
            <span className="text-[10px] font-mono-fx uppercase tracking-widest text-[#A1A1AA]">
              {isMulti ? formatMultiSelectionSummary(merged) : selectedLayer.kind}
            </span>
            {!isMulti && selectedParent && (
              <button
                type="button"
                onClick={() => onSelectLayer(selectedParent.id)}
                className="text-[10px] px-2 py-0.5 rounded-full bg-white/10 hover:bg-white/15 text-[#A1A1AA] hover:text-white truncate max-w-[190px]"
              >
                ↑ {selectedParent.name}
              </button>
            )}
          </div>
          <input aria-label="Nome da camada"
            value={isMulti && merged.common.name.mixed ? "" : merged.common.name.value}
            placeholder={isMulti && merged.common.name.mixed ? "misto" : "Nome da camada"}
            onChange={(e) =>
              patchTarget((l) => ({ ...l, name: e.target.value }), "coalesced")
            }
            className="w-full bg-black/60 border border-white/10 rounded-xl px-3 py-1.5 text-xs font-medium text-white placeholder:italic placeholder:text-[#71717A] outline-none focus:border-white/25"
          />

          {/* 3 Clean Layer Sub-Tabs */}
          <div className="grid grid-cols-3 gap-1 bg-black/50 p-1 rounded-xl border border-white/10">
            {(
              [
                { id: "CONTENT" as LayerTab, label: "Conteúdo" },
                { id: "STYLE" as LayerTab, label: "Vidro & Cor" },
                { id: "TRANSFORM" as LayerTab, label: "Posição" },
              ]
            ).map((t) => (
              <button
                key={t.id}
                type="button"
                onClick={() => setLayerTab(t.id)}
                className={`py-1.5 rounded-lg text-[11px] font-medium transition ${
                  layerTab === t.id
                    ? "bg-white text-black"
                    : "text-[#A1A1AA] hover:text-white"
                }`}
              >
                {t.label}
              </button>
            ))}
          </div>
        </div>

        {/* Tab Body */}
        <div className="flex-1 overflow-y-auto p-4">
          <AnimatePresence mode="wait" initial={false}>
          <motion.div key={layerTab} className="space-y-3.5" {...TAB_LITE}>
          {layerTab === "CONTENT" && (
            <>
              {selectedLayer.kind === "TEXT" && selectedLayer.text && (
                <>
                  <div className="space-y-1.5">
                    <div className="flex items-center justify-between">
                      <span className="text-xs text-[#A1A1AA]">
                        Texto / Fórmula{" "}
                        {isMulti && merged.text?.content.mixed && (
                          <span className="italic text-[#71717A]">(misto)</span>
                        )}
                      </span>
                      <div className="flex items-center gap-1">
                        {(
                          [
                            { m: "upper" as const, l: "AA", t: "MAIÚSCULAS" },
                            { m: "lower" as const, l: "aa", t: "minúsculas" },
                            { m: "capitalize" as const, l: "Aa", t: "Capitalizar" },
                          ] as const
                        ).map((c) => (
                          <button
                            key={c.m}
                            type="button"
                            title={c.t}
                            onClick={() =>
                              patchTarget(
                                (l) =>
                                  l.text
                                    ? {
                                        ...l,
                                        text: {
                                          ...l.text,
                                          content: transformTextCase(l.text.content, c.m),
                                        },
                                      }
                                    : l,
                                "discrete"
                              )
                            }
                            className="px-1.5 py-0.5 rounded-md bg-white/5 hover:bg-white/15 font-mono-fx text-[10px] text-[#A1A1AA] hover:text-white"
                          >
                            {c.l}
                          </button>
                        ))}
                        {!isMulti && (
                          <FxButton
                            active={selectedLayer.text.content.includes("$")}
                            onClick={() =>
                              onOpenFx({
                                label: "Conteúdo / Fórmula",
                                initialExpr: selectedLayer.text!.content,
                                fallback: selectedLayer.text!.content,
                                onApply: (expr) =>
                                  patchTarget(
                                    (l) => ({ ...l, text: { ...l.text!, content: expr } }),
                                    "discrete"
                                  ),
                                onClear: () =>
                                  patchTarget(
                                    (l) => ({ ...l, text: { ...l.text!, content: "18:45" } }),
                                    "discrete"
                                  ),
                              })
                            }
                          />
                        )}
                      </div>
                    </div>
                    <textarea aria-label="Texto da camada"
                      rows={2}
                      value={isMulti && merged.text?.content.mixed ? "" : selectedLayer.text.content}
                      placeholder={isMulti && merged.text?.content.mixed ? "misto" : undefined}
                      onChange={(e) =>
                        patchTarget(
                          (l) =>
                            l.text ? { ...l, text: { ...l.text, content: e.target.value } } : l,
                          "coalesced"
                        )
                      }
                      className="w-full bg-black/60 border border-white/10 rounded-xl px-3 py-2 font-mono-fx text-xs text-white placeholder:italic placeholder:text-[#71717A] outline-none resize-none focus:border-white/25"
                    />
                  </div>

                  <div className="flex items-center gap-2">
                    <Select aria-label="Família da fonte"
                      value={selectedLayer.text.fontFamily}
                      mixed={Boolean(isMulti && merged.text?.fontFamily.mixed)}
                      onChange={(e) =>
                        patchTarget(
                          (l) =>
                            l.text
                              ? {
                                  ...l,
                                  text: { ...l.text, fontFamily: e.target.value, fontPath: "" },
                                }
                              : l,
                          "discrete"
                        )
                      }
                      className="flex-1 bg-black border border-white/15 rounded-xl px-2.5 py-1.5 text-xs text-white outline-none"
                    >
                      {[selectedLayer.text.fontFamily, ...systemFonts]
                        .filter((v, i, a) => Boolean(v) && a.indexOf(v) === i)
                        .map((f) => (
                          <option key={f} value={f} style={{ fontFamily: `"${f}", sans-serif` }}>
                            {f}
                          </option>
                        ))}
                    </Select>
                    <button
                      type="button"
                      onClick={() => onPickFile("font")}
                      title="Importar fonte .ttf / .otf"
                      className="prism-pill-idle px-2.5 py-1.5 rounded-xl text-[11px] flex items-center gap-1 shrink-0"
                    >
                      <Upload className="w-3 h-3" /> .ttf
                    </button>
                  </div>

                  <InspectorSlider
                    label="Tamanho"
                    value={selectedLayer.text.fontSize}
                    mixed={Boolean(isMulti && merged.text?.fontSize.mixed)}
                    min={10}
                    max={340}
                    step={1}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.text ? { ...l, text: { ...l.text, fontSize: v } } : l
                      )
                    }
                  />

                  <InspectorSlider
                    label="Peso da Fonte"
                    value={selectedLayer.text.fontWeight}
                    mixed={Boolean(isMulti && merged.text?.fontWeight.mixed)}
                    min={200}
                    max={800}
                    step={100}
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.text ? { ...l, text: { ...l.text, fontWeight: v } } : l
                      )
                    }
                  />

                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs text-[#A1A1AA]">
                      Alinhamento{" "}
                      {isMulti && merged.text?.align.mixed && (
                        <span className="italic text-[#71717A]">(misto)</span>
                      )}
                    </span>
                    <div className="flex items-center gap-1">
                      {(
                        [
                          { id: "left" as const, Icon: AlignLeft },
                          { id: "center" as const, Icon: AlignCenter },
                          { id: "right" as const, Icon: AlignRight },
                        ]
                      ).map(({ id, Icon }) => {
                        const active =
                          !(isMulti && merged.text?.align.mixed) &&
                          selectedLayer.text?.align === id;
                        return (
                          <button
                            key={id}
                            type="button"
                            aria-label={id === "left" ? "Alinhar à esquerda" : id === "center" ? "Centralizar" : "Alinhar à direita"}
                            title={id === "left" ? "Alinhar à esquerda" : id === "center" ? "Centralizar" : "Alinhar à direita"}
                            aria-pressed={active}
                            onClick={() =>
                              patchTarget(
                                (l) => (l.text ? { ...l, text: { ...l.text, align: id } } : l),
                                "discrete"
                              )
                            }
                            className={`p-1.5 rounded-lg transition ${
                              active
                                ? "bg-white text-black"
                                : "bg-white/5 text-[#A1A1AA] hover:text-white"
                            }`}
                          >
                            <Icon className="w-3.5 h-3.5" />
                          </button>
                        );
                      })}
                    </div>
                  </div>

                  <InspectorSlider
                    label="Espaçamento"
                    value={selectedLayer.text.letterSpacing}
                    mixed={Boolean(isMulti && merged.text?.letterSpacing.mixed)}
                    min={-0.08}
                    max={0.4}
                    step={0.01}
                    unit="em"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.text ? { ...l, text: { ...l.text, letterSpacing: v } } : l
                      )
                    }
                  />

                  <InspectorSlider
                    label="Escala Vertical"
                    value={selectedLayer.text.stemStretch}
                    mixed={Boolean(isMulti && merged.text?.stemStretch.mixed)}
                    min={1}
                    max={2.4}
                    step={0.05}
                    unit="×"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.text ? { ...l, text: { ...l.text, stemStretch: v } } : l
                      )
                    }
                  />

                  <InspectorSlider
                    label="Sombra do Texto"
                    value={selectedLayer.text.shadowBlur}
                    mixed={Boolean(isMulti && merged.text?.shadowBlur.mixed)}
                    min={0}
                    max={48}
                    step={1}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.text ? { ...l, text: { ...l.text, shadowBlur: v } } : l
                      )
                    }
                  />

                  <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                    <span className="text-xs text-[#A1A1AA]">Cor do Texto</span>
                    <div className="flex items-center gap-2">
                      <ColorInput aria-label="Cor do texto"
                        mixed={Boolean(isMulti && merged.text?.color.mixed)}
                        value={toHexColorInput(selectedLayer.text.color)}
                        onChange={(e) =>
                          patchTarget(
                            (l) =>
                              l.text ? { ...l, text: { ...l.text, color: e.target.value } } : l,
                            "coalesced"
                          )
                        }
                      />
                      <FxButton
                        active={Boolean(selectedLayer.fx?.color)}
                        onClick={() =>
                          onOpenFx({
                            label: "Cor do Texto (ƒx)",
                            initialExpr: selectedLayer.fx?.color ?? "",
                            fallback: selectedLayer.text!.color,
                            onApply: (expr) =>
                              patchTarget(
                                (l) => ({ ...l, fx: { ...l.fx, color: expr } }),
                                "discrete"
                              ),
                            onClear: () =>
                              patchTarget(
                                (l) => {
                                  const fx = { ...l.fx };
                                  delete fx.color;
                                  return { ...l, fx };
                                },
                                "discrete"
                              ),
                          })
                        }
                      />
                    </div>
                  </div>

                  <InspectorSlider
                    label="Alpha da Cor (Transparência)"
                    value={Math.round((selectedLayer.text.colorAlpha ?? 1) * 100)}
                    mixed={Boolean(isMulti && merged.text?.colorAlpha.mixed)}
                    min={0}
                    max={100}
                    step={1}
                    unit="%"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.text
                          ? {
                              ...l,
                              text: { ...l.text, colorAlpha: Number((v / 100).toFixed(2)) },
                            }
                          : l
                      )
                    }
                  />
                </>
              )}

              {selectedLayer.kind === "VIDEO" && selectedLayer.video && (
                <>
                  {!isMulti && (
                    <button
                      type="button"
                      onClick={() => onPickFile("video")}
                      className="w-full prism-pill-active rounded-xl py-2.5 text-xs flex items-center justify-center gap-2"
                    >
                      <Upload className="w-3.5 h-3.5" /> Escolher Vídeo (.mp4 / .webm)
                    </button>
                  )}
                  <div className="grid grid-cols-2 gap-1.5">
                    {(["cover", "contain"] as const).map((fit) => {
                      const active =
                        !(isMulti && merged.video?.objectFit.mixed) &&
                        selectedLayer.video?.objectFit === fit;
                      return (
                        <button
                          key={fit}
                          type="button"
                          onClick={() =>
                            patchTarget(
                              (l) => (l.video ? { ...l, video: { ...l.video, objectFit: fit } } : l),
                              "discrete"
                            )
                          }
                          className={`py-1.5 rounded-xl text-[11px] ${
                            active ? "prism-pill-active" : "prism-pill-idle"
                          }`}
                        >
                          {fit === "cover" ? "Preencher (Cover)" : "Ajustar (Contain)"}
                        </button>
                      );
                    })}
                  </div>
                  <InspectorSlider
                    label="Velocidade"
                    value={selectedLayer.video.speed}
                    mixed={Boolean(isMulti && merged.video?.speed.mixed)}
                    min={0.25}
                    max={2.5}
                    step={0.05}
                    unit="×"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.video ? { ...l, video: { ...l.video, speed: v } } : l
                      )
                    }
                  />
                  <InspectorSlider
                    label="Raio do Recorte"
                    value={selectedLayer.video.clipRadius}
                    mixed={Boolean(isMulti && merged.video?.clipRadius.mixed)}
                    min={0}
                    max={120}
                    step={2}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.video ? { ...l, video: { ...l.video, clipRadius: v } } : l
                      )
                    }
                  />
                  <label className="flex items-center justify-between frosted-chrome-card px-3 py-2 text-xs">
                    <span>
                      Sem áudio{" "}
                      {isMulti && merged.video?.muted.mixed && (
                        <span className="italic text-[#71717A]">(misto)</span>
                      )}
                    </span>
                    <input
                      type="checkbox"
                      checked={!(isMulti && merged.video?.muted.mixed) && selectedLayer.video.muted}
                      onChange={(e) =>
                        patchTarget(
                          (l) =>
                            l.video ? { ...l, video: { ...l.video, muted: e.target.checked } } : l,
                          "discrete"
                        )
                      }
                    />
                  </label>
                  {effectiveLayers.some((l) => l.video && !l.video.muted) && (
                    <InspectorSlider
                      label="Volume"
                      value={selectedLayer.video.volume}
                      mixed={Boolean(isMulti && merged.video?.volume.mixed)}
                      min={0}
                      max={1}
                      step={0.05}
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) =>
                          l.video ? { ...l, video: { ...l.video, volume: v } } : l
                        )
                      }
                    />
                  )}
                </>
              )}

              {selectedLayer.kind === "IMAGE" && selectedLayer.image && (
                <>
                  <div className="grid grid-cols-2 gap-1.5">
                    {(
                      [
                        { id: "FILE" as const, label: "Arquivo Local" },
                        { id: "MEDIA_COVER" as const, label: "Capa da Música" },
                      ]
                    ).map((srcMode) => {
                      const active =
                        !(isMulti && merged.image?.sourceMode.mixed) &&
                        (selectedLayer.image?.sourceMode ?? "FILE") === srcMode.id;
                      return (
                        <button
                          key={srcMode.id}
                          type="button"
                          onClick={() =>
                            patchTarget(
                              (l) =>
                                l.image
                                  ? {
                                      ...l,
                                      image: {
                                        ...l.image,
                                        sourceMode: srcMode.id,
                                        objectFit:
                                          srcMode.id === "MEDIA_COVER" ? "cover" : l.image.objectFit,
                                      },
                                    }
                                  : l,
                              "discrete"
                            )
                          }
                          className={`py-1.5 rounded-xl text-[11px] ${
                            active ? "prism-pill-active" : "prism-pill-idle"
                          }`}
                        >
                          {srcMode.label}
                        </button>
                      );
                    })}
                  </div>
                  {!isMulti && (selectedLayer.image.sourceMode ?? "FILE") === "FILE" && (
                    <button
                      type="button"
                      onClick={() => onPickFile("image")}
                      className="w-full prism-pill-active rounded-xl py-2.5 text-xs flex items-center justify-center gap-2"
                    >
                      <Upload className="w-3.5 h-3.5" /> Carregar PNG / WebP / JPG
                    </button>
                  )}
                  <div className="grid grid-cols-2 gap-1.5">
                    {(["contain", "cover"] as const).map((fit) => {
                      const active =
                        !(isMulti && merged.image?.objectFit.mixed) &&
                        selectedLayer.image?.objectFit === fit;
                      return (
                        <button
                          key={fit}
                          type="button"
                          onClick={() =>
                            patchTarget(
                              (l) => (l.image ? { ...l, image: { ...l.image, objectFit: fit } } : l),
                              "discrete"
                            )
                          }
                          className={`py-1.5 rounded-xl text-[11px] ${
                            active ? "prism-pill-active" : "prism-pill-idle"
                          }`}
                        >
                          {fit === "contain" ? "Recorte Alpha" : "Preencher (Cover)"}
                        </button>
                      );
                    })}
                  </div>
                  <InspectorSlider
                    label="Raio da Borda"
                    value={selectedLayer.image.borderRadius}
                    mixed={Boolean(isMulti && merged.image?.borderRadius.mixed)}
                    min={0}
                    max={160}
                    step={2}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.image ? { ...l, image: { ...l.image, borderRadius: v } } : l
                      )
                    }
                  />
                </>
              )}

              {selectedLayer.kind === "PROGRESS" && selectedLayer.progress && (
                <>
                  <div className="space-y-1.5">
                    <span className="block text-xs text-[#A1A1AA]">
                      Fonte da Barra{" "}
                      {isMulti && merged.progress?.source.mixed && (
                        <span className="italic text-[#71717A]">(misto)</span>
                      )}
                    </span>
                    <div className="grid grid-cols-2 gap-1.5">
                      {(
                        [
                          { id: "MEDIA" as const, label: "Música (Timeline)" },
                          { id: "CPU" as const, label: "Uso de CPU" },
                          { id: "RAM" as const, label: "Uso de RAM" },
                          { id: "BATTERY" as const, label: "Bateria" },
                        ]
                      ).map((src) => {
                        const active =
                          !(isMulti && merged.progress?.source.mixed) &&
                          selectedLayer.progress?.source === src.id;
                        return (
                          <button
                            key={src.id}
                            type="button"
                            onClick={() =>
                              patchTarget(
                                (l) =>
                                  l.progress
                                    ? { ...l, progress: { ...l.progress, source: src.id } }
                                    : l,
                                "discrete"
                              )
                            }
                            className={`py-1.5 rounded-xl text-[11px] ${
                              active ? "prism-pill-active" : "prism-pill-idle"
                            }`}
                          >
                            {src.label}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                  <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                    <span className="text-xs text-[#A1A1AA]">Cor do Preenchimento</span>
                    <ColorInput aria-label="Cor do preenchimento"
                      mixed={Boolean(isMulti && merged.progress?.fillColor.mixed)}
                      value={toHexColorInput(selectedLayer.progress.fillColor)}
                      onChange={(e) =>
                        patchTarget(
                          (l) =>
                            l.progress
                              ? { ...l, progress: { ...l.progress, fillColor: e.target.value } }
                              : l,
                          "coalesced"
                        )
                      }
                    />
                  </div>
                  <InspectorSlider
                    label="Alpha do Preenchimento"
                    value={Math.round(selectedLayer.progress.fillAlpha * 100)}
                    mixed={Boolean(isMulti && merged.progress?.fillAlpha.mixed)}
                    min={0}
                    max={100}
                    step={2}
                    unit="%"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.progress
                          ? {
                              ...l,
                              progress: { ...l.progress, fillAlpha: Number((v / 100).toFixed(2)) },
                            }
                          : l
                      )
                    }
                  />
                  <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                    <span className="text-xs text-[#A1A1AA]">Cor da Trilha</span>
                    <ColorInput aria-label="Cor da trilha"
                      mixed={Boolean(isMulti && merged.progress?.trackColor.mixed)}
                      value={toHexColorInput(selectedLayer.progress.trackColor)}
                      onChange={(e) =>
                        patchTarget(
                          (l) =>
                            l.progress
                              ? { ...l, progress: { ...l.progress, trackColor: e.target.value } }
                              : l,
                          "coalesced"
                        )
                      }
                    />
                  </div>
                  <InspectorSlider
                    label="Alpha da Trilha"
                    value={Math.round(selectedLayer.progress.trackAlpha * 100)}
                    mixed={Boolean(isMulti && merged.progress?.trackAlpha.mixed)}
                    min={0}
                    max={100}
                    step={2}
                    unit="%"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.progress
                          ? {
                              ...l,
                              progress: { ...l.progress, trackAlpha: Number((v / 100).toFixed(2)) },
                            }
                          : l
                      )
                    }
                  />
                  <InspectorSlider
                    label="Arredondamento"
                    value={selectedLayer.progress.borderRadius}
                    mixed={Boolean(isMulti && merged.progress?.borderRadius.mixed)}
                    min={0}
                    max={999}
                    step={1}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.progress ? { ...l, progress: { ...l.progress, borderRadius: v } } : l
                      )
                    }
                  />
                </>
              )}

              {selectedLayer.kind === "SVG" && selectedLayer.svg && (
                <>
                  {!isMulti && (
                    <button
                      type="button"
                      onClick={() => onPickFile("svg")}
                      className="w-full prism-pill-active rounded-xl py-2 text-xs flex items-center justify-center gap-2"
                    >
                      <Upload className="w-3.5 h-3.5" /> Importar Arquivo .svg
                    </button>
                  )}
                  <div className="grid grid-cols-4 gap-1.5">
                    {Object.keys(BUILTIN_SVG_ICONS).map((ic) => {
                      const active =
                        !(isMulti && merged.svg?.iconName.mixed) &&
                        selectedLayer.svg?.iconName === ic &&
                        !selectedLayer.svg?.customSvg;
                      return (
                        <button
                          key={ic}
                          type="button"
                          onClick={() =>
                            patchTarget(
                              (l) =>
                                l.svg ? { ...l, svg: { ...l.svg, iconName: ic, customSvg: "" } } : l,
                              "discrete"
                            )
                          }
                          className={`py-1.5 rounded-xl text-[10px] uppercase ${
                            active ? "prism-pill-active" : "prism-pill-idle"
                          }`}
                        >
                          {ic}
                        </button>
                      );
                    })}
                  </div>
                  <InspectorSlider
                    label="Espessura do Traço"
                    value={selectedLayer.svg.strokeWidth}
                    mixed={Boolean(isMulti && merged.svg?.strokeWidth.mixed)}
                    min={0.6}
                    max={4}
                    step={0.1}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) => (l.svg ? { ...l, svg: { ...l.svg, strokeWidth: v } } : l))
                    }
                  />
                  <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                    <span className="text-xs text-[#A1A1AA]">Cor Vetorial</span>
                    <div className="flex items-center gap-2">
                      <ColorInput aria-label="Cor vetorial"
                        mixed={Boolean(isMulti && merged.svg?.color.mixed)}
                        value={toHexColorInput(selectedLayer.svg.color)}
                        onChange={(e) =>
                          patchTarget(
                            (l) => (l.svg ? { ...l, svg: { ...l.svg, color: e.target.value } } : l),
                            "coalesced"
                          )
                        }
                      />
                      <FxButton
                        active={Boolean(selectedLayer.fx?.color)}
                        onClick={() =>
                          onOpenFx({
                            label: "Cor Vetorial (ƒx)",
                            initialExpr: selectedLayer.fx?.color ?? "",
                            fallback: selectedLayer.svg!.color,
                            onApply: (expr) =>
                              patchTarget(
                                (l) => ({ ...l, fx: { ...l.fx, color: expr } }),
                                "discrete"
                              ),
                            onClear: () =>
                              patchTarget(
                                (l) => {
                                  const fx = { ...l.fx };
                                  delete fx.color;
                                  return { ...l, fx };
                                },
                                "discrete"
                              ),
                          })
                        }
                      />
                    </div>
                  </div>
                  <InspectorSlider
                    label="Alpha da Cor"
                    value={Math.round((selectedLayer.svg.colorAlpha ?? 1) * 100)}
                    mixed={Boolean(isMulti && merged.svg?.colorAlpha.mixed)}
                    min={0}
                    max={100}
                    step={2}
                    unit="%"
                    {...sliderHandlers}
                    onChange={(v) =>
                      patchTarget((l) =>
                        l.svg
                          ? { ...l, svg: { ...l.svg, colorAlpha: Number((v / 100).toFixed(2)) } }
                          : l
                      )
                    }
                  />
                </>
              )}

              {selectedLayer.kind === "GLASS" && (
                <div className="frosted-chrome-card p-3.5 space-y-2.5 text-xs text-[#A1A1AA]">
                  <p>Esta camada é uma superfície de vidro fosco translúcido.</p>
                  <button
                    type="button"
                    onClick={() => setLayerTab("STYLE")}
                    className="w-full prism-pill-active py-2 rounded-xl text-xs"
                  >
                    Ajustar Blur, Borda e Transparência
                  </button>
                </div>
              )}

              {selectedLayer.kind === "GROUP" && (
                <div className="space-y-2.5">
                  <span className="block text-xs text-[#A1A1AA]">
                    {(selectedLayer.children ?? []).length} subcamadas neste grupo. Expanda na
                    árvore à esquerda para editar cada item.
                  </span>
                  <div className="grid grid-cols-3 gap-1.5">
                    {(
                      [
                        { kind: "TEXT" as LayerKind, label: "+ Texto" },
                        { kind: "GLASS" as LayerKind, label: "+ Vidro" },
                        { kind: "SVG" as LayerKind, label: "+ Ícone" },
                      ]
                    ).map((item) => (
                      <button
                        key={item.kind}
                        type="button"
                        onClick={() => onAddChildToGroup(selectedLayer.id, item.kind)}
                        className="prism-pill-idle py-1.5 rounded-xl text-[11px]"
                      >
                        {item.label}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {selectedLayer.kind === "SCENE_3D" && (() => {
                const s3d = selectedLayer.scene3d ?? defaultScene3D();
                const patch3d = (
                  fn: (cur: Scene3DConfig) => Scene3DConfig,
                  commit: "raw" | "discrete" | "coalesced" = "raw"
                ) =>
                  onPatchLayer(
                    selectedLayer.id,
                    (l) => ({ ...l, scene3d: fn(l.scene3d ?? defaultScene3D()) }),
                    commit
                  );
                const objs = normalizeScene3DObjects(s3d);
                const activeObj = objs.find((o) => o.id === selected3dObjectId) ?? objs[0];
                const patchObjects = (
                  fn: (cur: Scene3DObject[]) => Scene3DObject[],
                  commit: "raw" | "discrete" | "coalesced" = "raw"
                ) =>
                  patch3d((c) => {
                    const cur = normalizeScene3DObjects(c);
                    const nextObjs = fn(cur);
                    return { ...c, objects: nextObjs };
                  }, commit);
                const patchActiveObj = (
                  fn: (o: Scene3DObject) => Scene3DObject,
                  commit: "raw" | "discrete" | "coalesced" = "raw"
                ) =>
                  patchObjects(
                    (list) => list.map((o) => (o.id === activeObj.id ? fn(o) : o)),
                    commit
                  );

                const activeObjInfo: Scene3DObjectInspectInfo | Scene3DInspectInfo | null =
                  scene3dInfo?.byObject?.[activeObj.id] ??
                  (activeObj.id === objs[0]?.id ? scene3dInfo : null);
                const activeVariant: Scene3DShowcaseVariant =
                  activeObj.showcaseVariant ?? s3d.showcaseVariant ?? "MONOLITH";
                const variantLabel =
                  activeVariant === "GYROSCOPE"
                    ? "Astrolábio Solar (Nativo)"
                    : activeVariant === "KINETIC_SLAB"
                    ? "Display Slab 3D (Nativo)"
                    : "Obsidian Monolith (Nativo)";
                const modelFileName =
                  activeObj.kind === "GLB" && activeObj.modelSrc
                    ? activeObj.modelSrc.split(/[\\/]/).pop() || activeObj.modelSrc
                    : variantLabel;
                const roomTris = scene3dInfo?.triangleCount ?? 0;
                const objTris = activeObjInfo?.triangleCount ?? 0;
                const vramMb = scene3dInfo?.estimatedVramMb ?? 0;
                const warnCount = scene3dInfo?.warnings?.length ?? 0;
                const budgetLabel =
                  warnCount === 0 && roomTris <= 60_000
                    ? "Ultra-Leve"
                    : roomTris <= 150_000 && vramMb <= 180
                    ? "Moderado"
                    : "Pesado";
                const budgetClass =
                  budgetLabel === "Ultra-Leve"
                    ? "bg-emerald-500/15 text-emerald-300 border border-emerald-400/25"
                    : budgetLabel === "Moderado"
                    ? "bg-amber-500/15 text-amber-200 border border-amber-400/25"
                    : "bg-red-500/15 text-red-200 border border-red-400/25";
                const fpsFollowsMonitor = s3d.fpsMode === "MONITOR";
                const fpsValue = clampScene3DFps(s3d.maxFps);
                const isAlreadyLightMode =
                  s3d.renderScale <= 0.72 && !fpsFollowsMonitor && fpsValue <= 30;
                const bgMode: Scene3DBackgroundMode = s3d.backgroundMode ?? "TRANSPARENT";
                const activeDynTex = activeObj.dynamicTextures ?? [];
                const activeSuggestions: Scene3DAutoSuggestion[] =
                  scene3dInfo?.byObject?.[activeObj.id]?.suggestions ?? [];
                const quickNodeNames = Array.from(
                  new Set([...(activeObjInfo?.boneNames ?? []), ...(activeObjInfo?.meshNames ?? [])])
                );

                return (
                  <div className="space-y-3">
                    {/* Room Objects Outliner */}
                    <div className="space-y-2">
                      <div className="flex items-center justify-between gap-2">
                        <span className="text-[11px] font-medium uppercase tracking-wider text-[#A1A1AA]">
                          Objetos da Sala ({objs.length})
                        </span>
                        <div className="flex items-center gap-1">
                          {onAdd3dGlbObject && (
                            <button
                              type="button"
                              onClick={onAdd3dGlbObject}
                              className="prism-pill-active px-2 py-0.5 rounded-full text-[10px]"
                            >
                              + .glb
                            </button>
                          )}
                          <button
                            type="button"
                            onClick={() => {
                              const nextObj = createScene3DObject("SHOWCASE", {
                                showcaseVariant: "MONOLITH",
                                position: [Number((objs.length * 1.6).toFixed(2)), 0, 0],
                              });
                              patchObjects((list) => [...list, nextObj], "discrete");
                              onSelect3dObjectId?.(nextObj.id);
                            }}
                            className="prism-pill-idle px-2 py-0.5 rounded-full text-[10px]"
                          >
                            + Escultura
                          </button>
                        </div>
                      </div>

                      <div className="space-y-1 max-h-36 overflow-y-auto pr-0.5">
                        {objs.map((obj) => {
                          const isSel = obj.id === activeObj.id;
                          return (
                            <div
                              key={obj.id}
                              onClick={() => onSelect3dObjectId?.(obj.id)}
                              className={`flex items-center justify-between gap-1.5 px-2.5 py-1.5 rounded-xl border cursor-pointer transition ${
                                isSel
                                  ? "border-white/50 bg-white/10 text-white"
                                  : "border-white/10 bg-white/[0.03] text-[#A1A1AA] hover:text-white"
                              }`}
                            >
                              <div className="flex items-center gap-1.5 min-w-0 flex-1">
                                <span className="px-1.5 py-0.5 rounded bg-black/50 border border-white/10 text-[9px] font-mono-fx shrink-0">
                                  {obj.kind === "GLB" ? "GLB" : "3D"}
                                </span>
                                <span
                                  className={`text-[11px] truncate ${obj.visible ? "" : "opacity-50 line-through"}`}
                                  title={obj.name}
                                >
                                  {obj.name}
                                </span>
                              </div>
                              <div className="flex items-center gap-0.5 shrink-0">
                                <button
                                  type="button"
                                  title={obj.visible ? "Ocultar objeto" : "Mostrar objeto"}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    patchObjects(
                                      (list) =>
                                        list.map((item) =>
                                          item.id === obj.id
                                            ? { ...item, visible: !item.visible }
                                            : item
                                        ),
                                      "discrete"
                                    );
                                  }}
                                  className="p-1 rounded hover:bg-white/10"
                                >
                                  {obj.visible ? (
                                    <Eye className="w-3 h-3" />
                                  ) : (
                                    <EyeOff className="w-3 h-3 opacity-60" />
                                  )}
                                </button>
                                <button aria-label="Duplicar objeto 3D"
                                  type="button"
                                  title="Duplicar objeto 3D"
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    let cloneId: string | null = null;
                                    patch3d((c) => {
                                      const res = duplicateScene3DObject(c, obj.id);
                                      cloneId = res.newId;
                                      return res.config;
                                    }, "discrete");
                                    if (cloneId) onSelect3dObjectId?.(cloneId);
                                  }}
                                  className="p-1 rounded hover:bg-white/10"
                                >
                                  <Copy className="w-3 h-3" />
                                </button>
                                {objs.length > 1 && (
                                  <button aria-label="Remover objeto 3D"
                                    type="button"
                                    title="Remover objeto 3D"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      const remaining = objs.filter((item) => item.id !== obj.id);
                                      patch3d((c) => removeScene3DObject(c, obj.id), "discrete");
                                      if (selected3dObjectId === obj.id && remaining[0]) {
                                        onSelect3dObjectId?.(remaining[0].id);
                                      }
                                    }}
                                    className="p-1 rounded hover:bg-white/10 text-[#A1A1AA] hover:text-red-300"
                                  >
                                    <Trash2 className="w-3 h-3" />
                                  </button>
                                )}
                              </div>
                            </div>
                          );
                        })}
                      </div>
                    </div>

                    {/* Modo Fácil: Ações Rápidas (mesmo estado que o Modo Pro abaixo) */}
                    <Scene3DQuickActions
                      objectId={activeObj.id}
                      primaryObjectId={objs[0]?.id ?? activeObj.id}
                      isGlb={activeObj.kind === "GLB" && Boolean(activeObj.modelSrc)}
                      suggestions={activeSuggestions}
                      materialNames={activeObjInfo?.materialNames ?? []}
                      nodeNames={quickNodeNames}
                      bindings={s3d.bindings ?? []}
                      dynamicTextures={activeDynTex}
                      pickedPart={pickedPart3d}
                      onPatchBindings={(fn, commit) =>
                        fn(s3d.bindings) !== s3d.bindings &&
                        patch3d((c) => {
                          const next = fn(c.bindings);
                          return next === c.bindings ? c : { ...c, bindings: next };
                        }, commit)
                      }
                      onPatchDynTex={(fn, commit) =>
                        fn(activeObj.dynamicTextures) !== activeObj.dynamicTextures &&
                        patchActiveObj((o) => {
                          const next = fn(o.dynamicTextures);
                          return next === o.dynamicTextures ? o : { ...o, dynamicTextures: next };
                        }, commit)
                      }
                      onPickCustomImage={
                        onPickDynamicTexImage
                          ? (texId) => onPickDynamicTexImage(activeObj.id, texId)
                          : undefined
                      }
                      onPickModel={() => onPickFile("model3d")}
                      onBeginTx={onBeginTx}
                      onCommitTx={onCommitTx}
                    />

                    <button
                      type="button"
                      onClick={() => setShow3dPro(!show3dPro)}
                      className="w-full flex items-center gap-2 pt-2 border-t border-white/10 text-[11px] font-medium text-[#A1A1AA] hover:text-white transition"
                      aria-expanded={show3dPro}
                    >
                      <span className="flex-1 text-left">
                        Modo Pro{" "}
                        <span className="text-[10px] font-normal text-[#71717A]">
                          · ossos, transform, clips, luz, câmera, bindings ({(s3d.bindings ?? []).length})
                        </span>
                      </span>
                      <span className="text-[10px]">{show3dPro ? "▾" : "▸"}</span>
                    </button>
                    {show3dPro && (<>
                    {/* Selected Object File & Blender GLB Replace */}
                    <div className="space-y-2 pt-2 border-t border-white/10">
                      <button
                        type="button"
                        onClick={() => onPickFile("model3d")}
                        className="w-full prism-pill-idle rounded-xl py-1.5 text-[11px] flex items-center justify-center gap-2"
                      >
                        <Upload className="w-3.5 h-3.5" /> Trocar Modelo deste Objeto (.glb / .gltf)
                      </button>

                      {activeObj.kind === "SHOWCASE" && (
                        <div className="grid grid-cols-3 gap-1">
                          {(
                            [
                              { id: "MONOLITH", label: "Monolith" },
                              { id: "GYROSCOPE", label: "Astrolábio" },
                              { id: "KINETIC_SLAB", label: "Display Slab" },
                            ] as Array<{ id: Scene3DShowcaseVariant; label: string }>
                          ).map((v) => (
                            <button
                              key={v.id}
                              type="button"
                              onClick={() =>
                                patchActiveObj(
                                  (o) => ({ ...o, showcaseVariant: v.id }),
                                  "discrete"
                                )
                              }
                              className={`py-1.5 rounded-lg text-[10px] ${
                                activeVariant === v.id ? "prism-pill-active" : "prism-pill-idle"
                              }`}
                            >
                              {v.label}
                            </button>
                          ))}
                        </div>
                      )}

                      <div className="frosted-chrome-card p-2.5 space-y-1.5">
                        <div className="flex items-center justify-between gap-2">
                          <span className="text-[11px] text-white truncate" title={activeObj.modelSrc}>
                            {modelFileName}
                          </span>
                          {activeObj.kind === "GLB" && activeObj.modelSrc && (
                            <button
                              type="button"
                              onClick={() =>
                                patchActiveObj(
                                  (o) => ({
                                    ...o,
                                    kind: "SHOWCASE",
                                    modelSrc: "",
                                    showcaseVariant: "MONOLITH",
                                    playback: { ...o.playback, clipName: "" },
                                  }),
                                  "discrete"
                                )
                              }
                              className="text-[10px] text-[#A1A1AA] hover:text-white shrink-0"
                            >
                              Resetar
                            </button>
                          )}
                        </div>
                        {activeObjInfo && (
                          <div className="flex flex-wrap items-center gap-1.5 text-[10px] font-mono-fx text-[#A1A1AA]">
                            <span className={`px-1.5 py-0.5 rounded ${budgetClass}`}>
                              {(objTris / 1000).toFixed(1)}k tris · {budgetLabel}
                            </span>
                            <span>{activeObjInfo.clipNames.length} clips</span>
                            <span>· {activeObjInfo.boneNames.length} ossos</span>
                            <span>· {activeObjInfo.materialNames.length} mats</span>
                            {(scene3dInfo?.textureCount ?? 0) > 0 && (
                              <span>
                                · {scene3dInfo?.textureCount} tex (
                                {(scene3dInfo?.maxTextureSize ?? 0) > 0
                                  ? `${scene3dInfo?.maxTextureSize}px`
                                  : "n/a"}
                                )
                              </span>
                            )}
                            {vramMb > 0 && <span>· ~{vramMb.toFixed(1)} MB VRAM</span>}
                          </div>
                        )}
                        {scene3dInfo && scene3dInfo.warnings.length > 0 && (
                          <div className="mt-1.5 rounded-lg bg-amber-500/10 border border-amber-400/25 p-2 space-y-1.5">
                            <span className="block text-[10px] font-medium text-amber-200">
                              Auditor de Performance (.glb)
                            </span>
                            <ul className="space-y-0.5 text-[10px] text-amber-100/80 list-disc pl-3.5">
                              {scene3dInfo.warnings.map((w, wi) => (
                                <li key={wi}>{w}</li>
                              ))}
                            </ul>
                            {!isAlreadyLightMode && (
                              <button
                                type="button"
                                onClick={() =>
                                  patch3d(
                                    (c) => ({
                                      ...c,
                                      renderScale: 0.7,
                                      maxFps: 30,
                                      fpsMode: "CAP",
                                    }),
                                    "discrete"
                                  )
                                }
                                className="w-full mt-1 py-1 rounded-lg bg-amber-400/20 hover:bg-amber-400/30 border border-amber-300/30 text-[10px] font-medium text-amber-100 transition"
                              >
                                Aplicar Modo Leve (70% · 30 FPS)
                              </button>
                            )}
                          </div>
                        )}
                        {activeObjInfo?.error && (
                          <p className="text-[10px] text-amber-300">{activeObjInfo.error}</p>
                        )}
                        {activeObj.kind === "GLB" && (
                          <div className="pt-1.5 space-y-1">
                            <span className="block text-[11px] font-medium text-[#A1A1AA]">
                              Ossos & Peças ({activeObj.name})
                            </span>
                            <Scene3DOutliner
                              key={activeObj.id}
                              objectId={activeObj.id}
                              roomMode={room3dMode}
                              editorApiRef={editorApiRef}
                              rev={outlinerRev}
                              pickedPart={pickedPart3d}
                              onPickPart={onPickPart3d}
                              onInteractPart={onInteractPickedPart}
                            />
                          </div>
                        )}
                      </div>
                    </div>

                    {/* Per-Object 3D Transform (Position / Rotation / Scale / Auto-Spin) */}
                    <div className="space-y-2 pt-2 border-t border-white/10">
                      <span className="block text-[11px] font-medium text-[#A1A1AA]">
                        Posição & Giro ({activeObj.name})
                      </span>
                      <Vec3Field
                        key={`pos-${activeObj.id}`}
                        label="Posição"
                        value={activeObj.position}
                        step={0.05}
                        onCommit={(v) => patchActiveObj((o) => ({ ...o, position: v }))}
                        onRecordStep={onRecordStep}
                        onFlushSave={onFlushSave}
                      />
                      <Vec3Field
                        key={`rot-${activeObj.id}`}
                        label="Rotação (°)"
                        value={activeObj.rotation}
                        step={1}
                        onCommit={(v) => patchActiveObj((o) => ({ ...o, rotation: v }))}
                        onRecordStep={onRecordStep}
                        onFlushSave={onFlushSave}
                      />
                      <Vec3Field
                        key={`scl-${activeObj.id}`}
                        label="Escala"
                        value={activeObj.scale}
                        step={0.05}
                        min={0.05}
                        onCommit={(v) => patchActiveObj((o) => ({ ...o, scale: v }))}
                        onRecordStep={onRecordStep}
                        onFlushSave={onFlushSave}
                      />
                      <InspectorSlider
                        label="Rotação Automática"
                        value={activeObj.autoRotateSpeed}
                        min={-1.5}
                        max={1.5}
                        step={0.05}
                        {...sliderHandlers}
                        onChange={(v) =>
                          patchActiveObj((o) => ({ ...o, autoRotateSpeed: v }))
                        }
                      />
                    </div>

                    {/* Animation Clips from Blender for Selected Object */}
                    {activeObjInfo && activeObjInfo.clipNames.length > 0 && (
                      <div className="space-y-2 pt-2 border-t border-white/10">
                        <label className="block text-xs text-[#A1A1AA]">
                          Animação ({activeObj.name})
                          <Select
                            value={activeObj.playback.clipName}
                            onChange={(e) =>
                              patchActiveObj(
                                (o) => ({
                                  ...o,
                                  playback: { ...o.playback, clipName: e.target.value },
                                }),
                                "discrete"
                              )
                            }
                            className="mt-1 w-full bg-black border border-white/15 rounded-xl px-2.5 py-1.5 text-xs text-white outline-none"
                          >
                            <option value="">Automática ({activeObjInfo.clipNames[0]})</option>
                            <option value="__NONE__">Pausada (Estático)</option>
                            {activeObjInfo.clipNames.map((name) => (
                              <option key={name} value={name}>
                                {name}
                              </option>
                            ))}
                          </Select>
                        </label>
                        <InspectorSlider
                          label="Velocidade da Animação"
                          value={activeObj.playback.speed}
                          min={0}
                          max={3}
                          step={0.05}
                          unit="×"
                          {...sliderHandlers}
                          onChange={(v) =>
                            patchActiveObj((o) => ({
                              ...o,
                              playback: { ...o.playback, speed: v },
                            }))
                          }
                        />
                        <label className="flex items-center justify-between text-xs text-[#A1A1AA]">
                          <span>Repetir em Loop</span>
                          <input
                            type="checkbox"
                            checked={activeObj.playback.loop}
                            onChange={(e) =>
                              patchActiveObj(
                                (o) => ({
                                  ...o,
                                  playback: { ...o.playback, loop: e.target.checked },
                                }),
                                "discrete"
                              )
                            }
                          />
                        </label>
                      </div>
                    )}

                    {/* Lighting, Room Background & IBL Environment */}
                    <div className="space-y-2 pt-2 border-t border-white/10">
                      <span className="block text-[11px] font-medium text-[#A1A1AA]">
                        Fundo da Sala & Iluminação (IBL)
                      </span>
                      <div className="grid grid-cols-3 gap-1.5">
                        <button
                          type="button"
                          onClick={() =>
                            patch3d((c) => ({ ...c, backgroundMode: "TRANSPARENT" }), "discrete")
                          }
                          className={`py-1.5 rounded-xl text-[11px] ${
                            bgMode === "TRANSPARENT" ? "prism-pill-active" : "prism-pill-idle"
                          }`}
                        >
                          Transparente
                        </button>
                        <button
                          type="button"
                          onClick={() =>
                            patch3d((c) => ({ ...c, backgroundMode: "SOLID" }), "discrete")
                          }
                          className={`py-1.5 rounded-xl text-[11px] ${
                            bgMode === "SOLID" ? "prism-pill-active" : "prism-pill-idle"
                          }`}
                        >
                          Cor Sólida
                        </button>
                        <button
                          type="button"
                          onClick={() =>
                            patch3d((c) => ({ ...c, backgroundMode: "ENVIRONMENT" }), "discrete")
                          }
                          className={`py-1.5 rounded-xl text-[11px] ${
                            bgMode === "ENVIRONMENT" ? "prism-pill-active" : "prism-pill-idle"
                          }`}
                        >
                          Ambiente
                        </button>
                      </div>
                      {bgMode === "SOLID" && (
                        <div className="flex items-center justify-between frosted-chrome-card px-3 py-1.5">
                          <span className="text-[11px] text-[#A1A1AA]">Cor do Fundo 3D</span>
                          <ColorInput aria-label="Cor do fundo 3D"
                            
                            value={toHexColorInput(s3d.backgroundColor, "#07080b")}
                            onChange={(e) =>
                              patch3d(
                                (c) => ({ ...c, backgroundColor: e.target.value }),
                                "coalesced"
                              )
                            }
                          />
                        </div>
                      )}
                      <div className="grid grid-cols-2 gap-1.5">
                        {(
                          [
                            { id: "STUDIO_DARK", label: "Studio Dark" },
                            { id: "CITY_NIGHT", label: "City Night" },
                            { id: "SUNSET", label: "Sunset" },
                            { id: "DAYLIGHT", label: "Daylight" },
                          ] as Array<{ id: Scene3DEnvPreset; label: string }>
                        ).map((env) => (
                          <button
                            key={env.id}
                            type="button"
                            onClick={() =>
                              patch3d((c) => ({ ...c, envPreset: env.id }), "discrete")
                            }
                            className={`py-1.5 rounded-xl text-[11px] ${
                              s3d.envPreset === env.id ? "prism-pill-active" : "prism-pill-idle"
                            }`}
                          >
                            {env.label}
                          </button>
                        ))}
                      </div>
                      <InspectorSlider
                        label="Intensidade IBL"
                        value={s3d.envIntensity}
                        min={0}
                        max={2.5}
                        step={0.05}
                        {...sliderHandlers}
                        onChange={(v) => patch3d((c) => ({ ...c, envIntensity: v }))}
                      />
                      <InspectorSlider
                        label="Exposição"
                        value={s3d.exposure}
                        min={0.4}
                        max={2.2}
                        step={0.05}
                        {...sliderHandlers}
                        onChange={(v) => patch3d((c) => ({ ...c, exposure: v }))}
                      />
                      <label className="flex items-center justify-between frosted-chrome-card px-3 py-2 text-xs text-white">
                        <span>Ciclo Dia / Noite (Sol Real)</span>
                        <input
                          type="checkbox"
                          checked={Boolean(s3d.autoDayNight)}
                          onChange={(e) =>
                            patch3d(
                              (c) => ({
                                ...c,
                                autoDayNight: e.target.checked,
                                dayNightHourOverride: e.target.checked ? c.dayNightHourOverride ?? null : null,
                              }),
                              "discrete"
                            )
                          }
                        />
                      </label>
                      {s3d.autoDayNight && (
                        <div className="space-y-1.5 frosted-chrome-card p-2.5">
                          <div className="flex items-center justify-between text-[10px] text-[#A1A1AA]">
                            <span>
                              {typeof s3d.dayNightHourOverride === "number"
                                ? `Prévia Solar: ${String(Math.floor(s3d.dayNightHourOverride)).padStart(2, "0")}:${String(
                                    Math.round((s3d.dayNightHourOverride % 1) * 60) % 60
                                  ).padStart(2, "0")}`
                                : "Seguindo relógio real do Windows"}
                            </span>
                            {typeof s3d.dayNightHourOverride === "number" && (
                              <button
                                type="button"
                                onClick={() =>
                                  patch3d((c) => ({ ...c, dayNightHourOverride: null }), "discrete")
                                }
                                className="text-white hover:underline"
                              >
                                Hora Real
                              </button>
                            )}
                          </div>
                          <InspectorSlider
                            label="Testar Horário"
                            value={
                              typeof s3d.dayNightHourOverride === "number"
                                ? s3d.dayNightHourOverride
                                : new Date().getHours()
                            }
                            min={0}
                            max={23.9}
                            step={0.25}
                            unit="h"
                            {...sliderHandlers}
                            onChange={(v) => patch3d((c) => ({ ...c, dayNightHourOverride: v }))}
                          />
                        </div>
                      )}
                    </div>

                    {/* Camera & Framing */}
                    <div className="space-y-2 pt-2 border-t border-white/10">
                      <span className="block text-[11px] font-medium text-[#A1A1AA]">
                        Câmera & Enquadramento
                      </span>
                      <InspectorSlider
                        label="Distância"
                        value={s3d.camera.distance}
                        min={1.4}
                        max={14}
                        step={0.1}
                        {...sliderHandlers}
                        onChange={(v) =>
                          patch3d((c) => ({ ...c, camera: { ...c.camera, distance: v } }))
                        }
                      />
                      <InspectorSlider
                        label="FOV (Lente)"
                        value={s3d.camera.fov}
                        min={24}
                        max={75}
                        step={1}
                        unit="°"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patch3d((c) => ({ ...c, camera: { ...c.camera, fov: v } }))
                        }
                      />
                      <InspectorSlider
                        label="Ângulo Horizontal"
                        value={s3d.camera.azimuthDeg}
                        min={-180}
                        max={180}
                        step={2}
                        unit="°"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patch3d((c) => ({ ...c, camera: { ...c.camera, azimuthDeg: v } }))
                        }
                      />
                      <InspectorSlider
                        label="Ângulo Vertical"
                        value={s3d.camera.elevationDeg}
                        min={-60}
                        max={75}
                        step={2}
                        unit="°"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patch3d((c) => ({ ...c, camera: { ...c.camera, elevationDeg: v } }))
                        }
                      />
                    </div>

                    {/* Desktop Mouse Orbit */}
                    <div className="space-y-2 pt-2 border-t border-white/10">
                      <span className="block text-[11px] font-medium text-[#A1A1AA]">
                        Órbita pelo Mouse (Desktop)
                      </span>
                      <InspectorSlider
                        label="Amplitude Horizontal"
                        value={s3d.camera.orbitParallaxX}
                        min={0}
                        max={35}
                        step={1}
                        unit="°"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patch3d((c) => ({ ...c, camera: { ...c.camera, orbitParallaxX: v } }))
                        }
                      />
                      <InspectorSlider
                        label="Amplitude Vertical"
                        value={s3d.camera.orbitParallaxY}
                        min={0}
                        max={25}
                        step={1}
                        unit="°"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patch3d((c) => ({ ...c, camera: { ...c.camera, orbitParallaxY: v } }))
                        }
                      />
                      <InspectorSlider
                        label="Suavidade (Spring)"
                        value={s3d.camera.damping}
                        min={0.03}
                        max={0.25}
                        step={0.01}
                        {...sliderHandlers}
                        onChange={(v) =>
                          patch3d((c) => ({ ...c, camera: { ...c.camera, damping: v } }))
                        }
                      />
                    </div>

                    {/* Phase 2: Reactive 3D Mouse, Bone Look-At & Raycast Bindings */}
                    <div ref={bindingsSectionRef} className="space-y-2.5 pt-2 border-t border-white/10">
                      <div className="flex items-center justify-between gap-2">
                        <span className="text-[11px] font-medium text-[#A1A1AA]">
                          Interações 3D (Mouse & Raycast)
                        </span>
                        {onToggleTest3d && (
                          <button
                            type="button"
                            onClick={onToggleTest3d}
                            className={`px-2 py-0.5 rounded-full text-[10px] transition ${
                              test3dMode ? "prism-pill-active" : "prism-pill-idle"
                            }`}
                          >
                            {test3dMode ? "Testando ✓" : "Testar no Preview"}
                          </button>
                        )}
                      </div>

                      {activeSuggestions.length > 0 && (
                        <div className="space-y-1.5 rounded-xl bg-white/[0.03] border border-white/10 p-2">
                          <div className="flex items-center gap-1.5 text-[10px] font-medium text-amber-200/90">
                            <Sparkles className="w-3 h-3 shrink-0" />
                            <span>Sugestões detectadas ({activeObj.name})</span>
                          </div>
                          <div className="flex flex-wrap gap-1">
                            {activeSuggestions.map((sug) => (
                              <button
                                key={sug.id}
                                type="button"
                                title={sug.detail}
                                onClick={() => {
                                  if (sug.kind === "BINDING" && sug.binding) {
                                    // Idempotente: se já existe binding equivalente (ex.: criado pelas Ações Rápidas), só seleciona
                                    const sb = sug.binding;
                                    const twin = (s3d.bindings ?? []).find(
                                      (x) =>
                                        (x.objectId || objs[0]?.id) === activeObj.id &&
                                        x.source === sb.source &&
                                        x.targetType === sb.targetType &&
                                        x.targetName === sb.targetName
                                    );
                                    if (twin) {
                                      onSelectBindingId?.(twin.id);
                                      return;
                                    }
                                    const b: ReactiveBinding3D = {
                                      ...sug.binding,
                                      id: `b3d-${Math.random().toString(36).slice(2, 9)}`,
                                      objectId: activeObj.id,
                                    };
                                    patch3d(
                                      (c) => ({ ...c, bindings: [...(c.bindings ?? []), b] }),
                                      "discrete"
                                    );
                                    onSelectBindingId?.(b.id);
                                  } else if (sug.kind === "DYNAMIC_TEXTURE" && sug.dynamicTexture) {
                                    const existing = (activeObj.dynamicTextures ?? []).find(
                                      (dt) =>
                                        dt.source === sug.dynamicTexture!.source &&
                                        dt.materialName === sug.dynamicTexture!.materialName
                                    );
                                    if (existing) {
                                      setSelectedDynTexId(existing.id);
                                    } else {
                                      const nextItem: Scene3DDynamicTexture = {
                                        ...sug.dynamicTexture,
                                        id: `dt-${Math.random().toString(36).slice(2, 9)}`,
                                      };
                                      patchActiveObj(
                                        (o) => ({
                                          ...o,
                                          dynamicTextures: [...(o.dynamicTextures ?? []), nextItem],
                                        }),
                                        "discrete"
                                      );
                                      setSelectedDynTexId(nextItem.id);
                                    }
                                  }
                                }}
                                className="px-2 py-1 rounded-full text-[10px] bg-amber-400/15 hover:bg-amber-400/25 border border-amber-300/30 text-amber-100 transition"
                              >
                                + {sug.label}
                              </button>
                            ))}
                          </div>
                        </div>
                      )}

                      <div className="grid grid-cols-2 gap-1">
                        {SCENE3D_ACTION_LIST.map((actionMeta) => (
                          <button
                            key={actionMeta.targetType}
                            type="button"
                            title={`${actionMeta.label} — ${actionMeta.example}`}
                            onClick={() => {
                              const b = createBindingForTargetType(
                                actionMeta.targetType,
                                activeObjInfo,
                                activeObj.id
                              );
                              patch3d(
                                (c) => ({ ...c, bindings: [...(c.bindings ?? []), b] }),
                                "discrete"
                              );
                              onSelectBindingId?.(b.id);
                            }}
                            className="prism-pill-idle py-1.5 px-2 rounded-xl text-[10px] truncate"
                          >
                            + {actionMeta.shortLabel}
                          </button>
                        ))}
                      </div>

                      {(s3d.bindings ?? []).length === 0 ? (
                        <p className="text-[11px] text-[#71717A]">
                          Adicione uma das 8 ações acima para reagir ao mouse, áudio (WASAPI), teclado (anônimo), música ou CPU.
                        </p>
                      ) : (
                        <div className="space-y-2">
                          {(s3d.bindings ?? []).map((b) => {
                            const hasExplicitObj = Boolean(b.objectId);
                            const resolvedObj = hasExplicitObj
                              ? objs.find((o) => o.id === b.objectId) ?? null
                              : objs[0] ?? null;
                            const bObjId = resolvedObj?.id ?? b.objectId ?? "";
                            const bObjInfo: Scene3DObjectInspectInfo | Scene3DInspectInfo | null =
                              resolvedObj
                                ? scene3dInfo?.byObject?.[resolvedObj.id] ??
                                  (resolvedObj.id === objs[0]?.id ? scene3dInfo : null)
                                : null;
                            const bTree: Scene3DObjectTree | null =
                              room3dMode && resolvedObj
                                ? editorApiRef?.current?.getObjectTree(resolvedObj.id) ?? null
                                : null;

                            // Collect valid target options strictly for this action's target kind (from E3-L3 tree + inspect fallback)
                            const treeNodeNames = new Set<string>();
                            const nodeOptions: BindingTargetOption[] = [
                              { value: "__ROOT__", label: "Modelo Inteiro (__ROOT__)" },
                            ];
                            const seenNodes = new Set<string>(["__ROOT__"]);
                            const morphOptions: BindingTargetOption[] = [];
                            const seenMorphs = new Set<string>();
                            const materialOptions: BindingTargetOption[] = [
                              { value: "*", label: "Todos os materiais (*)" },
                            ];
                            const seenMaterials = new Set<string>(["*"]);
                            const clipOptions: BindingTargetOption[] = [];
                            const seenClips = new Set<string>();

                            if (bTree) {
                              for (const n of bTree.flat) {
                                if (n.name && n.name !== "(sem nome)") {
                                  treeNodeNames.add(n.name);
                                  if (!seenNodes.has(n.name)) {
                                    seenNodes.add(n.name);
                                    const kindTag =
                                      n.type === "bone"
                                        ? "Osso"
                                        : n.type === "mesh"
                                        ? "Malha"
                                        : "Grupo";
                                    nodeOptions.push({
                                      value: n.name,
                                      label: `${kindTag} · ${n.name}`,
                                    });
                                  }
                                }
                                for (const mp of n.morphs) {
                                  if (mp && !seenMorphs.has(mp)) {
                                    seenMorphs.add(mp);
                                    morphOptions.push({ value: mp, label: `Morph · ${mp}` });
                                  }
                                }
                                for (const mat of n.materials) {
                                  if (mat && !seenMaterials.has(mat)) {
                                    seenMaterials.add(mat);
                                    materialOptions.push({ value: mat, label: `Mat · ${mat}` });
                                  }
                                }
                              }
                            }
                            for (const n of bObjInfo?.boneNames ?? []) {
                              if (n && !seenNodes.has(n)) {
                                seenNodes.add(n);
                                nodeOptions.push({ value: n, label: `Osso · ${n}` });
                              }
                            }
                            for (const n of bObjInfo?.meshNames ?? []) {
                              if (n && !seenNodes.has(n)) {
                                seenNodes.add(n);
                                nodeOptions.push({ value: n, label: `Malha · ${n}` });
                              }
                            }
                            for (const mp of bObjInfo?.morphNames ?? []) {
                              if (mp && !seenMorphs.has(mp)) {
                                seenMorphs.add(mp);
                                morphOptions.push({ value: mp, label: `Morph · ${mp}` });
                              }
                            }
                            for (const mat of bObjInfo?.materialNames ?? []) {
                              if (mat && !seenMaterials.has(mat)) {
                                seenMaterials.add(mat);
                                materialOptions.push({ value: mat, label: `Mat · ${mat}` });
                              }
                            }
                            for (const clip of bObjInfo?.clipNames ?? []) {
                              if (clip && !seenClips.has(clip)) {
                                seenClips.add(clip);
                                clipOptions.push({ value: clip, label: `Clip · ${clip}` });
                              }
                            }

                            const actionMeta =
                              SCENE3D_ACTION_META[b.targetType] ?? SCENE3D_ACTION_META.NODE_SCALE_PULSE;
                            const validation = validateReactiveBinding3D(
                              b,
                              objs,
                              bObjInfo,
                              treeNodeNames.size > 0 ? treeNodeNames : null
                            );

                            const updateBinding = (
                              patch: Partial<ReactiveBinding3D>,
                              commit: "raw" | "discrete" | "coalesced" = "discrete"
                            ) =>
                              patch3d(
                                (c) => ({
                                  ...c,
                                  bindings: (c.bindings ?? []).map((item) =>
                                    item.id === b.id
                                      ? normalizeReactiveBinding3D({ ...item, ...patch })
                                      : item
                                  ),
                                }),
                                commit
                              );

                            const removeBinding = () => {
                              if (selectedBindingId === b.id) onSelectBindingId?.(null);
                              patch3d(
                                (c) => ({
                                  ...c,
                                  bindings: (c.bindings ?? []).filter((item) => item.id !== b.id),
                                }),
                                "discrete"
                              );
                            };

                            const sourceTitleMap: Record<Scene3DBindingSource, string> = {
                              CURSOR_LOOK: "Cursor → Olhar",
                              HOVER_MESH: "Hover na Malha",
                              CLICK_MESH: "Clique na Malha",
                              AUDIO_BASS: "Áudio · Graves",
                              AUDIO_MID: "Áudio · Médios",
                              AUDIO_TREBLE: "Áudio · Agudos",
                              KEY_PULSE: "Teclado (Anônimo)",
                              MEDIA_PLAYING: "Música Tocando",
                              CPU_USAGE: "Uso de CPU",
                            };

                            const targetOptions =
                              actionMeta.targetKind === "NODE"
                                ? nodeOptions
                                : actionMeta.targetKind === "MORPH"
                                ? morphOptions
                                : actionMeta.targetKind === "MATERIAL"
                                ? materialOptions
                                : clipOptions;
                            const targetLabel =
                              actionMeta.targetKind === "NODE"
                                ? "Alvo (Osso / Malha / Grupo)"
                                : actionMeta.targetKind === "MORPH"
                                ? "Alvo (Morph / Shape Key)"
                                : actionMeta.targetKind === "MATERIAL"
                                ? "Alvo (Material)"
                                : "Alvo (Animação / Clip)";
                            const targetEmptyLabel =
                              actionMeta.targetKind === "MORPH"
                                ? "(Objeto sem Shape Keys / Morphs)"
                                : actionMeta.targetKind === "CLIP"
                                ? "(Objeto sem clips de animação)"
                                : "(Sem alvos disponíveis)";
                            const effectiveTargetValue =
                              actionMeta.targetKind === "NODE"
                                ? b.targetName || "__ROOT__"
                                : actionMeta.targetKind === "MATERIAL"
                                ? b.targetName || "*"
                                : b.targetName;

                            const isSelectedBinding = selectedBindingId === b.id;

                            return (
                              <div
                                key={b.id}
                                data-binding-id={b.id}
                                onClick={() => onSelectBindingId?.(b.id)}
                                className={`frosted-chrome-card p-2.5 space-y-2 transition ${
                                  !validation.valid
                                    ? "border-red-400/50 bg-red-500/[0.06]"
                                    : ""
                                } ${
                                  isSelectedBinding
                                    ? "ring-1 ring-amber-300/80 border-amber-300/60"
                                    : ""
                                }`}
                              >
                                <div className="flex items-center justify-between gap-1.5">
                                  <label className="flex items-center gap-1.5 text-[11px] text-white font-medium min-w-0">
                                    <input
                                      type="checkbox"
                                      checked={b.enabled}
                                      onChange={(e) => updateBinding({ enabled: e.target.checked })}
                                    />
                                    <span className="truncate">
                                      {sourceTitleMap[b.source] ?? b.source}
                                    </span>
                                  </label>
                                  <div className="flex items-center gap-1 shrink-0">
                                    {!validation.valid && (
                                      <span
                                        title={validation.reasons.join(" ")}
                                        className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-full bg-red-500/20 border border-red-400/40 text-[9px] font-medium text-red-200"
                                      >
                                        <AlertTriangle className="w-2.5 h-2.5 shrink-0" />
                                        Inválido
                                      </span>
                                    )}
                                    <button aria-label="Remover regra"
                                      type="button"
                                      onClick={(e) => {
                                        e.stopPropagation();
                                        removeBinding();
                                      }}
                                      className="p-1 text-[#A1A1AA] hover:text-white"
                                      title="Remover regra"
                                    >
                                      <Trash2 className="w-3 h-3" />
                                    </button>
                                  </div>
                                </div>

                                {!validation.valid && (
                                  <div className="rounded-lg bg-red-500/10 border border-red-400/30 px-2 py-1 text-[10px] text-red-200 leading-snug">
                                    {validation.reasons.join(" ")}
                                  </div>
                                )}

                                <label className="block text-[10px] text-[#A1A1AA]">
                                  Objeto na Sala
                                  <Select
                                    value={bObjId}
                                    onChange={(e) => updateBinding({ objectId: e.target.value })}
                                    className={`mt-1 w-full bg-black border rounded-lg px-2 py-1 text-[11px] text-white outline-none ${
                                      validation.missingObject
                                        ? "border-red-400/60"
                                        : "border-white/15"
                                    }`}
                                  >
                                    {validation.missingObject && bObjId && (
                                      <option value={bObjId}>
                                        ⚠ Objeto ausente ({bObjId})
                                      </option>
                                    )}
                                    {objs.map((o) => (
                                      <option key={o.id} value={o.id}>
                                        {o.name}
                                      </option>
                                    ))}
                                  </Select>
                                </label>

                                <div className="space-y-1">
                                  <div className="grid grid-cols-2 gap-1.5">
                                    <label className="block text-[10px] text-[#A1A1AA]">
                                      Fonte do Sinal
                                      <Select
                                        value={b.source}
                                        onChange={(e) => {
                                          const nextSrc = e.target.value as Scene3DBindingSource;
                                          const nextTarget: Scene3DBindingTarget =
                                            nextSrc === "CURSOR_LOOK"
                                              ? "NODE_LOOK_AT"
                                              : b.targetType === "NODE_LOOK_AT"
                                              ? "NODE_ROTATE_SWING"
                                              : b.targetType;
                                          const nextMeta = SCENE3D_ACTION_META[nextTarget];
                                          const patch: Partial<ReactiveBinding3D> = {
                                            source: nextSrc,
                                            targetType: nextTarget,
                                          };
                                          if (nextTarget !== b.targetType) {
                                            patch.strength = nextMeta.defaultStrength;
                                            patch.smoothing = nextMeta.defaultSmoothing;
                                            if (nextMeta.usesAxis && !b.axis) {
                                              patch.axis = nextMeta.defaultAxis ?? "Z";
                                            }
                                            if (actionMeta.targetKind !== nextMeta.targetKind) {
                                              patch.targetName = "__ROOT__";
                                            }
                                          }
                                          updateBinding(patch);
                                        }}
                                        className="mt-1 w-full bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                      >
                                        <option value="CURSOR_LOOK">Mouse (Olhar)</option>
                                        <option value="HOVER_MESH">Hover em Malha</option>
                                        <option value="CLICK_MESH">Clique em Malha</option>
                                        <option value="AUDIO_BASS">Áudio · Graves</option>
                                        <option value="AUDIO_MID">Áudio · Médios</option>
                                        <option value="AUDIO_TREBLE">Áudio · Agudos</option>
                                        <option value="KEY_PULSE">Teclado (Anônimo)</option>
                                        <option value="MEDIA_PLAYING">Música Tocando</option>
                                        <option value="CPU_USAGE">Uso de CPU</option>
                                      </Select>
                                    </label>

                                    <label className="block text-[10px] text-[#A1A1AA]">
                                      Ação 3D
                                      <Select
                                        value={b.targetType}
                                        onChange={(e) => {
                                          const nextType = e.target.value as Scene3DBindingTarget;
                                          const nextMeta = SCENE3D_ACTION_META[nextType];
                                          let nextTargetName = b.targetName;
                                          if (nextMeta.targetKind !== actionMeta.targetKind) {
                                            if (nextMeta.targetKind === "NODE") {
                                              nextTargetName = "__ROOT__";
                                            } else if (nextMeta.targetKind === "CLIP") {
                                              nextTargetName = clipOptions[0]?.value ?? "";
                                            } else if (nextMeta.targetKind === "MATERIAL") {
                                              nextTargetName =
                                                materialOptions[1]?.value ??
                                                materialOptions[0]?.value ??
                                                "*";
                                            } else {
                                              nextTargetName = morphOptions[0]?.value ?? "";
                                            }
                                          }
                                          updateBinding({
                                            source:
                                              nextType === "NODE_LOOK_AT"
                                                ? "CURSOR_LOOK"
                                                : b.source === "CURSOR_LOOK"
                                                ? "AUDIO_BASS"
                                                : b.source,
                                            targetType: nextType,
                                            targetName: nextTargetName,
                                            strength: nextMeta.defaultStrength,
                                            smoothing: nextMeta.defaultSmoothing,
                                            axis: nextMeta.usesAxis
                                              ? b.axis ?? nextMeta.defaultAxis ?? "Z"
                                              : undefined,
                                            invertAxis: nextMeta.usesAxis
                                              ? Boolean(b.invertAxis)
                                              : undefined,
                                          });
                                        }}
                                        className="mt-1 w-full bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                      >
                                        {SCENE3D_ACTION_LIST.map((m) => (
                                          <option key={m.targetType} value={m.targetType}>
                                            {m.label}
                                          </option>
                                        ))}
                                      </Select>
                                    </label>
                                  </div>
                                  <p className="text-[10px] text-[#71717A] leading-tight">
                                    {actionMeta.example}
                                  </p>
                                </div>

                                {(b.source === "HOVER_MESH" || b.source === "CLICK_MESH") && (
                                  <label className="block text-[10px] text-[#A1A1AA]">
                                    Malha Gatilho (Raycast)
                                    <Select
                                      value={b.sourceNode || "*"}
                                      onChange={(e) => updateBinding({ sourceNode: e.target.value })}
                                      className={`mt-1 w-full bg-black border rounded-lg px-2 py-1 text-[11px] text-white outline-none ${
                                        validation.missingSourceNode
                                          ? "border-red-400/60"
                                          : "border-white/15"
                                      }`}
                                    >
                                      <option value="*">Qualquer malha do modelo (*)</option>
                                      {validation.missingSourceNode && b.sourceNode && (
                                        <option value={b.sourceNode}>
                                          ⚠ Inválido: {b.sourceNode}
                                        </option>
                                      )}
                                      {(bObjInfo?.meshNames ?? []).map((m) => (
                                        <option key={m} value={m}>
                                          {m}
                                        </option>
                                      ))}
                                    </Select>
                                  </label>
                                )}

                                <BindingTargetPicker
                                  label={targetLabel}
                                  targetKind={actionMeta.targetKind}
                                  value={effectiveTargetValue}
                                  options={targetOptions}
                                  invalid={validation.missingTarget}
                                  emptyLabel={targetEmptyLabel}
                                  onChange={(nextVal) => updateBinding({ targetName: nextVal })}
                                />

                                {actionMeta.usesStrength && (
                                  <InspectorSlider
                                    label={actionMeta.strengthLabel}
                                    value={b.strength}
                                    min={actionMeta.strengthMin}
                                    max={actionMeta.strengthMax}
                                    step={actionMeta.strengthStep}
                                    unit={actionMeta.strengthUnit}
                                    {...sliderHandlers}
                                    onChange={(v) => updateBinding({ strength: v }, "raw")}
                                  />
                                )}

                                {actionMeta.usesSmoothing && (
                                  <InspectorSlider
                                    label="Suavidade Spring"
                                    value={b.smoothing}
                                    min={SCENE3D_BINDING_SMOOTHING_MIN}
                                    max={SCENE3D_BINDING_SMOOTHING_MAX}
                                    step={0.01}
                                    {...sliderHandlers}
                                    onChange={(v) => updateBinding({ smoothing: v }, "raw")}
                                  />
                                )}

                                {actionMeta.usesAxis && (
                                  <div className="flex items-center justify-between gap-2 text-[11px] text-[#A1A1AA]">
                                    <span>Eixo</span>
                                    <div className="flex items-center gap-1">
                                      {(["X", "Y", "Z"] as Scene3DAxis[]).map((ax) => (
                                        <button
                                          key={ax}
                                          type="button"
                                          onClick={() => updateBinding({ axis: ax })}
                                          className={`w-7 py-0.5 rounded-lg text-[10px] font-mono-fx ${
                                            (b.axis ?? "Y") === ax
                                              ? "prism-pill-active"
                                              : "prism-pill-idle"
                                          }`}
                                        >
                                          {ax}
                                        </button>
                                      ))}
                                      <label className="flex items-center gap-1 ml-1.5">
                                        <input
                                          type="checkbox"
                                          checked={Boolean(b.invertAxis)}
                                          onChange={(e) =>
                                            updateBinding({ invertAxis: e.target.checked })
                                          }
                                        />
                                        Inverter
                                      </label>
                                    </div>
                                  </div>
                                )}

                                {actionMeta.usesInvertXY && (
                                  <div className="flex items-center gap-4 text-[11px] text-[#A1A1AA]">
                                    <label className="flex items-center gap-1.5">
                                      <input
                                        type="checkbox"
                                        checked={Boolean(b.invertX)}
                                        onChange={(e) => updateBinding({ invertX: e.target.checked })}
                                      />
                                      Inverter X
                                    </label>
                                    <label className="flex items-center gap-1.5">
                                      <input
                                        type="checkbox"
                                        checked={Boolean(b.invertY)}
                                        onChange={(e) => updateBinding({ invertY: e.target.checked })}
                                      />
                                      Inverter Y
                                    </label>
                                  </div>
                                )}
                              </div>
                            );
                          })}
                        </div>
                      )}
                    </div>

                    {/* Phase 3: Dynamic 3D Material Textures (Live Media Album Cover / Custom Image) */}
                    <div ref={dynTexSectionRef} className="space-y-2 pt-2 border-t border-white/10">
                      <div className="flex items-center justify-between gap-2">
                        <span className="text-[11px] font-medium text-[#A1A1AA]">
                          Texturas Dinâmicas ({activeObj.name})
                        </span>
                        <button
                          type="button"
                          onClick={() => {
                            const nextItem: Scene3DDynamicTexture = {
                              id: `dt-${Math.random().toString(36).slice(2, 9)}`,
                              materialName: activeObjInfo?.materialNames[0] ?? "*",
                              source: "MEDIA_COVER",
                              emissiveMap: true,
                            };
                            patchActiveObj(
                              (o) => ({
                                ...o,
                                dynamicTextures: [...(o.dynamicTextures ?? []), nextItem],
                              }),
                              "discrete"
                            );
                          }}
                          className="prism-pill-idle px-2 py-0.5 rounded-full text-[10px]"
                        >
                          + Capa da Música
                        </button>
                      </div>
                      {activeDynTex.length === 0 ? (
                        <p className="text-[11px] text-[#71717A]">
                          Projete a capa do Spotify/YouTube ao vivo ou uma imagem dentro de qualquer material deste objeto 3D (ex: monitor ou quadro).
                        </p>
                      ) : (
                        <div className="space-y-2">
                          {activeDynTex.map((dt) => {
                            const updateDt = (patch: Partial<Scene3DDynamicTexture>) =>
                              patchActiveObj(
                                (o) => ({
                                  ...o,
                                  dynamicTextures: (o.dynamicTextures ?? []).map((item) =>
                                    item.id === dt.id ? { ...item, ...patch } : item
                                  ),
                                }),
                                "discrete"
                              );
                            const removeDt = () => {
                              if (selectedDynTexId === dt.id) setSelectedDynTexId(null);
                              patchActiveObj(
                                (o) => ({
                                  ...o,
                                  dynamicTextures: (o.dynamicTextures ?? []).filter(
                                    (item) => item.id !== dt.id
                                  ),
                                }),
                                "discrete"
                              );
                            };
                            const isSelectedDynTex = selectedDynTexId === dt.id;
                            return (
                              <div
                                key={dt.id}
                                data-dyntex-id={dt.id}
                                onClick={() => setSelectedDynTexId(dt.id)}
                                className={`frosted-chrome-card p-2.5 space-y-2 transition ${
                                  isSelectedDynTex
                                    ? "ring-1 ring-amber-300/80 border-amber-300/60"
                                    : ""
                                }`}
                              >
                                <div className="flex items-center justify-between gap-2">
                                  <span className="text-[11px] text-white font-medium truncate">
                                    Material: {dt.materialName === "*" ? "Todos (*)" : dt.materialName}
                                  </span>
                                  <button aria-label="Remover textura dinâmica"
                                    type="button"
                                    onClick={removeDt}
                                    className="p-1 text-[#A1A1AA] hover:text-white"
                                    title="Remover textura dinâmica"
                                  >
                                    <Trash2 className="w-3 h-3" />
                                  </button>
                                </div>
                                <div className="grid grid-cols-2 gap-1.5">
                                  <Select aria-label="Material"
                                    value={dt.materialName || "*"}
                                    onChange={(e) => updateDt({ materialName: e.target.value })}
                                    className="bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                  >
                                    <option value="*">Todos os materiais (*)</option>
                                    {(activeObjInfo?.materialNames ?? []).map((mat) => (
                                      <option key={mat} value={mat}>
                                        {mat}
                                      </option>
                                    ))}
                                  </Select>
                                  <Select aria-label="Origem da textura"
                                    value={dt.source}
                                    onChange={(e) =>
                                      updateDt({
                                        source: e.target.value as "MEDIA_COVER" | "CUSTOM_IMAGE",
                                      })
                                    }
                                    className="bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                  >
                                    <option value="MEDIA_COVER">Capa da Música</option>
                                    <option value="CUSTOM_IMAGE">Imagem Local</option>
                                  </Select>
                                </div>
                                {dt.source === "CUSTOM_IMAGE" && onPickDynamicTexImage && (
                                  <button
                                    type="button"
                                    onClick={() => onPickDynamicTexImage(activeObj.id, dt.id)}
                                    className="w-full prism-pill-idle py-1 rounded-lg text-[10px] flex items-center justify-center gap-1.5"
                                  >
                                    <Upload className="w-3 h-3" />
                                    {dt.customSrc
                                      ? dt.customSrc.split(/[\\/]/).pop()
                                      : "Escolher Imagem…"}
                                  </button>
                                )}
                                <label className="flex items-center justify-between text-[11px] text-[#A1A1AA]">
                                  <span>Modo Tela Luminosa (Emissive)</span>
                                  <input
                                    type="checkbox"
                                    checked={Boolean(dt.emissiveMap)}
                                    onChange={(e) => updateDt({ emissiveMap: e.target.checked })}
                                  />
                                </label>
                              </div>
                            );
                          })}
                        </div>
                      )}
                    </div>

                    {/* GPU Thermal Budget */}
                    <div className="space-y-2 pt-2 border-t border-white/10">
                      <span className="block text-[11px] font-medium text-[#A1A1AA]">
                        Orçamento GPU (Ultra-Leve)
                      </span>
                      <InspectorSlider
                        label="Escala de Resolução"
                        value={Math.round(s3d.renderScale * 100)}
                        min={50}
                        max={100}
                        step={5}
                        unit="%"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patch3d((c) => ({ ...c, renderScale: Number((v / 100).toFixed(2)) }))
                        }
                      />
                      <label className="flex items-center justify-between frosted-chrome-card px-3 py-2 text-xs text-white">
                        <span title="Acompanha a taxa de atualização do monitor (60, 144, 240 Hz…)">
                          Taxa do monitor (sem limite)
                        </span>
                        <input
                          type="checkbox"
                          checked={fpsFollowsMonitor}
                          onChange={(e) =>
                            patch3d(
                              (c) => ({ ...c, fpsMode: e.target.checked ? "MONITOR" : "CAP" }),
                              "discrete"
                            )
                          }
                        />
                      </label>
                      {!fpsFollowsMonitor && (
                        <InspectorSlider
                          label="Limite de FPS"
                          value={fpsValue}
                          min={SCENE3D_FPS_MIN}
                          max={SCENE3D_FPS_MAX}
                          step={SCENE3D_FPS_STEP}
                          unit=" FPS"
                          {...sliderHandlers}
                          onChange={(v) =>
                            patch3d((c) => ({ ...c, maxFps: clampScene3DFps(v), fpsMode: "CAP" }))
                          }
                        />
                      )}
                      <p className="text-[10px] text-[#71717A]">
                        A cena só desenha quando algo se move; parada, o consumo cai a zero. No modo
                        Econômico o teto vira 30 FPS e no Ultra, 15 FPS.
                      </p>
                    </div>
                    </>)}
                  </div>
                );
              })()}
            </>
          )}

          {layerTab === "STYLE" && (() => {
            const supportsFullGlass =
              selectedLayer.kind === "GLASS" ||
              selectedLayer.kind === "TEXT" ||
              selectedLayer.kind === "GROUP" ||
              selectedLayer.kind === "IMAGE";
            const anyGlassEnabled = effectiveLayers.some((l) => l.glass.enabled);
            return (
            <>
              {supportsFullGlass ? (
                <label className="flex items-center justify-between frosted-chrome-card px-3 py-2 text-xs font-medium text-white">
                  <span>
                    Vidro Óptico & Desfoque{" "}
                    {isMulti && merged.common.glass.enabled.mixed && (
                      <span className="italic text-[#71717A] font-normal">(misto)</span>
                    )}
                  </span>
                  <input
                    type="checkbox"
                    checked={!(isMulti && merged.common.glass.enabled.mixed) && selectedLayer.glass.enabled}
                    onChange={(e) => {
                      const checked = e.target.checked;
                      patchTarget(
                        (l) => {
                          const nextText =
                            l.kind === "TEXT" && l.text && (l.glass.textMode ?? "GLYPH") === "GLYPH"
                              ? checked && (l.text.colorAlpha ?? 1) >= 0.85
                                ? { ...l.text, colorAlpha: 0.16 }
                                : !checked && (l.text.colorAlpha ?? 1) <= 0.25
                                ? { ...l.text, colorAlpha: 1 }
                                : l.text
                              : l.text;
                          return {
                            ...l,
                            text: nextText,
                            glass: { ...l.glass, enabled: checked },
                          };
                        },
                        "discrete"
                      );
                    }}
                  />
                </label>
              ) : selectedLayer.kind === "SVG" ? (
                <div className="space-y-2.5">
                  <label className="flex items-center justify-between frosted-chrome-card px-3 py-2 text-xs font-medium text-white">
                    <span>
                      Dispersão Prismática no Ícone{" "}
                      {isMulti && merged.common.glass.enabled.mixed && (
                        <span className="italic text-[#71717A] font-normal">(misto)</span>
                      )}
                    </span>
                    <input
                      type="checkbox"
                      checked={!(isMulti && merged.common.glass.enabled.mixed) && selectedLayer.glass.enabled}
                      onChange={(e) =>
                        patchTarget(
                          (l) => ({
                            ...l,
                            glass: {
                              ...l.glass,
                              enabled: e.target.checked,
                              chromaticAberration:
                                e.target.checked && l.glass.chromaticAberration <= 0.05
                                  ? 1.2
                                  : l.glass.chromaticAberration,
                            },
                          }),
                          "discrete"
                        )
                      }
                    />
                  </label>
                  {anyGlassEnabled && (
                    <InspectorSlider
                      label="Dispersão RGB"
                      value={selectedLayer.glass.chromaticAberration}
                      mixed={Boolean(isMulti && merged.common.glass.chromaticAberration.mixed)}
                      min={0}
                      max={6}
                      step={0.2}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, chromaticAberration: v },
                        }))
                      }
                    />
                  )}
                </div>
              ) : (
                <p className="frosted-chrome-card px-3 py-2 text-[11px] text-[#71717A]">
                  Vidro óptico disponível em Placa de Vidro, Texto, Grupo e Imagem.
                </p>
              )}

              {supportsFullGlass && anyGlassEnabled && (
                <div className="space-y-3">
                  {/* 1-Click Optical Glass Presets */}
                  <div className="space-y-1.5">
                    <span className="block text-[10px] uppercase tracking-wider text-[#71717A]">
                      Estilos de Vidro
                    </span>
                    <div className="grid grid-cols-2 gap-1.5">
                      {GLASS_STYLE_PRESETS.map((preset) => (
                        <button
                          key={preset.label}
                          type="button"
                          onClick={() =>
                            patchTarget(
                              (l) => ({
                                ...l,
                                text:
                                  l.kind === "TEXT" &&
                                  l.text &&
                                  (l.glass.textMode ?? "GLYPH") === "GLYPH" &&
                                  (l.text.colorAlpha ?? 1) >= 0.85
                                    ? { ...l.text, colorAlpha: 0.16 }
                                    : l.text,
                                glass: { ...l.glass, ...preset.patch, enabled: true },
                              }),
                              "discrete"
                            )
                          }
                          className="prism-pill-idle py-1.5 px-2 rounded-xl text-[11px] text-left truncate"
                        >
                          {preset.label}
                        </button>
                      ))}
                    </div>
                  </div>

                  {selectedLayer.kind === "TEXT" && (
                    <div className="space-y-1.5">
                      <span className="block text-[11px] text-[#A1A1AA]">
                        Modo do Vidro no Texto{" "}
                        {isMulti && merged.common.glass.textMode.mixed && (
                          <span className="italic text-[#71717A]">(misto)</span>
                        )}
                      </span>
                      <div className="grid grid-cols-2 gap-1.5">
                        {(
                          [
                            { id: "GLYPH" as const, label: "Nas Letras (Vidro)" },
                            { id: "PILL" as const, label: "Pílula Atrás" },
                          ]
                        ).map((tm) => {
                          const active =
                            !(isMulti && merged.common.glass.textMode.mixed) &&
                            (selectedLayer.glass.textMode ?? "GLYPH") === tm.id;
                          return (
                            <button
                              key={tm.id}
                              type="button"
                              onClick={() =>
                                patchTarget(
                                  (l) => ({
                                    ...l,
                                    text:
                                      l.text
                                        ? tm.id === "GLYPH" && (l.text.colorAlpha ?? 1) >= 0.85
                                          ? { ...l.text, colorAlpha: 0.16 }
                                          : tm.id === "PILL" && (l.text.colorAlpha ?? 1) <= 0.25
                                          ? { ...l.text, colorAlpha: 1 }
                                          : l.text
                                        : l.text,
                                    glass: { ...l.glass, textMode: tm.id },
                                  }),
                                  "discrete"
                                )
                              }
                              className={`py-1.5 rounded-xl text-[11px] ${
                                active ? "prism-pill-active" : "prism-pill-idle"
                              }`}
                            >
                              {tm.label}
                            </button>
                          );
                        })}
                      </div>
                      {(selectedLayer.glass.textMode ?? "GLYPH") === "GLYPH" &&
                        selectedLayer.text &&
                        (selectedLayer.text.colorAlpha ?? 1) > 0.35 && (
                          <div className="frosted-chrome-card px-2.5 py-2 flex items-center justify-between gap-2">
                            <span className="text-[10px] text-[#A1A1AA] leading-tight">
                              Cor da letra em {Math.round((selectedLayer.text.colorAlpha ?? 1) * 100)}% cobre a refração.
                            </span>
                            <button
                              type="button"
                              onClick={() =>
                                patchTarget(
                                  (l) =>
                                    l.text
                                      ? { ...l, text: { ...l.text, colorAlpha: 0.14 } }
                                      : l,
                                  "discrete"
                                )
                              }
                              className="prism-pill-active px-2 py-1 rounded-lg text-[10px] shrink-0"
                            >
                              Usar 14%
                            </button>
                          </div>
                        )}
                    </div>
                  )}

                  {/* Unified Optical Blur/Frost & Lens Refraction Controls */}
                  <div className="space-y-2.5 pt-2 border-t border-white/10">
                    <span className="block text-[10px] uppercase tracking-wider text-[#71717A]">
                      Jateamento & Refração Óptica
                    </span>
                    <InspectorSlider
                      label="Jateamento / Desfoque"
                      value={selectedLayer.glass.blurPx}
                      mixed={Boolean(isMulti && merged.common.glass.blurPx.mixed)}
                      min={0}
                      max={80}
                      step={1}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, blurPx: v },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Refração Óptica (Lente)"
                      value={selectedLayer.glass.refractionScale}
                      mixed={Boolean(isMulti && merged.common.glass.refractionScale.mixed)}
                      min={0}
                      max={60}
                      step={1}
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, refractionScale: v },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Chanfro da Lente (Bevel)"
                      value={selectedLayer.glass.bevelWidth ?? 22}
                      mixed={Boolean(isMulti && merged.common.glass.bevelWidth.mixed)}
                      min={4}
                      max={64}
                      step={2}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, bevelWidth: v },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Brilho Especular"
                      value={Math.round((selectedLayer.glass.specularIntensity ?? 0.45) * 100)}
                      mixed={Boolean(isMulti && merged.common.glass.specularIntensity.mixed)}
                      min={0}
                      max={100}
                      step={2}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, specularIntensity: v / 100 },
                        }))
                      }
                    />
                  </div>

                  {/* Surface Tint, Grain & Optical Transmission */}
                  <div className="space-y-2.5 pt-2 border-t border-white/10">
                    <span className="block text-[10px] uppercase tracking-wider text-[#71717A]">
                      Véu, Textura & Luz
                    </span>
                    <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                      <span className="text-xs text-[#A1A1AA]">Cor do Véu (Tint)</span>
                      <ColorInput aria-label="Cor do véu"
                        mixed={Boolean(isMulti && merged.common.glass.tintColor.mixed)}
                        value={toHexColorInput(selectedLayer.glass.tintColor)}
                        onChange={(e) =>
                          patchTarget(
                            (l) => ({ ...l, glass: { ...l.glass, tintColor: e.target.value } }),
                            "coalesced"
                          )
                        }
                      />
                    </div>
                    <InspectorSlider
                      label="Opacidade do Véu"
                      value={Math.round(selectedLayer.glass.frostOpacity * 100)}
                      mixed={Boolean(isMulti && merged.common.glass.frostOpacity.mixed)}
                      min={0}
                      max={80}
                      step={1}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, frostOpacity: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Granulação (Areia Fina)"
                      value={Math.round((selectedLayer.glass.noiseOpacity ?? 0.04) * 100)}
                      mixed={Boolean(isMulti && merged.common.glass.noiseOpacity.mixed)}
                      min={0}
                      max={35}
                      step={1}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, noiseOpacity: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Luminosidade"
                      value={Math.round((selectedLayer.glass.brightness ?? 1.06) * 100)}
                      mixed={Boolean(isMulti && merged.common.glass.brightness.mixed)}
                      min={60}
                      max={160}
                      step={2}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, brightness: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Contraste"
                      value={Math.round((selectedLayer.glass.contrast ?? 1.04) * 100)}
                      mixed={Boolean(isMulti && merged.common.glass.contrast.mixed)}
                      min={80}
                      max={160}
                      step={2}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, contrast: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Saturação"
                      value={Math.round(selectedLayer.glass.saturation * 100)}
                      mixed={Boolean(isMulti && merged.common.glass.saturation.mixed)}
                      min={0}
                      max={240}
                      step={5}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, saturation: v / 100 },
                        }))
                      }
                    />
                  </div>

                  {/* Geometry & Finish */}
                  <div className="space-y-2.5 pt-2 border-t border-white/10">
                    <span className="block text-[10px] uppercase tracking-wider text-[#71717A]">
                      Acabamento
                    </span>
                    <InspectorSlider
                      label="Borda Sutil"
                      value={Math.round(selectedLayer.glass.borderOpacity * 100)}
                      mixed={Boolean(isMulti && merged.common.glass.borderOpacity.mixed)}
                      min={0}
                      max={70}
                      step={2}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, borderOpacity: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Raio da Borda"
                      value={selectedLayer.glass.borderRadius}
                      mixed={Boolean(isMulti && merged.common.glass.borderRadius.mixed)}
                      min={0}
                      max={160}
                      step={2}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, borderRadius: v },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Sombra"
                      value={selectedLayer.glass.shadowBlur}
                      mixed={Boolean(isMulti && merged.common.glass.shadowBlur.mixed)}
                      min={0}
                      max={64}
                      step={2}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        patchTarget((l) => ({
                          ...l,
                          glass: { ...l.glass, shadowBlur: v },
                        }))
                      }
                    />
                  </div>

                  <button
                    type="button"
                    onClick={() => setShowOptics((v) => !v)}
                    className="w-full text-left text-[11px] text-[#A1A1AA] hover:text-white pt-1"
                  >
                    {showOptics ? "▾ Ocultar dispersão prismática" : "▸ Dispersão prismática opcional (RGB)"}
                  </button>
                  <AnimatePresence initial={false}>{showOptics && (
                    <motion.div {...COLLAPSE}><div className="space-y-2.5 pl-2 border-l border-white/10">
                      <InspectorSlider
                        label="Dispersão RGB"
                        value={selectedLayer.glass.chromaticAberration}
                        mixed={Boolean(isMulti && merged.common.glass.chromaticAberration.mixed)}
                        min={0}
                        max={6}
                        step={0.2}
                        unit="px"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patchTarget((l) => ({
                            ...l,
                            glass: { ...l.glass, chromaticAberration: v },
                          }))
                        }
                      />
                    </div></motion.div>
                  )}</AnimatePresence>
                </div>
              )}

              <div className="space-y-2.5 pt-2.5 border-t border-white/10">
                <InspectorSlider
                  label="Opacidade"
                  value={selectedLayer.opacity}
                  mixed={Boolean(isMulti && merged.common.opacity.mixed)}
                  min={0}
                  max={1}
                  step={0.02}
                  fxActive={Boolean(selectedLayer.fx?.opacity)}
                  onFx={() =>
                    onOpenFx({
                      label: "Opacidade (ƒx)",
                      initialExpr: selectedLayer.fx?.opacity ?? "",
                      fallback: String(selectedLayer.opacity),
                      onApply: (expr) =>
                        patchTarget(
                          (l) => ({ ...l, fx: { ...l.fx, opacity: expr } }),
                          "discrete"
                        ),
                      onClear: () =>
                        patchTarget(
                          (l) => {
                            const fx = { ...l.fx };
                            delete fx.opacity;
                            return { ...l, fx };
                          },
                          "discrete"
                        ),
                    })
                  }
                  {...sliderHandlers}
                  onChange={(v) => patchTarget((l) => ({ ...l, opacity: v }))}
                />

                <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                  <span className="text-xs text-[#A1A1AA]">Visibilidade (ƒx)</span>
                  <FxButton
                    active={Boolean(selectedLayer.fx?.visible)}
                    onClick={() =>
                      onOpenFx({
                        label: "Visibilidade Condicional (ƒx)",
                        initialExpr: selectedLayer.fx?.visible ?? "",
                        fallback: "1",
                        onApply: (expr) =>
                          patchTarget(
                            (l) => ({ ...l, fx: { ...l.fx, visible: expr } }),
                            "discrete"
                          ),
                        onClear: () =>
                          patchTarget(
                            (l) => {
                              const fx = { ...l.fx };
                              delete fx.visible;
                              return { ...l, fx };
                            },
                            "discrete"
                          ),
                      })
                    }
                  />
                </div>

                <label className="block text-xs text-[#A1A1AA]">
                  Blend Mode
                  <Select
                    value={selectedLayer.blendMode}
                    mixed={Boolean(isMulti && merged.common.blendMode.mixed)}
                    onChange={(e) =>
                      patchTarget(
                        (l) => ({ ...l, blendMode: e.target.value as BlendModeCss }),
                        "discrete"
                      )
                    }
                    className="mt-1 w-full bg-black border border-white/15 rounded-xl px-2.5 py-1.5 text-xs text-white outline-none"
                  >
                    {BLEND_MODES.map((bm) => (
                      <option key={bm} value={bm}>
                        {bm}
                      </option>
                    ))}
                  </Select>
                </label>
              </div>
            </>
            );
          })()}

          {layerTab === "TRANSFORM" && (
            <>
              {renderAlignAndTransformQuickBar(merged.count)}

              <InspectorSlider
                label={isMulti ? "Posição X (Relativa)" : "Posição X"}
                value={Number(((isMulti ? merged.common.x.value : selectedLayer.x) * 100).toFixed(1))}
                mixed={Boolean(isMulti && merged.common.x.mixed)}
                min={0}
                max={100}
                step={0.5}
                unit="%"
                {...sliderHandlers}
                onChange={(v) => {
                  if (isMulti) {
                    const delta = v / 100 - merged.common.x.value;
                    patchTarget((l) => ({
                      ...l,
                      x: Math.max(0, Math.min(1, Number((l.x + delta).toFixed(4)))),
                    }));
                  } else {
                    patchTarget((l) => ({ ...l, x: Number((v / 100).toFixed(4)) }));
                  }
                }}
              />
              <InspectorSlider
                label={isMulti ? "Posição Y (Relativa)" : "Posição Y"}
                value={Number(((isMulti ? merged.common.y.value : selectedLayer.y) * 100).toFixed(1))}
                mixed={Boolean(isMulti && merged.common.y.mixed)}
                min={0}
                max={100}
                step={0.5}
                unit="%"
                {...sliderHandlers}
                onChange={(v) => {
                  if (isMulti) {
                    const delta = v / 100 - merged.common.y.value;
                    patchTarget((l) => ({
                      ...l,
                      y: Math.max(0, Math.min(1, Number((l.y + delta).toFixed(4)))),
                    }));
                  } else {
                    patchTarget((l) => ({ ...l, y: Number((v / 100).toFixed(4)) }));
                  }
                }}
              />

              {selectedLayer.kind !== "TEXT" && (
                <>
                  <InspectorSlider
                    label="Largura"
                    value={selectedLayer.width}
                    mixed={Boolean(isMulti && merged.common.width.mixed)}
                    min={selectedLayer.kind === "PROGRESS" ? 16 : 8}
                    max={2200}
                    step={selectedLayer.kind === "SVG" ? 2 : 4}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) => patchTarget((l) => ({ ...l, width: v }))}
                  />
                  <InspectorSlider
                    label="Altura"
                    value={selectedLayer.height}
                    mixed={Boolean(isMulti && merged.common.height.mixed)}
                    min={selectedLayer.kind === "PROGRESS" ? 2 : 8}
                    max={1400}
                    step={selectedLayer.kind === "PROGRESS" ? 1 : selectedLayer.kind === "SVG" ? 2 : 4}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) => patchTarget((l) => ({ ...l, height: v }))}
                  />
                </>
              )}

              <InspectorSlider
                label="Rotação"
                value={selectedLayer.rotation}
                mixed={Boolean(isMulti && merged.common.rotation.mixed)}
                min={-180}
                max={180}
                step={1}
                unit="°"
                fxActive={Boolean(selectedLayer.fx?.rotation)}
                onFx={() =>
                  onOpenFx({
                    label: "Rotação (ƒx)",
                    initialExpr: selectedLayer.fx?.rotation ?? "",
                    fallback: String(selectedLayer.rotation),
                    onApply: (expr) =>
                      patchTarget(
                        (l) => ({ ...l, fx: { ...l.fx, rotation: expr } }),
                        "discrete"
                      ),
                    onClear: () =>
                      patchTarget(
                        (l) => {
                          const fx = { ...l.fx };
                          delete fx.rotation;
                          return { ...l, fx };
                        },
                        "discrete"
                      ),
                  })
                }
                {...sliderHandlers}
                onChange={(v) => patchTarget((l) => ({ ...l, rotation: v }))}
              />

              <div className="space-y-2 pt-2.5 border-t border-white/10">
                <label className="block text-xs text-[#A1A1AA]">
                  Ação ao Clicar (Widgets / Player)
                  <Select
                    value={selectedLayer.clickAction ?? "NONE"}
                    mixed={Boolean(isMulti && merged.common.clickAction.mixed)}
                    onChange={(e) =>
                      patchTarget(
                        (l) => ({ ...l, clickAction: e.target.value as ClickAction }),
                        "discrete"
                      )
                    }
                    className="mt-1 w-full bg-black border border-white/15 rounded-xl px-2.5 py-1.5 text-xs text-white outline-none"
                  >
                    <option value="NONE">Nenhuma</option>
                    <option value="MEDIA_PLAY_PAUSE">Mídia · Play / Pause</option>
                    <option value="MEDIA_NEXT">Mídia · Próxima Faixa</option>
                    <option value="MEDIA_PREV">Mídia · Faixa Anterior</option>
                    <option value="OPEN_URL">Abrir URL</option>
                    <option value="OPEN_APP">Abrir App / Arquivo</option>
                    <option value="TOGGLE_GLOBAL">Alternar Variável (liga/desliga)</option>
                    <option value="SET_GLOBAL">Definir Variável</option>
                    <option value="NEXT_GLOBAL_LIST">Próximo Valor da Lista</option>
                  </Select>
                </label>
                {!isMulti && (() => {
                  const action = selectedLayer.clickAction ?? "NONE";
                  const field = (
                    key: "clickUrl" | "clickTarget" | "clickGlobal" | "clickValue",
                    label: string,
                    placeholder: string
                  ) => (
                    <label key={key} className="block text-xs text-[#A1A1AA]">
                      {label}
                      <input
                        value={selectedLayer[key] ?? ""}
                        placeholder={placeholder}
                        onChange={(e) =>
                          patchTarget((l) => ({ ...l, [key]: e.target.value }), "discrete")
                        }
                        className="mt-1 w-full bg-black border border-white/15 rounded-xl px-2.5 py-1.5 text-xs text-white outline-none"
                      />
                    </label>
                  );
                  if (action === "OPEN_URL") return field("clickUrl", "URL", "https://…");
                  if (action === "OPEN_APP") return field("clickTarget", "Caminho", "C:/Programas/app.exe");
                  if (action === "TOGGLE_GLOBAL" || action === "NEXT_GLOBAL_LIST")
                    return field("clickGlobal", "Variável", "nome");
                  if (action === "SET_GLOBAL")
                    return (
                      <>
                        {field("clickGlobal", "Variável", "nome")}
                        {field("clickValue", "Valor", "1")}
                      </>
                    );
                  return null;
                })()}
                {selectedLayer.clickAction && !selectedLayer.clickAction.startsWith("MEDIA") && selectedLayer.clickAction !== "NONE" && (
                  <p className="text-[10px] text-[#71717A]">Executado pelo motor nativo ao tocar no desktop.</p>
                )}
              </div>

              <div className="space-y-2.5 pt-2.5 border-t border-white/10">
                <div className="flex items-center justify-between">
                  <span className="text-[11px] font-medium text-[#A1A1AA]">Parallax 3D (Mouse)</span>
                  <span className="text-[10px] text-[#71717A]">Ativo no desktop</span>
                </div>
                <InspectorSlider
                  label="Eixo X"
                  value={selectedLayer.parallaxX}
                  mixed={Boolean(isMulti && merged.common.parallaxX.mixed)}
                  min={-1.2}
                  max={1.2}
                  step={0.05}
                  {...sliderHandlers}
                  onChange={(v) => patchTarget((l) => ({ ...l, parallaxX: v }))}
                />
                <InspectorSlider
                  label="Eixo Y"
                  value={selectedLayer.parallaxY}
                  mixed={Boolean(isMulti && merged.common.parallaxY.mixed)}
                  min={-1.2}
                  max={1.2}
                  step={0.05}
                  {...sliderHandlers}
                  onChange={(v) => patchTarget((l) => ({ ...l, parallaxY: v }))}
                />
              </div>
            </>
          )}
          </motion.div>
          </AnimatePresence>
        </div>
      </div>
    );
  };

  return (
    // Conteúdo da metade de cima do painel único (`StudioSidePanel`): sem posição nem moldura próprias.
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <NativeCompatPanel issues={compatIssues} onSelectLayer={onSelectLayer} />
      {/* Top 3-Way Context Switcher */}
      <div className="p-2.5 border-b border-white/10 grid grid-cols-3 gap-1">
        {(
          [
            { id: "LAYER" as InspectorMode, label: "Camada", Icon: SlidersHorizontal },
            { id: "SCENE" as InspectorMode, label: "Fundo", Icon: Palette },
            { id: "GLOBALS" as InspectorMode, label: "Globais", Icon: Variable },
          ]
        ).map(({ id, label, Icon }) => (
          <button
            key={id}
            type="button"
            onClick={() => onModeChange(id)}
            className={`py-1.5 px-2 rounded-xl text-[11px] font-medium flex items-center justify-center gap-1.5 transition ${
              mode === id ? "prism-pill-active" : "prism-pill-idle border-transparent"
            }`}
          >
            <Icon className="w-3 h-3" />
            {label}
          </button>
        ))}
      </div>

      <AnimatePresence mode="wait" initial={false}>
        <motion.div key={mode} className="flex min-h-0 flex-1 flex-col" {...TAB_LITE}>
          {mode === "SCENE"
            ? renderSceneTab()
            : mode === "GLOBALS"
            ? renderGlobalsTab()
            : renderLayerTab()}
        </motion.div>
      </AnimatePresence>
    </div>
  );
};
export { createGroupChildLayer } from "./InspectorControls";
