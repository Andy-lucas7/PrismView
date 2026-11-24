import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AnimatePresence,
  animate,
  motion,
  useMotionValue,
  useReducedMotion,
  useTransform,
  type AnimationPlaybackControls,
  type MotionValue,
} from "framer-motion";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { BLUR, DURATION, EASE, SCALE, SPRING, sec } from "../../engine/motion";
import { FrameJankMonitor } from "../../engine/frameJank";
import { HubCardContext } from "./HubCardContext";

/* ───────────── geometria (unidades de card: d = índice − posição) ───────────── */
const WINDOW = 3; // slots montados por lado
const DRAG_THRESHOLD_PX = 6;
const FLING_PROJECTION_S = 0.16;
const WHEEL_IDLE_MS = 110;
const NOTCH_LOCK_MS = 140;
const clamp = (v: number, lo: number, hi: number) => (v < lo ? lo : v > hi ? hi : v);
const smooth = (a: number) => a * a * (3 - 2 * a);

/** Resistência elástica fora de [min,max]: o excesso tende a 0,42 card (nunca "corta"). */
function rubber(x: number, min: number, max: number): number {
  const over = x < min ? min - x : x > max ? x - max : 0;
  if (over === 0) return x;
  const r = (0.42 * (over * 2)) / (1 + over * 2);
  return x < min ? min - r : max + r;
}

const slotX = (d: number, step: number) => {
  const a = Math.abs(d);
  return Math.sign(d) * (a <= 1 ? a : 1 + (a - 1) * 0.55) * step;
};
const slotScale = (d: number) => {
  const a = Math.abs(d);
  return a <= 1 ? 1 - (1 - SCALE.carouselSide) * smooth(a) : SCALE.carouselSide - 0.05 * (Math.min(a, 3) - 1);
};
const slotOpacity = (d: number) => {
  const a = Math.abs(d);
  return a <= 1 ? 1 - 0.18 * smooth(a) : Math.max(0, 0.82 - (a - 1) * 0.42);
};

/* ───────────── slot ───────────── */
interface SlotProps {
  index: number;
  pos: MotionValue<number>;
  step: number;
  cardW: number;
  rank: number;
  effects: boolean;
  reduced: boolean;
  entering: boolean;
  total: number;
  onSelect: (i: number) => void;
  children: React.ReactNode;
}

const Slot = React.memo(function Slot({
  index, pos, step, cardW, rank, effects, reduced, entering, total, onSelect, children,
}: SlotProps) {
  const isCenter = rank === 0;
  const d = useTransform(pos, (v) => index - v);
  const x = useTransform(d, (v) => slotX(v, step));
  const y = useTransform(d, (v) => smooth(Math.min(Math.abs(v), 1)) * 14);
  const scale = useTransform(d, slotScale);
  const opacity = useTransform(d, slotOpacity);
  const rotY = useTransform(d, (v) => -clamp(v, -1.4, 1.4) * 13);
  const blur = useTransform(d, (v) => (effects && !reduced ? `blur(${(Math.min(Math.abs(v), 2) * 1.3).toFixed(2)}px)` : "none"));
  const dim = useTransform(d, (v) => Math.min(Math.abs(v), 1) * 0.26);
  const z = useTransform(d, (v) => Math.round(100 - Math.abs(v) * 10));

  const ctx = useMemo(() => ({ rank, effects }), [rank, effects]);
  const inertProps = isCenter ? {} : ({ inert: "" } as Record<string, string>);

  return (
    <motion.div
      className="[grid-area:1/1] justify-self-center"
      style={{ width: cardW, zIndex: z }}
      initial={entering ? { opacity: 0, y: 28, scale: 0.96, filter: reduced ? "none" : `blur(${BLUR.enter}px)` } : false}
      animate={{ opacity: 1, y: 0, scale: 1, filter: "blur(0px)" }}
      transition={{
        duration: sec(DURATION.hero),
        ease: [...EASE.decelerate],
        delay: entering ? Math.abs(rank) * 0.07 : 0,
      }}
    >
      <motion.div
        role="group"
        aria-roledescription="slide"
        aria-label={`${index + 1} de ${total}`}
        aria-hidden={!isCenter}
        {...inertProps}
        className="relative will-change-transform"
        style={{ x, y, scale, opacity, rotateY: rotY, filter: blur, transformOrigin: "50% 60%" }}
      >
        <HubCardContext.Provider value={ctx}>{children}</HubCardContext.Provider>
        <motion.div aria-hidden className="pointer-events-none absolute inset-0 rounded-[18px] bg-black" style={{ opacity: dim }} />
        {!isCenter && (
          <button
            type="button"
            tabIndex={-1}
            aria-label={`Ir para ${index + 1}`}
            onClick={() => onSelect(index)}
            className="absolute inset-0 z-10 cursor-pointer rounded-[18px] bg-transparent"
          />
        )}
      </motion.div>
    </motion.div>
  );
});

/* ───────────── fundo ambiente (cor da View central, sem filtro de blur) ───────────── */
const Ambient = React.memo(function Ambient({ index, pos, color }: { index: number; pos: MotionValue<number>; color: string }) {
  const opacity = useTransform(pos, (v) => Math.max(0, 1 - Math.abs(index - v)) * 0.34);
  return (
    <motion.div
      aria-hidden
      className="absolute inset-0"
      style={{ opacity, background: `radial-gradient(ellipse 52% 60% at 50% 46%, ${color} 0%, transparent 70%)` }}
    />
  );
});

/* ───────────── componente ───────────── */
export interface ViewCarouselProps<T> {
  items: T[];
  getKey: (item: T) => string;
  renderItem: (item: T, index: number) => React.ReactNode;
  /** Cor CSS usada no brilho ambiente atrás do card (opcional). */
  getAmbient?: (item: T) => string;
  /** Enter no card central. */
  onActivate?: (item: T) => void;
  /** `false` desliga blur de profundidade e vidro WebGL das miniaturas. */
  effects: boolean;
  /** Chamado 1× quando o detector de engasgo pede para desligar efeitos. */
  onJank?: () => void;
  ariaLabel?: string;
  /** Card central inicial (pula a animação de entrada: usado ao voltar do Studio). */
  initialKey?: string;
  /** Avisa a View central (para o HubLayout lembrar ao alternar para a grade e voltar). */
  onCenterKey?: (key: string) => void;
}

export function ViewCarousel<T>({
  items, getKey, renderItem, getAmbient, onActivate, effects, onJank, ariaLabel = "Views", initialKey, onCenterKey,
}: ViewCarouselProps<T>) {
  const n = items.length;
  const reduced = Boolean(useReducedMotion());
  const rootRef = useRef<HTMLDivElement | null>(null);
  const startIdx = useMemo(() => {
    const i = initialKey ? items.findIndex((it) => getKey(it) === initialKey) : -1;
    return i < 0 ? 0 : i;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const pos = useMotionValue(startIdx);
  const [center, setCenter] = useState(startIdx);
  const centerRef = useRef(startIdx);
  const targetRef = useRef(startIdx);
  const activeKeyRef = useRef<string | null>(initialKey && startIdx >= 0 && items[startIdx] !== undefined ? initialKey : null);
  const animRef = useRef<AnimationPlaybackControls | null>(null);
  const [geo, setGeo] = useState({ cardW: 560, step: 400 });
  const enteringRef = useRef(!(initialKey && startIdx >= 0 && items[startIdx] !== undefined && getKey(items[startIdx]) === initialKey));
  const itemsRef = useRef(items);
  itemsRef.current = items;
  const nRef = useRef(n);
  nRef.current = n;
  const stepRef = useRef(geo.step);
  stepRef.current = geo.step;

  // ── medição responsiva (só ao redimensionar)
  useEffect(() => {
    const el = rootRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    let raf = 0;
    const ro = new ResizeObserver(([e]) => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const w = e.contentRect.width;
        const cardW = Math.round(clamp(w * 0.46, 300, 620));
        setGeo((g) => (g.cardW === cardW ? g : { cardW, step: Math.round(cardW * 0.78) }));
      });
    });
    ro.observe(el);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, []);

  useEffect(() => {
    const t = window.setTimeout(() => (enteringRef.current = false), DURATION.hero + 500);
    return () => window.clearTimeout(t);
  }, []);

  // ── centro discreto (re-render só quando cruza meio card)
  useEffect(
    () =>
      pos.on("change", (v) => {
        const c = clamp(Math.round(v), 0, Math.max(0, nRef.current - 1));
        if (c !== centerRef.current) {
          centerRef.current = c;
          const it = itemsRef.current[c];
          if (it !== undefined) {
            activeKeyRef.current = getKeyRef.current(it);
            onCenterKeyRef.current?.(activeKeyRef.current);
          }
          setCenter(c);
        }
      }),
    [pos]
  );
  const getKeyRef = useRef(getKey);
  getKeyRef.current = getKey;
  const onCenterKeyRef = useRef(onCenterKey);
  onCenterKeyRef.current = onCenterKey;

  // ── lista mudou (filtro/adição/remoção): mantém a View ativa pela chave
  useEffect(() => {
    if (n === 0) return;
    const key = activeKeyRef.current;
    let idx = key ? items.findIndex((it) => getKey(it) === key) : -1;
    if (idx < 0) idx = clamp(centerRef.current, 0, n - 1);
    animRef.current?.stop();
    pos.set(idx);
    targetRef.current = idx;
    centerRef.current = idx;
    activeKeyRef.current = getKey(items[idx]);
    setCenter(idx);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items]);

  // ── detector de engasgo (só enquanto anima)
  const jank = useRef(new FrameJankMonitor());
  const jankRaf = useRef(0);
  const jankFired = useRef(false);
  const onJankRef = useRef(onJank);
  onJankRef.current = onJank;
  const stopJank = useCallback(() => {
    cancelAnimationFrame(jankRaf.current);
    jankRaf.current = 0;
  }, []);
  const startJank = useCallback(() => {
    if (jankRaf.current || jankFired.current) return;
    jank.current.reset();
    let last = 0;
    const tick = (t: number) => {
      if (last && jank.current.push(t - last)) {
        jankFired.current = true;
        onJankRef.current?.();
        jankRaf.current = 0;
        return;
      }
      last = t;
      jankRaf.current = requestAnimationFrame(tick);
    };
    jankRaf.current = requestAnimationFrame(tick);
  }, []);
  useEffect(() => stopJank, [stopJank]);

  // ── movimento
  const goTo = useCallback(
    (i: number, velocity = 0) => {
      const t = clamp(i, 0, Math.max(0, nRef.current - 1));
      targetRef.current = t;
      animRef.current?.stop();
      startJank();
      const ctl = reduced
        ? animate(pos, t, { duration: sec(DURATION.fast), ease: [...EASE.emphasized] })
        : animate(pos, t, { ...SPRING.carousel, velocity, restDelta: 0.0006, restSpeed: 0.01 });
      animRef.current = ctl;
      void ctl.then(() => {
        if (animRef.current === ctl) stopJank();
      });
    },
    [pos, reduced, startJank, stopJank]
  );

  // ── arraste (pointer events; só captura após o limiar, para botões continuarem clicáveis)
  const drag = useRef<{ id: number; x0: number; p0: number; lx: number; lt: number; v: number; on: boolean } | null>(null);
  const clickBlock = useRef(false);
  const [dragging, setDragging] = useState(false);

  const onPointerDown = (e: React.PointerEvent) => {
    if (n < 2 || (e.pointerType === "mouse" && e.button !== 0)) return;
    animRef.current?.stop();
    stopJank();
    clickBlock.current = false;
    drag.current = { id: e.pointerId, x0: e.clientX, p0: pos.get(), lx: e.clientX, lt: e.timeStamp, v: 0, on: false };
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d || e.pointerId !== d.id) return;
    const dx = e.clientX - d.x0;
    if (!d.on) {
      if (Math.abs(dx) < DRAG_THRESHOLD_PX) return;
      d.on = true;
      rootRef.current?.setPointerCapture(e.pointerId);
      setDragging(true);
    }
    pos.set(rubber(d.p0 - dx / stepRef.current, 0, nRef.current - 1));
    const dt = e.timeStamp - d.lt;
    if (dt > 0) {
      const inst = -((e.clientX - d.lx) / stepRef.current) / (dt / 1000);
      d.v = d.v * 0.6 + inst * 0.4;
      d.lx = e.clientX;
      d.lt = e.timeStamp;
    }
  };
  const endDrag = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d || e.pointerId !== d.id) return;
    drag.current = null;
    if (!d.on) return;
    setDragging(false);
    clickBlock.current = true;
    const v = e.timeStamp - d.lt > 80 ? 0 : clamp(d.v, -9, 9);
    goTo(Math.round(pos.get() + v * FLING_PROJECTION_S), v);
  };

  // ── roda do mouse / trackpad (listener nativo não-passivo)
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    let idle = 0;
    let lastNotch = 0;
    const onWheel = (e: WheelEvent) => {
      const ax = Math.abs(e.deltaX);
      const ay = Math.abs(e.deltaY);
      if (nRef.current < 2 || (ax < 1 && ay < 1)) return;
      e.preventDefault();
      const notch = e.deltaMode === 1 || (ax === 0 && ay >= 50);
      if (notch) {
        const now = performance.now();
        if (now - lastNotch < NOTCH_LOCK_MS) return;
        lastNotch = now;
        goTo(targetRef.current + (e.deltaY > 0 ? 1 : -1));
        return;
      }
      animRef.current?.stop();
      stopJank();
      const dx = ax > ay ? e.deltaX : e.deltaY;
      pos.set(rubber(pos.get() + dx / (stepRef.current * 1.2), 0, nRef.current - 1));
      window.clearTimeout(idle);
      idle = window.setTimeout(() => goTo(Math.round(pos.get())), WHEEL_IDLE_MS);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      el.removeEventListener("wheel", onWheel);
      window.clearTimeout(idle);
    };
  }, [goTo, pos, stopJank]);

  // ── teclado
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey || nRef.current < 1) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      if (document.querySelector('[role="dialog"],[role="alertdialog"]')) return;
      const onBody = !t || t === document.body || rootRef.current?.contains(t);
      if (!onBody) return;
      if (e.key === "ArrowRight") goTo(targetRef.current + 1);
      else if (e.key === "ArrowLeft") goTo(targetRef.current - 1);
      else if (e.key === "Home") goTo(0);
      else if (e.key === "End") goTo(nRef.current - 1);
      else if (e.key === "Enter" && t === document.body) {
        const it = itemsRef.current[centerRef.current];
        if (it !== undefined) onActivate?.(it);
      } else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [goTo, onActivate]);

  // ── progresso
  const trackPct = n > 0 ? 100 / n : 100;
  const thumbLeft = useTransform(pos, (v) => `${(clamp(v, 0, Math.max(1, n - 1)) / Math.max(1, n - 1)) * (100 - trackPct)}%`);
  const trackRef = useRef<HTMLDivElement | null>(null);
  const trackDrag = useRef<{ id: number; moved: boolean } | null>(null);
  /** x do ponteiro → posição contínua do carrossel (inversa de `thumbLeft`: o polegar fica centrado no ponteiro). */
  const posFromTrack = (clientX: number) => {
    const r = trackRef.current?.getBoundingClientRect();
    const cnt = nRef.current;
    if (!r || r.width < 2 || cnt < 2) return pos.get();
    const f = (clientX - r.left) / r.width;
    return clamp(((f - 0.5 / cnt) / (1 - 1 / cnt)) * (cnt - 1), 0, cnt - 1);
  };
  const onTrackDown = (e: React.PointerEvent) => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    e.preventDefault();
    animRef.current?.stop();
    stopJank();
    e.currentTarget.setPointerCapture(e.pointerId);
    trackDrag.current = { id: e.pointerId, moved: false };
    goTo(Math.round(posFromTrack(e.clientX)));
  };
  const onTrackMove = (e: React.PointerEvent) => {
    const d = trackDrag.current;
    if (!d || e.pointerId !== d.id) return;
    if (!d.moved) {
      d.moved = true;
      animRef.current?.stop();
      stopJank();
    }
    pos.set(posFromTrack(e.clientX));
  };
  const onTrackUp = (e: React.PointerEvent) => {
    const d = trackDrag.current;
    if (!d || e.pointerId !== d.id) return;
    trackDrag.current = null;
    if (d.moved) goTo(Math.round(pos.get()));
  };
  const onTrackKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowRight" || e.key === "ArrowUp") goTo(targetRef.current + 1);
    else if (e.key === "ArrowLeft" || e.key === "ArrowDown") goTo(targetRef.current - 1);
    else if (e.key === "Home") goTo(0);
    else if (e.key === "End") goTo(nRef.current - 1);
    else return;
    e.preventDefault();
    e.stopPropagation();
  };

  if (n === 0) return null;
  const lo = Math.max(0, center - WINDOW);
  const hi = Math.min(n - 1, center + WINDOW);
  const slots: React.ReactNode[] = [];
  for (let i = lo; i <= hi; i++) {
    const it = items[i];
    slots.push(
      <Slot
        key={getKey(it)}
        index={i}
        pos={pos}
        step={geo.step}
        cardW={geo.cardW}
        rank={Math.abs(i - center)}
        effects={effects}
        reduced={reduced}
        entering={enteringRef.current}
        total={n}
        onSelect={goTo}
        children={renderItem(it, i)}
      />
    );
  }

  return (
    <section aria-roledescription="carousel" aria-label={ariaLabel} className="relative select-none">
      <div
        ref={rootRef}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onClickCapture={(e) => {
          if (clickBlock.current) {
            e.stopPropagation();
            e.preventDefault();
            clickBlock.current = false;
          }
        }}
        className="relative overflow-x-clip py-8"
        style={{ touchAction: "pan-y", cursor: dragging ? "grabbing" : undefined, perspective: 1500 }}
      >
        {getAmbient && (
          <div aria-hidden className="pointer-events-none absolute inset-x-0 -inset-y-10">
            {items.slice(lo, hi + 1).map((it, k) => (
              <Ambient key={getKey(it)} index={lo + k} pos={pos} color={getAmbient(it)} />
            ))}
          </div>
        )}
        <div className="relative grid">{slots}</div>
        {n > 1 && (
          <>
            <NavButton side="left" disabled={center <= 0} onClick={() => goTo(targetRef.current - 1)} />
            <NavButton side="right" disabled={center >= n - 1} onClick={() => goTo(targetRef.current + 1)} />
          </>
        )}
      </div>

      {n > 1 && (
        <div className="mx-auto mt-2 flex w-full max-w-md items-center gap-3 px-4">
          <div
            ref={trackRef}
            role="slider"
            tabIndex={0}
            aria-label="Posição no carrossel"
            aria-valuemin={1}
            aria-valuemax={n}
            aria-valuenow={center + 1}
            onPointerDown={onTrackDown}
            onPointerMove={onTrackMove}
            onPointerUp={onTrackUp}
            onPointerCancel={onTrackUp}
            onKeyDown={onTrackKey}
            className="group relative flex h-5 flex-1 cursor-pointer touch-none items-center"
          >
            <div className="relative h-[3px] w-full overflow-hidden rounded-full bg-white/10 transition-[height] duration-150 group-hover:h-[5px]">
              <motion.div
                className="absolute top-0 h-full rounded-full bg-white/80"
                style={{ width: `${trackPct}%`, left: thumbLeft }}
              />
            </div>
          </div>
          <span className="relative inline-flex h-4 w-14 items-center justify-end gap-1 font-mono-fx text-[11px] tabular-nums text-[#A1A1AA]">
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.span
                key={center}
                initial={{ opacity: 0, y: 8, filter: reduced ? "none" : `blur(${BLUR.exit}px)` }}
                animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
                exit={{ opacity: 0, y: -8, filter: reduced ? "none" : `blur(${BLUR.exit}px)` }}
                transition={{ duration: sec(DURATION.base), ease: [...EASE.emphasized] }}
              >
                {center + 1}
              </motion.span>
            </AnimatePresence>
            <span aria-hidden>/ {n}</span>
          </span>
        </div>
      )}
    </section>
  );
}

const NavButton: React.FC<{ side: "left" | "right"; disabled: boolean; onClick: () => void }> = ({ side, disabled, onClick }) => (
  <motion.button
    type="button"
    aria-label={side === "left" ? "Anterior" : "Próxima"}
    disabled={disabled}
    onClick={onClick}
    whileTap={{ scale: 0.9 }}
    animate={{ opacity: disabled ? 0 : 1, scale: disabled ? 0.85 : 1 }}
    transition={SPRING.tactile}
    className={`absolute top-1/2 z-[200] -translate-y-1/2 grid h-11 w-11 place-items-center rounded-full border border-white/15 bg-black/55 text-white backdrop-blur-md hover:bg-white/10 ${side === "left" ? "left-3" : "right-3"} ${disabled ? "pointer-events-none" : ""}`}
  >
    {side === "left" ? <ChevronLeft className="h-5 w-5" /> : <ChevronRight className="h-5 w-5" />}
  </motion.button>
);
