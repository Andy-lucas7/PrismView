import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { GlassEffect } from "../types/prism";
import { getFrostedNoiseDataUrl, GlyphGlassMaps } from "../utils/glassLens";
import {
  BackdropSourceInfo,
  opticalGlassEngine,
  OpticalGlassRenderParams,
} from "../utils/opticalGlassWebGL";

export interface OpticalGlassSurfaceProps {
  mode: "plate" | "glyph";
  backdrop: BackdropSourceInfo | null;
  videoFrameTokenRef: React.RefObject<number>;
  registerFrameTick: (cb: () => void) => () => void;
  cursorNormRef?: React.RefObject<[number, number] | null>;
  parallaxMaxPx?: [number, number];
  mediaRev: number;
  viewportW: number;
  viewportH: number;
  centerX: number;
  centerY: number;
  width: number;
  height: number;
  rotationDeg: number;
  scaleY: number;
  glass: GlassEffect;
  containerScale: number;
  opacity: number;
  radiusPx: number;
  tintRgb: [number, number, number];
  glyphMaps?: GlyphGlassMaps | null;
  pigmentRgba?: [number, number, number, number];
  glyphDropShadowPx?: number;
  forceCssFallback?: boolean;
  isThumbnail?: boolean;
}

function buildPureCssBackdrop(
  blurPx: number,
  satPct: number,
  brightPct: number,
  contrastPct: number,
  isThumbnail: boolean
): string {
  if (isThumbnail) {
    const tb = Math.min(10, blurPx * 0.4);
    return `${tb > 0.1 ? `blur(${tb.toFixed(1)}px) ` : ""}saturate(${satPct}%) brightness(${brightPct}%)`;
  }
  const parts: string[] = [];
  if (blurPx > 0.1) parts.push(`blur(${blurPx.toFixed(1)}px)`);
  parts.push(`saturate(${satPct}%)`);
  parts.push(`brightness(${brightPct}%)`);
  parts.push(`contrast(${contrastPct}%)`);
  return parts.join(" ");
}

export const OpticalGlassSurface: React.FC<OpticalGlassSurfaceProps> = ({
  mode,
  backdrop,
  videoFrameTokenRef,
  registerFrameTick,
  cursorNormRef,
  parallaxMaxPx,
  mediaRev,
  viewportW,
  viewportH,
  centerX,
  centerY,
  width,
  height,
  rotationDeg,
  scaleY,
  glass,
  containerScale,
  opacity,
  radiusPx,
  tintRgb,
  glyphMaps = null,
  pigmentRgba,
  glyphDropShadowPx = 0,
  forceCssFallback = false,
  isThumbnail = false,
}) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const hostRef = useRef<HTMLDivElement | null>(null);
  const backdropLatestRef = useRef<BackdropSourceInfo | null>(backdrop);
  backdropLatestRef.current = backdrop;
  const parallaxMaxRef = useRef<[number, number] | undefined>(parallaxMaxPx);
  parallaxMaxRef.current = parallaxMaxPx;
  const [measured, setMeasured] = useState<{ w: number; h: number }>({ w: width, h: height });
  const [webglOk, setWebglOk] = useState<boolean>(() => !forceCssFallback && opticalGlassEngine.isAvailable());

  // Measure actual host dimensions for auto-sized elements (e.g. TEXT PILL)
  useEffect(() => {
    if (width > 1 && height > 1) {
      setMeasured((prev) =>
        Math.abs(prev.w - width) > 0.5 || Math.abs(prev.h - height) > 0.5
          ? { w: width, h: height }
          : prev
      );
      return;
    }
    const el = hostRef.current;
    if (!el) return;
    const update = () => {
      const w = el.clientWidth;
      const h = el.clientHeight;
      if (w > 1 && h > 1) {
        setMeasured((prev) =>
          Math.abs(prev.w - w) > 0.5 || Math.abs(prev.h - h) > 0.5 ? { w, h } : prev
        );
      }
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, [width, height]);

  const effW = width > 1 ? width : measured.w;
  const effH = height > 1 ? height : measured.h;

  const blurPx = Math.max(0, Math.min(80, glass.blurPx));
  const refrScalePx = Math.max(0, Math.min(60, glass.refractionScale)) * Math.max(0.5, containerScale);
  const bevelPx = Math.max(4, Math.min(80, glass.bevelWidth ?? 22)) * containerScale;
  const chromaPx = Math.max(0, Math.min(6, glass.chromaticAberration));
  const frostOpacity = Math.max(0, Math.min(0.85, glass.frostOpacity));
  const noiseOpacity = Math.max(0, Math.min(0.4, glass.noiseOpacity ?? 0.04));
  const specularIntensity = Math.max(0, Math.min(1, glass.specularIntensity ?? 0.45));
  const borderOpacity = Math.max(0, Math.min(0.8, glass.borderOpacity));
  const brightness = Math.max(0.5, Math.min(1.8, glass.brightness ?? 1.06));
  const contrast = Math.max(0.6, Math.min(1.8, glass.contrast ?? 1.04));
  const saturation = Math.max(0, Math.min(2.6, glass.saturation));

  const latestParamsRef = useRef<Omit<OpticalGlassRenderParams, "backdrop"> | null>(null);
  latestParamsRef.current = {
    mode,
    viewportW: Math.max(1, viewportW),
    viewportH: Math.max(1, viewportH),
    centerX,
    centerY,
    width: Math.max(2, effW),
    height: Math.max(2, effH),
    rotationDeg,
    scaleY,
    blurPx,
    refractionScale: refrScalePx,
    bevelPx,
    radiusPx,
    chromaticAberration: chromaPx,
    frostOpacity,
    tintRgb,
    noiseOpacity,
    specularIntensity,
    borderOpacity,
    brightness,
    contrast,
    saturation,
    opacity,
    glyphCanvas: glyphMaps?.webglCanvas ?? null,
    glyphHeightCanvas: glyphMaps?.heightCanvas ?? null,
    glyphKey: glyphMaps?.key,
    pigmentRgba,
  };

  // Mirror of `webglOk` so the hot path only calls setState on a real transition.
  const webglOkRef = useRef(webglOk);
  const commitWebglOk = useCallback((next: boolean) => {
    if (webglOkRef.current === next) return;
    webglOkRef.current = next;
    setWebglOk(next);
  }, []);

  const drawFrame = useCallback(() => {
    if (forceCssFallback) {
      commitWebglOk(false);
      return;
    }
    const canvas = canvasRef.current;
    const bg = backdropLatestRef.current;
    const params = latestParamsRef.current;
    if (!canvas || !bg || !params || params.width < 2 || params.height < 2) {
      commitWebglOk(false);
      return;
    }
    if (params.mode === "glyph" && (!params.glyphCanvas || !params.glyphHeightCanvas)) {
      commitWebglOk(false);
      return;
    }
    const pMax = parallaxMaxRef.current;
    const cNorm = cursorNormRef?.current;
    const pDx = pMax && cNorm ? (cNorm[0] - 0.5) * 2 * pMax[0] : 0;
    const pDy = pMax && cNorm ? (cNorm[1] - 0.5) * 2 * pMax[1] : 0;
    const ok = opticalGlassEngine.renderToCanvas(canvas, {
      ...params,
      centerX: params.centerX + pDx,
      centerY: params.centerY + pDy,
      backdrop: {
        ...bg,
        frameToken: bg.kind === "video" || bg.live ? videoFrameTokenRef.current ?? 0 : bg.frameToken,
      },
    });
    commitWebglOk(ok);
  }, [commitWebglOk, cursorNormRef, forceCssFallback, videoFrameTokenRef]);

  // Synchronous zero-lag render on any parameter, position, size, or backdrop media change
  useLayoutEffect(() => {
    drawFrame();
  }, [
    drawFrame,
    backdrop?.key,
    backdrop?.drawLeft,
    backdrop?.drawTop,
    backdrop?.drawW,
    backdrop?.drawH,
    mediaRev,
    viewportW,
    viewportH,
    centerX,
    centerY,
    effW,
    effH,
    rotationDeg,
    scaleY,
    blurPx,
    refrScalePx,
    bevelPx,
    radiusPx,
    chromaPx,
    frostOpacity,
    tintRgb[0],
    tintRgb[1],
    tintRgb[2],
    noiseOpacity,
    specularIntensity,
    borderOpacity,
    brightness,
    contrast,
    saturation,
    opacity,
    glyphMaps?.key,
    pigmentRgba?.[0],
    pigmentRgba?.[1],
    pigmentRgba?.[2],
    pigmentRgba?.[3],
  ]);

  // Subscribe to shared video/parallax frame ticks (zero React re-renders!)
  useEffect(() => {
    if (forceCssFallback) return;
    return registerFrameTick(drawFrame);
  }, [forceCssFallback, registerFrameTick, drawFrame]);

  const [tr, tg, tb] = tintRgb;
  const satPct = Math.round(saturation * 100);
  const brightPct = Math.round(brightness * 100);
  const contrastPct = Math.round(contrast * 100);
  const pureBackdrop = buildPureCssBackdrop(blurPx, satPct, brightPct, contrastPct, isThumbnail);

  const showCssFallback = forceCssFallback || !webglOk;
  const noiseUrl = showCssFallback && !isThumbnail && noiseOpacity > 0.005 ? getFrostedNoiseDataUrl() : "";
  const cssTintBg =
    frostOpacity > 0.002
      ? `linear-gradient(135deg, rgba(${tr},${tg},${tb},${Math.min(0.9, frostOpacity * 1.18).toFixed(3)}) 0%, rgba(${tr},${tg},${tb},${frostOpacity.toFixed(3)}) 52%, rgba(${tr},${tg},${tb},${(frostOpacity * 0.72).toFixed(3)}) 100%)`
      : "transparent";

  if (mode === "glyph") {
    const maskUrl = showCssFallback && glyphMaps ? glyphMaps.getMaskUrl() : "";
    const specUrl = showCssFallback && glyphMaps ? glyphMaps.getSpecularUrl() : "";
    const maskCss: React.CSSProperties = maskUrl
      ? {
          maskImage: `url("${maskUrl}")`,
          WebkitMaskImage: `url("${maskUrl}")`,
          maskSize: "100% 100%",
          WebkitMaskSize: "100% 100%",
          maskRepeat: "no-repeat",
          WebkitMaskRepeat: "no-repeat",
        }
      : {};
    const webglShadowFilter =
      glyphDropShadowPx > 0
        ? `drop-shadow(0 2px ${Math.max(1, Math.round(glyphDropShadowPx * 0.45))}px rgba(0,0,0,${(0.55 * opacity).toFixed(3)}))`
        : undefined;

    return (
      <div ref={hostRef} className="pointer-events-none absolute inset-0">
        {!forceCssFallback && (
          <canvas
            ref={canvasRef}
            className="pointer-events-none absolute inset-0 w-full h-full"
            style={{
              display: webglOk ? "block" : "none",
              filter: webglShadowFilter,
            }}
          />
        )}
        {showCssFallback && maskUrl && (
          <>
            {glyphDropShadowPx > 0 && (
              <div
                className="pointer-events-none absolute inset-0"
                style={{
                  transform: "translateY(2px)",
                  opacity: opacity * 0.5,
                  filter: `blur(${Math.max(1, Math.round(glyphDropShadowPx * 0.42))}px)`,
                }}
              >
                <div className="w-full h-full" style={{ background: "#000000", ...maskCss }} />
              </div>
            )}
            <div
              className="pointer-events-none absolute inset-0"
              style={{
                opacity,
                backdropFilter: pureBackdrop,
                WebkitBackdropFilter: pureBackdrop,
                background: cssTintBg,
                ...maskCss,
              }}
            />
            {noiseUrl && (
              <div
                className="pointer-events-none absolute inset-0"
                style={{
                  opacity: opacity * noiseOpacity,
                  backgroundImage: `url("${noiseUrl}")`,
                  backgroundRepeat: "repeat",
                  backgroundSize: "128px 128px",
                  mixBlendMode: "overlay",
                  ...maskCss,
                }}
              />
            )}
            {pigmentRgba && pigmentRgba[3] > 0.002 && (
              <div
                className="pointer-events-none absolute inset-0"
                style={{
                  opacity,
                  background: `rgba(${pigmentRgba[0]},${pigmentRgba[1]},${pigmentRgba[2]},${pigmentRgba[3].toFixed(3)})`,
                  ...maskCss,
                }}
              />
            )}
            {specUrl && (specularIntensity > 0.01 || borderOpacity > 0.01) && (
              <img
                src={specUrl}
                alt=""
                draggable={false}
                className="pointer-events-none absolute inset-0 w-full h-full"
                style={{
                  opacity: opacity * Math.min(1, specularIntensity * 1.15 + borderOpacity * 0.45),
                  mixBlendMode: "screen",
                }}
              />
            )}
          </>
        )}
      </div>
    );
  }

  // PLATE MODE (GLASS, TEXT PILL, GROUP, IMAGE, VIDEO)
  const specRim = Math.min(0.85, 0.14 + specularIntensity * 0.46);
  const hasChroma = chromaPx > 0.05;
  const shadowCss =
    glass.shadowBlur > 0
      ? `0 ${(glass.shadowBlur * 0.42).toFixed(0)}px ${glass.shadowBlur}px rgba(0,0,0,${(0.48 * opacity).toFixed(3)})`
      : undefined;

  return (
    <div
      ref={hostRef}
      className="pointer-events-none absolute inset-0"
      style={{
        borderRadius: `${radiusPx}px`,
        boxShadow: shadowCss,
      }}
    >
      {!forceCssFallback && (
        <canvas
          ref={canvasRef}
          className="pointer-events-none absolute inset-0 w-full h-full"
          style={{
            borderRadius: `${radiusPx}px`,
            display: webglOk ? "block" : "none",
          }}
        />
      )}

      {showCssFallback && (
        <div
          className="pointer-events-none absolute inset-0 overflow-hidden"
          style={{
            opacity,
            borderRadius: `${radiusPx}px`,
            backdropFilter: pureBackdrop,
            WebkitBackdropFilter: pureBackdrop,
            background: cssTintBg,
            border: `1px solid rgba(${tr},${tg},${tb},${Math.min(0.75, borderOpacity + specularIntensity * 0.12).toFixed(3)})`,
            boxShadow: hasChroma
              ? `inset 0 1px ${(1.5 + specularIntensity * 4).toFixed(1)}px rgba(255,255,255,${specRim.toFixed(2)}), inset 0 -1px 2px rgba(0,0,0,0.22), inset -${chromaPx}px 0 ${(chromaPx * 2.5).toFixed(1)}px rgba(255,30,66,0.22), inset ${chromaPx}px 0 ${(chromaPx * 2.5).toFixed(1)}px rgba(0,210,255,0.24)`
              : `inset 0 1px ${(1.5 + specularIntensity * 4).toFixed(1)}px rgba(255,255,255,${specRim.toFixed(2)}), inset 0 -1px 2px rgba(0,0,0,0.22)`,
          }}
        >
          {specularIntensity > 0.01 && (
            <div
              className="pointer-events-none absolute inset-0"
              style={{
                borderRadius: "inherit",
                background: `linear-gradient(135deg, rgba(255,255,255,${(specularIntensity * 0.34).toFixed(3)}) 0%, rgba(255,255,255,${(specularIntensity * 0.09).toFixed(3)}) 34%, rgba(255,255,255,0) 56%, rgba(255,255,255,${(specularIntensity * 0.12).toFixed(3)}) 100%)`,
              }}
            />
          )}
          {noiseUrl && (
            <div
              className="pointer-events-none absolute inset-0"
              style={{
                borderRadius: "inherit",
                opacity: noiseOpacity,
                backgroundImage: `url("${noiseUrl}")`,
                backgroundRepeat: "repeat",
                backgroundSize: "128px 128px",
                mixBlendMode: "overlay",
              }}
            />
          )}
        </div>
      )}
    </div>
  );
};
