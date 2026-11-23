// Heurísticas de partes (partHeuristics) + ações rápidas idempotentes (quickActions): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  detectCoverMaterial,
  detectPartSuggestions,
  matchBody,
  matchHead,
  splitNameTokens,
  type PartDetectionInput,
} from "../src/engine/scene3d/partHeuristics.ts";
import {
  applyQuickBinding,
  applyQuickCover,
  findQuickBindings,
  quickBindingId,
  suggestedQuickTarget,
  toggleSuggestionBinding,
} from "../src/engine/scene3d/quickActions.ts";

const bone = (name: string) => ({ name, isBone: true, isMesh: false });
const mesh = (name: string) => ({ name, isBone: false, isMesh: true });

function chibi(): PartDetectionInput {
  return {
    nodes: [
      bone("mixamorig:Hips"),
      bone("mixamorig:Spine"),
      bone("mixamorig:Spine1"),
      bone("mixamorig:Spine2"),
      bone("mixamorig:Neck"),
      bone("mixamorig:Head"),
      bone("mixamorig:HeadTop_End"),
      bone("mixamorig:LeftEye"),
      bone("mixamorig:RightHand"),
      mesh("Body"),
      mesh("Placa"),
    ],
    morphNames: ["Blink", "MouthOpen"],
    materials: [
      { name: "Skin", emissive: false },
      { name: "Wood", emissive: false },
      { name: "PlacaScreen", emissive: false },
    ],
    meshes: [
      { name: "Body", parentName: "Armature", materialNames: ["Skin"] },
      { name: "Placa", parentName: "Armature", materialNames: ["Wood", "PlacaScreen"] },
    ],
  };
}

const byRole = (input: PartDetectionInput, role: string) =>
  detectPartSuggestions(input).filter((s) => s.role === role);

test("tokens: camelCase, mixamo, acentos", () => {
  assert.deepEqual(splitNameTokens("mixamorig:Spine1"), ["mixamorig", "spine1"]);
  assert.deepEqual(splitNameTokens("UpperBody"), ["upper", "body"]);
  assert.deepEqual(splitNameTokens("Cabeça_L"), ["cabeça", "l"]);
});

test("cabeça: PT/EN/mixamo/VRM, ignora headphone e end-effector", () => {
  for (const n of ["head", "Cabeca", "cabeça", "Skull", "mixamorig:Head", "J_Bip_C_Head"]) {
    assert.ok(matchHead(n, splitNameTokens(n)), n);
  }
  assert.ok(!matchHead("Headphones", splitNameTokens("Headphones")));
  const head = byRole(chibi(), "HEAD");
  assert.equal(head.length, 1);
  assert.equal(head[0].targetName, "mixamorig:Head");
  assert.equal(head[0].binding?.targetType, "NODE_LOOK_AT");
});

test("corpo: spine/torso/chest/peito, sem quadril/cabeça; prefere base da coluna", () => {
  for (const n of ["body", "Corpo", "spine", "Spine2", "Torso", "Chest", "Peito", "UpperBody", "mixamorig:Spine1", "J_Bip_C_UpperChest"]) {
    assert.ok(matchBody(n, splitNameTokens(n)), n);
  }
  for (const n of ["LowerBody", "Hips", "Head", "Neck", "BodyCollider"]) {
    assert.ok(!matchBody(n, splitNameTokens(n)), n);
  }
  const body = byRole(chibi(), "BODY");
  assert.equal(body.length, 1);
  assert.equal(body[0].targetName, "mixamorig:Spine");
  assert.ok((body[0].binding?.strength ?? 99) < (byRole(chibi(), "HEAD")[0].binding?.strength ?? 0));
  assert.equal(body[0].binding?.source, "CURSOR_LOOK");
});

test("tela: material com nome forte > malha > emissivo", () => {
  assert.deepEqual(detectCoverMaterial(chibi().materials, chibi().meshes)?.materialName, "PlacaScreen");
  // sem material nomeado: malha "Placa" → 1º material
  const viaMesh = detectCoverMaterial(
    [{ name: "Wood", emissive: false }],
    [{ name: "Placa", parentName: "", materialNames: ["Wood"] }]
  );
  assert.equal(viaMesh?.materialName, "Wood");
  // fraco (frame) perde p/ malha forte (monitor)
  const weak = detectCoverMaterial(
    [{ name: "Frame", emissive: false }, { name: "Glass", emissive: false }],
    [{ name: "Monitor", parentName: "", materialNames: ["Glass"] }]
  );
  assert.equal(weak?.materialName, "Glass");
  // nada nomeado, mas material emissivo
  const glow = detectCoverMaterial([{ name: "Mat.001", emissive: true }], []);
  assert.equal(glow?.materialName, "Mat.001");
  assert.equal(detectCoverMaterial([{ name: "Mat.001", emissive: false }], []), null);
  for (const n of ["sign", "Board", "Tela", "Cover", "Album", "Capa", "Quadro", "Poster", "Canvas", "Display"]) {
    assert.ok(detectCoverMaterial([{ name: n, emissive: false }], []), n);
  }
});

test("preserva olhos/mandíbula e não duplica papéis", () => {
  const sugs = detectPartSuggestions(chibi());
  const roles = sugs.map((s) => s.role);
  assert.equal(new Set(roles).size, roles.length);
  assert.ok(roles.includes("EYE"));
  assert.ok(roles.includes("NECK"));
  const jaw = sugs.find((s) => s.role === "JAW");
  assert.equal(jaw?.binding?.targetType, "MORPH_WEIGHT");
  assert.equal(jaw?.targetName, "MouthOpen");
  const targets = sugs.filter((s) => s.kind === "BINDING").map((s) => s.targetName);
  assert.equal(new Set(targets).size, targets.length);
});

test("quick binding: ligar 2× não duplica, desligar remove, retarget atualiza", () => {
  const req = { kind: "LOOK" as const, objectId: "obj-a", primaryObjectId: "obj-a", targetName: "__ROOT__", enabled: true };
  const once = applyQuickBinding([], req);
  const twice = applyQuickBinding(once, req);
  assert.equal(once.length, 1);
  assert.equal(twice, once, "sem mudança → mesma referência");
  assert.equal(once[0].id, quickBindingId("LOOK", "obj-a"));
  assert.equal(once[0].targetType, "NODE_LOOK_AT");

  const withClick = applyQuickBinding(twice, { ...req, kind: "CLICK", targetName: "__ROOT__" });
  assert.equal(withClick.length, 2);

  const tuned = applyQuickBinding(withClick, { ...req, strength: 30 });
  assert.equal(tuned.find((b) => b.id === quickBindingId("LOOK", "obj-a"))?.strength, 30);
  const retarget = applyQuickBinding(tuned, { ...req, targetName: "Neck" });
  assert.equal(retarget.length, 2);
  assert.equal(retarget.find((b) => b.id === quickBindingId("LOOK", "obj-a"))?.targetName, "Neck");
  assert.equal(retarget.find((b) => b.id === quickBindingId("LOOK", "obj-a"))?.strength, 30);

  const off = applyQuickBinding(retarget, { ...req, targetName: "Neck", enabled: false });
  assert.equal(off.length, 1);
  assert.equal(off[0].source, "CLICK_MESH");
  assert.equal(applyQuickBinding(off, { ...req, enabled: false }), off);
});

test("quick binding: adota binding equivalente criado à mão ou preset (sem duplicar)", () => {
  const manual = {
    id: "b3d-default-look",
    enabled: false,
    source: "CURSOR_LOOK" as const,
    sourceNode: "*",
    targetType: "NODE_LOOK_AT" as const,
    targetName: "__ROOT__",
    strength: 18,
    smoothing: 0.1,
  };
  const req = { kind: "LOOK" as const, objectId: "obj-a", primaryObjectId: "obj-a", targetName: "__ROOT__", enabled: true };
  assert.equal(findQuickBindings([manual], "LOOK", "obj-a", "obj-a", "__ROOT__").length, 1);
  const on = applyQuickBinding([manual], req);
  assert.equal(on.length, 1);
  assert.equal(on[0].id, "b3d-default-look");
  assert.equal(on[0].enabled, true);
  assert.equal(on[0].strength, 18, "mantém ajuste Pro");
  // outro objeto não é afetado
  assert.equal(findQuickBindings([manual], "LOOK", "obj-b", "obj-a", "__ROOT__").length, 0);
  assert.equal(applyQuickBinding(on, { ...req, enabled: false }).length, 0);
});

test("quick cover: idempotente + emissive/material/imagem", () => {
  const on = applyQuickCover([], { objectId: "o", enabled: true, materialName: "Screen" });
  assert.equal(on.length, 1);
  assert.equal(on[0].source, "MEDIA_COVER");
  assert.equal(on[0].emissiveMap, true);
  assert.equal(applyQuickCover(on, { objectId: "o", enabled: true }), on);
  const dim = applyQuickCover(on, { objectId: "o", enabled: true, emissiveMap: false });
  assert.equal(dim.length, 1);
  assert.equal(dim[0].emissiveMap, false);
  const img = applyQuickCover(dim, { objectId: "o", enabled: true, source: "CUSTOM_IMAGE" });
  assert.equal(img[0].source, "CUSTOM_IMAGE");
  assert.equal(img[0].materialName, "Screen");
  assert.equal(applyQuickCover(img, { objectId: "o", enabled: false }).length, 0);
});

test("alvo sugerido e chip contextual: só atua quando o modelo tem partes nomeadas", () => {
  const sugs = detectPartSuggestions({ ...chibi(), nodes: [bone("Neck"), bone("Spine")] });
  assert.equal(suggestedQuickTarget(sugs, "LOOK"), "Neck");
  assert.equal(suggestedQuickTarget(sugs, "CLICK"), null);
  const spineSug = sugs.find((s) => s.role === "BODY")!;
  const added = toggleSuggestionBinding([], spineSug, "obj-a", "obj-a");
  assert.equal(added.length, 1);
  assert.equal(added[0].targetName, "Spine");
  const removed = toggleSuggestionBinding(added, spineSug, "obj-a", "obj-a");
  assert.equal(removed.length, 0);
});
