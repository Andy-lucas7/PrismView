// Conteúdo e busca da Ajuda (components/helpContent): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { evaluateFxTemplate, FX_FUNCTION_NAMES } from "../src/engine/fx/fxEngine.ts";
import {
  buildHelpSections,
  buildSearchIndex,
  HELP_GROUPS,
  normalizeSearch,
  SHORTCUT_GROUPS,
  searchSections,
  type HelpBlock,
} from "../src/components/helpContent.ts";

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), "utf8");
const sources = {
  hook: read("../src/hooks/useKeyboardShortcuts.ts"),
  app: read("../src/App.tsx"),
  editor: read("../src/components/StudioEditor.tsx"),
} as const;
const fixture = JSON.parse(read("../../../crates/prism-core/src/fx_parity_cases.json"));

const sections = buildHelpSections();
const index = buildSearchIndex(sections);

function sampleCtx() {
  const c = fixture.context;
  const [y, mo, d, h, mi, s] = c.now;
  const now = new Date(y, mo - 1, d, h, mi, s);
  const media = { ...c.telemetry.media, updated_at_ms: now.getTime() - c.media_updated_ago_ms };
  const globals: Record<string, string> = {};
  for (const [k, v] of Object.entries(c.globals)) globals[k.toLowerCase()] = String(v);
  return { now, telemetry: { ...c.telemetry, media }, globals };
}

test("seções: ids únicos, grupos válidos, título, resumo e conteúdo", () => {
  const ids = new Set<string>();
  for (const s of sections) {
    assert.ok(!ids.has(s.id), `id repetido: ${s.id}`);
    ids.add(s.id);
    assert.ok((HELP_GROUPS as readonly string[]).includes(s.group), `${s.id}: grupo inválido`);
    assert.ok(s.title.trim() && s.summary.trim(), `${s.id}: sem título ou resumo`);
    assert.ok(s.blocks.length > 0, `${s.id}: sem blocos`);
  }
  for (const g of HELP_GROUPS) assert.ok(sections.some((s) => s.group === g), `grupo vazio: ${g}`);
});

test("há uma seção de ƒx para cada função do parser", () => {
  for (const name of FX_FUNCTION_NAMES) {
    const hit = sections.find((s) => s.group === "Fórmulas ƒx" && s.id.startsWith("fx-") && (s.keywords ?? []).includes(name));
    assert.ok(hit, `função sem seção na Ajuda: ${name}`);
  }
});

test("seções esperadas existem (atalhos, tipos de camada, fluxo, alongamento, multisseleção)", () => {
  for (const id of ["fluxo", "camadas", "selecao", "texto", "atalhos", "fx-visao", "fx-tokens", "fx-propriedades", "fx-globais", "fx-receitas"]) {
    assert.ok(sections.some((s) => s.id === id), `seção ausente: ${id}`);
  }
});

test("blocos de tabela têm o mesmo número de colunas do cabeçalho", () => {
  for (const s of sections) {
    for (const b of s.blocks) {
      if (b.t !== "table") continue;
      for (const row of b.rows) assert.equal(row.length, b.head.length, `${s.id}: linha de tabela com colunas erradas`);
      for (const m of b.mono ?? []) assert.ok(m < b.head.length, `${s.id}: coluna mono fora do cabeçalho`);
    }
  }
});

test("exemplos escritos na própria Ajuda também produzem o resultado mostrado", () => {
  const ctx = sampleCtx();
  const failures: string[] = [];
  for (const s of sections) {
    for (const b of s.blocks as readonly HelpBlock[]) {
      if (b.t !== "example" || b.ex.expected === "") continue; // sem resultado = só a sintaxe
      const got = evaluateFxTemplate(b.ex.input, ctx);
      if (got !== b.ex.expected) failures.push(`[${s.id}] ${b.ex.input}: documentado ${JSON.stringify(b.ex.expected)}, motor ${JSON.stringify(got)}`);
    }
  }
  assert.deepEqual(failures, []);
});

test("cada atalho listado existe no código que o implementa", () => {
  const missing: string[] = [];
  for (const g of SHORTCUT_GROUPS) {
    for (const r of g.rows) {
      if (!r.hook) continue;
      const src = sources[r.src ?? "hook"];
      if (!src.includes(r.hook)) missing.push(`${r.keys} (${r.src ?? "hook"}): falta ${r.hook}`);
    }
  }
  assert.deepEqual(missing, []);
});

test("atalhos do editor no popover do Studio estão na Ajuda", () => {
  const listed = new Set(SHORTCUT_GROUPS.flatMap((g) => g.rows.map((r) => r.keys)));
  for (const keys of ["Ctrl+Shift+P", "F1"]) assert.ok(listed.has(keys), `Ajuda sem ${keys}`);
  // O popover do editor cita F1: ele precisa continuar abrindo a Ajuda.
  assert.ok(sources.editor.includes('["F1", "Ajuda e documentação"]'));
});

test("normalizeSearch ignora acentos e caixa", () => {
  assert.equal(normalizeSearch("Fórmulas ƒx — Ação"), "formulas ƒx — acao");
});

test("busca: acento/caixa, vários termos, ordem por título", () => {
  const titles = (q: string) => searchSections(index, q).map((s) => s.id);
  assert.ok(titles("formula").includes("fx-visao"), "formula sem acento deve achar a visão geral de ƒx");
  assert.ok(titles("FÓRMULA").includes("fx-visao"));
  assert.equal(titles("df")[0], "fx-df", "df() deve ser o 1º resultado de 'df'");
  assert.equal(titles("tc")[0], "fx-tc");
  // Nome de função tem prioridade sobre palavras que só o contêm ("rm" está dentro de "fórmula").
  for (const [q, id] of [["rm", "fx-rm"], ["sys", "fx-rm"], ["si", "fx-rm"], ["bi", "fx-bi"], ["mi", "fx-mi"], ["fl", "fx-fl"], ["if", "fx-if"], ["mu", "fx-mu"], ["ce", "fx-ce"], ["gv", "fx-gv"], ["tf", "fx-tf"], ["wi", "fx-wi"], ["ai", "fx-ai"]] as const) {
    assert.equal(titles(q)[0], id, `'${q}' deveria abrir ${id} primeiro`);
  }
  assert.equal(titles("atalhos")[0], "atalhos");
  assert.equal(titles("vidro")[0], "vidro");
  assert.ok(titles("atalho teclado").includes("atalhos"));
  assert.ok(titles("ctrl+g").includes("atalhos"));
  assert.deepEqual(titles("zzzz-nao-existe"), []);
  assert.equal(searchSections(index, "   ").length, sections.length, "busca vazia lista tudo");
  // Todos os termos precisam casar.
  assert.ok(!titles("df zzzz").length);
});

test("busca acha por conteúdo de exemplo e por tabela", () => {
  const ids = (q: string) => searchSections(index, q).map((s) => s.id);
  assert.ok(ids("lpad").includes("fx-tc"));
  assert.ok(ids("cpuutil").includes("fx-rm"));
  assert.ok(ids("sunrise").includes("fx-ai"));
  assert.ok(ids("alpha").includes("fx-ce"));
});
