import React, { useLayoutEffect, useMemo, useRef } from "react";
import { profileFromCanvas, warpCanvasWithProfile } from "../utils/stemStretch";

/** Texto comum (sem vidro) com alongamento por haste, desenhado em canvas na densidade do monitor. */
export interface StemTextProps {
  text: string;
  fontStack: string;
  fontSizePx: number;
  fontWeight: number;
  letterSpacingEm: number;
  align: "left" | "center" | "right";
  maxWidthPx?: number;
  stretch: number;
  /** Cor CSS (rgba) do preenchimento; trocar a cor NÃO refaz a deformação. */
  color: string;
  /** Sombra (px de blur do texto; 0 = nenhuma) e opacidade 0..1 da camada. */
  shadowPx: number;
  shadowAlpha: number;
  opacity?: number;
  /** Muda quando uma fonte termina de carregar (invalida o cache). */
  fontRev: number;
}

interface Mask {
  canvas: HTMLCanvasElement;
  cssW: number; // canvas inteiro (com margem)
  cssH: number;
  padPx: number;
  innerW: number; // caixa de layout (sem margem)
  innerH: number;
}

const CACHE_LIMIT = 12;
const CACHE_MAX_BYTES = 40 * 1024 * 1024;
const cache = new Map<string, Mask>();

function wrapLines(text: string, maxW: number, measure: (s: string) => number): string[] {
  const out: string[] = [];
  for (const para of text.split("\n")) {
    if (maxW <= 0 || !para) {
      out.push(para);
      continue;
    }
    let cur = "";
    for (const token of para.split(/(\s+)/)) {
      if (!token) continue;
      const cand = cur ? cur + token : token;
      if (!cur || measure(cand) <= maxW) cur = cand;
      else if (/^\s+$/.test(token)) {
        out.push(cur);
        cur = "";
      } else {
        out.push(cur.trimEnd());
        cur = token;
      }
    }
    if (cur) out.push(cur);
  }
  return out;
}

function buildMask(p: StemTextProps, dpr: number): Mask | null {
  const fs = Math.max(6, p.fontSizePx);
  const lsPx = p.letterSpacingEm * fs;
  const measureCv = document.createElement("canvas").getContext("2d");
  if (!measureCv) return null;
  const font = `${p.fontWeight} ${fs}px ${p.fontStack}`;
  measureCv.font = font;
  const supportsLs = "letterSpacing" in measureCv;
  if (supportsLs) (measureCv as unknown as { letterSpacing: string }).letterSpacing = `${lsPx}px`;
  const measure = (s: string): number => {
    const w = measureCv.measureText(s).width;
    return supportsLs || Math.abs(lsPx) < 0.01 ? w : w + lsPx * Array.from(s).length;
  };
  const lines = wrapLines(p.text || " ", p.maxWidthPx && p.maxWidthPx > 1 ? p.maxWidthPx : 0, measure);
  const lineH = fs * 1.06;
  const innerW = Math.max(1, Math.ceil(p.maxWidthPx && p.maxWidthPx > 1 ? p.maxWidthPx : Math.max(...lines.map(measure), 1)));
  const innerH0 = Math.max(1, Math.ceil(lines.length * lineH));
  const pad = Math.max(8, Math.ceil(fs * 0.3));
  const cssW = innerW + pad * 2;
  const cssH0 = innerH0 + pad * 2;
  const scale = Math.min(dpr, 2048 / Math.max(cssW, cssH0 * Math.ceil(p.stretch)));
  const src = document.createElement("canvas");
  src.width = Math.max(2, Math.round(cssW * scale));
  src.height = Math.max(2, Math.round(cssH0 * scale));
  const ctx = src.getContext("2d");
  if (!ctx) return null;
  ctx.scale(scale, scale);
  ctx.font = font;
  ctx.fillStyle = "#fff";
  // Baseline igual à do CSS (line-height 1.06): meia-folga + ascent da fonte, para não "pular" ao ligar o alongamento.
  const fm = measureCv.measureText("Mg");
  const asc = fm.fontBoundingBoxAscent;
  const desc = fm.fontBoundingBoxDescent;
  const useMetrics = Number.isFinite(asc) && Number.isFinite(desc) && asc + desc > 0;
  ctx.textBaseline = useMetrics ? "alphabetic" : "middle";
  ctx.textAlign = "left";
  if (supportsLs) (ctx as unknown as { letterSpacing: string }).letterSpacing = `${lsPx}px`;
  lines.forEach((line, i) => {
    const lw = measure(line);
    const x = pad + (p.align === "center" ? (innerW - lw) / 2 : p.align === "right" ? innerW - lw : 0);
    const y = useMetrics ? pad + lineH * i + (lineH - (asc + desc)) / 2 + asc : pad + lineH * (i + 0.5);
    if (supportsLs || Math.abs(lsPx) < 0.01) ctx.fillText(line, x, y);
    else {
      let cx = x;
      for (const ch of Array.from(line)) {
        ctx.fillText(ch, cx, y);
        cx += ctx.measureText(ch).width + lsPx;
      }
    }
  });
  const prof = profileFromCanvas(src, fs * scale);
  const warped = prof ? warpCanvasWithProfile(src, prof, p.stretch) : null;
  const canvas = warped ? warped.canvas : src;
  const ratio = warped ? warped.ratio : 1;
  return {
    canvas,
    cssW,
    cssH: cssH0 * ratio,
    padPx: pad,
    innerW,
    innerH: innerH0 + (cssH0 * ratio - cssH0),
  };
}

function getMask(p: StemTextProps, dpr: number): Mask | null {
  const key = [p.text, p.fontStack, p.fontSizePx.toFixed(2), p.fontWeight, p.letterSpacingEm.toFixed(3), p.align, p.maxWidthPx ?? 0, p.stretch.toFixed(3), dpr, p.fontRev].join("|");
  const hit = cache.get(key);
  if (hit) {
    cache.delete(key);
    cache.set(key, hit);
    return hit;
  }
  const m = buildMask(p, dpr);
  if (!m) return null;
  cache.set(key, m);
  let bytes = 0;
  cache.forEach((v) => (bytes += v.canvas.width * v.canvas.height * 4));
  for (const k of cache.keys()) {
    if (cache.size <= CACHE_LIMIT && bytes <= CACHE_MAX_BYTES) break;
    if (k === key) continue;
    const v = cache.get(k);
    if (v) bytes -= v.canvas.width * v.canvas.height * 4;
    cache.delete(k);
  }
  return m;
}

export const StemText = React.memo(function StemText(props: StemTextProps) {
  const dpr = Math.min(2, typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1);
  const mask = useMemo(
    () => getMask(props, dpr),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [props.text, props.fontStack, props.fontSizePx, props.fontWeight, props.letterSpacingEm, props.align, props.maxWidthPx, props.stretch, props.fontRev, dpr]
  );
  const ref = useRef<HTMLCanvasElement | null>(null);
  useLayoutEffect(() => {
    const cv = ref.current;
    if (!cv || !mask) return;
    cv.width = mask.canvas.width;
    cv.height = mask.canvas.height;
    const ctx = cv.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, cv.width, cv.height);
    ctx.globalCompositeOperation = "source-over";
    ctx.drawImage(mask.canvas, 0, 0);
    ctx.globalCompositeOperation = "source-in";
    ctx.fillStyle = props.color;
    ctx.fillRect(0, 0, cv.width, cv.height);
  }, [mask, props.color]);
  if (!mask) return null;
  return (
    <div style={{ position: "relative", width: mask.innerW, height: mask.innerH }}>
      <canvas
        ref={ref}
        aria-hidden
        style={{
          position: "absolute",
          left: -mask.padPx,
          top: -mask.padPx,
          width: mask.cssW,
          height: mask.cssH,
          opacity: props.opacity,
          filter: props.shadowPx > 0 ? `drop-shadow(0 2px ${props.shadowPx}px rgba(0,0,0,${props.shadowAlpha.toFixed(3)}))` : undefined,
        }}
      />
    </div>
  );
});
