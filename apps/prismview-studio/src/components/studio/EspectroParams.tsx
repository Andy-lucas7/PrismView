// Controles gerados das declarações `//@param` de um efeito Espectro.
// Usado no Inspector (camada SHADER selecionada) e no painel do Modo Espectro.
import React, { useMemo } from "react";
import { parseEspectroParams, resolveParamValue, type EspectroEffect, type EspectroParamValue } from "../../types/prism";
import { InspectorSlider } from "../InspectorControls";
import { ColorInput } from "../ui/Controls";

export interface EspectroParamsProps {
  effect: EspectroEffect;
  onChange: (name: string, value: EspectroParamValue, commit: "raw" | "discrete" | "coalesced") => void;
  onBeginTx: () => void;
  onCommitTx: () => void;
  onRecordStep: () => void;
  onFlushSave?: () => void;
}

function rangeFor(min: number | undefined, max: number | undefined, def: number): [number, number] {
  const lo = min ?? Math.min(0, def);
  const hi = max ?? Math.max(1, def * 2 || 1);
  return hi > lo ? [lo, hi] : [lo, lo + 1];
}

export const EspectroParams: React.FC<EspectroParamsProps> = ({ effect, onChange, onBeginTx, onCommitTx, onRecordStep, onFlushSave }) => {
  const parse = useMemo(() => parseEspectroParams(effect.code), [effect.code]);
  if (!parse.params.length) {
    return (
      <p className="text-[11px] text-[#71717A] leading-relaxed">
        Sem parâmetros. Declare no código, por exemplo:{" "}
        <code className="text-[#A1A1AA]">//@param forca: float = 0.5 range(0, 1) label("Força")</code>
      </p>
    );
  }
  return (
    <div className="space-y-2">
      {parse.params.map((def) => {
        const v = resolveParamValue(def, effect.params);
        if (def.type === "float") {
          const [lo, hi] = rangeFor(def.min, def.max, def.default as number);
          return (
            <InspectorSlider
              key={def.name}
              label={def.label}
              value={v as number}
              min={lo}
              max={hi}
              step={def.step ?? (hi - lo) / 200}
              onChange={(n) => onChange(def.name, n, "raw")}
              onBeginTx={onBeginTx}
              onCommitTx={onCommitTx}
              onRecordStep={onRecordStep}
              onFlushSave={onFlushSave}
            />
          );
        }
        if (def.type === "vec2") {
          const pair = v as [number, number];
          const [lo, hi] = rangeFor(def.min, def.max, Math.max(pair[0], pair[1]));
          return (
            <div key={def.name} className="space-y-1">
              {([0, 1] as const).map((axis) => (
                <InspectorSlider
                  key={axis}
                  label={`${def.label} ${axis === 0 ? "X" : "Y"}`}
                  value={pair[axis]}
                  min={lo}
                  max={hi}
                  step={def.step ?? (hi - lo) / 200}
                  onChange={(n) => onChange(def.name, axis === 0 ? [n, pair[1]] : [pair[0], n], "raw")}
                  onBeginTx={onBeginTx}
                  onCommitTx={onCommitTx}
                  onRecordStep={onRecordStep}
                  onFlushSave={onFlushSave}
                />
              ))}
            </div>
          );
        }
        if (def.type === "color") {
          return (
            <div key={def.name} className="flex items-center justify-between frosted-chrome-card px-3 py-2">
              <span className="text-xs text-[#A1A1AA]">{def.label}</span>
              <ColorInput
                aria-label={def.label}
                value={(v as string).slice(0, 7)}
                onChange={(e) => onChange(def.name, e.target.value, "coalesced")}
              />
            </div>
          );
        }
        return (
          <label key={def.name} className="flex items-center justify-between frosted-chrome-card px-3 py-2 cursor-pointer">
            <span className="text-xs text-[#A1A1AA]">{def.label}</span>
            <input
              type="checkbox"
              aria-label={def.label}
              checked={Boolean(v)}
              onChange={(e) => onChange(def.name, e.target.checked, "discrete")}
            />
          </label>
        );
      })}
    </div>
  );
};
