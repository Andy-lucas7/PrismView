import React, { useCallback, useMemo, useRef, useState } from "react";
import { motion, useReducedMotion } from "framer-motion";
import { GalleryHorizontal, LayoutGrid } from "lucide-react";
import { BLUR, DURATION, EASE, SPRING, sec } from "../../engine/motion";
import { HubCardContext } from "./HubCardContext";
import { ViewCarousel } from "./ViewCarousel";

export type HubView = "carousel" | "grid";

const VIEW_KEY = "prismview.hubView.v1";
const EFFECTS_KEY = "prismview.hubEffects.v1";

export function readHubView(): HubView {
  try {
    return localStorage.getItem(VIEW_KEY) === "grid" ? "grid" : "carousel";
  } catch {
    return "carousel";
  }
}
export function writeHubView(v: HubView): void {
  try {
    localStorage.setItem(VIEW_KEY, v);
  } catch {
    /* sem storage: só não persiste */
  }
}
function readEffects(): boolean {
  try {
    return sessionStorage.getItem(EFFECTS_KEY) !== "0";
  } catch {
    return true;
  }
}

const OPTIONS: { id: HubView; label: string; Icon: typeof LayoutGrid }[] = [
  { id: "carousel", label: "Carrossel", Icon: GalleryHorizontal },
  { id: "grid", label: "Grade", Icon: LayoutGrid },
];

export interface HubLayoutProps<T> {
  view: HubView;
  onViewChange: (v: HubView) => void;
  items: T[];
  getKey: (item: T) => string;
  renderItem: (item: T, index: number) => React.ReactNode;
  getAmbient?: (item: T) => string;
  onActivate?: (item: T) => void;
  /** Economia de energia ativa (ECO/ULTRA/FROZEN): sem vidro/blur nas miniaturas. */
  lowPower?: boolean;
  ariaLabel?: string;
  /** Card inicial do carrossel (ex.: voltar do Studio para o mesmo card). */
  initialKey?: string;
}

/** Hub: alterna Carrossel (principal) e Grade, com troca em fade + blur. */
export function HubLayout<T>({
  view, onViewChange, items, getKey, renderItem, getAmbient, onActivate, lowPower = false, ariaLabel, initialKey,
}: HubLayoutProps<T>) {
  const [effectsOk, setEffectsOk] = useState(readEffects);
  const effects = effectsOk && !lowPower;

  const onJank = useCallback(() => {
    setEffectsOk(false);
    try {
      sessionStorage.setItem(EFFECTS_KEY, "0");
    } catch {
      /* ignora */
    }
  }, []);

  const reduced = Boolean(useReducedMotion());
  const gridCtx = useMemo(() => ({ effects }), [effects]);
  // View central lembrada: ao voltar da grade o carrossel já abre nela, sem animação de entrada.
  const centerKeyRef = useRef<string | undefined>(initialKey);
  const [lastKey, setLastKey] = useState<string | undefined>(initialKey);
  const onCenterKey = useCallback((k: string) => {
    centerKeyRef.current = k;
  }, []);
  const changeView = (v: HubView) => {
    setLastKey(centerKeyRef.current);
    onViewChange(v);
  };
  if (items.length === 0) return null;
  return (
    <div>
      <div className="mb-1 flex justify-end">
        <div role="radiogroup" aria-label="Visualização" className="relative flex rounded-full border border-white/10 bg-white/[0.04] p-0.5">
          {OPTIONS.map(({ id, label, Icon }) => {
            const active = view === id;
            return (
              <button
                key={id}
                type="button"
                role="radio"
                aria-checked={active}
                title={label}
                onClick={() => {
                  if (!active) changeView(id);
                }}
                className={`relative grid h-8 w-9 place-items-center rounded-full transition-colors ${active ? "text-black" : "text-[#A1A1AA] hover:text-white"}`}
              >
                {active && (
                  <motion.span
                    layoutId={`hub-view-pill-${ariaLabel ?? "x"}`}
                    transition={SPRING.tactile}
                    className="absolute inset-0 rounded-full bg-white"
                  />
                )}
                <Icon className="relative h-4 w-4" />
              </button>
            );
          })}
        </div>
      </div>

      {/* Troca em fade + blur curto só no contêiner (o morph por layoutId remedia cada card com
          miniatura WebGL e travava; também refazia a medição a cada re-render, gerando "pulinhos"). */}
      <motion.div
        key={view}
        initial={reduced ? false : { opacity: 0, filter: `blur(${BLUR.exit}px)` }}
        animate={{ opacity: 1, filter: "blur(0px)" }}
        transition={{ duration: sec(DURATION.base), ease: [...EASE.decelerate] }}
      >
        {view === "carousel" ? (
          <ViewCarousel
            key="carousel"
            items={items}
            getKey={getKey}
            renderItem={renderItem}
            getAmbient={getAmbient}
            onActivate={onActivate}
            effects={effects}
            onJank={onJank}
            ariaLabel={ariaLabel}
            initialKey={lastKey}
            onCenterKey={onCenterKey}
          />
        ) : (
          <HubCardContext.Provider value={gridCtx}>
            <div
              key="grid"
              className="prism-spot grid grid-cols-1 gap-5 md:grid-cols-2 xl:grid-cols-3"
              onPointerMove={(e) => {
                // Spotlight que segue o ponteiro no card sob ele (CSS vars; sem re-render).
                const card = (e.target as HTMLElement).closest<HTMLElement>(".prism-hub-card");
                if (!card) return;
                const r = card.getBoundingClientRect();
                card.style.setProperty("--mx", `${e.clientX - r.left}px`);
                card.style.setProperty("--my", `${e.clientY - r.top}px`);
              }}
            >
              {items.map((it, i) => (
                <React.Fragment key={getKey(it)}>{renderItem(it, i)}</React.Fragment>
              ))}
            </div>
          </HubCardContext.Provider>
        )}
      </motion.div>
    </div>
  );
}
