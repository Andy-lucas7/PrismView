import React, { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { PrismProject } from "../types/prism";
import { HUB_MOCK_TELEMETRY } from "../engine/hubMock";
import { UnifiedCompositor } from "./UnifiedCompositor";
import { HubLiveBudget } from "../engine/hubLiveBudget";

/** Miniaturas `full` (vidro/3D/vídeo reais): só 3 por vez (centro + 2 laterais). Teto Chromium ~16 WebGL. */
const hubBudget = new HubLiveBudget(3);

const POSTER_FALLBACK = "radial-gradient(circle at 50% 18%, #181920 0%, #09090C 62%, #050507 100%)";

function useIsLive(id: string): boolean {
  return useSyncExternalStore(
    (cb) => hubBudget.subscribe(cb),
    () => hubBudget.isLive(id),
    () => false
  );
}

/**
 * Miniatura do Hub, 3 níveis:
 *  - `full`: dentro do orçamento (3 melhores ranks) → vidro, 3D e vídeo reais;
 *  - `basic`: visível mas fora do orçamento → só o básico (texto, imagem, formas; sem vidro WebGL/3D/vídeo);
 *  - poster: fora da tela → só a cor de fundo; compositor desmontado (libera WebGL/vídeo).
 * Hora/data/clima/telemetria são fixos (`hubMock`). `rank` explícito (carrossel: 0 = centro,
 * 1 = laterais, ...) sobrescreve o rank por visibilidade. Memo: nunca re-renderiza por telemetria.
 */
export const HubThumbnail = React.memo(function HubThumbnail({
  project,
  rank,
}: {
  project: PrismProject;
  rank?: number;
}) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const [visRank, setVisRank] = useState<number | null>(null);
  const live = useIsLive(project.id);
  const effective = rank ?? visRank;

  useEffect(() => {
    const el = hostRef.current;
    if (!el || rank !== undefined || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      ([e]) => setVisRank(e.isIntersecting ? 1 - Math.min(1, Math.max(0, e.intersectionRatio)) : null),
      { rootMargin: "160px", threshold: [0, 0.25, 0.5, 0.75, 1] }
    );
    io.observe(el);
    return () => io.disconnect();
  }, [rank]);

  useEffect(() => {
    hubBudget.request(project.id, effective);
  }, [project.id, effective]);

  useEffect(() => () => hubBudget.request(project.id, null), [project.id]);

  return (
    <div
      ref={hostRef}
      className="relative w-full h-full overflow-hidden"
      style={{ background: project.backgroundColor && project.backgroundColor !== "#000000" ? project.backgroundColor : POSTER_FALLBACK }}
    >
      {effective !== null && (
        <div className="absolute inset-0 prism-thumb-in">
          <UnifiedCompositor
            project={project}
            telemetry={HUB_MOCK_TELEMETRY}
            paused
            isThumbnail
            thumbnailTier={live ? "full" : "basic"}
            interactive={false}
          />
        </div>
      )}
    </div>
  );
});
