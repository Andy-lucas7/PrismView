import React, { useCallback, useMemo, useState } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
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
  renderItem: (item: T) => React.ReactNode;
  getAmbient?: (item: T) => string;
  onActivate?: (item: T) => void;
  /** Economia de energia ativa (ECO/ULTRA/FROZEN): sem vidro/blur nas miniaturas. */
  lowPower?: boolean;
  ariaLabel?: string;
}

/** Hub: alterna Carrossel (principal) e Grade, com troca em fade + blur. */
export function HubLayout<T>({
  view, onViewChange, items, getKey, renderItem, getAmbient, onActivate, lowPower = false, ariaLabel,
}: HubLayoutProps<T>) {
  const reduced = Boolean(useReducedMotion());
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

  const gridCtx = useMemo(() => ({ effects }), [effects]);
  if (items.length === 0) return null;

  const enter = { opacity: 0, scale: 0.985, filter: reduced ? "none" : `blur(${BLUR.exit}px)` };
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
                  if (!active) onViewChange(id);
                }}
                className={`relative grid h-8 w-9 place-items-center rounded-full transition-colors ${active ? "text-black" : "text-[#A1A1AA] hover:text-white"}`}
              >
                {active && (
                  <motion.span
                    layoutId="hub-view-pill"
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

      <AnimatePresence mode="wait" initial={false}>
        {view === "carousel" ? (
          <motion.div
            key="carousel"
            initial={enter}
            animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
            exit={enter}
            transition={{ duration: sec(DURATION.base), ease: [...EASE.emphasized] }}
          >
            <ViewCarousel
              items={items}
              getKey={getKey}
              renderItem={renderItem}
              getAmbient={getAmbient}
              onActivate={onActivate}
              effects={effects}
              onJank={onJank}
              ariaLabel={ariaLabel}
            />
          </motion.div>
        ) : (
          <motion.div
            key="grid"
            initial={enter}
            animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
            exit={enter}
            transition={{ duration: sec(DURATION.base), ease: [...EASE.emphasized] }}
          >
            <HubCardContext.Provider value={gridCtx}>
              <div className="grid grid-cols-1 gap-5 md:grid-cols-2 xl:grid-cols-3">
                {items.map((it) => (
                  <React.Fragment key={getKey(it)}>{renderItem(it)}</React.Fragment>
                ))}
              </div>
            </HubCardContext.Provider>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
