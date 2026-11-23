// Orçamento de miniaturas vivas do Hub (engine/hubLiveBudget): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { HubLiveBudget } from "../src/engine/hubLiveBudget.ts";

test("mantém só as `capacity` de menor rank", () => {
  const b = new HubLiveBudget(2);
  b.request("a", 0.5);
  b.request("b", 0);
  b.request("c", 1);
  b.flush();
  assert.deepEqual(["a", "b", "c"].filter((i) => b.isLive(i)), ["a", "b"]);
});

test("desistir libera a vaga para o próximo", () => {
  const b = new HubLiveBudget(1);
  b.request("a", 0);
  b.request("b", 1);
  b.flush();
  assert.ok(b.isLive("a") && !b.isLive("b"));
  b.request("a", null);
  b.flush();
  assert.ok(!b.isLive("a") && b.isLive("b"));
});

test("empate de rank: o mais antigo fica", () => {
  const b = new HubLiveBudget(1);
  b.request("x", 1);
  b.request("y", 1);
  b.flush();
  assert.ok(b.isLive("x") && !b.isLive("y"));
});

test("só notifica quando o conjunto vivo muda", () => {
  const b = new HubLiveBudget(2);
  let n = 0;
  b.subscribe(() => n++);
  b.request("a", 0);
  b.flush();
  b.request("a", 0.0);
  b.flush();
  b.request("a", 0.3);
  b.flush();
  assert.equal(n, 1);
});
