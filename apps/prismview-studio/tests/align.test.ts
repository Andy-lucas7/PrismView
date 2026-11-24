// Testes puros da Etapa 8 (alinhamento, distribuição, igualar tamanho, espelhar, girar 90° e guias inteligentes): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import type { GlassEffect, LayerKind, SceneLayer } from "../src/types/prismBase.ts";
import {
  alignLayers,
  centerLayersOnScreen,
  collectLayerIdsByKindPure,
  distributeLayers,
  flipLayers,
  matchLayersSize,
  moveLayersZExtremePure,
  rotateLayersBy90,
  snapAxisInto,
  type SnapAxisBuffer,
} from "../src/utils/align.ts";

function makeGlass(enabled = false): GlassEffect {
  return {
    enabled,
    blurPx: 22,
    refractionScale: 16,
    chromaticAberration: 0,
    frostOpacity: 0.08,
    tintColor: "#FFFFFF",
    borderRadius: 24,
    borderOpacity: 0.16,
    shadowBlur: 28,
    saturation: 1.2,
    brightness: 1.06,
    contrast: 1.04,
    noiseOpacity: 0.04,
    bevelWidth: 22,
    specularIntensity: 0.45,
    textMode: "GLYPH",
  };
}

function box(id: string, x: number, y: number, w = 200, h = 100, kind: LayerKind = "GLASS"): SceneLayer {
  return {
    id,
    name: id,
    kind,
    x,
    y,
    width: w,
    height: h,
    rotation: 0,
    opacity: 1,
    blendMode: "normal",
    hidden: false,
    locked: false,
    parallaxX: 0,
    parallaxY: 0,
    glass: makeGlass(true),
    fx: {},
  };
}

test("alignLayers: 1 item alinha às bordas e centro da tela respeitando meia-largura/altura", () => {
  const layers = [box("a", 0.5, 0.5, 192, 108)]; // 10% de 1920x1080 → half = 0.05
  const left = alignLayers(layers, ["a"], "left", "screen", 1920, 1080);
  assert.equal(left[0].x, 0.05);

  const right = alignLayers(layers, ["a"], "right", "screen", 1920, 1080);
  assert.equal(right[0].x, 0.95);

  const bottom = alignLayers(layers, ["a"], "bottom", "screen", 1920, 1080);
  assert.equal(bottom[0].y, 0.95);
});

test("alignLayers: 2+ itens alinham entre os selecionados pela caixa envolvente", () => {
  const layers = [
    box("a", 0.2, 0.3, 192, 108), // left = 0.15, right = 0.25
    box("b", 0.7, 0.8, 384, 108), // halfW = 0.10 → left = 0.60, right = 0.80
  ];
  const alignedLeft = alignLayers(layers, ["a", "b"], "left", "selection", 1920, 1080);
  assert.equal(alignedLeft[0].x, 0.2); // left continua 0.15
  assert.equal(alignedLeft[1].x, 0.25); // 0.15 + halfW(0.10) = 0.25

  const alignedRight = alignLayers(layers, ["a", "b"], "right", "selection", 1920, 1080);
  assert.equal(alignedRight[0].x, 0.75); // 0.80 - 0.05
  assert.equal(alignedRight[1].x, 0.7);
});

test("centerLayersOnScreen: move conjunto rígido para o centro (0.5, 0.5) preservando distâncias relativas", () => {
  const layers = [box("a", 0.1, 0.2, 192, 108), box("b", 0.3, 0.4, 192, 108)];
  // BBox X: [0.05 .. 0.35] → mid = 0.20 → dx = +0.30; BBox Y: [0.15 .. 0.45] → mid = 0.30 → dy = +0.20
  const centered = centerLayersOnScreen(layers, ["a", "b"], 1920, 1080);
  assert.equal(centered[0].x, 0.4);
  assert.equal(centered[1].x, 0.6);
  assert.equal(centered[0].y, 0.4);
  assert.equal(centered[1].y, 0.6);
});

test("distributeLayers: espaça 3 itens uniformemente entre os extremos", () => {
  const layers = [
    box("a", 0.1, 0.5, 192, 100),
    box("mid", 0.22, 0.5, 192, 100),
    box("c", 0.9, 0.5, 192, 100),
  ];
  const dist = distributeLayers(layers, ["a", "mid", "c"], "horizontal", 1920, 1080);
  assert.equal(dist[0].x, 0.1);
  assert.equal(dist[1].x, 0.5);
  assert.equal(dist[2].x, 0.9);
});

test("matchLayersSize, rotateLayersBy90, flipLayers e Z extremo funcionam de forma pura", () => {
  const layers = [box("a", 0.2, 0.5, 100, 80), box("b", 0.6, 0.5, 400, 220)];

  const matched = matchLayersSize(layers, ["a", "b"], "both");
  assert.equal(matched[0].width, 400);
  assert.equal(matched[0].height, 220);

  const rotated = rotateLayersBy90(layers, ["a", "b"], 1);
  assert.equal(rotated[0].rotation, 90);
  const rotatedBack = rotateLayersBy90(rotated, ["a"], -1);
  assert.equal(rotatedBack[0].rotation, 0);

  const flipped = flipLayers(layers, ["a", "b"], "horizontal");
  assert.equal(flipped[0].flipX, true);
  assert.equal(flipped[1].flipX, true);
  // Posições espelhadas em torno do pivô (0.2 + 0.6)/2 = 0.4
  assert.equal(flipped[0].x, 0.6);
  assert.equal(flipped[1].x, 0.2);

  const zList = [box("z0", 0.5, 0.5), box("z1", 0.5, 0.5), box("z2", 0.5, 0.5)];
  const front = moveLayersZExtremePure(zList, new Set(["z0"]), true);
  assert.deepEqual(front.map((l) => l.id), ["z1", "z2", "z0"]);
  const back = moveLayersZExtremePure(front, new Set(["z0"]), false);
  assert.deepEqual(back.map((l) => l.id), ["z0", "z1", "z2"]);

  assert.deepEqual(collectLayerIdsByKindPure(zList, "GLASS"), ["z0", "z1", "z2"]);
});

test("snapAxisInto: encaixa centro e bordas reutilizando buffer (zero alocação)", () => {
  const buf: SnapAxisBuffer = { snapped: 0, guide: null };
  const targets = [0, 0.5, 1, 0.24];

  // Centro próximo de 0.5
  const r1 = snapAxisInto(buf, 0.494, 0.1, targets, 0.012);
  assert.equal(r1, buf);
  assert.equal(buf.snapped, 0.5);
  assert.equal(buf.guide, 0.5);

  // Borda esquerda (center - 0.1 = 0.244) encaixa no alvo 0.24 → center = 0.34
  snapAxisInto(buf, 0.344, 0.1, targets, 0.012);
  assert.ok(Math.abs(buf.snapped - 0.34) < 1e-6);
  assert.equal(buf.guide, 0.24);

  // Borda direita (center + 0.1 = 0.236) encaixa no alvo 0.24 → center = 0.14
  snapAxisInto(buf, 0.136, 0.1, targets, 0.012);
  assert.ok(Math.abs(buf.snapped - 0.14) < 1e-6);
  assert.equal(buf.guide, 0.24);
});
