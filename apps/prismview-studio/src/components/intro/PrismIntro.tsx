import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AnimatePresence, animate, motion, useMotionValue, useTransform } from "framer-motion";
import { ArrowRight } from "lucide-react";
import { DURATION, EASE, sec } from "../../engine/motion";
import { HERO_FRAG, HERO_VERT } from "./heroShader";

/** Linha do tempo (igual à do Prismlock): 5,2 s lineares; fases em frações do total. */
const TIMELINE_S = DURATION.intro / 1000;
const MELT_S = 1.6;
const PHRASES = ["OPTICAL GLASS", "ELASTIC TYPE", "LIVE WALLPAPERS"];
const BRAND_P = "P";
const BRAND_REST = "rismView";
const BRAND = BRAND_P + BRAND_REST;

const clamp01 = (v: number) => (v < 0 ? 0 : v > 1 ? 1 : v);
const phase = (t: number, a: number, b: number) => clamp01((t - a) / Math.max(b - a, 1e-5));
const smooth = (t: number, a: number, b: number) => {
  const x = phase(t, a, b);
  return x * x * (3 - 2 * x);
};
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;

interface BrandLayout {
  fontPx: number;
  lineH: number;
  fullW: number;
  pW: number;
  restStart: number;
  restW: number;
  asc: number;
  desc: number;
}

/** Peso da marca: Outfit Light (300), leve mas não ultrafino. */
const BRAND_WEIGHT = 300;
const FONT_FAMILY = '"Outfit", "Inter", "Segoe UI", sans-serif';

function measureBrand(fontPx: number): BrandLayout {
  const cv = document.createElement("canvas").getContext("2d")!;
  cv.font = `${BRAND_WEIGHT} ${fontPx}px ${FONT_FAMILY}`;
  const ls = -0.011 * fontPx;
  (cv as unknown as { letterSpacing: string }).letterSpacing = `${ls}px`;
  const w = (s: string) => cv.measureText(s).width;
  const fullW = w(BRAND);
  const restW = w(BRAND_REST);
  const m = cv.measureText("Mg");
  return {
    fontPx,
    lineH: Math.round(fontPx * 1.2),
    fullW,
    pW: w(BRAND_P),
    restStart: fullW - restW,
    restW,
    asc: m.fontBoundingBoxAscent || fontPx * 0.95,
    desc: m.fontBoundingBoxDescent || fontPx * 0.25,
  };
}

export interface PrismIntroProps {
  /** Início do derretimento: o Hub já pode montar por baixo. */
  onEnterStart: () => void;
  /** Fim: remover a intro. */
  onDone: () => void;
  /** Energia ECO/ULTRA: shader em resolução menor. */
  lowPower?: boolean;
}

/**
 * Abertura animada do PrismView. Base = hero do Prismlock (feixes convergindo, "P" que acende e
 * desliza, "rismView" entrando com blur, frases, botão); ao entrar, o hero DERRETE como vidro a
 * partir do botão (shader) até revelar o Hub.
 */
export const PrismIntro: React.FC<PrismIntroProps> = ({ onEnterStart, onDone, lowPower = false }) => {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const t = useMotionValue(0);
  const melt = useMotionValue(0);
  const [size, setSize] = useState({ w: window.innerWidth, h: window.innerHeight });
  const [fontsReady, setFontsReady] = useState(false);
  const [melting, setMelting] = useState(false);
  const [phraseIdx, setPhraseIdx] = useState(0);
  const meltingRef = useRef(false);
  const doneRef = useRef(false);
  const titleTexRef = useRef<{ cv: HTMLCanvasElement; rect: [number, number, number, number] } | null>(null);

  // ── fontes (Outfit) antes de medir/começar
  useEffect(() => {
    let alive = true;
    const fin = () => alive && setFontsReady(true);
    const fonts = document.fonts;
    if (!fonts?.load) {
      fin();
      return;
    }
    void Promise.race([
      Promise.all([fonts.load(`${BRAND_WEIGHT} 80px ${FONT_FAMILY}`, BRAND), fonts.ready]),
      new Promise((r) => window.setTimeout(r, 900)),
    ]).then(fin, fin);
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    const on = () => setSize({ w: window.innerWidth, h: window.innerHeight });
    window.addEventListener("resize", on);
    return () => window.removeEventListener("resize", on);
  }, []);

  const fontPx = Math.round(Math.max(52, Math.min(size.w * 0.075, size.h * 0.135, 150)));
  const brand = useMemo(() => (fontsReady ? measureBrand(fontPx) : null), [fontsReady, fontPx]);

  // ── linha do tempo
  useEffect(() => {
    if (!fontsReady) return;
    const ctl = animate(t, 1, { duration: TIMELINE_S, ease: "linear" });
    return () => ctl.stop();
  }, [fontsReady, t]);

  // frases
  const btnProg = useTransform(t, (v) => smooth(v, 0.74, 0.98));
  useEffect(() => {
    if (melting) return;
    let id = 0;
    const tick = () => {
      if (t.get() > 0.76) setPhraseIdx((i) => (i + 1) % PHRASES.length);
      id = window.setTimeout(tick, 1850);
    };
    id = window.setTimeout(tick, 1850);
    return () => window.clearTimeout(id);
  }, [melting, t]);

  // ── derivadas do título
  const pIgnite = useTransform(t, (v) => smooth(v, 0.13, 0.23));
  const slideProg = useTransform(t, (v) => smooth(v, 0.22, 0.57));
  const sweepProg = useTransform(t, (v) => smooth(v, 0.38, 0.8));

  const titleCenterY = useTransform(sweepProg, (s) => lerp(size.h * 0.42, size.h * 0.28, s));
  const brandTop = useTransform(titleCenterY, (c) => c - (brand?.lineH ?? 0) * 0.5);
  const wordScale = useTransform(sweepProg, (s) => lerp(0.92, 1, s));
  const finalLeft = brand ? (size.w - brand.fullW) * 0.5 : 0;
  const pLeft = useTransform(slideProg, (s) => lerp(brand ? (size.w - brand.pW) * 0.5 : 0, finalLeft, s));
  const restTarget = finalLeft + (brand?.restStart ?? 0);
  const slideRight = Math.max(28, fontPx * 0.78);
  const restLeft = useTransform(slideProg, (s) => lerp(restTarget + slideRight, restTarget, s));
  const restBlur = useTransform(slideProg, (s) => `blur(${(16 * (1 - s)).toFixed(2)}px)`);
  const pShadow = useTransform(slideProg, (s) => {
    const a = Math.max(0.35, Math.min(0.9, 0.85 - 0.45 * s));
    return `0 0 ${lerp(38, 16, s).toFixed(1)}px rgba(255,255,255,${a.toFixed(3)})`;
  });
  const restShadow = useTransform(slideProg, (s) => `0 0 14px rgba(255,255,255,${(0.35 * s).toFixed(3)})`);
  const restBg = useTransform(slideProg, (s) => {
    const restW = brand?.restW ?? 1;
    const feather = restW * 0.65;
    const end = lerp(0, restW + feather * 1.15, s);
    const start = end - feather;
    const at = (f: number) => `${(start + (end - start) * f).toFixed(1)}px`;
    return `linear-gradient(90deg, #fff 0px, #fff ${start.toFixed(1)}px, #00E5FF ${at(0.28)}, #8B5CF6 ${at(0.56)}, #F43F5E ${at(0.8)}, rgba(244,63,94,0) ${end.toFixed(1)}px)`;
  });
  const btnY = useTransform(btnProg, (b) => (1 - b) * 28);

  // ── WebGL
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !fontsReady || !brand) return;
    const gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: true, antialias: false, depth: false, stencil: false });
    if (!gl) {
      // Sem WebGL2: pula direto (a intro é enfeite, nunca bloqueia o app).
      doneRef.current = true;
      onEnterStart();
      onDone();
      return;
    }
    const mk = (type: number, src: string) => {
      const sh = gl.createShader(type)!;
      gl.shaderSource(sh, src);
      gl.compileShader(sh);
      return sh;
    };
    const prog = gl.createProgram()!;
    gl.attachShader(prog, mk(gl.VERTEX_SHADER, HERO_VERT));
    gl.attachShader(prog, mk(gl.FRAGMENT_SHADER, HERO_FRAG));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
      doneRef.current = true;
      onEnterStart();
      onDone();
      return;
    }
    gl.useProgram(prog);
    const U = (n: string) => gl.getUniformLocation(prog, n);
    const loc = {
      res: U("uRes"), conv: U("uConv"), titleCenter: U("uTitleCenter"), time: U("uTime"), amb: U("uAmb"),
      melt: U("uMelt"), origin: U("uMeltOrigin"), title: U("uTitle"), rect: U("uTitleRect"), titleOn: U("uTitleOn"),
    };
    const tex = gl.createTexture();
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([0, 0, 0, 0]));
    gl.uniform1i(loc.title, 0);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, true);
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    gl.disable(gl.BLEND);

    let uploadedTitle = false;
    let raf = 0;
    let lost = false;
    let ambOrigin: number | null = null;
    const onLost = (e: Event) => {
      e.preventDefault();
      lost = true;
    };
    canvas.addEventListener("webglcontextlost", onLost);

    const scale = (lowPower ? 0.5 : 0.8) * Math.min(window.devicePixelRatio || 1, 1.25);
    const draw = (now: number) => {
      raf = requestAnimationFrame(draw);
      if (lost) {
        cancelAnimationFrame(raf);
        if (!doneRef.current) {
          doneRef.current = true;
          onEnterStart();
          onDone();
        }
        return;
      }
      const m = melt.get();
      // Durante o derretimento renderiza menor (o desfoque esconde a perda): 0,75× da escala normal.
      const w = Math.max(2, Math.min(1920, Math.round(size.w * (m > 0.0005 ? scale * 0.75 : scale))));
      const h = Math.max(2, Math.round((size.h * w) / size.w));
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
        gl.viewport(0, 0, w, h);
      }
      const k = w / size.w; // px CSS → px do canvas
      const tt = t.get();
      const sw = smooth(tt, 0.38, 0.8);
      gl.uniform2f(loc.res, w, h);
      gl.uniform2f(loc.conv, w * 0.5, h * 0.42);
      gl.uniform2f(loc.titleCenter, w * 0.5, lerp(h * 0.42, h * 0.28, sw));
      if (ambOrigin === null) ambOrigin = now;
      // Ciclos em 20 voltas (180 s): múltiplos 1.0, 0.85, 1.10 e 0.70 completam 20, 17, 22 e 14 voltas exatas (0 salto).
      const ambPhase = (((now - ambOrigin) % 180_000) / 9000) * Math.PI * 2;
      gl.uniform1f(loc.time, tt);
      gl.uniform1f(loc.amb, ambPhase);
      gl.uniform1f(loc.melt, m);
      const b = btnRef.current?.getBoundingClientRect();
      gl.uniform2f(loc.origin, (b ? b.left + b.width / 2 : size.w * 0.5) * k, (b ? b.top + b.height / 2 : size.h * 0.8) * k);
      const tex2 = titleTexRef.current;
      if (tex2 && !uploadedTitle) {
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, tex2.cv);
        gl.generateMipmap(gl.TEXTURE_2D);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        uploadedTitle = true;
      }
      if (tex2) gl.uniform4f(loc.rect, tex2.rect[0] * k, tex2.rect[1] * k, tex2.rect[2] * k, tex2.rect[3] * k);
      gl.uniform1f(loc.titleOn, uploadedTitle ? 1 : 0);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    raf = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(raf);
      canvas.removeEventListener("webglcontextlost", onLost);
      gl.deleteTexture(tex);
      gl.deleteProgram(prog);
      gl.deleteVertexArray(vao);
      gl.getExtension("WEBGL_lose_context")?.loseContext();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fontsReady, brand, size.w, size.h, lowPower]);

  // ── entrar: o título final vira textura e o hero derrete
  const enter = useCallback(
    (fast = false) => {
      if (meltingRef.current || !brand) return;
      if (!fast && btnProg.get() < 0.35) return; // botão ainda aparecendo
      meltingRef.current = true;
      if (t.get() < 1) t.set(1);
      // Título final (branco com brilho) desenhado na mesma geometria do DOM.
      const pad = Math.ceil(fontPx * 0.5);
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      const rw = Math.ceil(brand.fullW + pad * 2);
      const rh = brand.lineH + pad * 2;
      const cv = document.createElement("canvas");
      cv.width = Math.ceil(rw * dpr);
      cv.height = Math.ceil(rh * dpr);
      const c = cv.getContext("2d")!;
      c.scale(dpr, dpr);
      c.font = `${BRAND_WEIGHT} ${brand.fontPx}px ${FONT_FAMILY}`;
      (c as unknown as { letterSpacing: string }).letterSpacing = `${-0.011 * brand.fontPx}px`;
      c.textBaseline = "alphabetic";
      c.fillStyle = "#fff";
      c.shadowColor = "rgba(255,255,255,0.35)";
      c.shadowBlur = 14;
      const baseline = pad + (brand.lineH - (brand.asc + brand.desc)) / 2 + brand.asc;
      c.fillText(BRAND, pad, baseline);
      const left = (size.w - brand.fullW) * 0.5 - pad;
      const top = size.h * 0.28 - brand.lineH * 0.5 - pad;
      titleTexRef.current = { cv, rect: [left, top, rw, rh] };
      meltingRef.current = true;
      setMelting(true);
      onEnterStart();
      // Inspeção visual: `?introMelt=0.5` congela o derretimento nesse ponto (sem terminar).
      const frozen = Number(new URLSearchParams(window.location.search).get("introMelt"));
      if (Number.isFinite(frozen) && frozen > 0) {
        melt.set(Math.min(1, frozen));
        return;
      }
      const ctl = animate(melt, 1, {
        duration: fast ? 1.25 : MELT_S,
        ease: [...EASE.emphasized] as [number, number, number, number],
      });
      void ctl.then(() => {
        if (doneRef.current) return;
        doneRef.current = true;
        onDone();
      });
    },
    [brand, btnProg, fontPx, melt, onDone, onEnterStart, size.h, size.w, t]
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") enter(true);
      else if ((e.key === "Enter" || e.key === " ") && btnProg.get() > 0.35) {
        e.preventDefault();
        enter();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [btnProg, enter]);

  useLayoutEffect(() => {
    rootRef.current?.focus({ preventScroll: true });
  }, []);

  const taglineOpacity = useTransform(btnProg, (b) => (melting ? 0 : b));
  const dripClass = melting ? "opacity-0 blur-md scale-95" : "";

  return (
    <div
      ref={rootRef}
      tabIndex={-1}
      role="dialog"
      aria-label="Abertura do PrismView"
      className="fixed inset-0 z-[500] select-none overflow-hidden bg-transparent outline-none"
    >
      <canvas ref={canvasRef} className="absolute inset-0 h-full w-full" />

      {brand && (
        <div className="pointer-events-none absolute inset-0">
          <motion.div
            className="absolute left-0 top-0 h-full w-full"
            style={{ scale: wordScale, transformOrigin: `50% ${(size.h * 0.35).toFixed(0)}px`, opacity: melting ? 0 : 1 }}
          >
            <motion.span
              className="absolute whitespace-pre font-display text-white"
              style={{
                x: pLeft,
                y: brandTop,
                opacity: pIgnite,
                fontSize: brand.fontPx,
                lineHeight: `${brand.lineH}px`,
                letterSpacing: "-0.011em",
                textShadow: pShadow,
                fontFamily: FONT_FAMILY,
                fontWeight: BRAND_WEIGHT,
              }}
            >
              {BRAND_P}
            </motion.span>
            <motion.span
              className="absolute whitespace-pre font-display"
              style={{
                x: restLeft,
                y: brandTop,
                opacity: slideProg,
                filter: restBlur,
                fontSize: brand.fontPx,
                lineHeight: `${brand.lineH}px`,
                letterSpacing: "-0.011em",
                fontFamily: FONT_FAMILY,
                fontWeight: BRAND_WEIGHT,
                backgroundImage: restBg,
                WebkitBackgroundClip: "text",
                backgroundClip: "text",
                color: "transparent",
                textShadow: restShadow,
              }}
            >
              {BRAND_REST}
            </motion.span>
          </motion.div>

          <motion.div className="absolute left-0 right-0 top-1/2 flex justify-center px-8" style={{ opacity: taglineOpacity }}>
            <div className="relative h-5 w-full max-w-md text-center">
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span
                  key={phraseIdx}
                  className="absolute inset-0 text-[13px] font-medium tracking-[0.28em] text-white/80"
                  initial={{ opacity: 0, y: 6, filter: "blur(6px)" }}
                  animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
                  exit={{ opacity: 0, y: -6, filter: "blur(6px)" }}
                  transition={{ duration: 0.65, ease: [...EASE.emphasized] }}
                >
                  {PHRASES[phraseIdx % PHRASES.length]}
                </motion.span>
              </AnimatePresence>
            </div>
          </motion.div>
        </div>
      )}

      <motion.div className="absolute bottom-16 left-0 right-0 flex justify-center" style={{ y: btnY, opacity: btnProg }}>
        <button
          ref={btnRef}
          type="button"
          onClick={() => enter()}
          disabled={melting}
          className={`flex items-center gap-2.5 rounded-full bg-white px-7 py-4 text-sm font-bold tracking-[0.01em] text-black shadow-[0_0_44px_rgba(255,255,255,0.28),0_10px_36px_rgba(139,92,246,0.45)] transition-all duration-[380ms] hover:scale-[1.03] ${dripClass}`}
        >
          Entrar no PrismView
          <ArrowRight className="h-[18px] w-[18px]" aria-hidden />
        </button>
      </motion.div>

      <motion.button
        type="button"
        onClick={() => enter(true)}
        disabled={melting}
        className="absolute right-6 top-5 rounded-full px-3 py-1.5 text-[11px] font-medium tracking-wider text-white/45 transition-colors hover:text-white"
        initial={{ opacity: 0 }}
        animate={{ opacity: melting ? 0 : 1 }}
        transition={{ delay: melting ? 0 : 1, duration: sec(DURATION.slow) }}
      >
        Pular
      </motion.button>
    </div>
  );
};
