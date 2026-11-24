// Alongamento por haste (utils/stemStretch): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyStemWarpRGBA, buildStemWarp, estimateStrokePx, stemWeights } from "../src/utils/stemStretch.ts";

type Bmp = { w: number; h: number; a: Uint8ClampedArray };
const bmp = (w: number, h: number, fill: (x: number, y: number) => boolean): Bmp => {
  const a = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) if (fill(x, y)) a[(y * w + x) * 4 + 3] = 255;
  return { w, h, a };
};
// "H": duas hastes de 8 px + travessa de 8 px no meio. 60x100.
const H = () => bmp(60, 100, (x, y) => x < 8 || x >= 52 || (y >= 46 && y < 54));
const rowsWithInk = (b: Bmp, x: number) => {
  let n = 0;
  for (let y = 0; y < b.h; y++) if (b.a[(y * b.w + x) * 4 + 3] > 127) n++;
  return n;
};

test("haste estimada ≈ 8 px no H", () => {
  const b = H();
  assert.equal(estimateStrokePx(b.a, b.w, b.h, 26), 8);
});

test("travessa é protegida (peso ~0) e hastes esticam (peso ~1)", () => {
  const b = H();
  const w = stemWeights(b.a, b.w, b.h, 8);
  assert.ok(w[50] < 0.1, `travessa ${w[50]}`);
  assert.ok(w[20] > 0.95 && w[80] > 0.95, `haste ${w[20]} ${w[80]}`);
});

test("stretch 2: haste cresce, travessa mantém ~8 px", () => {
  const b = H();
  const warp = buildStemWarp(stemWeights(b.a, b.w, b.h, 8), 2);
  const out = applyStemWarpRGBA(b.a, b.w, warp);
  const o: Bmp = { w: b.w, h: warp.outRows, a: out };
  assert.ok(o.h > 192 && o.h < 208, `altura ${o.h} (alvo ~200)`);
  const stem = rowsWithInk(o, 3); // coluna só da haste
  assert.ok(stem >= o.h - 1, "haste contínua");
  const barRows = rowsWithInk(o, 30) ; // coluna só da travessa
  assert.ok(barRows >= 7 && barRows <= 11, `travessa ${barRows}px`);
});

test("traço só horizontal (travessão) não cresce", () => {
  const b = bmp(80, 40, (x, y) => y >= 16 && y < 24 && x >= 4 && x < 76);
  const stroke = estimateStrokePx(b.a, b.w, b.h, 10);
  assert.equal(stroke, 0);
  const warp = buildStemWarp(stemWeights(b.a, b.w, b.h, stroke), 2.4);
  assert.equal(warp.outRows, b.h);
});

test("haste pura (I) escala ~ stretch", () => {
  const b = bmp(8, 100, () => true);
  const warp = buildStemWarp(stemWeights(b.a, b.w, b.h, 8), 2);
  // bordas do quadro viram "sem tinta" no suavizador: perde um pouco, mas a maior parte dobra
  assert.ok(warp.outRows > 180, `altura ${warp.outRows}`);
});

test("mapa é monotônico e interpola sem estourar alfa", () => {
  const b = H();
  const warp = buildStemWarp(stemWeights(b.a, b.w, b.h, 8), 2.4);
  for (let y = 0; y < warp.srcRows; y++) assert.ok(warp.cum[y + 1] > warp.cum[y]);
  const out = applyStemWarpRGBA(b.a, b.w, warp);
  for (let i = 3; i < out.length; i += 4) assert.ok(out[i] >= 0 && out[i] <= 255);
});
