import React, { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { DEFAULT_TELEMETRY, type PrismProject } from "../types/prism";
import { UnifiedCompositor } from "./UnifiedCompositor";
import { HubLiveBudget } from "../engine/hubLiveBudget";

/** Máx. de miniaturas com render real ao mesmo tempo (cada uma pode custar 1 WebGL; teto Chromium ~16). */
const hubBudget = new HubLiveBudget(8);

const POSTER_FALLBACK = "radial-gradient(circle at 50% 18%, #181920 0%, #09090C 62%, #050507 100%)";

function useIsLive(id: string): boolean {
  return useSyncExternalStore(
    (cb) => hubBudget.subscribe(cb),
    () => hubBudget.isLive(id),
    () => false
  );
}

/**
 * Miniatura do Hub: só renderiza de verdade quando visível E dentro do orçamento.
 * Fora disso mostra um poster leve e o compositor é desmontado (libera WebGL/vídeo).
 * `rank` explícito (carrossel: distância ao centro) sobrescreve o rank por visibilidade.
 * Memo + telemetria fixa: não re-renderiza no tick de 1,5 s.
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
      {live && (
        <div className="absolute inset-0 prism-thumb-in">
          <UnifiedCompositor
            project={project}
            telemetry={DEFAULT_TELEMETRY}
            paused
            isThumbnail
            interactive={false}
          />
        </div>
      )}
    </div>
  );
});
