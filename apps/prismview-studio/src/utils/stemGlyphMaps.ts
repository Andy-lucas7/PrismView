import type { GlyphGlassMaps } from "./glassLens";
import { profileFromCanvas, warpCanvasWithProfile } from "./stemStretch";

const CACHE_LIMIT = 8;
const cache = new Map<string, GlyphGlassMaps>();

/** Mesma conta do fallback CSS de glassLens (Sobel do heightfield → normal empacotada), sobre os canvases já deformados. */
function packFallback(mask: HTMLCanvasElement, height: HTMLCanvasElement, sigmaPx: number): Uint8ClampedArray | null {
  const mctx = mask.getContext("2d", { willReadFrequently: true });
  const hctx = height.getContext("2d", { willReadFrequently: true });
  if (!mctx || !hctx) return null;
  const cw = mask.width;
  const ch = mask.height;
  const sharp = mctx.getImageData(0, 0, cw, ch).data;
  const hData = hctx.getImageData(0, 0, cw, ch).data;
  const out = new Uint8ClampedArray(cw * ch * 4);
  const sampleH = (x: number, y: number): number => {
    const cx = x < 0 ? 0 : x >= cw ? cw - 1 : x;
    const cy = y < 0 ? 0 : y >= ch ? ch - 1 : y;
    return hData[(cy * cw + cx) * 4 + 3] / 255;
  };
  for (let y = 0; y < ch; y++) {
    for (let x = 0; x < cw; x++) {
      const idx = (y * cw + x) * 4;
      const sharpA = sharp[idx + 3];
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
      const gx = tl + 2 * l + bl - (tr + 2 * r + br);
      const gy = tl + 2 * t + tr - (bl + 2 * b + br);
      const slope = Math.hypot(gx, gy);
      if (slope > 0.004) {
        const gain = Math.min(1, slope * Math.max(2.2, sigmaPx * 0.9));
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
  return out;
}

/**
 * Versão "alongada por haste" dos mapas de vidro do glifo: a máscara nítida e o heightfield são
 * deformados com o MESMO perfil (calculado da máscara), então bisel/refração acompanham o
 * glifo novo. A altura CSS cresce de acordo; o chamador passa `scaleY = 1` ao vidro.
 */
export function warpGlyphMaps(maps: GlyphGlassMaps, stretch: number, fontPx: number): GlyphGlassMaps {
  if (stretch <= 1.01) return maps;
  const key = `${maps.key}|stem|${stretch.toFixed(3)}|${Math.round(fontPx)}`;
  const hit = cache.get(key);
  if (hit) {
    cache.delete(key);
    cache.set(key, hit);
    return hit;
  }
  const renderScale = maps.webglCanvas.width / Math.max(1, maps.width);
  const prof = profileFromCanvas(maps.webglCanvas, fontPx * renderScale);
  if (!prof) return maps;
  const m = warpCanvasWithProfile(maps.webglCanvas, prof, stretch);
  const h = warpCanvasWithProfile(maps.heightCanvas, prof, stretch);
  if (!m || !h) return maps;

  let packed: Uint8ClampedArray | null | undefined;
  const getPacked = () => (packed === undefined ? (packed = packFallback(m.canvas, h.canvas, maps.heightSigmaPx)) : packed);
  let maskUrl: string | null = null;
  let specUrl: string | null = null;
  const cw = m.canvas.width;
  const ch = m.canvas.height;
  const paint = (fill: (p: Uint8ClampedArray, o: Uint8ClampedArray, i: number) => void): string => {
    const p = getPacked();
    const c = document.createElement("canvas");
    c.width = cw;
    c.height = ch;
    const cx = c.getContext("2d");
    if (!p || !cx) return "";
    const img = cx.createImageData(cw, ch);
    for (let i = 0; i < p.length; i += 4) fill(p, img.data, i);
    cx.putImageData(img, 0, 0);
    return c.toDataURL("image/png");
  };

  const out: GlyphGlassMaps = {
    key,
    width: maps.width,
    height: Math.round(maps.height * m.ratio),
    webglCanvas: m.canvas,
    heightCanvas: h.canvas,
    heightSigmaPx: maps.heightSigmaPx,
    padPx: maps.padPx,
    getMaskUrl: () =>
      (maskUrl ??= paint((p, o, i) => {
        const a = p[i + 3];
        if (a > 0) {
          o[i] = 255;
          o[i + 1] = 255;
          o[i + 2] = 255;
          o[i + 3] = a;
        }
      })),
    getSpecularUrl: () =>
      (specUrl ??= paint((p, o, i) => {
        const a = p[i + 3] / 255;
        const rim = p[i + 2] / 255;
        if (!(a > 0.02 && rim > 0.03)) return;
        const nx = (p[i] - 128) / 126;
        const ny = (p[i + 1] - 128) / 126;
        const ndotl = nx * -0.65 + ny * -0.76;
        if (ndotl > 0) {
          const spec = Math.min(1, Math.pow(ndotl, 1.3) * rim * 1.4 + rim * 0.2);
          o[i] = 255;
          o[i + 1] = 255;
          o[i + 2] = 255;
          o[i + 3] = Math.round(spec * a * 235);
        } else {
          const back = Math.min(1, Math.abs(ndotl) * rim * 0.45);
          o[i] = 225;
          o[i + 1] = 232;
          o[i + 2] = 245;
          o[i + 3] = Math.round(back * a * 135);
        }
      })),
  };
  cache.set(key, out);
  if (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
  return out;
}
