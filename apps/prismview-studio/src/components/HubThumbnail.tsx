import React, { useCallback, useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { PrismProject } from "../types/prism";
import { HUB_MOCK_TELEMETRY } from "../engine/hubMock";
import { HubLiveBudget } from "../engine/hubLiveBudget";
import { StillCache } from "../engine/hubStillCache";
import { captureStill, hubStills, stillLayerIds, stillSignature } from "../engine/hubStillCapture";
import { UnifiedCompositor } from "./UnifiedCompositor";
import { useIntroHold } from "../engine/introGate";
import { HubCardContext } from "./hub/HubCardContext";

/** Miniaturas `full` (vidro/3D/vídeo vivos): só 3 por vez (centro + 2 laterais). Teto Chromium ~16 WebGL. */
const hubBudget = new HubLiveBudget(3);

const POSTER_FALLBACK = "radial-gradient(circle at 50% 18%, #181920 0%, #09090C 62%, #050507 100%)";
const CAPTURE_DEBOUNCE_MS = 600;

function useIsFull(id: string): boolean {
  return useSyncExternalStore(
    (cb) => hubBudget.subscribe(cb),
    () => hubBudget.isLive(id),
    () => false
  );
}

/** Stills (layerId → URL) deste projeto + callback que captura quadros do compositor `full`. */
export function useProjectStills(project: PrismProject) {
  const sig = useMemo(() => stillSignature(project), [project]);
  const layerIds = useMemo(() => stillLayerIds(project), [project]);
  // Snapshot só das URLs deste projeto: um still novo de outro card não re-renderiza este.
  const stillsKey = useSyncExternalStore(
    (cb) => hubStills.subscribe(cb),
    () => {
      let k = "";
      for (const lid of layerIds) k += `${hubStills.peek(StillCache.key(project.id, lid, sig)) ?? ""}
`;
      return k;
    },
    () => ""
  );
  const stills = useMemo(() => {
    const out: Record<string, string> = {};
    for (const lid of layerIds) {
      const url = hubStills.get(StillCache.key(project.id, lid, sig));
      if (url) out[lid] = url;
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stillsKey, layerIds, project.id, sig]);
  const timers = useRef(new Map<string, number>());
  useEffect(
    () => () => {
      timers.current.forEach((t) => window.clearTimeout(t));
      timers.current.clear();
    },
    []
  );
  const onThumbFrame = useCallback(
    (layerId: string, el: HTMLCanvasElement | HTMLVideoElement) => {
      // Já tem still desta assinatura: não recaptura a cada quadro (evita laço captura → notifica → re-render).
      if (hubStills.has(StillCache.key(project.id, layerId, sig))) return;
      const prev = timers.current.get(layerId);
      if (prev !== undefined) window.clearTimeout(prev);
      timers.current.set(
        layerId,
        window.setTimeout(() => {
          timers.current.delete(layerId);
          void captureStill(el).then((url) => {
            if (url) hubStills.set(StillCache.key(project.id, layerId, sig), url);
          });
        }, CAPTURE_DEBOUNCE_MS)
      );
    },
    [project.id, sig]
  );
  const complete = layerIds.every((lid) => hubStills.has(StillCache.key(project.id, lid, sig)));
  return { stills, onThumbFrame, complete };
}

/**
 * Miniatura do Hub, 3 níveis:
 *  - `full`: dentro do orçamento (3 melhores ranks) → vidro, 3D e vídeo reais (paradas);
 *  - `still`: visível fora do orçamento → wallpaper e tudo, mas parado: vídeo/3D viram imagem em
 *    cache (capturada quando o card foi `full` ou pelo HubStillWarmer) e o vidro é só CSS;
 *  - poster: fora da tela → só a cor de fundo; compositor desmontado.
 * Hora/data/clima/telemetria fixos (`hubMock`). `rank` explícito (carrossel: 0 = centro, 1 = laterais,
 * ...) sobrescreve o rank por visibilidade. Memo: nunca re-renderiza por telemetria.
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
  const ctx = useContext(HubCardContext);
  const fixedRank = rank ?? ctx.rank;
  const full = useIsFull(project.id) && ctx.effects;
  const hold = useIntroHold();
  const effective = hold ? null : fixedRank ?? visRank;
  const { stills, onThumbFrame } = useProjectStills(project);

  useEffect(() => {
    const el = hostRef.current;
    if (!el || fixedRank !== undefined || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      ([e]) => setVisRank(e.isIntersecting ? 1 - Math.min(1, Math.max(0, e.intersectionRatio)) : null),
      { rootMargin: "160px", threshold: [0, 0.25, 0.5, 0.75, 1] }
    );
    io.observe(el);
    return () => io.disconnect();
  }, [fixedRank]);

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
            thumbnailTier={full ? "full" : "still"}
            stillFrames={stills}
            onThumbFrame={full ? onThumbFrame : undefined}
            interactive={false}
          />
        </div>
      )}
    </div>
  );
});

/**
 * Gera os stills que faltam, 1 projeto por vez (compositor `full` fora da tela), para que os cards
 * `still` já tenham wallpaper de vídeo/3D. Desiste do projeto após `timeoutMs`.
 */
export const HubStillWarmer = React.memo(function HubStillWarmer({
  projects,
  timeoutMs = 8000,
}: {
  projects: PrismProject[];
  timeoutMs?: number;
}) {
  const [skipped, setSkipped] = useState<ReadonlySet<string>>(() => new Set());
  const sigs = useMemo(() => new Map(projects.map((p) => [p.id, stillSignature(p)])), [projects]);
  useSyncExternalStore(
    (cb) => hubStills.subscribe(cb),
    () => hubStills.version(),
    () => 0
  );
  const target = projects.find((p) => {
    const ids = stillLayerIds(p);
    if (ids.length === 0 || skipped.has(`${p.id}|${sigs.get(p.id)}`)) return false;
    return !ids.every((lid) => hubStills.has(StillCache.key(p.id, lid, sigs.get(p.id) as string)));
  });
  const targetKey = target ? `${target.id}|${sigs.get(target.id)}` : null;

  useEffect(() => {
    if (!targetKey) return;
    const t = window.setTimeout(() => setSkipped((s) => new Set(s).add(targetKey)), timeoutMs);
    return () => window.clearTimeout(t);
  }, [targetKey, timeoutMs]);

  if (!target) return null;
  return <WarmSlot key={targetKey as string} project={target} />;
});

const WarmSlot: React.FC<{ project: PrismProject }> = ({ project }) => {
  const { onThumbFrame } = useProjectStills(project);
  return (
    <div
      aria-hidden
      style={{ position: "fixed", left: -10000, top: 0, width: 480, height: 270, pointerEvents: "none", overflow: "hidden" }}
    >
      <UnifiedCompositor
        project={project}
        telemetry={HUB_MOCK_TELEMETRY}
        paused
        isThumbnail
        thumbnailTier="full"
        onThumbFrame={onThumbFrame}
        interactive={false}
      />
    </div>
  );
};
