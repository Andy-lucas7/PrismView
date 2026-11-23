/**
 * Detector de engasgo para animações do Hub. Recebe o intervalo entre quadros (ms) enquanto algo
 * anima. Aprende o intervalo "bom" do monitor (60/120/144 Hz) como a menor mediana já vista e
 * pede para desligar efeitos caros se o p90 recente passar de `ratio`× esse valor, 2 janelas
 * seguidas. Puro, sem DOM.
 */
export class FrameJankMonitor {
  private readonly buf: Float32Array;
  private n = 0;
  private head = 0;
  private best = Infinity;
  private strikes = 0;
  private sinceEval = 0;
  private readonly ratio: number;
  private readonly evalEvery: number;
  private readonly scratch: Float32Array;

  constructor(opts: { window?: number; ratio?: number; evalEvery?: number } = {}) {
    const w = opts.window ?? 60;
    this.buf = new Float32Array(w);
    this.scratch = new Float32Array(w);
    this.ratio = opts.ratio ?? 1.7;
    this.evalEvery = opts.evalEvery ?? 20;
  }

  /** Descarta o histórico (início de uma nova animação) sem esquecer o intervalo aprendido. */
  reset(): void {
    this.n = 0;
    this.head = 0;
    this.strikes = 0;
    this.sinceEval = 0;
  }

  /** Retorna true quando é hora de degradar. Ignora intervalos > 250 ms (aba oculta/pausa). */
  push(dtMs: number): boolean {
    if (!(dtMs > 0) || dtMs > 250) return false;
    this.buf[this.head] = dtMs;
    this.head = (this.head + 1) % this.buf.length;
    if (this.n < this.buf.length) this.n++;
    if (++this.sinceEval < this.evalEvery || this.n < this.buf.length) return false;
    this.sinceEval = 0;
    const s = this.scratch;
    s.set(this.buf);
    s.sort();
    const p50 = s[Math.floor(s.length * 0.5)];
    const p90 = s[Math.floor(s.length * 0.9)];
    if (p50 < this.best) this.best = p50;
    if (p90 > this.best * this.ratio) {
      this.strikes++;
    } else {
      this.strikes = 0;
    }
    return this.strikes >= 2;
  }
}
