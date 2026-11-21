import React, { useState } from "react";
import {
  AlignCenter,
  AlignLeft,
  AlignRight,
  Copy,
  Eye,
  EyeOff,
  Palette,
  Plus,
  SlidersHorizontal,
  Trash2,
  Upload,
  Variable,
} from "lucide-react";
import {
  BlendModeCss,
  BUILTIN_SVG_ICONS,
  ClickAction,
  clampScene3DFps,
  createBinding3D,
  createLayer,
  createScene3DObject,
  defaultScene3D,
  GLASS_STYLE_PRESETS,
  GlobalVar,
  LayerKind,
  normalizeScene3DObjects,
  PrismProject,
  ReactiveBinding3D,
  Scene3DAxis,
  Scene3DBackgroundMode,
  Scene3DBindingSource,
  Scene3DBindingTarget,
  Scene3DConfig,
  Scene3DDynamicTexture,
  Scene3DEnvPreset,
  Scene3DInspectInfo,
  Scene3DObject,
  Scene3DObjectInspectInfo,
  Scene3DShowcaseVariant,
  SCENE3D_FPS_MAX,
  SCENE3D_FPS_MIN,
  SCENE3D_FPS_STEP,
  SceneLayer,
} from "../types/prism";
import { FxButton } from "./FxExpressionPopover";

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
  mode: InspectorMode;
  onModeChange: (mode: InspectorMode) => void;
  selectedLayer: SceneLayer | null;
  selectedParent: SceneLayer | null;
  scene3dInfo?: Scene3DInspectInfo | null;
  selected3dObjectId?: string | null;
  onSelect3dObjectId?: (id: string) => void;
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
  project,
  mode,
  onModeChange,
  selectedLayer,
  selectedParent,
  scene3dInfo = null,
  selected3dObjectId = null,
  onSelect3dObjectId,
  onAdd3dGlbObject,
  test3dMode = false,
  onToggleTest3d,
  systemFonts,
  onSelectLayer,
  onPatchLayer,
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

  const sliderHandlers = {
    onBeginTx,
    onCommitTx,
    onRecordStep,
    onFlushSave,
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
                  <input
                    type="color"
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
                <input
                  type="color"
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
                <input
                  value={g.name}
                  placeholder="nome"
                  onChange={(e) => updateGlobal({ name: e.target.value })}
                  className="flex-1 bg-black/60 border border-white/10 rounded-lg px-2 py-1 font-mono-fx text-xs text-white outline-none"
                />
                <select
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
                </select>
                <button
                  type="button"
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
              <input
                value={g.label}
                placeholder="Rótulo no Hub"
                onChange={(e) => updateGlobal({ label: e.target.value })}
                className="w-full bg-black/60 border border-white/10 rounded-lg px-2 py-1 text-xs text-[#E4E4E7] outline-none"
              />
              <div className="flex items-center justify-between gap-2">
                {g.kind === "COLOR" ? (
                  <input
                    type="color"
                    value={/^#[0-9a-f]{6}$/i.test(g.value) ? g.value : "#ffffff"}
                    onChange={(e) => updateGlobal({ value: e.target.value })}
                  />
                ) : (
                  <input
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
    if (!selectedLayer) {
      return (
        <div className="flex-1 flex flex-col items-center justify-center p-6 text-center text-xs text-[#A1A1AA] space-y-2">
          <p>Selecione uma camada à esquerda ou clique no canvas.</p>
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

    return (
      <div className="flex-1 flex flex-col min-h-0">
        {/* Selected Layer Header */}
        <div className="px-4 pt-3 pb-2.5 border-b border-white/10 space-y-2">
          <div className="flex items-center justify-between gap-2">
            <span className="text-[10px] font-mono-fx uppercase tracking-widest text-[#A1A1AA]">
              {selectedLayer.kind}
            </span>
            {selectedParent && (
              <button
                type="button"
                onClick={() => onSelectLayer(selectedParent.id)}
                className="text-[10px] px-2 py-0.5 rounded-full bg-white/10 hover:bg-white/15 text-[#A1A1AA] hover:text-white truncate max-w-[190px]"
              >
                ↑ {selectedParent.name}
              </button>
            )}
          </div>
          <input
            value={selectedLayer.name}
            onChange={(e) =>
              onPatchLayer(selectedLayer.id, (l) => ({ ...l, name: e.target.value }), "coalesced")
            }
            className="w-full bg-black/60 border border-white/10 rounded-xl px-3 py-1.5 text-xs font-medium text-white outline-none focus:border-white/25"
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
        <div className="flex-1 overflow-y-auto p-4 space-y-3.5">
          {layerTab === "CONTENT" && (
            <>
              {selectedLayer.kind === "TEXT" && selectedLayer.text && (
                <>
                  <div className="space-y-1.5">
                    <div className="flex items-center justify-between">
                      <span className="text-xs text-[#A1A1AA]">Texto / Fórmula</span>
                      <FxButton
                        active={selectedLayer.text.content.includes("$")}
                        onClick={() =>
                          onOpenFx({
                            label: "Conteúdo / Fórmula",
                            initialExpr: selectedLayer.text!.content,
                            fallback: selectedLayer.text!.content,
                            onApply: (expr) =>
                              onPatchLayer(
                                selectedLayer.id,
                                (l) => ({ ...l, text: { ...l.text!, content: expr } }),
                                "discrete"
                              ),
                            onClear: () =>
                              onPatchLayer(
                                selectedLayer.id,
                                (l) => ({ ...l, text: { ...l.text!, content: "18:45" } }),
                                "discrete"
                              ),
                          })
                        }
                      />
                    </div>
                    <textarea
                      rows={2}
                      value={selectedLayer.text.content}
                      onChange={(e) =>
                        onPatchLayer(
                          selectedLayer.id,
                          (l) => ({ ...l, text: { ...l.text!, content: e.target.value } }),
                          "coalesced"
                        )
                      }
                      className="w-full bg-black/60 border border-white/10 rounded-xl px-3 py-2 font-mono-fx text-xs text-white outline-none resize-none focus:border-white/25"
                    />
                  </div>

                  <div className="flex items-center gap-2">
                    <select
                      value={selectedLayer.text.fontFamily}
                      onChange={(e) =>
                        onPatchLayer(
                          selectedLayer.id,
                          (l) => ({
                            ...l,
                            text: { ...l.text!, fontFamily: e.target.value, fontPath: "" },
                          }),
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
                    </select>
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
                    min={10}
                    max={340}
                    step={1}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        text: { ...l.text!, fontSize: v },
                      }))
                    }
                  />

                  <InspectorSlider
                    label="Peso da Fonte"
                    value={selectedLayer.text.fontWeight}
                    min={200}
                    max={800}
                    step={100}
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        text: { ...l.text!, fontWeight: v },
                      }))
                    }
                  />

                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs text-[#A1A1AA]">Alinhamento</span>
                    <div className="flex items-center gap-1">
                      {(
                        [
                          { id: "left" as const, Icon: AlignLeft },
                          { id: "center" as const, Icon: AlignCenter },
                          { id: "right" as const, Icon: AlignRight },
                        ]
                      ).map(({ id, Icon }) => (
                        <button
                          key={id}
                          type="button"
                          onClick={() =>
                            onPatchLayer(
                              selectedLayer.id,
                              (l) => ({ ...l, text: { ...l.text!, align: id } }),
                              "discrete"
                            )
                          }
                          className={`p-1.5 rounded-lg transition ${
                            selectedLayer.text?.align === id
                              ? "bg-white text-black"
                              : "bg-white/5 text-[#A1A1AA] hover:text-white"
                          }`}
                        >
                          <Icon className="w-3.5 h-3.5" />
                        </button>
                      ))}
                    </div>
                  </div>

                  <InspectorSlider
                    label="Espaçamento"
                    value={selectedLayer.text.letterSpacing}
                    min={-0.08}
                    max={0.4}
                    step={0.01}
                    unit="em"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        text: { ...l.text!, letterSpacing: v },
                      }))
                    }
                  />

                  <InspectorSlider
                    label="Escala Vertical"
                    value={selectedLayer.text.stemStretch}
                    min={1}
                    max={2.4}
                    step={0.05}
                    unit="×"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        text: { ...l.text!, stemStretch: v },
                      }))
                    }
                  />

                  <InspectorSlider
                    label="Sombra do Texto"
                    value={selectedLayer.text.shadowBlur}
                    min={0}
                    max={48}
                    step={1}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        text: { ...l.text!, shadowBlur: v },
                      }))
                    }
                  />

                  <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                    <span className="text-xs text-[#A1A1AA]">Cor do Texto</span>
                    <div className="flex items-center gap-2">
                      <input
                        type="color"
                        value={toHexColorInput(selectedLayer.text.color)}
                        onChange={(e) =>
                          onPatchLayer(
                            selectedLayer.id,
                            (l) => ({ ...l, text: { ...l.text!, color: e.target.value } }),
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
                              onPatchLayer(
                                selectedLayer.id,
                                (l) => ({ ...l, fx: { ...l.fx, color: expr } }),
                                "discrete"
                              ),
                            onClear: () =>
                              onPatchLayer(
                                selectedLayer.id,
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
                    min={0}
                    max={100}
                    step={1}
                    unit="%"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        text: { ...l.text!, colorAlpha: Number((v / 100).toFixed(2)) },
                      }))
                    }
                  />
                </>
              )}

              {selectedLayer.kind === "VIDEO" && selectedLayer.video && (
                <>
                  <button
                    type="button"
                    onClick={() => onPickFile("video")}
                    className="w-full prism-pill-active rounded-xl py-2.5 text-xs flex items-center justify-center gap-2"
                  >
                    <Upload className="w-3.5 h-3.5" /> Escolher Vídeo (.mp4 / .webm)
                  </button>
                  <div className="grid grid-cols-2 gap-1.5">
                    {(["cover", "contain"] as const).map((fit) => (
                      <button
                        key={fit}
                        type="button"
                        onClick={() =>
                          onPatchLayer(
                            selectedLayer.id,
                            (l) => ({ ...l, video: { ...l.video!, objectFit: fit } }),
                            "discrete"
                          )
                        }
                        className={`py-1.5 rounded-xl text-[11px] ${
                          selectedLayer.video?.objectFit === fit
                            ? "prism-pill-active"
                            : "prism-pill-idle"
                        }`}
                      >
                        {fit === "cover" ? "Preencher (Cover)" : "Ajustar (Contain)"}
                      </button>
                    ))}
                  </div>
                  <InspectorSlider
                    label="Velocidade"
                    value={selectedLayer.video.speed}
                    min={0.25}
                    max={2.5}
                    step={0.05}
                    unit="×"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        video: { ...l.video!, speed: v },
                      }))
                    }
                  />
                  <InspectorSlider
                    label="Raio do Recorte"
                    value={selectedLayer.video.clipRadius}
                    min={0}
                    max={120}
                    step={2}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        video: { ...l.video!, clipRadius: v },
                      }))
                    }
                  />
                  <label className="flex items-center justify-between frosted-chrome-card px-3 py-2 text-xs">
                    <span>Sem áudio</span>
                    <input
                      type="checkbox"
                      checked={selectedLayer.video.muted}
                      onChange={(e) =>
                        onPatchLayer(
                          selectedLayer.id,
                          (l) => ({ ...l, video: { ...l.video!, muted: e.target.checked } }),
                          "discrete"
                        )
                      }
                    />
                  </label>
                  {!selectedLayer.video.muted && (
                    <InspectorSlider
                      label="Volume"
                      value={selectedLayer.video.volume}
                      min={0}
                      max={1}
                      step={0.05}
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          video: { ...l.video!, volume: v },
                        }))
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
                    ).map((srcMode) => (
                      <button
                        key={srcMode.id}
                        type="button"
                        onClick={() =>
                          onPatchLayer(
                            selectedLayer.id,
                            (l) => ({
                              ...l,
                              image: {
                                ...l.image!,
                                sourceMode: srcMode.id,
                                objectFit: srcMode.id === "MEDIA_COVER" ? "cover" : l.image!.objectFit,
                              },
                            }),
                            "discrete"
                          )
                        }
                        className={`py-1.5 rounded-xl text-[11px] ${
                          (selectedLayer.image?.sourceMode ?? "FILE") === srcMode.id
                            ? "prism-pill-active"
                            : "prism-pill-idle"
                        }`}
                      >
                        {srcMode.label}
                      </button>
                    ))}
                  </div>
                  {(selectedLayer.image.sourceMode ?? "FILE") === "FILE" && (
                    <button
                      type="button"
                      onClick={() => onPickFile("image")}
                      className="w-full prism-pill-active rounded-xl py-2.5 text-xs flex items-center justify-center gap-2"
                    >
                      <Upload className="w-3.5 h-3.5" /> Carregar PNG / WebP / JPG
                    </button>
                  )}
                  <div className="grid grid-cols-2 gap-1.5">
                    {(["contain", "cover"] as const).map((fit) => (
                      <button
                        key={fit}
                        type="button"
                        onClick={() =>
                          onPatchLayer(
                            selectedLayer.id,
                            (l) => ({ ...l, image: { ...l.image!, objectFit: fit } }),
                            "discrete"
                          )
                        }
                        className={`py-1.5 rounded-xl text-[11px] ${
                          selectedLayer.image?.objectFit === fit
                            ? "prism-pill-active"
                            : "prism-pill-idle"
                        }`}
                      >
                        {fit === "contain" ? "Recorte Alpha" : "Preencher (Cover)"}
                      </button>
                    ))}
                  </div>
                  <InspectorSlider
                    label="Raio da Borda"
                    value={selectedLayer.image.borderRadius}
                    min={0}
                    max={160}
                    step={2}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        image: { ...l.image!, borderRadius: v },
                      }))
                    }
                  />
                </>
              )}

              {selectedLayer.kind === "PROGRESS" && selectedLayer.progress && (
                <>
                  <div className="space-y-1.5">
                    <span className="block text-xs text-[#A1A1AA]">Fonte da Barra</span>
                    <div className="grid grid-cols-2 gap-1.5">
                      {(
                        [
                          { id: "MEDIA" as const, label: "Música (Timeline)" },
                          { id: "CPU" as const, label: "Uso de CPU" },
                          { id: "RAM" as const, label: "Uso de RAM" },
                          { id: "BATTERY" as const, label: "Bateria" },
                        ]
                      ).map((src) => (
                        <button
                          key={src.id}
                          type="button"
                          onClick={() =>
                            onPatchLayer(
                              selectedLayer.id,
                              (l) => ({ ...l, progress: { ...l.progress!, source: src.id } }),
                              "discrete"
                            )
                          }
                          className={`py-1.5 rounded-xl text-[11px] ${
                            selectedLayer.progress?.source === src.id
                              ? "prism-pill-active"
                              : "prism-pill-idle"
                          }`}
                        >
                          {src.label}
                        </button>
                      ))}
                    </div>
                  </div>
                  <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                    <span className="text-xs text-[#A1A1AA]">Cor do Preenchimento</span>
                    <input
                      type="color"
                      value={toHexColorInput(selectedLayer.progress.fillColor)}
                      onChange={(e) =>
                        onPatchLayer(
                          selectedLayer.id,
                          (l) => ({ ...l, progress: { ...l.progress!, fillColor: e.target.value } }),
                          "coalesced"
                        )
                      }
                    />
                  </div>
                  <InspectorSlider
                    label="Alpha do Preenchimento"
                    value={Math.round(selectedLayer.progress.fillAlpha * 100)}
                    min={0}
                    max={100}
                    step={2}
                    unit="%"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        progress: { ...l.progress!, fillAlpha: Number((v / 100).toFixed(2)) },
                      }))
                    }
                  />
                  <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                    <span className="text-xs text-[#A1A1AA]">Cor da Trilha</span>
                    <input
                      type="color"
                      value={toHexColorInput(selectedLayer.progress.trackColor)}
                      onChange={(e) =>
                        onPatchLayer(
                          selectedLayer.id,
                          (l) => ({ ...l, progress: { ...l.progress!, trackColor: e.target.value } }),
                          "coalesced"
                        )
                      }
                    />
                  </div>
                  <InspectorSlider
                    label="Alpha da Trilha"
                    value={Math.round(selectedLayer.progress.trackAlpha * 100)}
                    min={0}
                    max={100}
                    step={2}
                    unit="%"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        progress: { ...l.progress!, trackAlpha: Number((v / 100).toFixed(2)) },
                      }))
                    }
                  />
                  <InspectorSlider
                    label="Arredondamento"
                    value={selectedLayer.progress.borderRadius}
                    min={0}
                    max={999}
                    step={1}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        progress: { ...l.progress!, borderRadius: v },
                      }))
                    }
                  />
                </>
              )}

              {selectedLayer.kind === "SVG" && selectedLayer.svg && (
                <>
                  <button
                    type="button"
                    onClick={() => onPickFile("svg")}
                    className="w-full prism-pill-active rounded-xl py-2 text-xs flex items-center justify-center gap-2"
                  >
                    <Upload className="w-3.5 h-3.5" /> Importar Arquivo .svg
                  </button>
                  <div className="grid grid-cols-4 gap-1.5">
                    {Object.keys(BUILTIN_SVG_ICONS).map((ic) => (
                      <button
                        key={ic}
                        type="button"
                        onClick={() =>
                          onPatchLayer(
                            selectedLayer.id,
                            (l) => ({ ...l, svg: { ...l.svg!, iconName: ic, customSvg: "" } }),
                            "discrete"
                          )
                        }
                        className={`py-1.5 rounded-xl text-[10px] uppercase ${
                          selectedLayer.svg?.iconName === ic && !selectedLayer.svg?.customSvg
                            ? "prism-pill-active"
                            : "prism-pill-idle"
                        }`}
                      >
                        {ic}
                      </button>
                    ))}
                  </div>
                  <InspectorSlider
                    label="Espessura do Traço"
                    value={selectedLayer.svg.strokeWidth}
                    min={0.6}
                    max={4}
                    step={0.1}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        svg: { ...l.svg!, strokeWidth: v },
                      }))
                    }
                  />
                  <div className="flex items-center justify-between frosted-chrome-card px-3 py-2">
                    <span className="text-xs text-[#A1A1AA]">Cor Vetorial</span>
                    <div className="flex items-center gap-2">
                      <input
                        type="color"
                        value={toHexColorInput(selectedLayer.svg.color)}
                        onChange={(e) =>
                          onPatchLayer(
                            selectedLayer.id,
                            (l) => ({ ...l, svg: { ...l.svg!, color: e.target.value } }),
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
                              onPatchLayer(
                                selectedLayer.id,
                                (l) => ({ ...l, fx: { ...l.fx, color: expr } }),
                                "discrete"
                              ),
                            onClear: () =>
                              onPatchLayer(
                                selectedLayer.id,
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
                    min={0}
                    max={100}
                    step={2}
                    unit="%"
                    {...sliderHandlers}
                    onChange={(v) =>
                      onPatchLayer(selectedLayer.id, (l) => ({
                        ...l,
                        svg: { ...l.svg!, colorAlpha: Number((v / 100).toFixed(2)) },
                      }))
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
                    return {
                      ...c,
                      modelSrc: nextObjs[0]?.modelSrc ?? "",
                      objects: nextObjs,
                    };
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
                  scene3dInfo?.byObject?.[activeObj.id] ?? scene3dInfo;
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
                const activeUniformScale = Number((activeObj.scale?.[0] ?? 1).toFixed(2));

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
                                autoRotateSpeed: 0.22,
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
                                <button
                                  type="button"
                                  title="Duplicar objeto 3D"
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    const cloneId = `obj3d-${Math.random().toString(36).slice(2, 9)}`;
                                    const clone: Scene3DObject = {
                                      ...JSON.parse(JSON.stringify(obj)),
                                      id: cloneId,
                                      name: `${obj.name} (Cópia)`,
                                      position: [
                                        Number(((obj.position?.[0] ?? 0) + 0.8).toFixed(2)),
                                        obj.position?.[1] ?? 0,
                                        obj.position?.[2] ?? 0,
                                      ],
                                    };
                                    patchObjects((list) => [...list, clone], "discrete");
                                    onSelect3dObjectId?.(cloneId);
                                  }}
                                  className="p-1 rounded hover:bg-white/10"
                                >
                                  <Copy className="w-3 h-3" />
                                </button>
                                {objs.length > 1 && (
                                  <button
                                    type="button"
                                    title="Remover objeto 3D"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      const remaining = objs.filter((item) => item.id !== obj.id);
                                      patch3d((c) => {
                                        const nextObjs = normalizeScene3DObjects(c).filter(
                                          (item) => item.id !== obj.id
                                        );
                                        return {
                                          ...c,
                                          modelSrc: nextObjs[0]?.modelSrc ?? "",
                                          objects: nextObjs,
                                          bindings: (c.bindings ?? []).filter(
                                            (b) => b.objectId !== obj.id
                                          ),
                                        };
                                      }, "discrete");
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
                      </div>
                    </div>

                    {/* Per-Object 3D Transform (Position / Rotation / Scale / Auto-Spin) */}
                    <div className="space-y-2 pt-2 border-t border-white/10">
                      <span className="block text-[11px] font-medium text-[#A1A1AA]">
                        Posição & Giro ({activeObj.name})
                      </span>
                      <InspectorSlider
                        label="Posição X"
                        value={activeObj.position?.[0] ?? 0}
                        min={-8}
                        max={8}
                        step={0.05}
                        {...sliderHandlers}
                        onChange={(v) =>
                          patchActiveObj((o) => ({
                            ...o,
                            position: [v, o.position?.[1] ?? 0, o.position?.[2] ?? 0],
                          }))
                        }
                      />
                      <InspectorSlider
                        label="Posição Y"
                        value={activeObj.position?.[1] ?? 0}
                        min={-4}
                        max={4}
                        step={0.05}
                        {...sliderHandlers}
                        onChange={(v) =>
                          patchActiveObj((o) => ({
                            ...o,
                            position: [o.position?.[0] ?? 0, v, o.position?.[2] ?? 0],
                          }))
                        }
                      />
                      <InspectorSlider
                        label="Posição Z"
                        value={activeObj.position?.[2] ?? 0}
                        min={-8}
                        max={8}
                        step={0.05}
                        {...sliderHandlers}
                        onChange={(v) =>
                          patchActiveObj((o) => ({
                            ...o,
                            position: [o.position?.[0] ?? 0, o.position?.[1] ?? 0, v],
                          }))
                        }
                      />
                      <InspectorSlider
                        label="Rotação X"
                        value={activeObj.rotation?.[0] ?? 0}
                        min={-180}
                        max={180}
                        step={2}
                        unit="°"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patchActiveObj((o) => ({
                            ...o,
                            rotation: [v, o.rotation?.[1] ?? 0, o.rotation?.[2] ?? 0],
                          }))
                        }
                      />
                      <InspectorSlider
                        label="Rotação Y"
                        value={activeObj.rotation?.[1] ?? 0}
                        min={-180}
                        max={180}
                        step={2}
                        unit="°"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patchActiveObj((o) => ({
                            ...o,
                            rotation: [o.rotation?.[0] ?? 0, v, o.rotation?.[2] ?? 0],
                          }))
                        }
                      />
                      <InspectorSlider
                        label="Rotação Z"
                        value={activeObj.rotation?.[2] ?? 0}
                        min={-180}
                        max={180}
                        step={2}
                        unit="°"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patchActiveObj((o) => ({
                            ...o,
                            rotation: [o.rotation?.[0] ?? 0, o.rotation?.[1] ?? 0, v],
                          }))
                        }
                      />
                      <InspectorSlider
                        label="Escala"
                        value={activeUniformScale}
                        min={0.1}
                        max={4}
                        step={0.05}
                        unit="×"
                        {...sliderHandlers}
                        onChange={(v) =>
                          patchActiveObj((o) => ({ ...o, scale: [v, v, v] }))
                        }
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
                          <select
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
                          </select>
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
                      <div className="grid grid-cols-2 gap-1.5">
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
                      </div>
                      {bgMode === "SOLID" && (
                        <div className="flex items-center justify-between frosted-chrome-card px-3 py-1.5">
                          <span className="text-[11px] text-[#A1A1AA]">Cor do Fundo 3D</span>
                          <input
                            type="color"
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
                    <div className="space-y-2.5 pt-2 border-t border-white/10">
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

                      <div className="grid grid-cols-2 gap-1">
                        <button
                          type="button"
                          onClick={() => {
                            const b = createBinding3D("LOOK_AT", activeObjInfo, activeObj.id);
                            patch3d((c) => ({ ...c, bindings: [...(c.bindings ?? []), b] }), "discrete");
                          }}
                          className="prism-pill-idle py-1.5 rounded-xl text-[10px]"
                        >
                          + Olhar Mouse
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            const b = createBinding3D("AUDIO_BASS", activeObjInfo, activeObj.id);
                            patch3d((c) => ({ ...c, bindings: [...(c.bindings ?? []), b] }), "discrete");
                          }}
                          className="prism-pill-idle py-1.5 rounded-xl text-[10px]"
                        >
                          + Graves (Áudio)
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            const b = createBinding3D("KEY_PULSE", activeObjInfo, activeObj.id);
                            patch3d((c) => ({ ...c, bindings: [...(c.bindings ?? []), b] }), "discrete");
                          }}
                          className="prism-pill-idle py-1.5 rounded-xl text-[10px]"
                        >
                          + Pulso Teclado
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            const b = createBinding3D("CLICK_CLIP", activeObjInfo, activeObj.id);
                            patch3d((c) => ({ ...c, bindings: [...(c.bindings ?? []), b] }), "discrete");
                          }}
                          className="prism-pill-idle py-1.5 rounded-xl text-[10px]"
                        >
                          + Clique / Hover 3D
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            const b = createBinding3D("AUDIO_SWING", activeObjInfo, activeObj.id);
                            patch3d((c) => ({ ...c, bindings: [...(c.bindings ?? []), b] }), "discrete");
                          }}
                          title="Balança uma peça (asa, mandíbula, braço, agulha) no ritmo do grave"
                          className="prism-pill-idle py-1.5 rounded-xl text-[10px]"
                        >
                          + Balanço no Grave
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            const b = createBinding3D("AUDIO_PISTON", activeObjInfo, activeObj.id);
                            patch3d((c) => ({ ...c, bindings: [...(c.bindings ?? []), b] }), "discrete");
                          }}
                          title="Empurra uma peça (cone de caixa de som, tecla) no ritmo do grave"
                          className="prism-pill-idle py-1.5 rounded-xl text-[10px]"
                        >
                          + Pistão no Grave
                        </button>
                      </div>

                      {(s3d.bindings ?? []).length === 0 ? (
                        <p className="text-[11px] text-[#71717A]">
                          Adicione regras acima para reagir ao mouse, batidas de áudio (WASAPI), ritmo do teclado (anônimo), música ou CPU.
                        </p>
                      ) : (
                        <div className="space-y-2">
                          {(s3d.bindings ?? []).map((b) => {
                            const bObjId = b.objectId || objs[0]?.id || "";
                            const bObjInfo: Scene3DObjectInspectInfo | Scene3DInspectInfo | null =
                              (bObjId ? scene3dInfo?.byObject?.[bObjId] : undefined) ??
                              scene3dInfo;

                            const updateBinding = (
                              patch: Partial<ReactiveBinding3D>,
                              commit: "raw" | "discrete" | "coalesced" = "discrete"
                            ) =>
                              patch3d(
                                (c) => ({
                                  ...c,
                                  bindings: (c.bindings ?? []).map((item) =>
                                    item.id === b.id ? { ...item, ...patch } : item
                                  ),
                                }),
                                commit
                              );

                            const removeBinding = () =>
                              patch3d(
                                (c) => ({
                                  ...c,
                                  bindings: (c.bindings ?? []).filter((item) => item.id !== b.id),
                                }),
                                "discrete"
                              );

                            const nodeOptions = [
                              { value: "__ROOT__", label: "Modelo Inteiro" },
                              ...(bObjInfo?.boneNames ?? []).map((n) => ({
                                value: n,
                                label: `Osso · ${n}`,
                              })),
                              ...(bObjInfo?.meshNames ?? []).map((n) => ({
                                value: n,
                                label: `Malha · ${n}`,
                              })),
                            ];

                            const strengthMeta =
                              b.targetType === "NODE_LOOK_AT"
                                ? { label: "Ângulo Máx.", min: 2, max: 65, step: 1, unit: "°" }
                                : b.targetType === "EMISSIVE_GLOW"
                                ? { label: "Intensidade Brilho", min: 0.2, max: 4, step: 0.1, unit: "" }
                                : b.targetType === "NODE_SCALE_PULSE"
                                ? { label: "Pulso de Escala", min: 0.02, max: 0.45, step: 0.01, unit: "×" }
                                : b.targetType === "NODE_ROTATE_SPIN"
                                ? { label: "Velocidade Giro", min: 0.2, max: 8, step: 0.2, unit: "" }
                                : b.targetType === "NODE_ROTATE_SWING"
                                ? { label: "Ângulo do Balanço", min: 2, max: 120, step: 1, unit: "°" }
                                : b.targetType === "NODE_MOVE_OFFSET"
                                ? { label: "Deslocamento", min: 0.01, max: 1.5, step: 0.01, unit: "" }
                                : { label: "Peso Morph", min: 0.1, max: 1, step: 0.05, unit: "" };
                            const usesAxis =
                              b.targetType === "NODE_ROTATE_SWING" ||
                              b.targetType === "NODE_MOVE_OFFSET" ||
                              b.targetType === "NODE_ROTATE_SPIN";

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

                            return (
                              <div key={b.id} className="frosted-chrome-card p-2.5 space-y-2">
                                <div className="flex items-center justify-between gap-2">
                                  <label className="flex items-center gap-1.5 text-[11px] text-white font-medium">
                                    <input
                                      type="checkbox"
                                      checked={b.enabled}
                                      onChange={(e) => updateBinding({ enabled: e.target.checked })}
                                    />
                                    <span>{sourceTitleMap[b.source] ?? b.source}</span>
                                  </label>
                                  <button
                                    type="button"
                                    onClick={removeBinding}
                                    className="p-1 text-[#A1A1AA] hover:text-white"
                                    title="Remover regra"
                                  >
                                    <Trash2 className="w-3 h-3" />
                                  </button>
                                </div>

                                {objs.length > 1 && (
                                  <label className="block text-[10px] text-[#A1A1AA]">
                                    Objeto Alvo na Sala
                                    <select
                                      value={bObjId}
                                      onChange={(e) => updateBinding({ objectId: e.target.value })}
                                      className="mt-1 w-full bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                    >
                                      {objs.map((o) => (
                                        <option key={o.id} value={o.id}>
                                          {o.name}
                                        </option>
                                      ))}
                                    </select>
                                  </label>
                                )}

                                <div className="grid grid-cols-2 gap-1.5">
                                  <select
                                    value={b.source}
                                    onChange={(e) => {
                                      const nextSrc = e.target.value as Scene3DBindingSource;
                                      const nextTarget: Scene3DBindingTarget =
                                        nextSrc === "CURSOR_LOOK"
                                          ? "NODE_LOOK_AT"
                                          : b.targetType === "NODE_LOOK_AT"
                                          ? "NODE_SCALE_PULSE"
                                          : b.targetType;
                                      updateBinding({ source: nextSrc, targetType: nextTarget });
                                    }}
                                    className="bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
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
                                  </select>

                                  <select
                                    value={b.targetType}
                                    onChange={(e) => {
                                      const nextType = e.target.value as Scene3DBindingTarget;
                                      let nextTargetName = b.targetName;
                                      let nextStrength = b.strength;
                                      if (nextType === "NODE_LOOK_AT") {
                                        nextTargetName = nodeOptions[0]?.value ?? "__ROOT__";
                                        nextStrength = 22;
                                      } else if (nextType === "PLAY_CLIP") {
                                        nextTargetName = bObjInfo?.clipNames[0] ?? "";
                                        nextStrength = 1;
                                      } else if (nextType === "EMISSIVE_GLOW") {
                                        nextTargetName = bObjInfo?.materialNames[0] ?? "*";
                                        nextStrength = 1.4;
                                      } else if (nextType === "NODE_SCALE_PULSE") {
                                        nextTargetName = "__ROOT__";
                                        nextStrength = 0.14;
                                      } else if (nextType === "NODE_ROTATE_SPIN") {
                                        nextTargetName = "__ROOT__";
                                        nextStrength = 2.4;
                                      } else if (nextType === "NODE_ROTATE_SWING") {
                                        nextTargetName =
                                          b.targetType === "NODE_LOOK_AT" ||
                                          b.targetType === "NODE_SCALE_PULSE" ||
                                          b.targetType === "NODE_ROTATE_SPIN" ||
                                          b.targetType === "NODE_MOVE_OFFSET"
                                            ? b.targetName || "__ROOT__"
                                            : "__ROOT__";
                                        nextStrength = 25;
                                      } else if (nextType === "NODE_MOVE_OFFSET") {
                                        nextTargetName =
                                          b.targetType === "NODE_LOOK_AT" ||
                                          b.targetType === "NODE_SCALE_PULSE" ||
                                          b.targetType === "NODE_ROTATE_SPIN" ||
                                          b.targetType === "NODE_ROTATE_SWING"
                                            ? b.targetName || "__ROOT__"
                                            : "__ROOT__";
                                        nextStrength = 0.15;
                                      } else if (nextType === "MORPH_WEIGHT") {
                                        nextTargetName = bObjInfo?.morphNames[0] ?? "";
                                        nextStrength = 1;
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
                                        strength: nextStrength,
                                      });
                                    }}
                                    className="bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                  >
                                    <option value="NODE_LOOK_AT">Olhar p/ Cursor</option>
                                    <option value="NODE_SCALE_PULSE">Pulso de Escala</option>
                                    <option value="EMISSIVE_GLOW">Brilho Emissivo</option>
                                    <option value="NODE_ROTATE_SPIN">Girar / Impulso</option>
                                    <option value="NODE_ROTATE_SWING">Balançar (Dobradiça)</option>
                                    <option value="NODE_MOVE_OFFSET">Deslocar (Pistão)</option>
                                    <option value="PLAY_CLIP">Tocar Animação</option>
                                    <option value="MORPH_WEIGHT">Morph / Shape Key</option>
                                  </select>
                                </div>

                                {(b.source === "HOVER_MESH" || b.source === "CLICK_MESH") && (
                                  <label className="block text-[10px] text-[#A1A1AA]">
                                    Malha Gatilho (Raycast)
                                    <select
                                      value={b.sourceNode}
                                      onChange={(e) => updateBinding({ sourceNode: e.target.value })}
                                      className="mt-1 w-full bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                    >
                                      <option value="*">Qualquer malha do modelo (*)</option>
                                      {(bObjInfo?.meshNames ?? []).map((m) => (
                                        <option key={m} value={m}>
                                          {m}
                                        </option>
                                      ))}
                                    </select>
                                  </label>
                                )}

                                <label className="block text-[10px] text-[#A1A1AA]">
                                  Alvo da Ação
                                  {b.targetType === "NODE_LOOK_AT" ||
                                  b.targetType === "NODE_SCALE_PULSE" ||
                                  b.targetType === "NODE_ROTATE_SPIN" ||
                                  b.targetType === "NODE_ROTATE_SWING" ||
                                  b.targetType === "NODE_MOVE_OFFSET" ? (
                                    <select
                                      value={b.targetName || "__ROOT__"}
                                      onChange={(e) => updateBinding({ targetName: e.target.value })}
                                      className="mt-1 w-full bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                    >
                                      {nodeOptions.map((opt) => (
                                        <option key={opt.value} value={opt.value}>
                                          {opt.label}
                                        </option>
                                      ))}
                                    </select>
                                  ) : b.targetType === "PLAY_CLIP" ? (
                                    <select
                                      value={b.targetName}
                                      onChange={(e) => updateBinding({ targetName: e.target.value })}
                                      className="mt-1 w-full bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                    >
                                      {(bObjInfo?.clipNames ?? []).length === 0 ? (
                                        <option value="">(Modelo sem clips de animação)</option>
                                      ) : (
                                        (bObjInfo?.clipNames ?? []).map((clip) => (
                                          <option key={clip} value={clip}>
                                            {clip}
                                          </option>
                                        ))
                                      )}
                                    </select>
                                  ) : b.targetType === "EMISSIVE_GLOW" ? (
                                    <select
                                      value={b.targetName || "*"}
                                      onChange={(e) => updateBinding({ targetName: e.target.value })}
                                      className="mt-1 w-full bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                    >
                                      <option value="*">Todos os materiais (*)</option>
                                      {(bObjInfo?.materialNames ?? []).map((mat) => (
                                        <option key={mat} value={mat}>
                                          {mat}
                                        </option>
                                      ))}
                                    </select>
                                  ) : (
                                    <select
                                      value={b.targetName}
                                      onChange={(e) => updateBinding({ targetName: e.target.value })}
                                      className="mt-1 w-full bg-black border border-white/15 rounded-lg px-2 py-1 text-[11px] text-white outline-none"
                                    >
                                      {(bObjInfo?.morphNames ?? []).length === 0 ? (
                                        <option value="">(Modelo sem Shape Keys / Morphs)</option>
                                      ) : (
                                        (bObjInfo?.morphNames ?? []).map((mp) => (
                                          <option key={mp} value={mp}>
                                            {mp}
                                          </option>
                                        ))
                                      )}
                                    </select>
                                  )}
                                </label>

                                {b.targetType !== "PLAY_CLIP" && (
                                  <InspectorSlider
                                    label={strengthMeta.label}
                                    value={b.strength}
                                    min={strengthMeta.min}
                                    max={strengthMeta.max}
                                    step={strengthMeta.step}
                                    unit={strengthMeta.unit}
                                    {...sliderHandlers}
                                    onChange={(v) => updateBinding({ strength: v }, "raw")}
                                  />
                                )}

                                {b.targetType !== "PLAY_CLIP" && (
                                  <InspectorSlider
                                    label="Suavidade Spring"
                                    value={b.smoothing}
                                    min={0.03}
                                    max={0.3}
                                    step={0.01}
                                    {...sliderHandlers}
                                    onChange={(v) => updateBinding({ smoothing: v }, "raw")}
                                  />
                                )}

                                {usesAxis && (
                                  <div className="flex items-center justify-between gap-2 text-[11px] text-[#A1A1AA]">
                                    <span>Eixo</span>
                                    <div className="flex items-center gap-1">
                                      {(["X", "Y", "Z"] as Scene3DAxis[]).map((ax) => (
                                        <button
                                          key={ax}
                                          type="button"
                                          onClick={() => updateBinding({ axis: ax })}
                                          className={`w-7 py-0.5 rounded-lg text-[10px] font-mono-fx ${
                                            (b.axis ?? "Y") === ax ? "prism-pill-active" : "prism-pill-idle"
                                          }`}
                                        >
                                          {ax}
                                        </button>
                                      ))}
                                      <label className="flex items-center gap-1 ml-1.5">
                                        <input
                                          type="checkbox"
                                          checked={Boolean(b.invertAxis)}
                                          onChange={(e) => updateBinding({ invertAxis: e.target.checked })}
                                        />
                                        Inverter
                                      </label>
                                    </div>
                                  </div>
                                )}

                                {b.targetType === "NODE_LOOK_AT" && (
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
                    <div className="space-y-2 pt-2 border-t border-white/10">
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
                            const removeDt = () =>
                              patchActiveObj(
                                (o) => ({
                                  ...o,
                                  dynamicTextures: (o.dynamicTextures ?? []).filter(
                                    (item) => item.id !== dt.id
                                  ),
                                }),
                                "discrete"
                              );
                            return (
                              <div key={dt.id} className="frosted-chrome-card p-2.5 space-y-2">
                                <div className="flex items-center justify-between gap-2">
                                  <span className="text-[11px] text-white font-medium truncate">
                                    Material: {dt.materialName === "*" ? "Todos (*)" : dt.materialName}
                                  </span>
                                  <button
                                    type="button"
                                    onClick={removeDt}
                                    className="p-1 text-[#A1A1AA] hover:text-white"
                                    title="Remover textura dinâmica"
                                  >
                                    <Trash2 className="w-3 h-3" />
                                  </button>
                                </div>
                                <div className="grid grid-cols-2 gap-1.5">
                                  <select
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
                                  </select>
                                  <select
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
                                  </select>
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
            return (
            <>
              {supportsFullGlass ? (
                <label className="flex items-center justify-between frosted-chrome-card px-3 py-2 text-xs font-medium text-white">
                  <span>Vidro Óptico & Desfoque</span>
                  <input
                    type="checkbox"
                    checked={selectedLayer.glass.enabled}
                    onChange={(e) => {
                      const checked = e.target.checked;
                      onPatchLayer(
                        selectedLayer.id,
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
                    <span>Dispersão Prismática no Ícone</span>
                    <input
                      type="checkbox"
                      checked={selectedLayer.glass.enabled}
                      onChange={(e) =>
                        onPatchLayer(
                          selectedLayer.id,
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
                  {selectedLayer.glass.enabled && (
                    <InspectorSlider
                      label="Dispersão RGB"
                      value={selectedLayer.glass.chromaticAberration}
                      min={0}
                      max={6}
                      step={0.2}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
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

              {supportsFullGlass && selectedLayer.glass.enabled && (
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
                            onPatchLayer(
                              selectedLayer.id,
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
                      <span className="block text-[11px] text-[#A1A1AA]">Modo do Vidro no Texto</span>
                      <div className="grid grid-cols-2 gap-1.5">
                        {(
                          [
                            { id: "GLYPH" as const, label: "Nas Letras (Vidro)" },
                            { id: "PILL" as const, label: "Pílula Atrás" },
                          ]
                        ).map((tm) => (
                          <button
                            key={tm.id}
                            type="button"
                            onClick={() =>
                              onPatchLayer(
                                selectedLayer.id,
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
                              (selectedLayer.glass.textMode ?? "GLYPH") === tm.id
                                ? "prism-pill-active"
                                : "prism-pill-idle"
                            }`}
                          >
                            {tm.label}
                          </button>
                        ))}
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
                                onPatchLayer(
                                  selectedLayer.id,
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
                      min={0}
                      max={80}
                      step={1}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          glass: { ...l.glass, blurPx: v },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Refração Óptica (Lente)"
                      value={selectedLayer.glass.refractionScale}
                      min={0}
                      max={60}
                      step={1}
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          glass: { ...l.glass, refractionScale: v },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Chanfro da Lente (Bevel)"
                      value={selectedLayer.glass.bevelWidth ?? 22}
                      min={4}
                      max={64}
                      step={2}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          glass: { ...l.glass, bevelWidth: v },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Brilho Especular"
                      value={Math.round((selectedLayer.glass.specularIntensity ?? 0.45) * 100)}
                      min={0}
                      max={100}
                      step={2}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
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
                      <input
                        type="color"
                        value={toHexColorInput(selectedLayer.glass.tintColor)}
                        onChange={(e) =>
                          onPatchLayer(
                            selectedLayer.id,
                            (l) => ({ ...l, glass: { ...l.glass, tintColor: e.target.value } }),
                            "coalesced"
                          )
                        }
                      />
                    </div>
                    <InspectorSlider
                      label="Opacidade do Véu"
                      value={Math.round(selectedLayer.glass.frostOpacity * 100)}
                      min={0}
                      max={80}
                      step={1}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          glass: { ...l.glass, frostOpacity: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Granulação (Areia Fina)"
                      value={Math.round((selectedLayer.glass.noiseOpacity ?? 0.04) * 100)}
                      min={0}
                      max={35}
                      step={1}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          glass: { ...l.glass, noiseOpacity: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Luminosidade"
                      value={Math.round((selectedLayer.glass.brightness ?? 1.06) * 100)}
                      min={60}
                      max={160}
                      step={2}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          glass: { ...l.glass, brightness: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Contraste"
                      value={Math.round((selectedLayer.glass.contrast ?? 1.04) * 100)}
                      min={80}
                      max={160}
                      step={2}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          glass: { ...l.glass, contrast: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Saturação"
                      value={Math.round(selectedLayer.glass.saturation * 100)}
                      min={0}
                      max={240}
                      step={5}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
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
                      min={0}
                      max={70}
                      step={2}
                      unit="%"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          glass: { ...l.glass, borderOpacity: v / 100 },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Raio da Borda"
                      value={selectedLayer.glass.borderRadius}
                      min={0}
                      max={160}
                      step={2}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
                          ...l,
                          glass: { ...l.glass, borderRadius: v },
                        }))
                      }
                    />
                    <InspectorSlider
                      label="Sombra"
                      value={selectedLayer.glass.shadowBlur}
                      min={0}
                      max={64}
                      step={2}
                      unit="px"
                      {...sliderHandlers}
                      onChange={(v) =>
                        onPatchLayer(selectedLayer.id, (l) => ({
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
                  {showOptics && (
                    <div className="space-y-2.5 pl-2 border-l border-white/10">
                      <InspectorSlider
                        label="Dispersão RGB"
                        value={selectedLayer.glass.chromaticAberration}
                        min={0}
                        max={6}
                        step={0.2}
                        unit="px"
                        {...sliderHandlers}
                        onChange={(v) =>
                          onPatchLayer(selectedLayer.id, (l) => ({
                            ...l,
                            glass: { ...l.glass, chromaticAberration: v },
                          }))
                        }
                      />
                    </div>
                  )}
                </div>
              )}

              <div className="space-y-2.5 pt-2.5 border-t border-white/10">
                <InspectorSlider
                  label="Opacidade"
                  value={selectedLayer.opacity}
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
                        onPatchLayer(
                          selectedLayer.id,
                          (l) => ({ ...l, fx: { ...l.fx, opacity: expr } }),
                          "discrete"
                        ),
                      onClear: () =>
                        onPatchLayer(
                          selectedLayer.id,
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
                  onChange={(v) => onPatchLayer(selectedLayer.id, (l) => ({ ...l, opacity: v }))}
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
                          onPatchLayer(
                            selectedLayer.id,
                            (l) => ({ ...l, fx: { ...l.fx, visible: expr } }),
                            "discrete"
                          ),
                        onClear: () =>
                          onPatchLayer(
                            selectedLayer.id,
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
                  <select
                    value={selectedLayer.blendMode}
                    onChange={(e) =>
                      onPatchLayer(
                        selectedLayer.id,
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
                  </select>
                </label>
              </div>
            </>
            );
          })()}

          {layerTab === "TRANSFORM" && (
            <>
              <div className="space-y-1.5">
                <span className="block text-[11px] text-[#A1A1AA]">Alinhamento Rápido</span>
                <div className="grid grid-cols-3 gap-1.5">
                  <button
                    type="button"
                    onClick={() =>
                      onPatchLayer(selectedLayer.id, (l) => ({ ...l, x: 0.12 }), "discrete")
                    }
                    className="prism-pill-idle py-1.5 rounded-lg text-[10px]"
                  >
                    Esquerda
                  </button>
                  <button
                    type="button"
                    onClick={() =>
                      onPatchLayer(selectedLayer.id, (l) => ({ ...l, x: 0.5 }), "discrete")
                    }
                    className="prism-pill-idle py-1.5 rounded-lg text-[10px]"
                  >
                    Centro H
                  </button>
                  <button
                    type="button"
                    onClick={() =>
                      onPatchLayer(selectedLayer.id, (l) => ({ ...l, x: 0.88 }), "discrete")
                    }
                    className="prism-pill-idle py-1.5 rounded-lg text-[10px]"
                  >
                    Direita
                  </button>
                  <button
                    type="button"
                    onClick={() =>
                      onPatchLayer(selectedLayer.id, (l) => ({ ...l, y: 0.14 }), "discrete")
                    }
                    className="prism-pill-idle py-1.5 rounded-lg text-[10px]"
                  >
                    Topo
                  </button>
                  <button
                    type="button"
                    onClick={() =>
                      onPatchLayer(selectedLayer.id, (l) => ({ ...l, y: 0.5 }), "discrete")
                    }
                    className="prism-pill-idle py-1.5 rounded-lg text-[10px]"
                  >
                    Centro V
                  </button>
                  <button
                    type="button"
                    onClick={() =>
                      onPatchLayer(selectedLayer.id, (l) => ({ ...l, y: 0.86 }), "discrete")
                    }
                    className="prism-pill-idle py-1.5 rounded-lg text-[10px]"
                  >
                    Base
                  </button>
                </div>
              </div>

              <InspectorSlider
                label="Posição X"
                value={Number((selectedLayer.x * 100).toFixed(1))}
                min={0}
                max={100}
                step={0.5}
                unit="%"
                {...sliderHandlers}
                onChange={(v) =>
                  onPatchLayer(selectedLayer.id, (l) => ({ ...l, x: Number((v / 100).toFixed(4)) }))
                }
              />
              <InspectorSlider
                label="Posição Y"
                value={Number((selectedLayer.y * 100).toFixed(1))}
                min={0}
                max={100}
                step={0.5}
                unit="%"
                {...sliderHandlers}
                onChange={(v) =>
                  onPatchLayer(selectedLayer.id, (l) => ({ ...l, y: Number((v / 100).toFixed(4)) }))
                }
              />

              {selectedLayer.kind !== "TEXT" && (
                <>
                  <InspectorSlider
                    label="Largura"
                    value={selectedLayer.width}
                    min={selectedLayer.kind === "PROGRESS" ? 16 : 8}
                    max={2200}
                    step={selectedLayer.kind === "SVG" ? 2 : 4}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) => onPatchLayer(selectedLayer.id, (l) => ({ ...l, width: v }))}
                  />
                  <InspectorSlider
                    label="Altura"
                    value={selectedLayer.height}
                    min={selectedLayer.kind === "PROGRESS" ? 2 : 8}
                    max={1400}
                    step={selectedLayer.kind === "PROGRESS" ? 1 : selectedLayer.kind === "SVG" ? 2 : 4}
                    unit="px"
                    {...sliderHandlers}
                    onChange={(v) => onPatchLayer(selectedLayer.id, (l) => ({ ...l, height: v }))}
                  />
                </>
              )}

              <InspectorSlider
                label="Rotação"
                value={selectedLayer.rotation}
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
                      onPatchLayer(
                        selectedLayer.id,
                        (l) => ({ ...l, fx: { ...l.fx, rotation: expr } }),
                        "discrete"
                      ),
                    onClear: () =>
                      onPatchLayer(
                        selectedLayer.id,
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
                onChange={(v) => onPatchLayer(selectedLayer.id, (l) => ({ ...l, rotation: v }))}
              />

              <div className="space-y-2 pt-2.5 border-t border-white/10">
                <label className="block text-xs text-[#A1A1AA]">
                  Ação ao Clicar (Widgets / Player)
                  <select
                    value={selectedLayer.clickAction ?? "NONE"}
                    onChange={(e) =>
                      onPatchLayer(
                        selectedLayer.id,
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
                  </select>
                </label>
              </div>

              <div className="space-y-2.5 pt-2.5 border-t border-white/10">
                <div className="flex items-center justify-between">
                  <span className="text-[11px] font-medium text-[#A1A1AA]">Parallax 3D (Mouse)</span>
                  <span className="text-[10px] text-[#71717A]">Ativo no desktop</span>
                </div>
                <InspectorSlider
                  label="Eixo X"
                  value={selectedLayer.parallaxX}
                  min={-1.2}
                  max={1.2}
                  step={0.05}
                  {...sliderHandlers}
                  onChange={(v) => onPatchLayer(selectedLayer.id, (l) => ({ ...l, parallaxX: v }))}
                />
                <InspectorSlider
                  label="Eixo Y"
                  value={selectedLayer.parallaxY}
                  min={-1.2}
                  max={1.2}
                  step={0.05}
                  {...sliderHandlers}
                  onChange={(v) => onPatchLayer(selectedLayer.id, (l) => ({ ...l, parallaxY: v }))}
                />
              </div>
            </>
          )}
        </div>
      </div>
    );
  };

  return (
    <aside className="fixed top-20 right-4 bottom-4 z-30 w-[352px] frosted-chrome frosted-chrome-panel flex flex-col overflow-hidden">
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

      {mode === "SCENE"
        ? renderSceneTab()
        : mode === "GLOBALS"
        ? renderGlobalsTab()
        : renderLayerTab()}
    </aside>
  );
};

interface InspectorSliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  unit?: string;
  fxActive?: boolean;
  onFx?: () => void;
  onChange: (v: number) => void;
  onBeginTx: () => void;
  onCommitTx: () => void;
  onRecordStep: () => void;
  onFlushSave?: () => void;
}

const InspectorSlider: React.FC<InspectorSliderProps> = ({
  label,
  value,
  min,
  max,
  step,
  unit = "",
  fxActive = false,
  onFx,
  onChange,
  onBeginTx,
  onCommitTx,
  onRecordStep,
  onFlushSave,
}) => {
  const clamp = (n: number) => Math.max(min, Math.min(max, Number(n.toFixed(3))));
  const adjustKeys = new Set([
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    "Home",
    "End",
    "PageUp",
    "PageDown",
  ]);
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs text-[#A1A1AA] truncate">{label}</span>
        <div className="flex items-center gap-1 shrink-0">
          <button
            type="button"
            onClick={() => {
              const next = clamp(value - step);
              if (next === value) return;
              onRecordStep();
              onChange(next);
              onFlushSave?.();
            }}
            className="w-5 h-5 rounded bg-white/10 hover:bg-white hover:text-black text-[11px] text-white transition"
          >
            −
          </button>
          <span className="min-w-[44px] text-center font-mono-fx text-[11px] text-white">
            {Number.isInteger(step) ? Math.round(value) : value.toFixed(2)}
            {unit}
          </span>
          <button
            type="button"
            onClick={() => {
              const next = clamp(value + step);
              if (next === value) return;
              onRecordStep();
              onChange(next);
              onFlushSave?.();
            }}
            className="w-5 h-5 rounded bg-white/10 hover:bg-white hover:text-black text-[11px] text-white transition"
          >
            +
          </button>
          {onFx && <FxButton active={fxActive} onClick={onFx} />}
        </div>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onPointerDown={onBeginTx}
        onKeyDown={(e) => {
          if (adjustKeys.has(e.key)) onBeginTx();
        }}
        onChange={(e) => onChange(clamp(Number(e.target.value)))}
        onPointerUp={onCommitTx}
        onPointerCancel={onCommitTx}
        onKeyUp={onCommitTx}
        onBlur={onCommitTx}
        className="prism-slider"
      />
    </div>
  );
};

// Re-export helper so StudioEditor can add children inside a GROUP cleanly
export function createGroupChildLayer(kind: LayerKind): SceneLayer {
  const child = createLayer(kind);
  if (kind === "TEXT" && child.text) {
    child.text.fontSize = 44;
  } else if (kind === "GLASS") {
    child.width = 240;
    child.height = 100;
  } else if (kind === "SVG") {
    child.width = 48;
    child.height = 48;
  }
  return child;
}
