import React, { useMemo, useState } from "react";
import { motion } from "framer-motion";
import { Check, Sparkles, Trash2, X } from "lucide-react";
import {
  evaluateFormula,
  GlobalVar,
  LiveTelemetry,
  SPRING_TACTILE,
} from "../types/prism";

export const FxButton: React.FC<{ active: boolean; onClick: () => void; title?: string }> = ({
  active,
  onClick,
  title,
}) => (
  <button
    type="button"
    onClick={onClick}
    title={title ?? (active ? "Editar fórmula ƒx" : "Associar fórmula ƒx")}
    className={`px-2 py-1 rounded-lg font-mono-fx text-[11px] font-semibold leading-none shrink-0 ${
      active ? "fx-btn-active" : "fx-btn-idle"
    }`}
  >
    ƒx
  </button>
);

const SNIPPETS = [
  { label: "df(HH:mm)", insert: "$df(HH:mm)$" },
  { label: "df(EEE dd MMM)", insert: "$tc(up, df(EEE dd MMM))$" },
  { label: "sys(cpu)", insert: "$sys(cpu)$%" },
  { label: "sys(ram)", insert: "$sys(ram)$%" },
  { label: "mi(title)", insert: "$mi(title)$" },
  { label: "mi(artist)", insert: "$mi(artist)$" },
  { label: "mi(pos)", insert: "$mi(pos)$" },
  { label: "mi(dur)", insert: "$mi(dur)$" },
  { label: "gv()", insert: "$gv(show_stats)$" },
  { label: "if()", insert: "$if(sys(cpu)>70, #FF6B6B, #FFFFFF)$" },
];

export interface FxExpressionPopoverProps {
  label: string;
  initialExpr: string;
  fallbackDisplay: string;
  telemetry: LiveTelemetry;
  globals: GlobalVar[];
  onApply: (expr: string) => void;
  onClear: () => void;
  onClose: () => void;
}

export const FxExpressionPopover: React.FC<FxExpressionPopoverProps> = ({
  label,
  initialExpr,
  fallbackDisplay,
  telemetry,
  globals,
  onApply,
  onClear,
  onClose,
}) => {
  const [expr, setExpr] = useState(initialExpr);

  const evaluated = useMemo(() => {
    const t = expr.trim();
    if (!t) return fallbackDisplay;
    return evaluateFormula(t, telemetry, globals, new Date());
  }, [expr, telemetry, globals, fallbackDisplay]);

  return (
    <motion.div
      initial={{ opacity: 0, x: 12, scale: 0.96 }}
      animate={{ opacity: 1, x: 0, scale: 1 }}
      exit={{ opacity: 0, x: 10, scale: 0.97 }}
      transition={SPRING_TACTILE}
      className="fixed top-20 right-[376px] z-50 w-[360px] frosted-chrome frosted-chrome-panel prismatic-edge p-4 flex flex-col gap-3"
    >
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2 min-w-0">
          <span className="fx-btn-active px-2 py-0.5 rounded-md font-mono-fx text-[11px] font-bold">ƒx</span>
          <span className="text-xs font-semibold text-white truncate">{label}</span>
        </div>
        <button
          type="button"
          aria-label="Fechar"
          title="Fechar"
          onClick={onClose}
          className="p-1 rounded-full text-[#A1A1AA] hover:text-white hover:bg-white/10 transition"
        >
          <X className="w-3.5 h-3.5" />
        </button>
      </div>

      <textarea aria-label="$df(HH:mm)$ · CPU $sys(cpu)$%"
        rows={3}
        autoFocus
        value={expr}
        onChange={(e) => setExpr(e.target.value)}
        placeholder="$df(HH:mm)$ · CPU $sys(cpu)$%"
        className="w-full rounded-xl bg-black/80 border border-white/15 focus:border-white/45 px-3 py-2 font-mono-fx text-xs text-white outline-none resize-none"
      />

      <div className="rounded-xl bg-[#0D0D0F] border border-white/10 px-3 py-2">
        <span className="block text-[10px] uppercase tracking-wider text-[#A1A1AA]">Resultado Real</span>
        <span className="block font-mono-fx text-xs text-white truncate mt-0.5">{evaluated || "—"}</span>
      </div>

      <div className="flex flex-wrap gap-1.5">
        {SNIPPETS.map((s) => (
          <button
            key={s.label}
            type="button"
            onClick={() => setExpr((prev) => `${prev}${s.insert}`)}
            className="px-2 py-1 rounded-lg bg-white/5 hover:bg-white hover:text-black border border-white/10 font-mono-fx text-[10px] text-[#A1A1AA] transition"
          >
            {s.label}
          </button>
        ))}
      </div>

      {globals.length > 0 && (
        <div className="flex flex-wrap gap-1.5 pt-1 border-t border-white/10">
          {globals.map((g) => (
            <button
              key={g.name}
              type="button"
              onClick={() => setExpr((prev) => `${prev}$gv(${g.name})$`)}
              className="px-2 py-0.5 rounded-full bg-white/10 hover:bg-white hover:text-black text-[10px] font-mono-fx text-white transition"
            >
              $gv({g.name})
            </button>
          ))}
        </div>
      )}

      <div className="flex items-center justify-between pt-1">
        <button
          type="button"
          onClick={() => {
            onClear();
            onClose();
          }}
          className="prism-pill-idle px-3 py-1.5 rounded-full text-[11px] flex items-center gap-1.5"
        >
          <Trash2 className="w-3 h-3" />
          Limpar
        </button>
        <button
          type="button"
          onClick={() => {
            onApply(expr.trim());
            onClose();
          }}
          className="prismatic-cta px-4 py-1.5 text-[11px] flex items-center gap-1.5"
        >
          <Sparkles className="w-3 h-3" />
          <Check className="w-3 h-3" />
          Aplicar ƒx
        </button>
      </div>
    </motion.div>
  );
};
