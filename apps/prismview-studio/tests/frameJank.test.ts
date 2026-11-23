// Detector de engasgo (engine/frameJank): `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { FrameJankMonitor } from "../src/engine/frameJank.ts";

const feed = (m: FrameJankMonitor, dt: number, n: number): boolean => {
  let out = false;
  for (let i = 0; i < n; i++) out = m.push(dt) || out;
  return out;
};

test("144 Hz estável não degrada", () => {
  assert.equal(feed(new FrameJankMonitor(), 6.9, 600), false);
});

test("queda sustentada para ~30 fps após aprender 144 Hz degrada", () => {
  const m = new FrameJankMonitor();
  feed(m, 6.9, 160);
  assert.equal(feed(m, 33, 400), true);
});

test("pico isolado (troca de card) não degrada", () => {
  const m = new FrameJankMonitor();
  feed(m, 6.9, 160);
  assert.equal(feed(m, 45, 6), false);
  assert.equal(feed(m, 6.9, 200), false);
});

test("1 quadro ruim a cada 10 (GC periódico) não degrada", () => {
  const m = new FrameJankMonitor();
  feed(m, 6.9, 160);
  let fired = false;
  for (let i = 0; i < 600; i++) fired = m.push(i % 10 === 0 ? 40 : 6.9) || fired;
  assert.equal(fired, false);
});

test("aquecimento lento (montagem) é ignorado após reset", () => {
  const m = new FrameJankMonitor();
  feed(m, 6.9, 160);
  m.reset();
  assert.equal(feed(m, 50, 10), false);
  assert.equal(feed(m, 6.9, 300), false);
});

test("intervalos de pausa (>250 ms) são ignorados", () => {
  const m = new FrameJankMonitor();
  feed(m, 16.7, 160);
  assert.equal(feed(m, 900, 100), false);
});

test("monitor 60 Hz: 60 fps estável não degrada, 25 fps sustentado degrada", () => {
  const m = new FrameJankMonitor();
  assert.equal(feed(m, 16.7, 300), false);
  assert.equal(feed(m, 40, 400), true);
});
