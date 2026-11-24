// Testes puros da Etapa 5 (multiseleção, "misto", patch em lote e colar estilo): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import type { GlassEffect, LayerKind, SceneLayer } from "../src/types/prismBase.ts";
import {
  applyLayerStyle,
  applyPatchToMany,
  collectGroupLeafLayers,
  extractLayerStyle,
  formatMultiSelectionSummary,
  mergeProps,
  patchGroupDescendants,
  resolveSelectedLayers,
  summarizeGroupKinds,
  transformTextCase,
} from "../src/utils/multiSelect.ts";

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

function makeLayer(id: string, kind: LayerKind, overrides: Partial<SceneLayer> = {}): SceneLayer {
  const base: SceneLayer = {
    id,
    name: `Camada ${id}`,
    kind,
    x: 0.5,
    y: 0.5,
    width: 320,
    height: 140,
    rotation: 0,
    opacity: 1,
    blendMode: "normal",
    hidden: false,
    locked: false,
    parallaxX: 0,
    parallaxY: 0,
    glass: makeGlass(kind === "GLASS"),
    fx: {},
  };
  if (kind === "TEXT") {
    base.text = {
      content: "18:45",
      fontFamily: "Outfit",
      fontPath: "",
      fontSize: 96,
      fontWeight: 600,
      letterSpacing: 0,
      align: "center",
      color: "#FFFFFF",
      colorAlpha: 1,
      stemStretch: 1,
      shadowBlur: 12,
    };
  } else if (kind === "IMAGE") {
    base.image = { src: "C:/a.png", objectFit: "contain", borderRadius: 12, sourceMode: "FILE" };
  } else if (kind === "SVG") {
    base.svg = { iconName: "prism", customSvg: "", color: "#FFFFFF", colorAlpha: 1, strokeWidth: 1.8 };
  }
  return { ...base, ...overrides };
}

test("mergeProps: 2 camadas do mesmo tipo identificam iguais e 'misto' corretamente", () => {
  const t1 = makeLayer("t1", "TEXT");
  const t2 = makeLayer("t2", "TEXT", {
    opacity: 0.5,
    text: {
      ...t1.text!,
      fontSize: 140,
      color: "#ff0055",
      fontFamily: "Outfit",
    },
  });

  const merged = mergeProps([t1, t2]);
  assert.equal(merged.count, 2);
  assert.equal(merged.sameKind, "TEXT");
  assert.equal(merged.common.opacity.mixed, true);
  assert.equal(merged.common.blendMode.mixed, false);
  assert.equal(merged.common.blendMode.value, "normal");
  assert.ok(merged.text);
  assert.equal(merged.text.fontFamily.mixed, false);
  assert.equal(merged.text.fontFamily.value, "Outfit");
  assert.equal(merged.text.fontSize.mixed, true);
  assert.equal(merged.text.color.mixed, true);
  assert.equal(merged.text.fontWeight.mixed, false);
  assert.equal(formatMultiSelectionSummary(merged), "2 itens · Texto");
});

test("mergeProps: tipos diferentes expõem apenas propriedades comuns (sameKind = null)", () => {
  const t = makeLayer("t1", "TEXT", { opacity: 0.8, rotation: 15 });
  const img = makeLayer("i1", "IMAGE", { opacity: 0.8, rotation: 0 });
  const merged = mergeProps([t, img]);

  assert.equal(merged.sameKind, null);
  assert.deepEqual(merged.kinds, ["TEXT", "IMAGE"]);
  assert.equal(merged.text, undefined);
  assert.equal(merged.image, undefined);
  assert.equal(merged.common.opacity.mixed, false);
  assert.equal(merged.common.opacity.value, 0.8);
  assert.equal(merged.common.rotation.mixed, true);
  assert.equal(merged.allSupportFullGlass, true);
  assert.equal(formatMultiSelectionSummary(merged), "2 itens · Tipos mistos");
});

test("applyPatchToMany: edita todos selecionados (inclusive em grupos) preservando referências intactas", () => {
  const keep = makeLayer("keep", "IMAGE");
  const childA = makeLayer("c1", "TEXT");
  const childB = makeLayer("c2", "TEXT");
  const group = makeLayer("g1", "GROUP", { children: [childA, childB] });
  const topText = makeLayer("t1", "TEXT");
  const tree = [keep, group, topText];

  const next = applyPatchToMany(tree, new Set(["c1", "t1"]), (l) => ({
    ...l,
    opacity: 0.42,
    text: l.text ? { ...l.text, fontSize: 128 } : l.text,
  }));

  assert.notEqual(next, tree);
  // Referências intocadas são preservadas (zero re-render desnecessário em MemoLayerNode)
  assert.equal(next[0], keep);
  assert.equal(next[1].children![1], childB);
  // Selecionados receberam o patch juntos
  const resolved = resolveSelectedLayers(next, ["c1", "t1"]);
  assert.equal(resolved.length, 2);
  assert.equal(resolved[0].opacity, 0.42);
  assert.equal(resolved[0].text?.fontSize, 128);
  assert.equal(resolved[1].opacity, 0.42);
  assert.equal(resolved[1].text?.fontSize, 128);
});

test("extractLayerStyle / applyLayerStyle: cola estilo visual sem sobrescrever conteúdo ou posição", () => {
  const src = makeLayer("s", "TEXT", {
    x: 0.15,
    y: 0.2,
    opacity: 0.75,
    blendMode: "screen",
    glass: { ...makeGlass(true), blurPx: 38, refractionScale: 24 },
    text: {
      content: "Texto origem",
      fontFamily: "Inter",
      fontPath: "",
      fontSize: 120,
      fontWeight: 300,
      letterSpacing: 0.12,
      align: "right",
      color: "#00D2FF",
      colorAlpha: 0.2,
      stemStretch: 1.4,
      shadowBlur: 20,
    },
  });
  const dst = makeLayer("d", "TEXT", { x: 0.8, y: 0.7 });

  const style = extractLayerStyle(src);
  const out = applyLayerStyle(dst, style);

  assert.equal(out.id, "d");
  assert.equal(out.x, 0.8);
  assert.equal(out.y, 0.7);
  assert.equal(out.text?.content, "18:45");
  assert.equal(out.opacity, 0.75);
  assert.equal(out.blendMode, "screen");
  assert.equal(out.glass.enabled, true);
  assert.equal(out.glass.blurPx, 38);
  assert.equal(out.text?.fontFamily, "Inter");
  assert.equal(out.text?.fontSize, 120);
  assert.equal(out.text?.color, "#00D2FF");
  assert.equal(out.text?.stemStretch, 1.4);
});

test("transformTextCase: altera texto literal e envolve fórmulas $...$ com tc() sem quebrar tokens", () => {
  assert.equal(transformTextCase("olá mundo", "upper"), "OLÁ MUNDO");
  assert.equal(transformTextCase("OLÁ MUNDO", "capitalize"), "Olá Mundo");
  assert.equal(
    transformTextCase("hoje: $df(EEEE, dd MMMM)$", "upper"),
    "HOJE: $tc(up, df(EEEE, dd MMMM))$"
  );
  // Trocar de upper para lower substitui tc(up, ...) sem aninhar tc(low, tc(up, ...))
  assert.equal(
    transformTextCase("$tc(up, df(EEEE))$", "lower"),
    "$tc(low, df(EEEE))$"
  );
});

test("collectGroupLeafLayers, summarizeGroupKinds e patchGroupDescendants: edição em conjunto em grupos", () => {
  const t1 = makeLayer("t1", "TEXT", { opacity: 0.8 });
  const t2 = makeLayer("t2", "TEXT", { opacity: 0.4 });
  const img = makeLayer("i1", "IMAGE", { opacity: 1 });
  const subGroup = makeLayer("sub", "GROUP", { children: [t2, img] });
  const rootGroup = makeLayer("g1", "GROUP", { children: [t1, subGroup] });

  const leaves = collectGroupLeafLayers(rootGroup);
  assert.deepEqual(leaves.map((l) => l.id), ["t1", "t2", "i1"]);
  assert.deepEqual(summarizeGroupKinds(leaves), [
    { kind: "TEXT", count: 2 },
    { kind: "IMAGE", count: 1 },
  ]);

  // Patch apenas nos textos preserva a referência exata da imagem não tocada
  const patched = patchGroupDescendants(rootGroup, new Set(["t1", "t2"]), (l) => ({
    ...l,
    opacity: 0.5,
  }));
  const patchedLeaves = collectGroupLeafLayers(patched);
  assert.equal(patchedLeaves[0].opacity, 0.5);
  assert.equal(patchedLeaves[1].opacity, 0.5);
  assert.equal(patchedLeaves[2], img);
});
