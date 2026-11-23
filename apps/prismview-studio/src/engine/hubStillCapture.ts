import { StillCache } from "./hubStillCache";
import type { PrismProject } from "../types/prism";

/** Stills compartilhados pelo Hub (≈ 40 imagens de 480 px). */
export const hubStills = new StillCache(40, (u) => URL.revokeObjectURL(u));

let scratch: HTMLCanvasElement | null = null;

/** Copia um canvas WebGL/<video> para webp pequeno. `null` se vazio, tainted ou sem frame. */
export function captureStill(el: HTMLCanvasElement | HTMLVideoElement, maxW = 480): Promise<string | null> {
  const sw = el instanceof HTMLVideoElement ? el.videoWidth : el.width;
  const sh = el instanceof HTMLVideoElement ? el.videoHeight : el.height;
  if (!(sw > 1 && sh > 1)) return Promise.resolve(null);
  if (el instanceof HTMLVideoElement && el.readyState < 2) return Promise.resolve(null);
  const w = Math.min(maxW, sw);
  const h = Math.max(1, Math.round((sh * w) / sw));
  const cv = scratch ?? (scratch = document.createElement("canvas"));
  cv.width = w;
  cv.height = h;
  const ctx = cv.getContext("2d");
  if (!ctx) return Promise.resolve(null);
  try {
    ctx.clearRect(0, 0, w, h);
    ctx.drawImage(el, 0, 0, w, h);
  } catch {
    return Promise.resolve(null);
  }
  return new Promise((resolve) => {
    try {
      cv.toBlob((b) => resolve(b ? URL.createObjectURL(b) : null), "image/webp", 0.82);
    } catch {
      resolve(null); // canvas tainted (mídia sem CORS)
    }
  });
}

/** Assinatura das partes que mudam o still (fonte do vídeo / config 3D). */
export function stillSignature(project: PrismProject): string {
  let h = 5381;
  const add = (s: string) => {
    for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  };
  const walk = (layers: PrismProject["layers"]) => {
    for (const l of layers) {
      if (l.kind === "VIDEO") add(`v:${l.id}:${l.video?.src ?? ""}`);
      else if (l.kind === "SCENE_3D") add(`s:${l.id}:${JSON.stringify(l.scene3d ?? null)}`);
      const kids = (l as { children?: PrismProject["layers"] }).children;
      if (kids) walk(kids);
    }
  };
  walk(project.layers);
  return (h >>> 0).toString(36);
}

/** Camadas que precisam de still (VIDEO e SCENE_3D, inclusive dentro de grupos). */
export function stillLayerIds(project: PrismProject): string[] {
  const out: string[] = [];
  const walk = (layers: PrismProject["layers"]) => {
    for (const l of layers) {
      if (l.kind === "VIDEO" || l.kind === "SCENE_3D") out.push(l.id);
      const kids = (l as { children?: PrismProject["layers"] }).children;
      if (kids) walk(kids);
    }
  };
  walk(project.layers);
  return out;
}
