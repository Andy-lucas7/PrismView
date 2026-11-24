// Controles reutilizáveis do Inspetor (PLANO_CORRECAO 5.1): Vec3Field, InspectorSlider, BindingTargetPicker e createGroupChildLayer.
import { NativeCompatPanel } from "./NativeCompatPanel";
import { Select } from "./ui/Controls";
import type { NativeCompatIssue } from "../utils/nativeCompat";
import React, { useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  AlignCenter,
  AlignLeft,
  AlignRight,
  Copy,
  Eye,
  EyeOff,
  Palette,
  Plus,
  Search,
  SlidersHorizontal,
  Sparkles,
  Trash2,
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


interface Vec3FieldProps {
  label: string;
  value: [number, number, number] | undefined;
  step: number;
  min?: number;
  onCommit: (v: [number, number, number]) => void;
  onRecordStep: () => void;
  onFlushSave?: () => void;
}

/** Numeric X/Y/Z entry: commits on blur/Enter (one undo step), reverts invalid text */
export const Vec3Field: React.FC<Vec3FieldProps> = ({
  label,
  value,
  step,
  min,
  onCommit,
  onRecordStep,
  onFlushSave,
}) => {
  const cur: [number, number, number] = [value?.[0] ?? 0, value?.[1] ?? 0, value?.[2] ?? 0];
  const [drafts, setDrafts] = useState<[string | null, string | null, string | null]>([
    null,
    null,
    null,
  ]);
  const setDraft = (i: number, v: string | null) =>
    setDrafts((d) => {
      const n: [string | null, string | null, string | null] = [d[0], d[1], d[2]];
      n[i] = v;
      return n;
    });
  const commit = (i: number) => {
    const raw = drafts[i];
    setDraft(i, null);
    if (raw === null) return;
    const parsed = Number(raw.replace(",", "."));
    if (raw.trim() === "" || !Number.isFinite(parsed)) return;
    const next = min !== undefined ? Math.max(min, parsed) : parsed;
    const rounded = Number(next.toFixed(3));
    if (rounded === cur[i]) return;
    const out: [number, number, number] = [cur[0], cur[1], cur[2]];
    out[i] = rounded;
    onRecordStep();
    onCommit(out);
    onFlushSave?.();
  };
  const axes = ["X", "Y", "Z"] as const;
  return (
    <div className="space-y-1">
      <span className="block text-xs text-[#A1A1AA]">{label}</span>
      <div className="grid grid-cols-3 gap-1.5">
        {axes.map((ax, i) => (
          <label
            key={ax}
            className="flex items-center gap-1 bg-black border border-white/15 rounded-lg px-1.5 py-1 focus-within:border-white/50"
          >
            <span className="text-[10px] font-mono-fx text-[#A1A1AA]">{ax}</span>
            <input
              type="number"
              step={step}
              min={min}
              value={drafts[i] ?? String(Number(cur[i].toFixed(3)))}
              onChange={(e) => setDraft(i, e.target.value)}
              onBlur={() => commit(i)}
              onKeyDown={(e) => {
                if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                else if (e.key === "Escape") setDraft(i, null);
              }}
              className="w-full min-w-0 bg-transparent text-[11px] font-mono-fx text-white outline-none"
            />
          </label>
        ))}
      </div>
    </div>
  );
};

interface InspectorSliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  unit?: string;
  /** Quando `true`, indica valores divergentes na multiseleção ("misto"). */
  mixed?: boolean;
  fxActive?: boolean;
  onFx?: () => void;
  onChange: (v: number) => void;
  onBeginTx: () => void;
  onCommitTx: () => void;
  onRecordStep: () => void;
  onFlushSave?: () => void;
}

export const InspectorSlider: React.FC<InspectorSliderProps> = ({
  label,
  value,
  min,
  max,
  step,
  unit = "",
  mixed = false,
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
              if (next === value && !mixed) return;
              onRecordStep();
              onChange(next);
              onFlushSave?.();
            }}
            className="w-5 h-5 rounded bg-white/10 hover:bg-white hover:text-black text-[11px] text-white transition"
          >
            −
          </button>
          <span
            className={`min-w-[44px] text-center font-mono-fx text-[11px] ${
              mixed ? "italic text-[#A1A1AA]" : "text-white"
            }`}
            title={mixed ? "Valores diferentes entre os itens selecionados" : undefined}
          >
            {mixed
              ? "misto"
              : `${Number.isInteger(step) ? Math.round(value) : value.toFixed(2)}${unit}`}
          </span>
          <button
            type="button"
            onClick={() => {
              const next = clamp(value + step);
              if (next === value && !mixed) return;
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
      <input aria-label={label}
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

export interface BindingTargetOption {
  value: string;
  label: string;
}

interface BindingTargetPickerProps {
  label: string;
  targetKind: Scene3DBindingTargetKind;
  value: string;
  options: BindingTargetOption[];
  invalid: boolean;
  emptyLabel: string;
  onChange: (next: string) => void;
}

export const BindingTargetPicker: React.FC<BindingTargetPickerProps> = ({
  label,
  targetKind,
  value,
  options,
  invalid,
  emptyLabel,
  onChange,
}) => {
  const [query, setQuery] = useState("");
  useEffect(() => {
    setQuery("");
  }, [targetKind]);

  const q = query.trim().toLowerCase();
  const filtered = q
    ? options.filter(
        (opt) => opt.label.toLowerCase().includes(q) || opt.value.toLowerCase().includes(q)
      )
    : options;
  const currentInAll = options.find((opt) => opt.value === value);
  const currentInFiltered = filtered.some((opt) => opt.value === value);

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-2 text-[10px] text-[#A1A1AA]">
        <span>{label}</span>
        {q && (
          <span className="font-mono-fx text-[9px] text-[#71717A]">
            {filtered.length}/{options.length}
          </span>
        )}
      </div>
      {options.length > 1 && (
        <div className="relative">
          <Search className="w-3 h-3 absolute left-2 top-1/2 -translate-y-1/2 text-[#71717A] pointer-events-none" />
          <input aria-label="Buscar alvo…"
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Buscar alvo…"
            className="w-full bg-black/40 border border-white/10 rounded-lg pl-6 pr-2 py-1 text-[10px] text-white placeholder:text-[#71717A] outline-none focus:border-white/30"
          />
        </div>
      )}
      <Select aria-label={label}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className={`w-full bg-black border rounded-lg px-2 py-1 text-[11px] text-white outline-none ${
          invalid ? "border-red-400/60" : "border-white/15"
        }`}
      >
        {invalid && value && !currentInAll && (
          <option value={value}>⚠ Inválido: {value}</option>
        )}
        {!invalid && currentInAll && !currentInFiltered && (
          <option value={currentInAll.value}>{currentInAll.label}</option>
        )}
        {filtered.length === 0 ? (
          !value ? (
            <option value="">
              {q ? "(Nenhum alvo corresponde à busca)" : emptyLabel}
            </option>
          ) : (
            <option disabled value="__NO_MATCH__">
              (Nenhum alvo corresponde à busca)
            </option>
          )
        ) : (
          filtered.map((opt) => (
            <option key={opt.value} value={opt.value}>
              {opt.label}
            </option>
          ))
        )}
      </Select>
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
