import React from "react";
import { motion, useTransform, type MotionValue } from "framer-motion";
import type { PrismProject } from "../../types/prism";
import { HUB_MOCK_TELEMETRY } from "../../engine/hubMock";
import { useProjectStills } from "../HubThumbnail";
import { UnifiedCompositor } from "../UnifiedCompositor";

export interface HeroRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Raio (px de tela) dos cantos do card; o hero interpola até 0 ao preencher a tela. */
export const HERO_CARD_RADIUS = 18;

interface HeroProps {
  project: PrismProject;
  /** Retângulo do card (tela). `null` = hero já em tela cheia (início do fechamento, antes de medir o alvo). */
  rect: HeroRect | null;
  /** Retângulo da caixa interna do widget no card (centralizada). */
  innerRect?: HeroRect | null;
  /** Progresso 0 (card) → 1 (tela cheia). */
  p: MotionValue<number>;
  opacity: MotionValue<number>;
}

/**
 * Elemento compartilhado card ⇄ Studio. É renderizado UMA vez em tela cheia (nítido no fim) e
 * levado ao retângulo do card por transform + clip-path (nunca anima layout): no card ele é a
 * miniatura reduzida (escala uniforme + recorte centralizado com cantos), no fim é a tela inteira.
 */
export const StudioHero = React.memo(function StudioHero({ project, rect, innerRect, p, opacity }: HeroProps) {
  const vw = typeof window !== "undefined" ? window.innerWidth : 1280;
  const vh = typeof window !== "undefined" ? window.innerHeight : 720;
  const r = rect ?? { left: 0, top: 0, width: vw, height: vh };
  const s = Math.min(1, Math.max(r.width / vw, r.height / vh));
  // Parte visível (em unidades locais) e deslocamento do recorte centralizado.
  const visW = r.width / s;
  const visH = r.height / s;
  const insetX = Math.max(0, (vw - visW) / 2);
  const insetY = Math.max(0, (vh - visH) / 2);
  const tx0 = r.left - insetX * s;
  const ty0 = r.top - insetY * s;
  const rad0 = HERO_CARD_RADIUS / s;

  const transform = useTransform(p, (v) => {
    const sc = s + (1 - s) * v;
    return `translate3d(${(tx0 * (1 - v)).toFixed(2)}px, ${(ty0 * (1 - v)).toFixed(2)}px, 0) scale(${sc.toFixed(5)})`;
  });
  const clip = useTransform(p, (v) => {
    const k = 1 - v;
    return `inset(${(insetY * k).toFixed(2)}px ${(insetX * k).toFixed(2)}px ${(insetY * k).toFixed(2)}px ${(insetX * k).toFixed(2)}px round ${(rad0 * k).toFixed(2)}px)`;
  });

  // Sem blur em camada cheia: só transform, clip-path e opacity (compostos na GPU sem piscar).

  const isWidget = project.packageType === "WIDGET";
  const wAspect = isWidget ? project.widget.width / project.widget.height : 1;
  // Largura da caixa do widget em unidades locais (o card a mostra com `innerRect.width` px de tela).
  const widgetW = isWidget ? Math.min(vw * 0.9, (innerRect ? innerRect.width : Math.min(340, r.width - 32)) / s) : 0;
  const { stills } = useProjectStills(project);

  return (
    <motion.div
      aria-hidden
      className="pointer-events-none fixed left-0 top-0 z-[400] overflow-hidden bg-pureBlack"
      style={{ width: vw, height: vh, transformOrigin: "0 0", transform, clipPath: clip, opacity, willChange: "transform, clip-path, opacity" }}
    >
      {isWidget ? (
        <div
          className="absolute inset-0 grid place-items-center bg-[#0D0D0F]"
          style={{ backgroundImage: "radial-gradient(rgba(255,255,255,0.08) 1px, transparent 1px)", backgroundSize: `${16 / s}px ${16 / s}px` }}
        >
          <div style={{ width: widgetW, aspectRatio: String(wAspect) }}>
            <UnifiedCompositor project={project} telemetry={HUB_MOCK_TELEMETRY} paused isThumbnail thumbnailTier="still" stillFrames={stills} interactive={false} />
          </div>
        </div>
      ) : (
        <UnifiedCompositor project={project} telemetry={HUB_MOCK_TELEMETRY} paused isThumbnail thumbnailTier="still" stillFrames={stills} interactive={false} />
      )}
    </motion.div>
  );
});
