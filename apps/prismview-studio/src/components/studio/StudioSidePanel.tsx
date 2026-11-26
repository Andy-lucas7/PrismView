import React, { useCallback, useEffect, useRef, useState } from "react";
import { motion } from "framer-motion";
import { SIDE_PANEL } from "../motion/Presence";
import {
  nudgeSplit,
  PANEL_WIDTH_PX,
  readSplit,
  SPLIT_DEFAULT,
  SPLIT_MAX,
  SPLIT_MIN,
  SPLIT_STEP,
  SPLIT_STEP_LARGE,
  splitFromPointer,
  writeSplit,
} from "../../utils/panelSplit";

export interface StudioSidePanelProps {
  /** Conteúdo de cima: propriedades da seleção (o inspetor). */
  properties: React.ReactNode;
  /** Conteúdo de baixo: camadas (criar, reordenar, agrupar). */
  layers: React.ReactNode;
}

function storage(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

/** Alturas mínimas (px) de cada metade, para a divisória nunca esconder uma delas. */
const MIN_TOP_PX = 170;
const MIN_BOTTOM_PX = 150;

/**
 * Painel único à direita do Estúdio: propriedades em cima, camadas embaixo, divisória arrastável
 * (ou pelo teclado) com a proporção salva. Só é montado quando aberto; o arraste mexe direto no
 * `flex-basis` do DOM (sem re-render por movimento) e confirma o valor ao soltar.
 */
export const StudioSidePanel: React.FC<StudioSidePanelProps> = ({ properties, layers }) => {
  const rootRef = useRef<HTMLElement | null>(null);
  const topRef = useRef<HTMLDivElement | null>(null);
  const [split, setSplit] = useState<number>(() => readSplit(storage()));
  const dragRef = useRef<{ pointerId: number; value: number } | null>(null);
  const rafRef = useRef<number | null>(null);

  /** Limita pela proporção E pelas alturas mínimas em px do contêiner atual. */
  const fit = useCallback((ratio: number): number => {
    const h = rootRef.current?.getBoundingClientRect().height ?? 0;
    let v = Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, ratio));
    if (h > MIN_TOP_PX + MIN_BOTTOM_PX) {
      v = Math.min(1 - MIN_BOTTOM_PX / h, Math.max(MIN_TOP_PX / h, v));
    }
    return v;
  }, []);

  const commit = useCallback(
    (value: number) => {
      const v = fit(value);
      setSplit(v);
      writeSplit(storage(), v);
    },
    [fit]
  );

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    dragRef.current = { pointerId: e.pointerId, value: split };
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    const root = rootRef.current;
    if (!drag || drag.pointerId !== e.pointerId || !root) return;
    const rect = root.getBoundingClientRect();
    drag.value = fit(splitFromPointer(e.clientY, rect.top, rect.height));
    if (rafRef.current === null) {
      rafRef.current = window.requestAnimationFrame(() => {
        rafRef.current = null;
        if (topRef.current && dragRef.current) topRef.current.style.flexBasis = `${(dragRef.current.value * 100).toFixed(2)}%`;
      });
    }
  };

  const endDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== e.pointerId) return;
    dragRef.current = null;
    if (rafRef.current !== null) {
      window.cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    commit(drag.value);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? SPLIT_STEP_LARGE : SPLIT_STEP;
    if (e.key === "ArrowUp") commit(nudgeSplit(split, -1, step));
    else if (e.key === "ArrowDown") commit(nudgeSplit(split, 1, step));
    else if (e.key === "Home") commit(SPLIT_MIN);
    else if (e.key === "End") commit(SPLIT_MAX);
    else if (e.key === "Enter") commit(SPLIT_DEFAULT);
    else return;
    e.preventDefault();
    e.stopPropagation();
  };

  // Se a janela encolhe, reaplica os mínimos em px sem mudar a preferência salva.
  useEffect(() => {
    const root = rootRef.current;
    if (!root || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      if (topRef.current && !dragRef.current) topRef.current.style.flexBasis = `${(fit(split) * 100).toFixed(2)}%`;
    });
    ro.observe(root);
    return () => ro.disconnect();
  }, [fit, split]);

  useEffect(
    () => () => {
      if (rafRef.current !== null) window.cancelAnimationFrame(rafRef.current);
    },
    []
  );

  return (
    <motion.aside
      {...SIDE_PANEL.right}
      ref={rootRef}
      aria-label="Painel do editor: propriedades e camadas"
      style={{ width: `min(${PANEL_WIDTH_PX}px, max(220px, 52vw))` }}
      className="fixed top-20 right-4 bottom-4 z-30 frosted-chrome frosted-chrome-panel flex flex-col overflow-hidden"
    >
      <section
        ref={topRef}
        aria-label="Propriedades"
        style={{ flexBasis: `${(split * 100).toFixed(2)}%` }}
        className="flex min-h-0 shrink-0 grow-0 flex-col overflow-y-auto"
      >
        {properties}
      </section>
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label="Divisória entre propriedades e camadas (setas movem, Enter restaura)"
        aria-valuemin={Math.round(SPLIT_MIN * 100)}
        aria-valuemax={Math.round(SPLIT_MAX * 100)}
        aria-valuenow={Math.round(split * 100)}
        tabIndex={0}
        title="Arraste para dar mais espaço às propriedades ou às camadas (duplo clique restaura)"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onDoubleClick={() => commit(SPLIT_DEFAULT)}
        onKeyDown={onKeyDown}
        className="group relative z-10 -my-px h-2.5 shrink-0 cursor-row-resize touch-none outline-none"
      >
        <span className="pointer-events-none absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-white/12 transition-colors group-hover:bg-white/35 group-focus-visible:bg-white/60" />
        <span className="pointer-events-none absolute left-1/2 top-1/2 h-1 w-9 -translate-x-1/2 -translate-y-1/2 rounded-full bg-white/20 transition-colors group-hover:bg-white/55 group-focus-visible:bg-white" />
      </div>
      <section aria-label="Camadas" className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        {layers}
      </section>
    </motion.aside>
  );
};
