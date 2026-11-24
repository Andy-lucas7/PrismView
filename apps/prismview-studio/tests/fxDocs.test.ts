// A Ajuda das fórmulas ƒx nunca diverge do motor: `npm test`.
// Cada exemplo documentado é AVALIADO pelo avaliador real; cada função do parser (TS e Rust)
// precisa de um verbete.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { evaluateFxTemplate, FX_FUNCTION_NAMES } from "../src/engine/fx/fxEngine.ts";
import {
  FX_DATE_TOKENS,
  FX_FUNCTION_DOCS,
  FX_GROUPS,
  FX_PROPERTY_DOCS,
  FX_RECIPES,
  FX_SYNTAX_SECTIONS,
  type FxExample,
} from "../src/engine/fx/fxDocs.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");
const fixture = JSON.parse(read("../../../crates/prism-core/src/fx_parity_cases.json"));
const engineSrc = read("../src/engine/fx/fxEngine.ts");
const rustSrc = read("../../../crates/prism-core/src/fx.rs");

function sampleCtx() {
  const c = fixture.context;
  const [y, mo, d, h, mi, s] = c.now;
  const now = new Date(y, mo - 1, d, h, mi, s);
  const media = { ...c.telemetry.media, updated_at_ms: now.getTime() - c.media_updated_ago_ms };
  const globals: Record<string, string> = {};
  for (const [k, v] of Object.entries(c.globals)) globals[k.toLowerCase()] = String(v);
  return { now, telemetry: { ...c.telemetry, media }, globals };
}

const allExamples = (): { where: string; ex: FxExample }[] => [
  ...FX_FUNCTION_DOCS.flatMap((f) => f.examples.map((ex) => ({ where: `função ${f.id}`, ex }))),
  ...FX_SYNTAX_SECTIONS.flatMap((s) => (s.examples ?? []).map((ex) => ({ where: `sintaxe ${s.id}`, ex }))),
  ...FX_PROPERTY_DOCS.map((p) => ({ where: `propriedade ${p.key}`, ex: p.example })),
  ...FX_RECIPES.map((r) => ({ where: `receita ${r.title}`, ex: r as FxExample })),
];

test("todo exemplo da Ajuda produz exatamente o resultado documentado", () => {
  const ctx = sampleCtx();
  const failures: string[] = [];
  for (const { where, ex } of allExamples()) {
    const got = evaluateFxTemplate(ex.input, ctx);
    if (got !== ex.expected) {
      failures.push(`[${where}] ${JSON.stringify(ex.input)}: documentado ${JSON.stringify(ex.expected)}, motor ${JSON.stringify(got)}`);
    }
  }
  assert.deepEqual(failures, []);
});

test("exemplos que também estão nos casos de paridade batem com eles", () => {
  const parity = new Map<string, string>(fixture.cases as [string, string][]);
  let shared = 0;
  for (const { where, ex } of allExamples()) {
    const want = parity.get(ex.input);
    if (want === undefined) continue;
    shared++;
    assert.equal(ex.expected, want, `[${where}] ${ex.input} diverge de fx_parity_cases.json`);
  }
  assert.ok(shared >= 25, `poucos exemplos compartilhados com a paridade (${shared})`);
});

test("cada função do parser tem um verbete (e só uma vez)", () => {
  const documented = FX_FUNCTION_DOCS.flatMap((f) => f.names);
  assert.equal(new Set(documented).size, documented.length, "nome documentado em dois verbetes");
  assert.deepEqual([...documented].sort(), [...FX_FUNCTION_NAMES].sort());
});

test("FX_FUNCTION_NAMES = funções do despacho do parser TS", () => {
  const body = engineSrc.slice(engineSrc.indexOf("private evalFn("), engineSrc.indexOf("private evalSys("));
  const cases = new Set<string>();
  for (const m of body.matchAll(/case "([a-z]{2,3})":/g)) cases.add(m[1]);
  assert.ok(engineSrc.includes('name === "df"'), "df deixou de ser tratada em Parser.call");
  cases.add("df");
  assert.deepEqual([...cases].sort(), [...FX_FUNCTION_NAMES].sort());
});

test("FX_FUNCTION_NAMES = funções do despacho do motor Rust (fx.rs)", () => {
  const body = rustSrc.slice(rustSrc.indexOf("fn eval_fn("), rustSrc.indexOf("fn eval_sys("));
  const names = new Set<string>();
  for (const line of body.split("\n")) {
    const arm = line.match(/^\s*((?:"[a-z]{2,3}"\s*\|?\s*)+)=>/);
    if (arm) for (const m of arm[1].matchAll(/"([a-z]{2,3})"/g)) names.add(m[1]);
  }
  assert.ok(/"df"/.test(rustSrc.slice(rustSrc.indexOf("fn call("), rustSrc.indexOf("fn eval_df("))), "df sumiu de Parser::call");
  names.add("df");
  assert.deepEqual([...names].sort(), [...FX_FUNCTION_NAMES].sort());
});

test("verbetes completos: resumo, assinatura, exemplos, grupos válidos e ids únicos", () => {
  const ids = new Set<string>();
  for (const f of FX_FUNCTION_DOCS) {
    assert.ok(!ids.has(f.id), `id repetido: ${f.id}`);
    ids.add(f.id);
    assert.ok(f.summary.length > 10, `${f.id}: resumo curto`);
    assert.ok(f.signature.startsWith(f.names[0]), `${f.id}: assinatura deve começar pelo nome principal`);
    assert.ok(f.examples.length >= 1, `${f.id}: sem exemplo`);
    assert.ok(FX_GROUPS.includes(f.group), `${f.id}: grupo inválido ${f.group}`);
    if (f.options) assert.ok(f.optionsTitle, `${f.id}: opções sem título`);
    for (const a of f.args) assert.ok(a.name && a.desc, `${f.id}: argumento incompleto`);
  }
  for (const g of FX_GROUPS) {
    assert.ok(FX_FUNCTION_DOCS.some((f) => f.group === g), `grupo sem função: ${g}`);
  }
});

test("chaves documentadas das funções de leitura devolvem algo (não vazio)", () => {
  const ctx = sampleCtx();
  // Chaves de rm/bi/mi/wi/ai com resultado sempre não vazio no contexto de exemplo.
  const skip = new Set(["mi(cover)", "mi(album)"]); // vazios no contexto de exemplo
  for (const f of FX_FUNCTION_DOCS) {
    if (!f.options || !["rm", "bi", "mi", "wi", "ai"].includes(f.names[0])) continue;
    for (const o of f.options) {
      for (const key of o.key.split("·").map((k) => k.trim())) {
        const call = `${f.names[0]}(${key})`;
        if (skip.has(call)) continue;
        const out = evaluateFxTemplate(`$${call}$`, ctx);
        assert.notEqual(out, "", `${call} documentado mas o motor devolve vazio`);
      }
    }
  }
});

test("tabela de tokens de df() confere com o motor", () => {
  const ctx = sampleCtx();
  for (const t of FX_DATE_TOKENS) {
    assert.equal(evaluateFxTemplate(`$df(${t.token})$`, ctx), t.sample, `df(${t.token})`);
  }
});
