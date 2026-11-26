// Modo Espectro: parser de `//@param`, layout dos uniforms, empacotamento e arquivo .prismfx.
// Também confere que o fixture usado pelo motor (`espectro_templates.json`, validado pelo naga em
// `cargo test -p prismview-engine`) está em dia com os modelos: `node tests/espectro.test.ts --write`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  ESPECTRO_TEMPLATES,
  buildEspectroShader,
  packEspectroParams,
  parseEspectroParams,
  parsePrismFx,
  parsePrismFxFull,
  serializePrismFx,
  setTextureSrc,
  usesPrevious,
  usesSimulation,
  withNativeEspectro,
} from "../src/types/prismEspectro.ts";

const FIXTURE = fileURLToPath(
  new URL("../../../crates/prismview-engine/tests/fixtures/espectro_templates.json", import.meta.url)
);

function fixtureNow(): string {
  const cases = ESPECTRO_TEMPLATES.map((t) => {
    const n = withNativeEspectro({ name: t.name, code: t.code, params: {} });
    return { id: t.id, wgsl: n.nativeWgsl, params: n.nativeParams };
  });
  return JSON.stringify(cases, null, 2) + "\n";
}

if (process.argv.includes("--write")) {
  writeFileSync(FIXTURE, fixtureNow());
}

test("layout segue o alinhamento do WGSL", () => {
  const p = parseEspectroParams(
    [
      '//@param a: float = 0.5 range(0, 1) label("A")',
      "//@param tint: color = #ff000080",
      "//@param b: float = 2",
      "//@param dir: vec2 = 0.25, -1",
      "//@param on: bool = true",
    ].join("\n")
  );
  assert.deepEqual(p.errors, []);
  assert.deepEqual(
    p.params.map((x) => [x.name, x.offset]),
    [["a", 0], ["tint", 16], ["b", 32], ["dir", 40], ["on", 48]]
  );
  assert.equal(p.paramsSize, 64);
  assert.equal(p.params[0].label, "A");
  assert.equal(p.params[0].min, 0);
  assert.equal(p.params[0].max, 1);

  const out = new Float32Array(p.paramsSize / 4);
  packEspectroParams(p, { a: 5 }, out);
  assert.equal(out[0], 1, "valor acima do range é limitado");
  assert.ok(Math.abs(out[4] - 1) < 1e-6 && Math.abs(out[7] - 128 / 255) < 1e-6);
  assert.equal(out[8], 2);
  assert.deepEqual([out[10], out[11]], [0.25, -1]);
  assert.equal(out[12], 1);
});

test("sem parâmetros o uniform ainda existe (16 bytes)", () => {
  const p = parseEspectroParams("fn effect(uv: vec2f) -> vec4f { return vec4f(1.0); }");
  assert.equal(p.paramsSize, 16);
  assert.match(buildEspectroShader("").wgsl, /_unused: vec4f/);
});

test("declaração inválida e repetida viram erro na linha certa", () => {
  const p = parseEspectroParams("\n//@param x: matrix = 1\n//@param y: float\n//@param y: float");
  assert.deepEqual(
    p.errors.map((e) => e.line),
    [2, 4]
  );
});

test("linha do código do usuário é mapeada", () => {
  const b = buildEspectroShader("// linha 1\nfn effect(uv: vec2f) -> vec4f { return vec4f(1.0); }");
  const lines = b.wgsl.split("\n");
  assert.equal(lines[b.userLineStart - 1], "// linha 1");
});

test(".prismfx ida e volta", () => {
  const src = { name: "Teste", code: "fn effect(uv: vec2f) -> vec4f { return vec4f(1.0); }", params: { a: 1 } };
  const back = parsePrismFx(serializePrismFx(src));
  assert.equal(back.name, "Teste");
  assert.equal(back.code, src.code);
  assert.deepEqual(back.params, { a: 1 });
  assert.throws(() => parsePrismFx("{}"), /não é um efeito/);
  assert.throws(() => parsePrismFx("nope"), /corrompido/);
  assert.throws(() => parsePrismFx(JSON.stringify({ format: "prismfx", version: 99, effect: src })), /mais nova/);
});

test("texturas viram slots e helpers tex_NOME", () => {
  const code = [
    '//@texture ruido label("Ruído")',
    "//@texture mapa",
    "fn effect(uv: vec2f) -> vec4f { return tex_ruido(uv) * tex_mapa(uv); }",
  ].join("\n");
  const p = parseEspectroParams(code);
  assert.deepEqual(p.textures.map((t) => [t.name, t.slot, t.label]), [["ruido", 0, "Ruído"], ["mapa", 1, "mapa"]]);
  assert.match(buildEspectroShader(code).wgsl, /fn tex_mapa\(uv: vec2f\) -> vec4f \{\n {2}return textureSampleLevel\(prism_tex1/);
  const fx = setTextureSrc({ name: "t", code, params: {} }, "mapa", "C:/a.png");
  assert.deepEqual(withNativeEspectro(fx).nativeTextures, ["", "C:/a.png", "", ""]);
  const five = Array.from({ length: 5 }, (_, i) => `//@texture t${i}`).join("\n");
  assert.equal(parseEspectroParams(five).errors.length, 1, "máximo de 4");
});

test("memória e simulação só quando o código usa", () => {
  const plain = "fn effect(uv: vec2f) -> vec4f { return vec4f(1.0); }";
  const sim =
    "fn simulate(uv: vec2f) -> vec4f { return sample_state(uv); }\nfn effect(uv: vec2f) -> vec4f { return sample_previous(uv); }";
  assert.equal(usesPrevious(plain), false);
  assert.equal(usesSimulation(plain), false);
  assert.equal(usesPrevious(sim), true);
  assert.equal(usesSimulation(sim), true);
  assert.doesNotMatch(buildEspectroShader(plain).wgsl, /prism_sim/);
  assert.match(buildEspectroShader(sim).wgsl, /@fragment fn prism_sim/);
  const n = withNativeEspectro({ name: "s", code: sim, params: {} });
  assert.equal(n.nativeFeedback, true);
  assert.equal(n.nativeSimulate, true);
});

test(".prismfx leva texturas embutidas e rejeita data URL estranha", () => {
  const png = "data:image/png;base64,iVBORw0KGgo=";
  const text = serializePrismFx({ name: "T", code: "fn effect(uv: vec2f) -> vec4f { return vec4f(1.0); }", params: {} }, [
    { name: "ruido", fileName: "r.png", dataUrl: png },
  ]);
  const back = parsePrismFxFull(text);
  assert.equal(back.textures.length, 1);
  assert.equal(back.textures[0].dataUrl, png);
  const evil = text.replace(png, "data:text/html;base64,PHNjcmlwdD4=");
  assert.equal(parsePrismFxFull(evil).textures.length, 0);
});

test("fixture do motor em dia com os modelos", () => {
  // Checkout com autocrlf grava CRLF: compara o conteúdo, não o fim de linha.
  assert.equal(readFileSync(FIXTURE, "utf8").replace(/
/g, "
"), fixtureNow(), "rode: node tests/espectro.test.ts --write");
});
