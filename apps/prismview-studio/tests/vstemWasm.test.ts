// Ponte WebAssembly do alongamento vetorial (crates/prism-vstem-wasm): `npm test`.
// Usa o .wasm versionado em public/wasm e as fontes embutidas do Estúdio. Se mudar `vstem.rs`
// ou o crate, refaça com `npm run build:vstem`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  OP_CLOSE,
  OP_LINE,
  OP_MOVE,
  traceCommands,
  VSTEM_ABI,
  VstemModule,
  type PathSink,
  type StemLayoutOptions,
} from "../src/engine/vstem/vstemWasm.ts";

const bytes = (rel: string) => new Uint8Array(readFileSync(new URL(rel, import.meta.url)));
const OUTFIT = bytes("../src-tauri/fonts/Outfit-400.ttf");
const MONO = bytes("../src-tauri/fonts/JetBrainsMono-400.ttf");
const wasm = readFileSync(new URL("../public/wasm/prism_vstem.wasm", import.meta.url));

const base: StemLayoutOptions = {
  size: 100,
  weight: 400,
  letterSpacingPx: 0,
  align: "left",
  maxWidth: 0,
  stretch: 2,
  deviceScale: 1,
};

async function setup() {
  const vs = await VstemModule.instantiate(wasm);
  const id = vs.addFont(OUTFIT);
  assert.ok(id > 0, "fonte Outfit não foi aceita");
  return { vs, id };
}

function bounds(cmds: Float32Array) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (let i = 0; i < cmds.length; ) {
    if (cmds[i] === OP_CLOSE) { i += 1; continue; }
    x0 = Math.min(x0, cmds[i + 1]); x1 = Math.max(x1, cmds[i + 1]);
    y0 = Math.min(y0, cmds[i + 2]); y1 = Math.max(y1, cmds[i + 2]);
    i += 3;
  }
  return { x0, y0, x1, y1 };
}

test("módulo: ABI e exports conferem; bytes inválidos são recusados", async () => {
  assert.equal(VSTEM_ABI, 1);
  const vs = await VstemModule.instantiate(wasm);
  assert.equal(vs.addFont(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])), 0);
  assert.equal(vs.addFont(new Uint8Array(0)), 0);
  await assert.rejects(VstemModule.instantiate(new Uint8Array([0, 1, 2])));
});

test("alongamento: altura da caixa = linhas × 1,06 × corpo × stretch e a tinta cresce", async () => {
  const { vs, id } = await setup();
  const plain = vs.layout(id, "08:29", { ...base, stretch: 1 });
  const tall = vs.layout(id, "08:29", base);
  assert.ok(plain && tall);
  assert.equal(plain.cmds.length, 0, "sem alongamento só vem a caixa");
  assert.ok(Math.abs(tall.width - plain.width) < 0.01, "a largura não muda");
  assert.ok(Math.abs(tall.height - 2 * 106) < 0.01, `altura ${tall.height}`);
  assert.equal(tall.lines, 1);
  const b = bounds(tall.cmds);
  const ink = b.y1 - b.y0;
  assert.ok(ink > 1.7 * 70 && ink < 2.3 * 74, `tinta com ${ink}px`);
  assert.ok(b.x0 >= -1 && b.x1 <= tall.width + 1, "a tinta fica dentro da caixa horizontalmente");
});

test("comandos: sempre M, depois L…, depois Z; coordenadas finitas", async () => {
  const { vs, id } = await setup();
  const r = vs.layout(id, "HEBA 0812:45", base);
  assert.ok(r && r.cmds.length > 0);
  let open = false;
  let contours = 0;
  for (let i = 0; i < r.cmds.length; ) {
    const op = r.cmds[i];
    if (op === OP_MOVE) {
      assert.equal(open, false, "M com contorno aberto");
      open = true;
      contours++;
      i += 3;
    } else if (op === OP_LINE) {
      assert.equal(open, true, "L sem M");
      i += 3;
    } else if (op === OP_CLOSE) {
      assert.equal(open, true, "Z sem contorno");
      open = false;
      i += 1;
    } else {
      assert.fail(`comando desconhecido ${op}`);
    }
  }
  assert.equal(open, false);
  assert.ok(contours >= 12, `poucos contornos (${contours})`);
  assert.ok(r.cmds.every((v) => Number.isFinite(v)));
});

test("quebra só entre palavras e a caixa estende para uma palavra larga", async () => {
  const { vs, id } = await setup();
  const natural = vs.layout(id, "00:06", { ...base, stretch: 1 })!.width;
  const narrow = vs.layout(id, "00:06", { ...base, maxWidth: natural * 0.4 })!;
  assert.equal(narrow.lines, 1);
  assert.ok(Math.abs(narrow.width - natural) < 0.01);
  const phrase = vs.layout(id, "Solus lesus salvat", { ...base, stretch: 1 })!;
  const wrapped = vs.layout(id, "Solus lesus salvat", { ...base, stretch: 1, maxWidth: phrase.width * 0.6 })!;
  assert.ok(wrapped.lines >= 2);
  assert.equal(vs.layout(id, "Solus lesus salvat", { ...base, stretch: 1, maxWidth: phrase.width * 0.99 })!.lines, 1, "tolerância de largura");
});

test("alinhamento e espaçamento entre letras", async () => {
  const { vs, id } = await setup();
  const left = (align: "left" | "center" | "right") =>
    bounds(vs.layout(id, "a", { ...base, align, maxWidth: 300 })!.cmds).x0;
  assert.ok(left("left") < left("center") && left("center") < left("right"));
  const a = vs.layout(id, "abc", { ...base, stretch: 1 })!.width;
  const b = vs.layout(id, "abc", { ...base, stretch: 1, letterSpacingPx: 10 })!.width;
  assert.ok(Math.abs(b - a - 30) < 0.01, "o espaçamento conta também depois da última letra (como o CSS)");
});

test("determinístico, com cache e texto UTF-8 multibyte", async () => {
  const { vs, id } = await setup();
  const first = vs.layout(id, "Ação · 12:34 ƒx", base)!;
  const second = vs.layout(id, "Ação · 12:34 ƒx", base)!;
  assert.deepEqual(Array.from(second.cmds), Array.from(first.cmds));
  assert.equal(second.width, first.width);
  vs.clearFontCaches(id);
  assert.deepEqual(Array.from(vs.layout(id, "Ação · 12:34 ƒx", base)!.cmds), Array.from(first.cmds));
});

test("entradas hostis não derrubam o módulo", async () => {
  const { vs, id } = await setup();
  for (const o of [
    { ...base, size: NaN },
    { ...base, stretch: Infinity },
    { ...base, maxWidth: NaN },
    { ...base, letterSpacingPx: -Infinity },
    { ...base, deviceScale: 0 },
  ]) {
    vs.layout(id, "x y", o);
  }
  assert.equal(vs.layout(999, "x", base), null, "fonte desconhecida");
  assert.ok(vs.layout(id, "", base));
  assert.ok(vs.layout(id, "\n\n", base));
  assert.ok(vs.layout(id, "x".repeat(20000), { ...base, stretch: 1 }));
});

test("fonte monoespaçada e remoção de fonte", async () => {
  const vs = await VstemModule.instantiate(wasm);
  const id = vs.addFont(MONO);
  assert.ok(id > 0);
  const r = vs.layout(id, "iiii", { ...base, stretch: 1 })!;
  const w = vs.layout(id, "WWWW", { ...base, stretch: 1 })!;
  assert.ok(Math.abs(r.width - w.width) < 0.01, "monoespaçada: mesma largura");
  vs.removeFont(id);
  assert.equal(vs.layout(id, "x", base), null);
});

test("traceCommands traduz os comandos e aplica deslocamento e escala", () => {
  const calls: string[] = [];
  const sink: PathSink = {
    moveTo: (x, y) => calls.push(`M${x},${y}`),
    lineTo: (x, y) => calls.push(`L${x},${y}`),
    closePath: () => calls.push("Z"),
  };
  traceCommands(sink, new Float32Array([OP_MOVE, 1, 2, OP_LINE, 3, 4, OP_CLOSE]), 10, 20, 2);
  assert.deepEqual(calls, ["M12,24", "L16,28", "Z"]);
  const bad: string[] = [];
  traceCommands({ moveTo: () => bad.push("m"), lineTo: () => bad.push("l"), closePath: () => bad.push("z") }, new Float32Array([9, 1, 2, OP_MOVE, 0, 0]));
  assert.deepEqual(bad, [], "comando desconhecido interrompe o desenho");
});
