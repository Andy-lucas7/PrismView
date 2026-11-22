// Testes do avaliador FX (Node 22.6+ com type stripping): `npm test`.
// Fora de `src/` para não entrar no `tsc` (sem @types/node).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { evaluateFxTemplate } from "../src/engine/fx/fxEngine.ts";

const fixture = JSON.parse(
  readFileSync(new URL("../../../crates/prism-core/src/fx_parity_cases.json", import.meta.url), "utf8")
);

function parityCtx() {
  const c = fixture.context;
  const [y, mo, d, h, mi, s] = c.now;
  const now = new Date(y, mo - 1, d, h, mi, s);
  const media = { ...c.telemetry.media, updated_at_ms: now.getTime() - c.media_updated_ago_ms };
  const globals: Record<string, string> = {};
  for (const [k, v] of Object.entries(c.globals)) globals[k.toLowerCase()] = String(v);
  return { now, telemetry: { ...c.telemetry, media }, globals };
}

test("casos de paridade (mesmos do Rust)", () => {
  const ctx = parityCtx();
  const failures: string[] = [];
  for (const [input, expected] of fixture.cases as [string, string][]) {
    const got = evaluateFxTemplate(input, ctx);
    if (got !== expected) failures.push(`${JSON.stringify(input)}: esperado ${JSON.stringify(expected)}, obtido ${JSON.stringify(got)}`);
  }
  assert.deepEqual(failures, []);
});

test("aninhamento tc(df) com tokens longos primeiro (sem SÁBE/NOVM)", () => {
  const ctx = parityCtx();
  assert.equal(evaluateFxTemplate("$tc(up, df(EEEE, dd MMMM))$", ctx), "SÁBADO, 21 NOVEMBRO");
  assert.equal(evaluateFxTemplate("$tc(up, df(EEEE MMMM))$", ctx), "SÁBADO NOVEMBRO");
});

test("aninhamento profundo não estoura a pilha", () => {
  const ctx = parityCtx();
  evaluateFxTemplate(`$${"(".repeat(500)}1${")".repeat(500)}$`, ctx);
  evaluateFxTemplate(`$${"-".repeat(500)}5$`, ctx);
});
