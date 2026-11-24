// Contrato de layout de texto Estúdio × motor: `npm test`.
// Os casos (`crates/prism-core/src/text_layout_cases.json`) são medidos de verdade no motor por
// `cargo test -p prismview-engine` (core/text.rs); aqui validamos o arquivo e as constantes do
// Estúdio das quais a paridade depende, para que mudar só um lado quebre o teste.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
const casesPath = resolve(repo, "crates/prism-core/src/text_layout_cases.json");
const compositorPath = resolve(here, "../src/components/UnifiedCompositor.tsx");
const enginePath = resolve(repo, "crates/prismview-engine/src/core/text.rs");

interface Fields {
  name?: string;
  text?: string;
  fontFamily?: string;
  weight?: number;
  letterSpacingEm?: number;
  align?: string;
  sizePx?: number;
  maxWidthEm?: number;
  expectLines?: number;
  minLines?: number;
  maxLines?: number;
  boxAtLeastMax?: boolean;
  boxAtLeastNatural?: boolean;
}
interface CasesFile {
  defaults: Fields;
  cases: Fields[];
  scaleInvariance: { text: string; sizesPx: number[]; tolerance: number };
}

const file = JSON.parse(readFileSync(casesPath, "utf8")) as CasesFile;

test("casos de layout: nomes únicos e campos obrigatórios", () => {
  const names = new Set<string>();
  for (const c of file.cases) {
    assert.ok(c.name, "caso sem nome");
    assert.ok(!names.has(c.name!), `nome repetido: ${c.name}`);
    names.add(c.name!);
    assert.ok(typeof c.text === "string" && c.text.length > 0, `${c.name}: sem texto`);
    const merged = { ...file.defaults, ...c };
    assert.ok(merged.fontFamily && merged.weight && merged.sizePx, `${c.name}: sem fonte/peso/tamanho`);
    const hasExpectation =
      c.expectLines !== undefined ||
      c.minLines !== undefined ||
      c.maxLines !== undefined ||
      c.boxAtLeastMax ||
      c.boxAtLeastNatural;
    assert.ok(hasExpectation, `${c.name}: caso sem expectativa`);
    if (c.boxAtLeastMax) assert.ok(c.maxWidthEm && c.maxWidthEm > 0, `${c.name}: boxAtLeastMax exige maxWidthEm`);
    if (c.minLines !== undefined && c.maxLines !== undefined) {
      assert.ok(c.minLines <= c.maxLines, `${c.name}: minLines > maxLines`);
    }
  }
  assert.ok(file.cases.length >= 8);
});

test("casos de layout cobrem relógio sem caixa, caixa estreita, espaçamento e palavra longa", () => {
  const names = file.cases.map((c) => c.name);
  for (const required of [
    "relogio-sem-caixa",
    "relogio-caixa-estreita-nunca-quebra",
    "relogio-com-espacamento-letras",
    "palavra-longa-estende-a-caixa",
    "quebra-explicita",
  ]) {
    assert.ok(names.includes(required), `falta o caso ${required}`);
  }
});

test("fontes dos casos existem entre as embutidas do Estúdio", () => {
  const families = new Set<string>([file.defaults.fontFamily!]);
  for (const c of file.cases) if (c.fontFamily) families.add(c.fontFamily);
  for (const family of families) {
    const stem = family.replace(/\s+/g, "");
    const any = [300, 400, 500, 600, 700, 800].some((w) =>
      existsSync(resolve(repo, `apps/prismview-studio/src-tauri/fonts/${stem}-${w}.ttf`))
    );
    assert.ok(any, `fonte sem TTF embutido: ${family}`);
  }
});

test("invariância de escala: tolerância e tamanhos válidos", () => {
  const s = file.scaleInvariance;
  assert.ok(s.sizesPx.length >= 3 && s.sizesPx.every((n) => n > 0));
  assert.ok(s.tolerance > 0 && s.tolerance <= 0.05);
});

test("entrelinha 1.06 e tolerância de quebra iguais nos dois lados", () => {
  const compositor = readFileSync(compositorPath, "utf8");
  const engine = readFileSync(enginePath, "utf8");
  assert.match(compositor, /lineHeight:\s*1\.06/, "Estúdio mudou a entrelinha (motor usa 1.06)");
  assert.match(engine, /size \* \(1\.06 \+ spacing\)/, "motor mudou a entrelinha (Estúdio usa 1.06)");
  // O Estúdio só quebra em espaços (pre-wrap + break-word); o motor usa Wrap::Word e estende a caixa.
  assert.match(compositor, /whiteSpace:\s*textMaxWPx > 0 \? "pre-wrap" : "pre"/);
  assert.match(engine, /Wrap::Word\b/);
  assert.doesNotMatch(
    engine.slice(engine.indexOf("fn shape_layout"), engine.indexOf("const WRAP_TOLERANCE_REL")),
    /Wrap::WordOrGlyph/,
    "o motor voltou a quebrar palavras no meio"
  );
});

test("o motor não escala a caixa do texto sozinho", () => {
  // `size_px * ui` (ui = min(w,h)/1080) é o único fator; o Estúdio usa `fontSize * containerScale`.
  const compositor = readFileSync(compositorPath, "utf8");
  assert.match(compositor, /scaledFont = layer\.text\.fontSize \* containerScale/);
  assert.match(compositor, /textMaxWPx = textMaxWLog > 0 \? textMaxWLog \* containerScale : 0/);
});
