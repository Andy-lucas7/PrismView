/**
 * Alongamento tipográfico por haste (estilo relógio Samsung/Xiaomi/Apple): em vez de esticar o
 * glifo inteiro, só as partes VERTICAIS crescem; barras horizontais (topo/base/travessa), arcos
 * e terminais mantêm espessura e curvatura.
 *
 * Método (independente de fonte, sem parsear outline): analisa o raster do texto linha a linha.
 * Em cada linha, corridas de tinta estreitas (≈ largura da haste) são "haste" e podem esticar;
 * corridas largas (barras, topo de arcos) são "traço horizontal" e ficam intactas. O peso por
 * linha é suavizado (contínuo) e integrado numa deformação vertical monotônica y → y'.
 * As funções puras (sem DOM) são testadas em tests/stemStretch.test.ts.
 */

const INK = 96; // alfa mínimo para contar como tinta

const smoothstep = (a: number, b: number, x: number): number => {
  const t = x <= a ? 0 : x >= b ? 1 : (x - a) / (b - a);
  return t * t * (3 - 2 * t);
};

/** Largura típica da haste (px): mediana das corridas curtas (≤ maxRunPx). 0 se não houver. */
export function estimateStrokePx(alpha: ArrayLike<number>, width: number, height: number, maxRunPx: number, stride = 4, off = 3): number {
  const hist = new Uint32Array(Math.max(2, Math.ceil(maxRunPx) + 2));
  let total = 0;
  for (let y = 0; y < height; y++) {
    let run = 0;
    const base = y * width * stride;
    for (let x = 0; x <= width; x++) {
      const ink = x < width && alpha[base + x * stride + off] >= INK;
      if (ink) {
        run++;
      } else if (run > 0) {
        if (run < hist.length - 1 && run <= maxRunPx) {
          hist[run]++;
          total++;
        }
        run = 0;
      }
    }
  }
  if (total === 0) return 0;
  let acc = 0;
  for (let r = 1; r < hist.length; r++) {
    acc += hist[r];
    if (acc * 2 >= total) return r;
  }
  return 0;
}

/** Peso de alongamento por linha (0 = proteger, 1 = esticar), já suavizado. */
export function stemWeights(alpha: ArrayLike<number>, width: number, height: number, strokePx: number, stride = 4, off = 3): Float32Array {
  const w = new Float32Array(height);
  if (strokePx <= 0) return w;
  const lo = strokePx * 1.3;
  const hi = strokePx * 2.2;
  for (let y = 0; y < height; y++) {
    let ink = 0;
    let stem = 0;
    let run = 0;
    const base = y * width * stride;
    for (let x = 0; x <= width; x++) {
      const on = x < width && alpha[base + x * stride + off] >= INK;
      if (on) {
        run++;
      } else if (run > 0) {
        ink += run;
        stem += run * (1 - smoothstep(lo, hi, run));
        run = 0;
      }
    }
    w[y] = ink > 0 ? smoothstep(0.25, 0.75, stem / ink) : 0;
  }
  // Dois suavizadores: o estreito preserva a espessura das barras; o largo tira o "serrilhado"
  // onde o peso oscila (curvas). O mínimo dos dois protege as barras sem deixar degraus nos arcos.
  const narrow = gaussRows(w, Math.max(0.7, strokePx * 0.12));
  const wide = gaussRows(w, Math.max(1.4, strokePx * 0.42));
  for (let y = 0; y < height; y++) narrow[y] = Math.min(narrow[y], wide[y]);
  return narrow;
}

function gaussRows(w: Float32Array, sigma: number): Float32Array {
  const height = w.length;
  const rad = Math.ceil(sigma * 3);
  const k = new Float32Array(rad * 2 + 1);
  let sum = 0;
  for (let i = -rad; i <= rad; i++) {
    const v = Math.exp(-(i * i) / (2 * sigma * sigma));
    k[i + rad] = v;
    sum += v;
  }
  const out = new Float32Array(height);
  for (let y = 0; y < height; y++) {
    let a = 0;
    for (let i = -rad; i <= rad; i++) {
      const yy = y + i;
      if (yy < 0 || yy >= height) continue; // fora do quadro = sem tinta (peso 0)
      a += w[yy] * k[i + rad];
    }
    out[y] = a / sum;
  }
  return out;
}

export interface StemWarp {
  srcRows: number;
  outRows: number;
  /** cum[y] = posição de saída do topo da linha y de origem (cum[srcRows] = outRows exato, fracionário). */
  cum: Float32Array;
}

/**
 * Deformação monotônica: cada linha de origem ocupa 1 + K·peso linhas de saída. K compensa as
 * zonas protegidas (barras/arcos) para que a ALTURA TOTAL da tinta fique perto de `stretch`×,
 * com teto (`MAX_BOOST`) para não distorcer demais as hastes.
 */
export const MAX_BOOST = 1.9;
export function buildStemWarp(weights: Float32Array, stretch: number): StemWarp {
  const n = weights.length;
  const cum = new Float32Array(n + 1);
  const extra = Math.max(0, stretch - 1);
  let first = -1;
  let last = -1;
  let wSum = 0;
  for (let y = 0; y < n; y++) {
    if (weights[y] > 0.002) {
      if (first < 0) first = y;
      last = y;
    }
  }
  if (first >= 0) for (let y = first; y <= last; y++) wSum += weights[y];
  const boost = wSum > 1e-3 ? Math.min(MAX_BOOST, Math.max(1, (last - first + 1) / wSum)) : 1;
  const k = extra * boost;
  for (let y = 0; y < n; y++) cum[y + 1] = cum[y] + 1 + k * weights[y];
  return { srcRows: n, outRows: Math.max(1, Math.round(cum[n])), cum };
}

/** Reamostra RGBA com interpolação linear vertical (largura inalterada). */
export function applyStemWarpRGBA(src: ArrayLike<number>, width: number, warp: StemWarp) {
  const { srcRows, outRows, cum } = warp;
  const out = new Uint8ClampedArray(new ArrayBuffer(width * outRows * 4));
  const scaleOut = cum[srcRows] / outRows; // normaliza o arredondamento da altura final
  let y = 0;
  const rowBytes = width * 4;
  for (let yo = 0; yo < outRows; yo++) {
    const target = (yo + 0.5) * scaleOut;
    while (y < srcRows - 1 && cum[y + 1] <= target) y++;
    const span = cum[y + 1] - cum[y] || 1;
    const cs = y + (target - cum[y]) / span; // coordenada contínua na origem (topo da linha = inteiro)
    const c = cs - 0.5; // centros de linha
    const y0 = Math.max(0, Math.min(srcRows - 1, Math.floor(c)));
    const y1 = Math.min(srcRows - 1, y0 + 1);
    const f = Math.max(0, Math.min(1, c - y0));
    const o0 = y0 * rowBytes;
    const o1 = y1 * rowBytes;
    const od = yo * rowBytes;
    for (let i = 0; i < rowBytes; i++) out[od + i] = src[o0 + i] * (1 - f) + src[o1 + i] * f;
  }
  return out;
}

/* ───────────────────────── canvas ───────────────────────── */

export interface StemWarpedCanvas {
  canvas: HTMLCanvasElement;
  /** altura de saída / altura de origem (≥ 1) */
  ratio: number;
}

function ctx2d(c: HTMLCanvasElement): CanvasRenderingContext2D | null {
  return c.getContext("2d", { willReadFrequently: true });
}

/** Perfil de peso a partir de um canvas de máscara (alfa) + `fontPx` na escala do próprio canvas. */
export function profileFromCanvas(mask: HTMLCanvasElement, fontPx: number): Float32Array | null {
  const ctx = ctx2d(mask);
  if (!ctx || mask.width < 2 || mask.height < 2) return null;
  const data = ctx.getImageData(0, 0, mask.width, mask.height).data;
  const stroke = estimateStrokePx(data, mask.width, mask.height, Math.max(2, fontPx * 0.26));
  return stemWeights(data, mask.width, mask.height, stroke);
}

/** Aplica um mesmo perfil a vários canvases do mesmo tamanho (ex.: máscara nítida + altura borrada). */
export function warpCanvasWithProfile(src: HTMLCanvasElement, weights: Float32Array, stretch: number): StemWarpedCanvas | null {
  const sctx = ctx2d(src);
  if (!sctx || weights.length !== src.height) return null;
  const warp = buildStemWarp(weights, stretch);
  const img = sctx.getImageData(0, 0, src.width, src.height);
  const outData = applyStemWarpRGBA(img.data, src.width, warp);
  const dst = document.createElement("canvas");
  dst.width = src.width;
  dst.height = warp.outRows;
  const dctx = ctx2d(dst);
  if (!dctx) return null;
  dctx.putImageData(new ImageData(outData, src.width, warp.outRows), 0, 0);
  return { canvas: dst, ratio: warp.outRows / src.height };
}
