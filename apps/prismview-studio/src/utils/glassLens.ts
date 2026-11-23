// Procedural Optical Glyph Heightfield Normal Map + Micro-Grain Generator
// Optimized with true LRU caching, bounded 2D dimensions, consistent letter-spacing measurement,
// and single-canvas retention (zero base64 PNG overhead on WebGL2).

// 2.4: small entry cap + byte budget (a 1080x1080 RGBA canvas is ~4.6 MB; 48 of them was ~220 MB).
const GLYPH_CACHE_LIMIT = 8;
const GLYPH_CACHE_MAX_BYTES = 48 * 1024 * 1024;

export interface GlyphGlassMaps {
  key: string;
  width: number;
  height: number;
  /** Sharp glyph alpha mask (alpha channel). The GPU derives normals from this + `heightCanvas`. */
  webglCanvas: HTMLCanvasElement;
  /** Blurred glyph heightfield (alpha channel) used by the GPU Sobel pass. */
  heightCanvas: HTMLCanvasElement;
  /** Lazily generated PNG data URLs only if pure CSS fallback is ever needed */
  getMaskUrl: () => string;
  getSpecularUrl: () => string;
}

export interface GlyphGlassParams {
  text: string;
  fontStack: string;
  fontSize: number;
  fontWeight: number;
  letterSpacingEm: number;
  align: "left" | "center" | "right";
  bevelPx: number;
  fontRev: number;
}

const glyphMapsCache = new Map<string, GlyphGlassMaps>();
let cachedNoiseUrl: string | null = null;

function setLru<K, V>(map: Map<K, V>, key: K, val: V, limit: number) {
  if (map.has(key)) map.delete(key);
  map.set(key, val);
  if (map.size > limit) {
    const oldest = map.keys().next().value;
    if (oldest !== undefined) map.delete(oldest);
  }
}

/**
 * Generates a 128x128 tileable monochrome micro-grain PNG for CSS fallback frosted glass texture.
 */
export function getFrostedNoiseDataUrl(): string {
  if (cachedNoiseUrl) return cachedNoiseUrl;
  if (typeof document === "undefined") return "";
  const size = 128;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  const img = ctx.createImageData(size, size);
  const data = img.data;
  let seed = 0x51a7c3d1;
  for (let i = 0; i < data.length; i += 4) {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    const v = (seed >>> 24) & 0xff;
    data[i] = v;
    data[i + 1] = v;
    data[i + 2] = v;
    data[i + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  cachedNoiseUrl = canvas.toDataURL("image/png");
  return cachedNoiseUrl;
}

/**
 * Renders exact text glyphs onto an offscreen canvas (with full access to Windows, Google & imported FontFace fonts)
 * and builds a packed RGBA normal/alpha canvas for 120fps WebGL2 optical glass refraction inside every letter stroke.
 */
export function getGlyphGlassMaps(params: GlyphGlassParams): GlyphGlassMaps | null {
  if (typeof document === "undefined") return null;
  const text = params.text || " ";
  const fontSize = Math.max(10, Math.min(360, Math.round(params.fontSize)));
  const bevel = Math.max(2, Math.min(32, Math.round(params.bevelPx * 0.48)));

  const key = [
    text,
    params.fontStack,
    fontSize,
    params.fontWeight,
    params.letterSpacingEm.toFixed(3),
    params.align,
    bevel,
    params.fontRev,
  ].join("|");

  const hit = glyphMapsCache.get(key);
  if (hit) {
    // True LRU refresh on cache hit
    glyphMapsCache.delete(key);
    glyphMapsCache.set(key, hit);
    return hit;
  }

  const lines = text.split("\n");
  const lineH = Math.ceil(fontSize * 1.06);
  const pad = Math.max(12, Math.ceil(fontSize * 0.14));

  const fontDecl = `${params.fontWeight} ${fontSize}px ${params.fontStack}`;
  const measureCanvas = document.createElement("canvas");
  const mctx = measureCanvas.getContext("2d");
  if (!mctx) return null;
  mctx.font = fontDecl;

  const letterSpacingPx = params.letterSpacingEm * fontSize;
  const usePerCharSpacing = Math.abs(letterSpacingPx) >= 0.25;

  // Measure using the exact same glyph stepping logic as drawLine so kerning/spacing never clips
  const measureLine = (line: string): number => {
    if (!line) return fontSize * 0.3;
    if (!usePerCharSpacing) {
      return mctx.measureText(line).width;
    }
    const chars = Array.from(line);
    let total = 0;
    for (let i = 0; i < chars.length; i++) {
      total += mctx.measureText(chars[i]).width;
      if (i < chars.length - 1) total += letterSpacingPx;
    }
    return Math.max(0, total);
  };

  let maxLineW = 0;
  for (const line of lines) {
    maxLineW = Math.max(maxLineW, measureLine(line));
  }

  const cssW = Math.max(24, Math.ceil(maxLineW + pad * 2));
  const cssH = Math.max(24, Math.ceil(lines.length * lineH + pad * 2));

  // Cap BOTH width and height so tall multi-line text never stalls CPU Sobel pass
  const maxDim = Math.max(cssW, cssH);
  const renderScale = maxDim > 1080 ? 1080 / maxDim : 1;
  const cw = Math.max(16, Math.round(cssW * renderScale));
  const ch = Math.max(16, Math.round(cssH * renderScale));

  const drawTextOnto = (ctx: CanvasRenderingContext2D, blurPx: number) => {
    ctx.clearRect(0, 0, cw, ch);
    ctx.save();
    ctx.scale(renderScale, renderScale);
    if (blurPx > 0) {
      ctx.filter = `blur(${blurPx}px)`;
    }
    ctx.font = fontDecl;
    ctx.fillStyle = "#FFFFFF";
    ctx.textBaseline = "middle";
    const drawLine = (line: string, y: number) => {
      const lw = measureLine(line);
      let x = pad;
      if (params.align === "center") x = (cssW - lw) * 0.5;
      else if (params.align === "right") x = cssW - pad - lw;

      ctx.textAlign = "left";
      if (!usePerCharSpacing) {
        ctx.fillText(line, x, y);
      } else {
        let cursor = x;
        for (const chStr of Array.from(line)) {
          ctx.fillText(chStr, cursor, y);
          cursor += ctx.measureText(chStr).width + letterSpacingPx;
        }
      }
    };
    lines.forEach((line, i) => {
      drawLine(line, pad + lineH * (i + 0.5));
    });
    ctx.restore();
  };

  // 4.3: NO CPU Sobel on the hot path. Two plain canvases are kept; the WebGL engine derives the
  // packed normal map (R=nx, G=ny, B=edgeSlope, A=sharpAlpha) on the GPU. The CPU pass below only
  // runs lazily for the CSS fallback.

  // 1. Sharp alpha mask canvas
  const maskCanvas = document.createElement("canvas");
  maskCanvas.width = cw;
  maskCanvas.height = ch;
  const maskCtx = maskCanvas.getContext("2d");
  if (!maskCtx) return null;
  drawTextOnto(maskCtx, 0);

  // 2. Blurred heightfield canvas
  const heightCanvas = document.createElement("canvas");
  heightCanvas.width = cw;
  heightCanvas.height = ch;
  const heightCtx = heightCanvas.getContext("2d");
  if (!heightCtx) return null;
  drawTextOnto(heightCtx, Math.max(2, bevel * 0.55));

  let lazyMaskUrl: string | null = null;
  let lazySpecUrl: string | null = null;
  let lazyPacked: Uint8ClampedArray | null = null;

  // CPU fallback only: same Sobel the shader runs, computed on demand and cached.
  const getPackedCpu = (): Uint8ClampedArray => {
    if (lazyPacked) return lazyPacked;
    const sharpData = maskCtx.getImageData(0, 0, cw, ch).data;
    const hData = heightCtx.getImageData(0, 0, cw, ch).data;
    const out = new Uint8ClampedArray(cw * ch * 4);
    const sampleH = (x: number, y: number): number => {
      const cx = x < 0 ? 0 : x >= cw ? cw - 1 : x;
      const cy = y < 0 ? 0 : y >= ch ? ch - 1 : y;
      return hData[(cy * cw + cx) * 4 + 3] / 255;
    };
    for (let y = 0; y < ch; y++) {
      for (let x = 0; x < cw; x++) {
        const idx = (y * cw + x) * 4;
        const sharpA = sharpData[idx + 3];
        if (sharpA < 3) {
          out[idx] = 128;
          out[idx + 1] = 128;
          continue;
        }
        const tl = sampleH(x - 1, y - 1);
        const t = sampleH(x, y - 1);
        const tr = sampleH(x + 1, y - 1);
        const l = sampleH(x - 1, y);
        const r = sampleH(x + 1, y);
        const bl = sampleH(x - 1, y + 1);
        const b = sampleH(x, y + 1);
        const br = sampleH(x + 1, y + 1);
        // Outward gradient (points from inside glyph toward outside edge)
        const gx = (tl + 2 * l + bl) - (tr + 2 * r + br);
        const gy = (tl + 2 * t + tr) - (bl + 2 * b + br);
        const slope = Math.hypot(gx, gy);
        if (slope > 0.004) {
          const gain = Math.min(1, slope * 2.2);
          out[idx] = Math.round(128 + (gx / slope) * gain * 126);
          out[idx + 1] = Math.round(128 + (gy / slope) * gain * 126);
          out[idx + 2] = Math.round(gain * 255);
        } else {
          out[idx] = 128;
          out[idx + 1] = 128;
        }
        out[idx + 3] = sharpA;
      }
    }
    lazyPacked = out;
    return out;
  };

  // Derive fallback PNGs strictly on demand
  const result: GlyphGlassMaps = {
    key,
    width: cssW,
    height: cssH,
    webglCanvas: maskCanvas,
    heightCanvas,
    getMaskUrl: () => {
      if (lazyMaskUrl !== null) return lazyMaskUrl;
      const pData = getPackedCpu();
      const mc = document.createElement("canvas");
      mc.width = cw;
      mc.height = ch;
      const mctx2 = mc.getContext("2d");
      if (!mctx2) {
        lazyMaskUrl = "";
        return lazyMaskUrl;
      }
      const mImg = mctx2.createImageData(cw, ch);
      const mOut = mImg.data;
      for (let i = 0; i < pData.length; i += 4) {
        const a = pData[i + 3];
        if (a > 0) {
          mOut[i] = 255;
          mOut[i + 1] = 255;
          mOut[i + 2] = 255;
          mOut[i + 3] = a;
        }
      }
      mctx2.putImageData(mImg, 0, 0);
      lazyMaskUrl = mc.toDataURL("image/png");
      return lazyMaskUrl;
    },
    getSpecularUrl: () => {
      if (lazySpecUrl !== null) return lazySpecUrl;
      const pData = getPackedCpu();
      const specCanvas = document.createElement("canvas");
      specCanvas.width = cw;
      specCanvas.height = ch;
      const sctx = specCanvas.getContext("2d");
      if (!sctx) {
        lazySpecUrl = "";
        return lazySpecUrl;
      }
      const sImg = sctx.createImageData(cw, ch);
      const sOut = sImg.data;
      const lx = -0.65;
      const ly = -0.76;
      for (let i = 0; i < pData.length; i += 4) {
        const a = pData[i + 3] / 255;
        const rim = pData[i + 2] / 255;
        if (a > 0.02 && rim > 0.03) {
          const nx = (pData[i] - 128) / 126;
          const ny = (pData[i + 1] - 128) / 126;
          const ndotl = nx * lx + ny * ly;
          if (ndotl > 0) {
            const spec = Math.min(1, Math.pow(ndotl, 1.3) * rim * 1.4 + rim * 0.2);
            sOut[i] = 255;
            sOut[i + 1] = 255;
            sOut[i + 2] = 255;
            sOut[i + 3] = Math.round(spec * a * 235);
          } else {
            const back = Math.min(1, Math.abs(ndotl) * rim * 0.45);
            sOut[i] = 225;
            sOut[i + 1] = 232;
            sOut[i + 2] = 245;
            sOut[i + 3] = Math.round(back * a * 135);
          }
        }
      }
      sctx.putImageData(sImg, 0, 0);
      lazySpecUrl = specCanvas.toDataURL("image/png");
      return lazySpecUrl;
    },
  };

  setLru(glyphMapsCache, key, result, GLYPH_CACHE_LIMIT);
  // Evict oldest entries (never the one just inserted) until under the byte budget.
  let bytes = 0;
  glyphMapsCache.forEach((m) => {
    bytes += m.width * m.height * 4;
  });
  for (const k of glyphMapsCache.keys()) {
    if (bytes <= GLYPH_CACHE_MAX_BYTES || glyphMapsCache.size <= 1) break;
    if (k === key) continue;
    const m = glyphMapsCache.get(k);
    if (m) bytes -= m.width * m.height * 4;
    glyphMapsCache.delete(k);
  }
  return result;
}
