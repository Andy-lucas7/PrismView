// Conversões de cor do seletor (utils/colorMath): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { hexToHsv, hexToRgb, hsvToHex, rgbToHex } from "../src/utils/colorMath.ts";

test("hex válido/inválido", () => {
  assert.deepEqual(hexToRgb("#ff8000"), [255, 128, 0]);
  assert.deepEqual(hexToRgb("0af"), [0, 170, 255]);
  assert.equal(hexToRgb("#12345"), null);
  assert.equal(hexToRgb("rgba(1,2,3,1)"), null);
});

test("ida e volta hex → hsv → hex é estável", () => {
  for (const hex of ["#000000", "#ffffff", "#ff0000", "#00ff00", "#0000ff", "#07080b", "#a1a1aa", "#3b82f6", "#f59e0b"]) {
    const hsv = hexToHsv(hex);
    assert.ok(hsv);
    assert.equal(hsvToHex(hsv), hex);
  }
});

test("cinza tem saturação 0 e primárias têm matiz certo", () => {
  assert.equal(hexToHsv("#808080")!.s, 0);
  assert.equal(Math.round(hexToHsv("#ff0000")!.h), 0);
  assert.equal(Math.round(hexToHsv("#00ff00")!.h), 120);
  assert.equal(Math.round(hexToHsv("#0000ff")!.h), 240);
  assert.equal(rgbToHex(300, -5, 12.4), "#ff000c");
});
