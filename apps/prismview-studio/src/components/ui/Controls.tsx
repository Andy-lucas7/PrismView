import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AnimatePresence, motion } from "framer-motion";
import { Check, ChevronDown, Pipette } from "lucide-react";
import { POP } from "../motion/Presence";
import { hexToHsv, hexToRgb, hsvToHex, type Hsv } from "../../utils/colorMath";

/* ───────────────────────── posicionamento do popover ───────────────────────── */

interface Anchor {
  left: number;
  top: number;
  width: number;
  placeAbove: boolean;
  maxH: number;
}

function measureAnchor(el: HTMLElement, wantH: number, minW: number): Anchor {
  const r = el.getBoundingClientRect();
  const below = window.innerHeight - r.bottom - 8;
  const above = r.top - 8;
  const placeAbove = below < Math.min(wantH, 220) && above > below;
  const maxH = Math.max(120, Math.min(wantH, placeAbove ? above : below));
  const width = Math.max(minW, r.width);
  const left = Math.max(8, Math.min(window.innerWidth - width - 8, r.left));
  return { left, top: placeAbove ? r.top - 6 : r.bottom + 6, width, placeAbove, maxH };
}

/** Fecha ao clicar fora, Esc, rolagem de outro container e resize. */
function useDismiss(open: boolean, close: () => void, refs: React.RefObject<HTMLElement | null>[]) {
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node | null;
      if (t && refs.some((r) => r.current?.contains(t))) return;
      close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close();
      }
    };
    const onScroll = (e: Event) => {
      const t = e.target as Node | null;
      if (t && refs.some((r) => r.current?.contains(t))) return;
      close();
    };
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", onScroll, true);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, close]);
}

const POPOVER_CLS =
  "fixed z-[300] overflow-hidden rounded-xl border border-white/12 bg-[#0D0D0F]/95 shadow-[0_18px_48px_rgba(0,0,0,0.7)] backdrop-blur-xl";

/* ───────────────────────── Select (substitui <select> nativo) ───────────────────────── */

interface Opt {
  value: string;
  label: React.ReactNode;
  disabled: boolean;
}

function collectOptions(children: React.ReactNode, out: Opt[] = []): Opt[] {
  React.Children.forEach(children, (child) => {
    if (!React.isValidElement(child)) return;
    const props = child.props as { value?: unknown; disabled?: boolean; children?: React.ReactNode };
    if (child.type === React.Fragment || child.type === "optgroup") {
      collectOptions(props.children, out);
    } else if (child.type === "option") {
      const label = props.children;
      const text = typeof label === "string" || typeof label === "number" ? String(label) : "";
      out.push({ value: props.value === undefined ? text : String(props.value), label, disabled: Boolean(props.disabled) });
    }
  });
  return out;
}

export interface SelectProps {
  value: string | number;
  onChange: (e: React.ChangeEvent<HTMLSelectElement>) => void;
  children: React.ReactNode;
  className?: string;
  disabled?: boolean;
  title?: string;
  id?: string;
  "aria-label"?: string;
}

/** Drop-in de `<select>`: mesmas props/`<option>`, mas com popover animado (o nativo é desenhado pelo SO). */
export const Select: React.FC<SelectProps> = ({ value, onChange, children, className = "", disabled, title, id, ...aria }) => {
  const options = useMemo(() => collectOptions(children), [children]);
  const cur = options.find((o) => o.value === String(value));
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [anchor, setAnchor] = useState<Anchor | null>(null);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const close = useCallback(() => setOpen(false), []);
  useDismiss(open, close, [btnRef, listRef]);

  const openList = () => {
    if (disabled || !btnRef.current) return;
    setAnchor(measureAnchor(btnRef.current, Math.min(300, options.length * 30 + 12), 140));
    setActive(Math.max(0, options.findIndex((o) => o.value === String(value))));
    setOpen(true);
  };
  const commit = (i: number) => {
    const o = options[i];
    if (!o || o.disabled) return;
    setOpen(false);
    btnRef.current?.focus();
    if (o.value !== String(value)) {
      const ev = { target: { value: o.value }, currentTarget: { value: o.value } } as unknown as React.ChangeEvent<HTMLSelectElement>;
      onChange(ev);
    }
  };
  const move = (dir: 1 | -1) => {
    setActive((a) => {
      let i = a;
      for (let k = 0; k < options.length; k++) {
        i = (i + dir + options.length) % options.length;
        if (!options[i].disabled) return i;
      }
      return a;
    });
  };
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        openList();
      }
      return;
    }
    if (e.key === "ArrowDown") move(1);
    else if (e.key === "ArrowUp") move(-1);
    else if (e.key === "Home") setActive(0);
    else if (e.key === "End") setActive(options.length - 1);
    else if (e.key === "Enter" || e.key === " ") commit(active);
    else if (e.key === "Tab") setOpen(false);
    else if (e.key.length === 1) {
      const k = e.key.toLowerCase();
      const start = active + 1;
      for (let n = 0; n < options.length; n++) {
        const i = (start + n) % options.length;
        const l = typeof options[i].label === "string" ? (options[i].label as string).toLowerCase() : "";
        if (l.startsWith(k) && !options[i].disabled) {
          setActive(i);
          break;
        }
      }
    } else return;
    e.preventDefault();
  };
  useLayoutEffect(() => {
    if (!open) return;
    listRef.current?.querySelector<HTMLElement>(`[data-i="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [open, active]);

  return (
    <>
      <button
        ref={btnRef}
        id={id}
        type="button"
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={aria["aria-label"]}
        title={title}
        disabled={disabled}
        onClick={() => (open ? setOpen(false) : openList())}
        onKeyDown={onKeyDown}
        className={`${className} flex items-center justify-between gap-2 text-left disabled:opacity-50`}
      >
        <span className="min-w-0 flex-1 truncate">{cur ? cur.label : "—"}</span>
        <ChevronDown className={`h-3 w-3 shrink-0 text-[#A1A1AA] transition-transform duration-200 ${open ? "rotate-180" : ""}`} aria-hidden />
      </button>
      {createPortal(
        <AnimatePresence>
          {open && anchor && (
            <motion.div
              ref={listRef}
              role="listbox"
              {...POP}
              className={`${POPOVER_CLS} overflow-y-auto p-1`}
              style={{
                left: anchor.left,
                width: anchor.width,
                maxHeight: anchor.maxH,
                ...(anchor.placeAbove ? { bottom: window.innerHeight - anchor.top } : { top: anchor.top }),
                transformOrigin: anchor.placeAbove ? "50% 100%" : "50% 0%",
              }}
            >
              {options.map((o, i) => {
                const selected = o.value === String(value);
                return (
                  <button
                    key={`${o.value}-${i}`}
                    type="button"
                    role="option"
                    aria-selected={selected}
                    data-i={i}
                    disabled={o.disabled}
                    onMouseEnter={() => setActive(i)}
                    onClick={() => commit(i)}
                    className={`flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[11px] transition-colors disabled:opacity-40 ${
                      i === active ? "bg-white/12 text-white" : "text-white/80"
                    }`}
                  >
                    <span className="min-w-0 flex-1 truncate">{o.label}</span>
                    {selected && <Check className="h-3 w-3 shrink-0 text-white" aria-hidden />}
                  </button>
                );
              })}
            </motion.div>
          )}
        </AnimatePresence>,
        document.body
      )}
    </>
  );
};

/* ───────────────────────── ColorInput (substitui <input type="color">) ───────────────────────── */

export interface ColorInputProps {
  value: string;
  onChange?: (e: React.ChangeEvent<HTMLInputElement>) => void;
  onBlur?: (e: React.FocusEvent<HTMLInputElement>) => void;
  className?: string;
  disabled?: boolean;
  title?: string;
  "aria-label"?: string;
}

const fakeEvent = (value: string) => ({ target: { value }, currentTarget: { value } }) as unknown as React.ChangeEvent<HTMLInputElement>;

declare global {
  interface Window {
    EyeDropper?: new () => { open: () => Promise<{ sRGBHex: string }> };
  }
}

/** Seletor HSV com popover animado. Mesmo contrato do `<input type="color">` (onChange contínuo, onBlur ao fechar). */
export const ColorInput: React.FC<ColorInputProps> = ({ value, onChange, onBlur, className = "", disabled, title, ...aria }) => {
  const [open, setOpen] = useState(false);
  const [anchor, setAnchor] = useState<Anchor | null>(null);
  const [hsv, setHsv] = useState<Hsv>(() => hexToHsv(value) ?? { h: 0, s: 0, v: 1 });
  const [hexDraft, setHexDraft] = useState(value);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const popRef = useRef<HTMLDivElement | null>(null);
  const areaRef = useRef<HTMLDivElement | null>(null);
  const hueRef = useRef<HTMLDivElement | null>(null);
  const lastValue = useRef(value);
  lastValue.current = value;

  // Valor externo mudou (undo, FX...) e não foi este seletor: ressincroniza o HSV.
  useEffect(() => {
    const ext = hexToHsv(value);
    if (ext && hsvToHex(hsv).toLowerCase() !== value.toLowerCase()) setHsv(ext);
    setHexDraft(value);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  const close = useCallback(() => {
    setOpen(false);
    onBlur?.(fakeEvent(lastValue.current) as unknown as React.FocusEvent<HTMLInputElement>);
  }, [onBlur]);
  useDismiss(open, close, [btnRef, popRef]);

  const emit = (next: Hsv) => {
    setHsv(next);
    const hex = hsvToHex(next);
    setHexDraft(hex);
    onChange?.(fakeEvent(hex));
  };
  const drag = (apply: (x: number, y: number) => void) => (e: React.PointerEvent) => {
    const el = e.currentTarget as HTMLElement;
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    const run = (ev: PointerEvent | React.PointerEvent) => {
      const r = el.getBoundingClientRect();
      apply(Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (ev.clientY - r.top) / r.height)));
    };
    run(e);
    const move = (ev: PointerEvent) => run(ev);
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };

  const toggle = () => {
    if (disabled || !btnRef.current) return;
    if (open) return close();
    setAnchor(measureAnchor(btnRef.current, 250, 232));
    setOpen(true);
  };
  const hueHex = hsvToHex({ h: hsv.h, s: 1, v: 1 });
  const rgb = hexToRgb(value);

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        aria-label={aria["aria-label"]}
        title={title}
        aria-expanded={open}
        disabled={disabled}
        onClick={toggle}
        className={`prism-color-swatch ${className}`}
        style={{ backgroundColor: rgb ? value : "#000000" }}
      />
      {createPortal(
        <AnimatePresence>
          {open && anchor && (
            <motion.div
              ref={popRef}
              {...POP}
              className={`${POPOVER_CLS} p-3`}
              style={{
                left: anchor.left,
                width: 232,
                ...(anchor.placeAbove ? { bottom: window.innerHeight - anchor.top } : { top: anchor.top }),
                transformOrigin: anchor.placeAbove ? "20% 100%" : "20% 0%",
              }}
            >
              <div
                ref={areaRef}
                onPointerDown={drag((x, y) => emit({ ...hsv, s: x, v: 1 - y }))}
                className="relative h-32 w-full cursor-crosshair touch-none rounded-lg"
                style={{
                  backgroundColor: hueHex,
                  backgroundImage: "linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, transparent)",
                }}
              >
                <span
                  className="pointer-events-none absolute h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-[0_0_0_1px_rgba(0,0,0,0.5)]"
                  style={{ left: `${hsv.s * 100}%`, top: `${(1 - hsv.v) * 100}%`, backgroundColor: hsvToHex(hsv) }}
                />
              </div>
              <div
                ref={hueRef}
                onPointerDown={drag((x) => emit({ ...hsv, h: x * 360 }))}
                className="relative mt-3 h-3 w-full cursor-pointer touch-none rounded-full"
                style={{ background: "linear-gradient(to right,#f00,#ff0,#0f0,#0ff,#00f,#f0f,#f00)" }}
              >
                <span
                  className="pointer-events-none absolute top-1/2 h-4 w-4 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-[0_0_0_1px_rgba(0,0,0,0.5)]"
                  style={{ left: `${(hsv.h / 360) * 100}%`, backgroundColor: hueHex }}
                />
              </div>
              <div className="mt-3 flex items-center gap-2">
                <span className="h-7 w-7 shrink-0 rounded-lg border border-white/15" style={{ backgroundColor: hsvToHex(hsv) }} />
                <input
                  aria-label="Cor em hexadecimal"
                  value={hexDraft}
                  spellCheck={false}
                  onChange={(e) => {
                    const v = e.target.value;
                    setHexDraft(v);
                    const n = hexToHsv(v);
                    if (n && /^#?[0-9a-f]{6}$/i.test(v.trim())) {
                      setHsv(n);
                      onChange?.(fakeEvent(v.startsWith("#") ? v : `#${v}`));
                    }
                  }}
                  className="min-w-0 flex-1 rounded-lg border border-white/10 bg-black/60 px-2 py-1 font-mono-fx text-xs uppercase text-white outline-none focus:border-white/30"
                />
                {typeof window !== "undefined" && window.EyeDropper && (
                  <button
                    type="button"
                    title="Conta-gotas"
                    onClick={async () => {
                      try {
                        const r = await new window.EyeDropper!().open();
                        const n = hexToHsv(r.sRGBHex);
                        if (n) emit(n);
                      } catch {
                        /* cancelado */
                      }
                    }}
                    className="grid h-7 w-7 shrink-0 place-items-center rounded-lg border border-white/10 text-[#A1A1AA] hover:bg-white/10 hover:text-white"
                  >
                    <Pipette className="h-3.5 w-3.5" />
                  </button>
                )}
              </div>
            </motion.div>
          )}
        </AnimatePresence>,
        document.body
      )}
    </>
  );
};
