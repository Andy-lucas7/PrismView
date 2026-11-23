import React, { useState } from "react";
import { AnimatePresence, motion } from "framer-motion";
import { COLLAPSE } from "./motion/Presence";
import { AlertTriangle, ChevronDown, ChevronRight } from "lucide-react";
import type { NativeCompatIssue } from "../utils/nativeCompat";

/**
 * "Compatível com o desktop": o que o Estúdio mostra mas o motor nativo não desenha.
 * Só aparece com problemas; avisa sem bloquear o "Aplicar".
 */
export const NativeCompatPanel: React.FC<{
  issues: NativeCompatIssue[];
  onSelectLayer: (id: string) => void;
}> = ({ issues, onSelectLayer }) => {
  const [open, setOpen] = useState(true);
  if (issues.length === 0) return null;
  return (
    <section className="border-b border-white/10 bg-amber-400/[0.06]" aria-label="Compatibilidade com o desktop">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="w-full flex items-center gap-2 px-3 py-2 text-[11px] text-amber-100"
      >
        <AlertTriangle className="w-3.5 h-3.5" aria-hidden />
        <span className="flex-1 text-left">
          {issues.length === 1 ? "1 item não aparece no desktop" : `${issues.length} itens não aparecem no desktop`}
        </span>
        {open ? <ChevronDown className="w-3.5 h-3.5" aria-hidden /> : <ChevronRight className="w-3.5 h-3.5" aria-hidden />}
      </button>
      <AnimatePresence initial={false}>{open && (
        <motion.ul {...COLLAPSE} className="px-3 pb-2.5 space-y-1.5">
          {issues.map((issue, i) => (
            <li key={`${issue.layerId}:${i}`} className="flex items-start gap-2 text-[11px] text-amber-100/85">
              <span className="flex-1 leading-relaxed">{issue.message}</span>
              <button
                type="button"
                onClick={() => onSelectLayer(issue.layerId)}
                className="shrink-0 underline hover:text-white"
              >
                Selecionar camada
              </button>
            </li>
          ))}
        </motion.ul>
      )}</AnimatePresence>
    </section>
  );
};
