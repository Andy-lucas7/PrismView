// Divisória do painel único do Estúdio (utils/panelSplit): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  clampSplit,
  nudgeSplit,
  PANEL_GAP_PX,
  PANEL_OPEN_KEY,
  PANEL_WIDTH_PX,
  readPanelOpen,
  readSplit,
  SPLIT_DEFAULT,
  SPLIT_KEY,
  SPLIT_MAX,
  SPLIT_MIN,
  splitFromPointer,
  stageRightInset,
  writePanelOpen,
  writeSplit,
  type KeyValueStore,
} from "../src/utils/panelSplit.ts";

function memoryStore(initial: Record<string, string> = {}): KeyValueStore & { data: Record<string, string> } {
  const data = { ...initial };
  return {
    data,
    getItem: (k) => (k in data ? data[k] : null),
    setItem: (k, v) => {
      data[k] = v;
    },
  };
}

test("clampSplit limita ao intervalo e trata valores inválidos", () => {
  assert.equal(clampSplit(0.5), 0.5);
  assert.equal(clampSplit(-3), SPLIT_MIN);
  assert.equal(clampSplit(9), SPLIT_MAX);
  assert.equal(clampSplit(NaN), SPLIT_DEFAULT);
  assert.equal(clampSplit(Infinity), SPLIT_DEFAULT);
});

test("persistência: ida e volta, padrão, lixo e storage que falha", () => {
  const s = memoryStore();
  assert.equal(readSplit(s), SPLIT_DEFAULT);
  writeSplit(s, 0.4);
  assert.equal(s.data[SPLIT_KEY], "0.4000");
  assert.equal(readSplit(s), 0.4);
  writeSplit(s, 5);
  assert.equal(readSplit(s), SPLIT_MAX, "grava já limitado");
  assert.equal(readSplit(memoryStore({ [SPLIT_KEY]: "abc" })), SPLIT_DEFAULT);
  assert.equal(readSplit(memoryStore({ [SPLIT_KEY]: "0.01" })), SPLIT_MIN);
  assert.equal(readSplit(null), SPLIT_DEFAULT);
  const broken: KeyValueStore = {
    getItem: () => {
      throw new Error("negado");
    },
    setItem: () => {
      throw new Error("cheio");
    },
  };
  assert.equal(readSplit(broken), SPLIT_DEFAULT);
  assert.doesNotThrow(() => writeSplit(broken, 0.5));
  assert.equal(readPanelOpen(broken), true);
  assert.doesNotThrow(() => writePanelOpen(broken, false));
});

test("painel: aberto por padrão, só '0' recolhe", () => {
  const s = memoryStore();
  assert.equal(readPanelOpen(s), true);
  writePanelOpen(s, false);
  assert.equal(s.data[PANEL_OPEN_KEY], "0");
  assert.equal(readPanelOpen(s), false);
  writePanelOpen(s, true);
  assert.equal(readPanelOpen(s), true);
  assert.equal(readPanelOpen(memoryStore({ [PANEL_OPEN_KEY]: "qualquer" })), true);
  assert.equal(readPanelOpen(null), true);
});

test("splitFromPointer converte a posição do ponteiro e limita", () => {
  assert.equal(splitFromPointer(300, 100, 400), 0.5);
  assert.equal(splitFromPointer(0, 100, 400), SPLIT_MIN);
  assert.equal(splitFromPointer(9999, 100, 400), SPLIT_MAX);
  assert.equal(splitFromPointer(300, 100, 0), SPLIT_DEFAULT, "altura zero");
  assert.equal(splitFromPointer(NaN, 100, 400), SPLIT_DEFAULT);
});

test("nudgeSplit anda por passos e nunca passa dos limites", () => {
  assert.ok(Math.abs(nudgeSplit(0.5, 1) - 0.53) < 1e-9);
  assert.ok(Math.abs(nudgeSplit(0.5, -2) - 0.44) < 1e-9);
  assert.equal(nudgeSplit(SPLIT_MAX, 5), SPLIT_MAX);
  assert.equal(nudgeSplit(SPLIT_MIN, -5), SPLIT_MIN);
  assert.equal(nudgeSplit(NaN, 1), SPLIT_DEFAULT + 0.03);
});

test("margem direita do canvas: painel aberto reserva a largura, recolhido só a folga", () => {
  assert.equal(stageRightInset(true), PANEL_WIDTH_PX + 2 * PANEL_GAP_PX);
  assert.equal(stageRightInset(false), PANEL_GAP_PX);
  assert.ok(stageRightInset(true) > stageRightInset(false));
});
