// Opções de saída do vídeo no desktop (FPS e resolução de decodificação): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  autoDecodeHeight,
  estimateFps,
  fpsLimitFor,
  heightOptions,
  maxFpsFor,
} from "../src/utils/videoOutput.ts";

test("estimateFps usa a mediana e encaixa em taxas comuns", () => {
  const d60 = Array(10).fill(1 / 60);
  d60.push(2 / 60); // quadro perdido não muda a mediana
  assert.equal(estimateFps(d60), 60);
  assert.equal(estimateFps(Array(8).fill(1001 / 30000)), 29.97);
  assert.equal(estimateFps([0.04, 0.04]), null);
  assert.equal(maxFpsFor(29.97), 30);
  assert.equal(maxFpsFor(null), 60);
});

test("altura automática: monitor se o vídeo for maior, senão a do arquivo", () => {
  assert.equal(autoDecodeHeight(2160, 1080), 1080);
  assert.equal(autoDecodeHeight(720, 1080), 720);
  assert.equal(autoDecodeHeight(2160, null), 2160);
});

test("opções de altura: sobe e desce, marca original/monitor/padrão", () => {
  const big = heightOptions(2160, 1080);
  assert.deepEqual(
    big.map((o) => o.height),
    [2160, 1440, 1080, 720, 540, 480, 360]
  );
  assert.equal(big.find((o) => o.isAuto)?.height, 1080);
  assert.equal(big[0].label, "2160p (original)");
  assert.equal(big[2].label, "1080p (monitor, padrão)");

  const small = heightOptions(720, 1440);
  assert.equal(small[0].height, 1440); // dá para subir até o monitor
  assert.equal(small.find((o) => o.isAuto)?.height, 720);
});

test("fpsLimitFor: no máximo = sem teto; 0 = quadro parado", () => {
  assert.equal(fpsLimitFor(60, 60), undefined);
  assert.equal(fpsLimitFor(30, 60), 30);
  assert.equal(fpsLimitFor(0, 60), 0);
});
