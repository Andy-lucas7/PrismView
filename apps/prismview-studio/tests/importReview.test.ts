// Revisão pós-import das ações OPEN_APP (utils/importReview): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { collectOpenAppReview, resolveOpenAppReview } from "../src/utils/importReview.ts";
import type { SceneLayer } from "../src/types/prism.ts";

function layer(id: string, extra: Partial<SceneLayer> = {}): SceneLayer {
  return { id, name: `L-${id}`, ...extra } as SceneLayer;
}

const flagged = (id: string, target: string): SceneLayer =>
  layer(id, {
    clickAction: "OPEN_APP",
    clickTarget: "",
    clickTargetPending: target,
    clickTargetNeedsReview: true,
  });

test("collectOpenAppReview lista camadas marcadas, inclusive filhas", () => {
  const layers = [layer("a"), layer("g", { children: [flagged("b", "C:/x/app.exe")] })];
  assert.deepEqual(collectOpenAppReview(layers), [
    { layerId: "b", layerName: "L-b", target: "C:/x/app.exe" },
  ]);
});

test("resolveOpenAppReview só devolve o alvo às permitidas e remove as marcas", () => {
  const layers = [flagged("a", "C:/a.exe"), flagged("b", "C:/b.exe"), layer("c")];
  const out = resolveOpenAppReview(layers, new Set(["a"]));
  assert.equal(out[0].clickTarget, "C:/a.exe");
  assert.equal(out[1].clickTarget, "");
  for (const l of out.slice(0, 2)) {
    assert.equal(l.clickTargetNeedsReview, undefined);
    assert.equal(l.clickTargetPending, undefined);
  }
  assert.equal(out[2], layers[2]);
  assert.equal(collectOpenAppReview(out).length, 0);
});
