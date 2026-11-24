// Cache de stills das miniaturas (engine/hubStillCache): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { StillCache } from "../src/engine/hubStillCache.ts";

test("LRU: evita o mais antigo e revoga a URL", () => {
  const revoked: string[] = [];
  const c = new StillCache(2, (u) => revoked.push(u));
  c.set("a", "ua");
  c.set("b", "ub");
  c.get("a");
  c.set("c", "uc");
  assert.ok(c.has("a") && c.has("c") && !c.has("b"));
  assert.deepEqual(revoked, ["ub"]);
});

test("substituir a mesma chave revoga a URL antiga e notifica", () => {
  const revoked: string[] = [];
  const c = new StillCache(3, (u) => revoked.push(u));
  let n = 0;
  c.subscribe(() => n++);
  c.set("a", "u1");
  c.set("a", "u2");
  assert.deepEqual(revoked, ["u1"]);
  assert.equal(c.get("a"), "u2");
  assert.equal(n, 2);
  assert.equal(c.version(), 2);
});
