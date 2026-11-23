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
  const m = new FrameJankMonitor();
  assert.equal(feed(m, 6.9, 300), false);
});

test("queda para ~30 fps após aprender 144 Hz degrada", () => {
  const m = new FrameJankMonitor();
  feed(m, 6.9, 80);
  assert.equal(feed(m, 33, 120), true);
});

test("pico isolado não degrada", () => {
  const m = new FrameJankMonitor();
  feed(m, 6.9, 80);
  assert.equal(feed(m, 40, 3), false);
  assert.equal(feed(m, 6.9, 100), false);
});

test("intervalos de pausa (>250 ms) são ignorados", () => {
  const m = new FrameJankMonitor();
  feed(m, 16.7, 80);
  assert.equal(feed(m, 900, 100), false);
});
