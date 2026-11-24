/**
 * Detector de engasgo para animações do Hub. Recebe o intervalo entre quadros (ms) enquanto algo
 * anima. Aprende o intervalo "bom" do monitor (60/120/144 Hz) como a menor mediana já vista e
 * só pede para desligar efeitos caros diante de lentidão SUSTENTADA:
 *  - ignora os primeiros quadros de cada animação (montagem de cards causa picos normais);
 *  - em cada avaliação exige p90 > `ratio`× o intervalo bom E ≥ `badFraction` dos quadros ruins
 *    (dt > `badRatio`× o intervalo bom), então um GC ou uma troca de card isolada não dispara;
 *  - exige `strikesNeeded` avaliações seguidas (≈ 3 × `evalEvery` quadros novos).
 * Puro, sem DOM.
 */
export interface FrameJankOptions {
  window?: number;
  evalEvery?: number;
  ratio?: number;
  badRatio?: number;
  badFraction?: number;
  strikesNeeded?: number;
  warmup?: number;
}

export class FrameJankMonitor {
  private readonly buf: Float32Array;
  private readonly scratch: Float32Array;
  private n = 0;
  private head = 0;
  private best = Infinity;
  private strikes = 0;
  private sinceEval = 0;
  private skipLeft: number;
  private readonly o: Required<FrameJankOptions>;

  constructor(opts: FrameJankOptions = {}) {
    this.o = {
      window: opts.window ?? 90,
      evalEvery: opts.evalEvery ?? 30,
      ratio: opts.ratio ?? 1.7,
      badRatio: opts.badRatio ?? 2.2,
      badFraction: opts.badFraction ?? 0.25,
      strikesNeeded: opts.strikesNeeded ?? 3,
      warmup: opts.warmup ?? 12,
    };
    this.buf = new Float32Array(this.o.window);
    this.scratch = new Float32Array(this.o.window);
    this.skipLeft = this.o.warmup;
  }

  /** Início de uma nova animação: descarta histórico e quadros de aquecimento (mantém o intervalo aprendido). */
  reset(): void {
    this.n = 0;
    this.head = 0;
    this.strikes = 0;
    this.sinceEval = 0;
    this.skipLeft = this.o.warmup;
  }

  /** Retorna true quando é hora de degradar. Ignora intervalos > 250 ms (aba oculta/pausa). */
  push(dtMs: number): boolean {
    if (!(dtMs > 0) || dtMs > 250) return false;
    if (this.skipLeft > 0) {
      this.skipLeft--;
      return false;
    }
    this.buf[this.head] = dtMs;
    this.head = (this.head + 1) % this.buf.length;
    if (this.n < this.buf.length) this.n++;
    if (++this.sinceEval < this.o.evalEvery || this.n < this.buf.length) return false;
    this.sinceEval = 0;
    const s = this.scratch;
    s.set(this.buf);
    s.sort();
    const p50 = s[Math.floor(s.length * 0.5)];
    const p90 = s[Math.floor(s.length * 0.9)];
    if (p50 < this.best) this.best = p50;
    const badAbove = this.best * this.o.badRatio;
    let bad = 0;
    for (let i = 0; i < s.length; i++) if (s[i] > badAbove) bad++;
    if (p90 > this.best * this.o.ratio && bad / s.length >= this.o.badFraction) this.strikes++;
    else this.strikes = 0;
    return this.strikes >= this.o.strikesNeeded;
  }
}
