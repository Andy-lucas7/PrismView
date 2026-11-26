import React from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { BLUR, DURATION, EASE, SCALE, SPRING, sec } from "../../engine/motion";

/**
 * Entrada/saída padrão de tudo que aparece e some (popovers, avisos, seções, abas), em tokens:
 *  - POP: popover/chip (fade + sobe + escala + blur curto);
 *  - COLLAPSE: seção que abre/fecha (altura auto + fade; `overflow: hidden` obrigatório);
 *  - TAB: troca de página/aba (fade + desliza + blur).
 * O blur é desligado com "reduzir movimento" (MotionConfig já zera as transformações).
 */
const enterT = { duration: sec(DURATION.base), ease: [...EASE.decelerate] as [number, number, number, number] };
const exitT = { duration: sec(DURATION.fast), ease: [...EASE.accelerate] as [number, number, number, number] };

export const POP = {
  initial: { opacity: 0, y: 6, scale: SCALE.enter },
  animate: { opacity: 1, y: 0, scale: 1, transition: enterT },
  exit: { opacity: 0, y: 3, scale: SCALE.exit, transition: exitT },
} as const;

export const COLLAPSE = {
  initial: { height: 0, opacity: 0 },
  animate: { height: "auto", opacity: 1, transition: { height: { ...SPRING.expand }, opacity: enterT } },
  exit: { height: 0, opacity: 0, transition: { height: { duration: sec(DURATION.base), ease: [...EASE.emphasized] as [number, number, number, number] }, opacity: exitT } },
  style: { overflow: "hidden" },
} as const;

export const TAB = {
  initial: { opacity: 0, y: 8 },
  animate: { opacity: 1, y: 0, transition: enterT },
  exit: { opacity: 0, y: -4, transition: exitT },
} as const;

/** Linha de lista que entra/sai (camadas): altura + fade + deslize; `overflow: clip` com margem p/ o brilho da seleção. */
export const ROW = {
  initial: { opacity: 0, height: 0, x: -12 },
  animate: { opacity: 1, height: "auto", x: 0, transition: { height: { ...SPRING.expand }, opacity: enterT, x: enterT } },
  exit: { opacity: 0, height: 0, x: 12, transition: { height: { duration: sec(DURATION.base), ease: [...EASE.emphasized] as [number, number, number, number] }, opacity: exitT, x: exitT } },
  style: { overflow: "clip", overflowClipMargin: 12 },
} as const;

/** Painel lateral que entra/sai pela borda (painel do Studio à direita, Modo Espectro à esquerda). */
const panelEnter = { duration: sec(DURATION.slow), ease: [...EASE.decelerate] as [number, number, number, number] };
const panel = (dx: number) =>
  ({
    initial: { opacity: 0, x: dx },
    animate: { opacity: 1, x: 0, transition: panelEnter },
    exit: { opacity: 0, x: dx, transition: exitT },
  }) as const;
export const SIDE_PANEL = { left: panel(-24), right: panel(24) } as const;

/** Troca de aba com muito conteúdo (inspetor): só fade + deslize curto, sem blur no painel inteiro. */
export const TAB_LITE = {
  initial: { opacity: 0, y: 6 },
  animate: { opacity: 1, y: 0, transition: enterT },
  exit: { opacity: 0, y: -4, transition: exitT },
} as const;

/** Texto que troca suavemente sem blur rasterizado (evita piscadas durante transições). */
export const SwapText: React.FC<{ id: string; children: React.ReactNode; className?: string }> = ({ id, children, className }) => {
  return (
    <span className={`relative block ${className ?? ""}`}>
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={id}
          className="block"
          initial={{ opacity: 0, y: 4 }}
          animate={{ opacity: 1, y: 0, transition: enterT }}
          exit={{ opacity: 0, y: -4, transition: exitT }}
        >
          {children}
        </motion.span>
      </AnimatePresence>
    </span>
  );
};

/** Texto que se revela palavra por palavra sem CSS blur caro na GPU/WebView. */
export const BlurText: React.FC<{ text: string; className?: string; delay?: number; stagger?: number }> = ({
  text,
  className,
  delay = 0,
  stagger = 0.03,
}) => {
  const reduced = Boolean(useReducedMotion());
  if (reduced) return <span className={className}>{text}</span>;
  const words = text.split(" ");
  return (
    <span className={className} aria-label={text}>
      {words.map((w, i) => (
        <motion.span
          key={`${w}-${i}`}
          aria-hidden
          className="inline-block whitespace-pre"
          initial={{ opacity: 0, y: 5 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: sec(DURATION.base), ease: [...EASE.decelerate], delay: delay + i * stagger }}
        >
          {w}
          {i < words.length - 1 ? " " : ""}
        </motion.span>
      ))}
    </span>
  );
};
