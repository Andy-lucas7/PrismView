/**
 * Avaliador de fórmulas FX (`$...$`) — tokenizer/parser de passada única.
 *
 * ESPELHO 1:1 de `crates/prism-core/src/fx.rs` (motor nativo). Qualquer mudança aqui DEVE ser
 * replicada lá. Casos compartilhados: `crates/prism-core/src/fx_parity_cases.json`
 * (rodados por `npm test` aqui e por `cargo test -p prism-core` no Rust).
 *
 * Regras:
 * - Template: texto literal + trechos `$expr$`. `$$` = "$" literal. `$` sem fechamento = literal.
 * - Expressão: `||`, `&&`, comparação (`= == != <> < > <= >=`), `+ -`, `* / %`, unário `- !`,
 *   grupos `( )`, strings `"..."`/`'...'` (escape `\`), números, chamadas `nome(args)`.
 *   Itens justapostos são concatenados preservando o espaço original (`Olá mundo`, `AC/DC`).
 *   Operadores só valem se houver operando depois; aritmética com texto vira concatenação.
 * - `df(...)`: argumento cru até o `)` correspondente (vírgulas incluídas). Se contiver `(` ou `"`
 *   é avaliado como expressão. Padrão lido por PALAVRAS: só uma palavra que seja inteira um token
 *   válido (yyyy yy MMMM MMM MM M EEEE EEE dd d HH H hh h mm m ss s a) é convertida; "de", "às"
 *   etc. saem literais. `'texto'` também é literal. Cada token vira texto final e nunca é re-lido.
 */

export interface FxMedia {
  title: string;
  artist: string;
  album: string;
  playing: boolean;
  cover_path?: string;
  position_secs?: number;
  duration_secs?: number;
  updated_at_ms?: number;
}

export interface FxTelemetry {
  cpu_percent: number;
  ram_percent: number;
  ram_used_gb: number;
  ram_total_gb: number;
  hostname: string;
  battery_percent: number;
  battery_charging: boolean;
  media: FxMedia;
}

export interface FxWeather {
  temp_c: number | null;
  condition: string;
  code: number;
  city: string;
  is_day: boolean;
}

export const DEFAULT_FX_WEATHER: FxWeather = {
  temp_c: null,
  condition: "Céu limpo",
  code: 0,
  city: "",
  is_day: true,
};

export interface FxContext {
  now: Date;
  telemetry: FxTelemetry;
  /** Chaves em minúsculas. */
  globals: Record<string, string>;
  weather?: FxWeather;
}

// ───────────────────────── valores ─────────────────────────

interface KVal {
  text: string;
  num: number | null;
}

const MAX_DEPTH = 64;
const MAX_FL_DEPTH = 2;
const MAX_FL_ITERS = 64;

const NUM_RE = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;
const WS_TRIM_RE = /^[ \t\n\r\f]+|[ \t\n\r\f]+$/g;

function trimWs(s: string): string {
  return s.replace(WS_TRIM_RE, "");
}

function parseNum(s: string): number | null {
  const t = trimWs(s);
  if (!NUM_RE.test(t)) return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

function clampAbs(n: number): number {
  if (!Number.isFinite(n)) return 0;
  return Math.min(1e12, Math.max(-1e12, n));
}

function roundHalfUp(x: number): number {
  return Math.floor(x + 0.5);
}

/** Inteiro quando possível; senão até 2 casas, sem zeros à direita. */
function formatNum(n: number): string {
  if (!Number.isFinite(n)) return "0";
  const cents = Math.floor(clampAbs(n) * 100 + 0.5);
  const neg = cents < 0;
  const abs = Math.abs(cents);
  const int = Math.floor(abs / 100);
  const frac = abs % 100;
  let s = (neg ? "-" : "") + String(int);
  if (frac !== 0) {
    s += "." + (frac % 10 === 0 ? String(frac / 10) : String(frac).padStart(2, "0"));
  }
  return s;
}

/** Sempre `d` casas (arredondamento meio-para-cima, idêntico ao Rust). */
function fixed(n: number, d: number): string {
  const p = 10 ** d;
  const v = Math.floor(clampAbs(n) * p + 0.5);
  const neg = v < 0;
  const abs = Math.abs(v);
  const int = Math.floor(abs / p);
  const frac = abs % p;
  return (neg ? "-" : "") + String(int) + (d > 0 ? "." + String(frac).padStart(d, "0") : "");
}

const EMPTY: KVal = { text: "", num: null };
const ofText = (text: string): KVal => ({ text, num: parseNum(text) });
const ofNum = (n: number): KVal => (Number.isFinite(n) ? { text: formatNum(n), num: n } : { text: "0", num: 0 });
const ofBool = (b: boolean): KVal => ({ text: b ? "1" : "0", num: b ? 1 : 0 });

function truthy(v: KVal): boolean {
  if (v.num !== null) return v.num !== 0;
  const t = trimWs(v.text);
  return t !== "" && t.toLowerCase() !== "false";
}

/** Ordem por code point (= ordem de bytes UTF-8 do Rust). */
function cmpText(a: string, b: string): number {
  const A = Array.from(a);
  const B = Array.from(b);
  const n = Math.min(A.length, B.length);
  for (let i = 0; i < n; i++) {
    const d = (A[i].codePointAt(0) ?? 0) - (B[i].codePointAt(0) ?? 0);
    if (d !== 0) return d < 0 ? -1 : 1;
  }
  return A.length === B.length ? 0 : A.length < B.length ? -1 : 1;
}

const hasOwn = (o: object, k: string) => Object.prototype.hasOwnProperty.call(o, k);

const pad2 = (n: number) => String(n).padStart(2, "0");

// ───────────────────────── padrões de data/duração ─────────────────────────

type PatTok = { kind: "run"; ch: string; n: number } | { kind: "lit"; text: string };

const isAsciiLetter = (c: string) => (c >= "a" && c <= "z") || (c >= "A" && c <= "Z");
const ALPHA_RE = /^\p{Alphabetic}$/u;
/** Letra Unicode (= `char::is_alphabetic` no Rust): delimita palavras do padrão. */
const isWordChar = (c: string) => isAsciiLetter(c) || ((c.codePointAt(0) ?? 0) > 0x7f && ALPHA_RE.test(c));

/** Tokens de `df()`: yyyy yy MMMM MMM MM M EEEE EEE dd d HH H hh h mm m ss s a. */
function isDateToken(ch: string, n: number): boolean {
  switch (ch) {
    case "y": return n === 2 || n === 4;
    case "M": return n >= 1 && n <= 4;
    case "E": return n === 3 || n === 4;
    case "d": case "H": case "h": case "m": case "s": return n === 1 || n === 2;
    case "a": return n === 1;
    default: return false;
  }
}

/** Tokens de `tf()`: D DD H HH m mm s ss. */
function isDurationToken(ch: string, n: number): boolean {
  return (ch === "D" || ch === "H" || ch === "m" || ch === "s") && (n === 1 || n === 2);
}

/**
 * Passada única. Uma PALAVRA (corrida máxima de letras Unicode) só vira token se for
 * inteiramente um token válido (mesma letra repetida, comprimento aceito); senão sai literal
 * ("de", "em", "às", "da"...). Separadores (espaço, pontuação, dígitos) e `'texto'`
 * (`''` = aspa) são literais. Tokens colados (`HHmm`) precisam de separador.
 */
function tokenizePattern(p: string, isToken: (ch: string, n: number) => boolean): PatTok[] {
  const out: PatTok[] = [];
  const chars = Array.from(p);
  let lit = "";
  let i = 0;
  const flush = () => {
    if (lit !== "") {
      out.push({ kind: "lit", text: lit });
      lit = "";
    }
  };
  while (i < chars.length) {
    const ch = chars[i];
    if (ch === "'") {
      if (chars[i + 1] === "'") {
        lit += "'";
        i += 2;
        continue;
      }
      let j = i + 1;
      while (j < chars.length) {
        if (chars[j] === "'") {
          if (chars[j + 1] === "'") {
            lit += "'";
            j += 2;
            continue;
          }
          break;
        }
        lit += chars[j];
        j++;
      }
      i = j + 1;
      continue;
    }
    if (isWordChar(ch)) {
      let n = 1;
      while (i + n < chars.length && isWordChar(chars[i + n])) n++;
      const word = chars.slice(i, i + n);
      const first = word[0];
      if (isAsciiLetter(first) && word.every((c) => c === first) && isToken(first, n)) {
        flush();
        out.push({ kind: "run", ch: first, n });
      } else {
        lit += word.join("");
      }
      i += n;
      continue;
    }
    lit += ch;
    i++;
  }
  flush();
  return out;
}

const DAYS_SHORT = ["Dom", "Seg", "Ter", "Qua", "Qui", "Sex", "Sáb"];
const DAYS_FULL = ["Domingo", "Segunda-feira", "Terça-feira", "Quarta-feira", "Quinta-feira", "Sexta-feira", "Sábado"];
const MONTHS_SHORT = ["Jan", "Fev", "Mar", "Abr", "Mai", "Jun", "Jul", "Ago", "Set", "Out", "Nov", "Dez"];
const MONTHS_FULL = [
  "Janeiro", "Fevereiro", "Março", "Abril", "Maio", "Junho",
  "Julho", "Agosto", "Setembro", "Outubro", "Novembro", "Dezembro",
];

function dateRun(ch: string, n: number, now: Date): string {
  const num = (v: number) => (n >= 2 ? pad2(v) : String(v));
  switch (ch) {
    case "y": {
      const y = now.getFullYear();
      return n === 2 ? pad2(((y % 100) + 100) % 100) : String(y);
    }
    case "M": {
      const m = now.getMonth();
      if (n >= 4) return MONTHS_FULL[m];
      if (n === 3) return MONTHS_SHORT[m];
      return num(m + 1);
    }
    case "d":
      return num(now.getDate());
    case "E":
      return n >= 4 ? DAYS_FULL[now.getDay()] : DAYS_SHORT[now.getDay()];
    case "H":
      return num(now.getHours());
    case "h": {
      const h = now.getHours() % 12;
      return num(h === 0 ? 12 : h);
    }
    case "m":
      return num(now.getMinutes());
    case "s":
      return num(now.getSeconds());
    case "a":
      return now.getHours() < 12 ? "AM" : "PM";
    default:
      return ch.repeat(n);
  }
}

function formatDate(toks: PatTok[], now: Date): string {
  let out = "";
  for (const t of toks) out += t.kind === "lit" ? t.text : dateRun(t.ch, t.n, now);
  return out;
}

/** Duração: D dias, H horas, m minutos, s segundos (cada unidade absorve as ausentes). */
function formatDuration(secs: number, pattern: string): string {
  const t = Math.floor(Math.max(0, clampAbs(secs)));
  const p = pattern === "" ? (t >= 3600 ? "H:mm:ss" : "mm:ss") : pattern;
  const toks = tokenizePattern(p, isDurationToken);
  const has = (c: string) => toks.some((k) => k.kind === "run" && k.ch === c);
  let r = t;
  let D = 0;
  let H = 0;
  let M = 0;
  if (has("D")) {
    D = Math.floor(r / 86400);
    r %= 86400;
  }
  if (has("H")) {
    H = Math.floor(r / 3600);
    r %= 3600;
  }
  if (has("m")) {
    M = Math.floor(r / 60);
    r %= 60;
  }
  const S = r;
  let out = "";
  for (const k of toks) {
    if (k.kind === "lit") {
      out += k.text;
      continue;
    }
    const v = k.ch === "D" ? D : k.ch === "H" ? H : k.ch === "m" ? M : k.ch === "s" ? S : null;
    out += v === null ? k.ch.repeat(k.n) : k.n >= 2 ? pad2(v) : String(v);
  }
  return out;
}

function mediaProgress(m: FxMedia, nowMs: number): { pos: number; dur: number; ratio: number } {
  const dur = Math.max(0, m.duration_secs ?? 0);
  const base = Math.max(0, m.position_secs ?? 0);
  const updated = m.updated_at_ms ?? 0;
  const elapsed = m.playing && updated > 0 ? Math.max(0, (nowMs - updated) / 1000) : 0;
  const pos = dur > 0 ? Math.min(dur, base + elapsed) : base + elapsed;
  const ratio = dur > 0.5 ? Math.max(0, Math.min(1, pos / dur)) : 0;
  return { pos, dur, ratio };
}

// ───────────────────────── cores ─────────────────────────

function parseArgb(raw: string): number | null {
  const s = trimWs(raw).replace(/^#+/, "");
  if (!/^[0-9a-fA-F]+$/.test(s)) return null;
  if (s.length === 6) return (0xff000000 | parseInt(s, 16)) >>> 0;
  if (s.length === 8) return parseInt(s, 16) >>> 0;
  return null;
}

const formatArgb = (c: number) => "#" + (c >>> 0).toString(16).toUpperCase().padStart(8, "0");

// ───────────────────────── parser ─────────────────────────

const isWs = (c: string) => c === " " || c === "\t" || c === "\n" || c === "\r" || c === "\f";
const isDigit = (c: string) => c >= "0" && c <= "9";
const isIdentStart = (c: string) => isAsciiLetter(c) || c === "_";
const isIdent = (c: string) => isIdentStart(c) || isDigit(c);

const CMP_OPS = ["<=", ">=", "==", "!=", "<>", "=", "<", ">"];

class Parser {
  src: string;
  pos: number;
  ctx: FxContext;
  locals: Record<string, KVal>;
  depth: number;
  flDepth: number;

  constructor(src: string, ctx: FxContext, locals: Record<string, KVal>, depth: number, flDepth: number) {
    this.src = src;
    this.pos = 0;
    this.ctx = ctx;
    this.locals = locals;
    this.depth = depth;
    this.flDepth = flDepth;
  }

  private at(p: number): string {
    return p < this.src.length ? this.src[p] : "";
  }

  private skipWs(): void {
    while (this.pos < this.src.length && isWs(this.src[this.pos])) this.pos++;
  }

  private wsEnd(from: number): number {
    let p = from;
    while (p < this.src.length && isWs(this.src[p])) p++;
    return p;
  }

  private operandAt(p: number): boolean {
    const c = this.at(this.wsEnd(p));
    return c !== "" && c !== "," && c !== ")";
  }

  private immediateOperand(p: number): boolean {
    const c = this.at(p);
    return c !== "" && (isIdentStart(c) || isDigit(c) || c === "(" || c === '"' || c === "'" || c === "." || c === "-" || c === "!");
  }

  private takeChar(): string {
    const cp = this.src.codePointAt(this.pos) ?? 0;
    const ch = String.fromCodePoint(cp);
    this.pos += ch.length;
    return ch;
  }

  private tryOp(ops: string[]): string | null {
    const save = this.pos;
    this.skipWs();
    for (const op of ops) {
      if (this.src.startsWith(op, this.pos)) {
        const after = this.pos + op.length;
        if (this.operandAt(after)) {
          this.pos = after;
          return op;
        }
        break;
      }
    }
    this.pos = save;
    return null;
  }

  private sub(src: string, locals: Record<string, KVal>, flDepth: number): Parser {
    return new Parser(src, this.ctx, locals, this.depth + 1, flDepth);
  }

  /** Sequência de itens justapostos (até o fim, ou `,`/`)` quando são delimitadores). */
  parseSeq(stopComma: boolean, stopParen: boolean): KVal {
    if (this.depth >= MAX_DEPTH) {
      this.pos = this.src.length;
      return EMPTY;
    }
    this.depth++;
    let first: KVal = EMPTY;
    let acc = "";
    let count = 0;
    for (;;) {
      const save = this.pos;
      this.skipWs();
      if (this.pos >= this.src.length) break;
      const c = this.src[this.pos];
      if ((c === "," && stopComma) || (c === ")" && stopParen)) break;
      const gapEnd = this.pos;
      let v: KVal;
      if (c === "," || c === ")") {
        this.pos++;
        v = ofText(c);
      } else {
        const start = this.pos;
        v = this.parseOr();
        if (this.pos === start) v = ofText(this.takeChar());
      }
      if (count === 0) {
        first = v;
        acc = v.text;
      } else {
        acc += this.src.slice(save, gapEnd) + v.text;
      }
      count++;
    }
    this.depth--;
    if (count === 0) return EMPTY;
    if (count === 1) return first;
    return ofText(acc);
  }

  private parseOr(): KVal {
    let left = this.parseAnd();
    while (this.tryOp(["||"])) {
      const right = this.parseAnd();
      left = ofBool(truthy(left) || truthy(right));
    }
    return left;
  }

  private parseAnd(): KVal {
    let left = this.parseCmp();
    while (this.tryOp(["&&"])) {
      const right = this.parseCmp();
      left = ofBool(truthy(left) && truthy(right));
    }
    return left;
  }

  private parseCmp(): KVal {
    const left = this.parseAdd();
    const op = this.tryOp(CMP_OPS);
    if (op === null) return left;
    const right = this.parseAdd();
    let ok: boolean;
    if (left.num !== null && right.num !== null) {
      const a = left.num;
      const b = right.num;
      switch (op) {
        case "<=": ok = a <= b; break;
        case ">=": ok = a >= b; break;
        case "==":
        case "=": ok = Math.abs(a - b) < 1e-9; break;
        case "!=":
        case "<>": ok = Math.abs(a - b) >= 1e-9; break;
        case "<": ok = a < b; break;
        default: ok = a > b; break;
      }
    } else if (op === "=" || op === "==" || op === "!=" || op === "<>") {
      const eq = left.text.toLowerCase() === right.text.toLowerCase();
      ok = op === "=" || op === "==" ? eq : !eq;
    } else {
      const c = cmpText(left.text, right.text);
      ok = op === "<=" ? c <= 0 : op === ">=" ? c >= 0 : op === "<" ? c < 0 : c > 0;
    }
    return ofBool(ok);
  }

  private parseAdd(): KVal {
    let left = this.parseMul();
    for (;;) {
      const save = this.pos;
      const op = this.tryOp(["+", "-"]);
      if (op === null) break;
      const rs = this.wsEnd(this.pos);
      const right = this.parseMul();
      if (left.num !== null && right.num !== null) {
        left = ofNum(op === "+" ? left.num + right.num : left.num - right.num);
      } else if (op === "+") {
        left = ofText(left.text + right.text);
      } else {
        left = ofText(left.text + this.src.slice(save, rs) + right.text);
      }
    }
    return left;
  }

  private parseMul(): KVal {
    let left = this.parseUnary();
    for (;;) {
      const save = this.pos;
      const op = this.tryOp(["*", "/", "%"]);
      if (op === null) break;
      const rs = this.wsEnd(this.pos);
      const right = this.parseUnary();
      if (left.num !== null && right.num !== null) {
        const a = left.num;
        const b = right.num;
        if (op === "*") left = ofNum(a * b);
        else if (Math.abs(b) < 1e-9) left = ofNum(0);
        else left = ofNum(op === "/" ? a / b : a % b);
      } else {
        left = ofText(left.text + this.src.slice(save, rs) + right.text);
      }
    }
    return left;
  }

  private parseUnary(): KVal {
    this.skipWs();
    const c = this.at(this.pos);
    if ((c === "-" || c === "!") && this.immediateOperand(this.pos + 1)) {
      if (this.depth >= MAX_DEPTH) {
        this.pos = this.src.length;
        return EMPTY;
      }
      this.pos++;
      this.depth++;
      const v = this.parseUnary();
      this.depth--;
      if (c === "!") return ofBool(!truthy(v));
      return v.num !== null ? ofNum(-v.num) : ofText("-" + v.text);
    }
    return this.parsePrimary();
  }

  private parsePrimary(): KVal {
    this.skipWs();
    if (this.pos >= this.src.length) return EMPTY;
    const c = this.src[this.pos];
    if (c === "," || c === ")") return EMPTY;
    if (c === "(") {
      this.pos++;
      const v = this.parseSeq(false, true);
      if (this.at(this.pos) === ")") this.pos++;
      return v;
    }
    if (c === '"' || c === "'") return ofText(this.readString());
    if (isDigit(c) || (c === "." && isDigit(this.at(this.pos + 1)))) {
      const start = this.pos;
      while (this.pos < this.src.length && (isDigit(this.src[this.pos]) || this.src[this.pos] === ".")) this.pos++;
      const raw = this.src.slice(start, this.pos);
      return { text: raw, num: parseNum(raw) };
    }
    if (isIdentStart(c)) {
      const start = this.pos;
      while (this.pos < this.src.length && isIdent(this.src[this.pos])) this.pos++;
      const ident = this.src.slice(start, this.pos);
      const lower = ident.toLowerCase();
      if (this.at(this.pos) === "(") {
        this.pos++;
        return this.call(lower);
      }
      return hasOwn(this.locals, lower) ? this.locals[lower] : ofText(ident);
    }
    return ofText(this.takeChar());
  }

  private readString(): string {
    const quote = this.src[this.pos];
    this.pos++;
    let out = "";
    while (this.pos < this.src.length) {
      const c = this.src[this.pos];
      if (c === "\\" && this.pos + 1 < this.src.length) {
        this.pos++;
        out += this.takeChar();
      } else if (c === quote) {
        this.pos++;
        break;
      } else {
        out += this.takeChar();
      }
    }
    return out;
  }

  /** Índice do `)` que fecha a chamada atual (respeita aninhamento e aspas). */
  private findCallEnd(from: number): number {
    let depth = 0;
    let quote = "";
    let p = from;
    while (p < this.src.length) {
      const c = this.src[p];
      if (quote !== "") {
        if (c === "\\") {
          p += 2;
          continue;
        }
        if (c === quote) quote = "";
      } else if (c === '"' || c === "'") {
        quote = c;
      } else if (c === "(") {
        depth++;
      } else if (c === ")") {
        if (depth === 0) return p;
        depth--;
      }
      p++;
    }
    return this.src.length;
  }

  private call(name: string): KVal {
    if (name === "df") {
      const end = this.findCallEnd(this.pos);
      const raw = this.src.slice(this.pos, Math.min(end, this.src.length));
      this.pos = end < this.src.length ? end + 1 : this.src.length;
      return this.evalDf(raw);
    }
    const args: KVal[] = [];
    this.skipWs();
    if (this.at(this.pos) === ")") {
      this.pos++;
    } else {
      for (;;) {
        args.push(this.parseSeq(true, true));
        this.skipWs();
        const c = this.at(this.pos);
        if (c === ",") {
          this.pos++;
          continue;
        }
        if (c === ")") this.pos++;
        break;
      }
    }
    return this.evalFn(name, args);
  }

  private evalDf(raw: string): KVal {
    const trimmed = trimWs(raw);
    let pattern =
      trimmed.includes("(") || trimmed.includes('"')
        ? this.sub(trimmed, this.locals, this.flDepth).parseSeq(false, false).text
        : trimmed;
    if (pattern === "") pattern = "HH:mm";
    const now = this.ctx.now;
    if (pattern === "S") return ofNum(Math.floor(now.getTime() / 1000));
    return ofText(formatDate(tokenizePattern(pattern, isDateToken), now));
  }

  private evalFn(name: string, args: KVal[]): KVal {
    const arg = (i: number) => (i < args.length ? args[i] : null);
    const key = (i: number, def: string) => {
      const a = arg(i);
      return a === null ? def : trimWs(a.text).toLowerCase();
    };
    switch (name) {
      case "tf": {
        const secs = arg(0)?.num ?? 0;
        const a1 = arg(1);
        return ofText(formatDuration(secs, a1 === null ? "" : trimWs(a1.text)));
      }
      case "gv": {
        const k = key(0, "");
        return ofText(hasOwn(this.ctx.globals, k) ? this.ctx.globals[k] : arg(1)?.text ?? "");
      }
      case "rm":
      case "sys":
      case "si":
        return this.evalSys(name, key(0, "cpu"));
      case "bi":
        return this.evalSys(name, key(0, "level"));
      case "mi":
        return this.evalMi(key(0, "title"));
      case "wi":
        return this.evalWi(key(0, "temp"));
      case "ai":
        return this.evalAi(key(0, "isday"));
      case "if": {
        let i = 0;
        while (i + 1 < args.length) {
          if (truthy(args[i])) return args[i + 1];
          i += 2;
        }
        return i < args.length ? args[i] : EMPTY;
      }
      case "ce":
        return this.evalCe(args);
      case "tc":
        return this.evalTc(args);
      case "mu":
        return this.evalMu(args);
      case "fl":
        return this.evalFl(args);
      default:
        return EMPTY;
    }
  }

  private evalSys(name: string, k: string): KVal {
    const t = this.ctx.telemetry;
    const rm = (): KVal | null => {
      switch (k) {
        case "cpu":
        case "cpuutil":
          return ofNum(roundHalfUp(t.cpu_percent));
        case "ram":
        case "mem":
        case "mused":
          return ofNum(roundHalfUp(t.ram_percent));
        case "ram_gb":
        case "mused_gb":
          return ofText(fixed(t.ram_used_gb, 1));
        case "ram_total_gb":
        case "mtot_gb":
          return ofText(fixed(t.ram_total_gb, 1));
        case "host":
        case "model":
          return ofText(t.hostname);
        default:
          return null;
      }
    };
    const bi = (): KVal | null => {
      switch (k) {
        case "level":
        case "bat":
          return ofNum(t.battery_percent);
        case "charging":
          return ofBool(t.battery_charging);
        case "status":
          return ofText(t.battery_charging ? "Carregando" : "Na Bateria");
        default:
          return null;
      }
    };
    if (name === "rm") return rm() ?? EMPTY;
    if (name === "bi") return bi() ?? EMPTY;
    return rm() ?? bi() ?? EMPTY;
  }

  private evalMi(k: string): KVal {
    const m = this.ctx.telemetry.media;
    switch (k) {
      case "title":
        return ofText(m.title !== "" ? m.title : "Sem mídia");
      case "artist":
        return ofText(m.artist);
      case "album":
        return ofText(m.album);
      case "cover":
        return ofText(m.cover_path ?? "");
      case "state":
        return ofText(m.playing ? "PLAYING" : "PAUSED");
      case "playing":
        return ofBool(m.playing);
      case "pos":
      case "dur":
      case "pct": {
        const p = mediaProgress(m, this.ctx.now.getTime());
        if (k === "pos") return ofText(formatDuration(p.pos, "mm:ss"));
        if (k === "dur") return ofText(formatDuration(p.dur, "mm:ss"));
        return ofNum(roundHalfUp(p.ratio * 100));
      }
      default:
        return EMPTY;
    }
  }

  private evalWi(k: string): KVal {
    const w = this.ctx.weather ?? DEFAULT_FX_WEATHER;
    switch (k) {
      case "temp":
        return w.temp_c === null ? ofText("--") : ofNum(roundHalfUp(w.temp_c));
      case "cond":
        return ofText(w.condition);
      case "code":
        return ofNum(w.code);
      case "city":
        return ofText(w.city !== "" ? w.city : "Local");
      case "isday":
        return ofBool(w.is_day);
      default:
        return EMPTY;
    }
  }

  private evalAi(k: string): KVal {
    const w = this.ctx.weather ?? DEFAULT_FX_WEATHER;
    switch (k) {
      case "sunrise":
        return ofText("06:00");
      case "sunset":
        return ofText("18:00");
      case "season": {
        const m = this.ctx.now.getMonth() + 1;
        if (m === 12 || m <= 2) return ofText("Verão");
        if (m <= 5) return ofText("Outono");
        if (m <= 8) return ofText("Inverno");
        return ofText("Primavera");
      }
      default:
        return ofBool(w.is_day);
    }
  }

  private evalCe(args: KVal[]): KVal {
    const c1 = (args.length > 0 ? parseArgb(args[0].text) : null) ?? 0xffffffff;
    if (args.length < 2) return ofText(formatArgb(c1));
    const op = args[1].text.toLowerCase();
    const a1 = (c1 >>> 24) & 0xff;
    const r1 = (c1 >>> 16) & 0xff;
    const g1 = (c1 >>> 8) & 0xff;
    const b1 = c1 & 0xff;
    switch (op) {
      case "alpha": {
        const pct = Math.min(100, Math.max(0, args[2]?.num ?? 100));
        const a = roundHalfUp((pct / 100) * 255);
        return ofText(formatArgb(((c1 & 0x00ffffff) | (a << 24)) >>> 0));
      }
      case "mix": {
        const c2 = (args.length > 2 ? parseArgb(args[2].text) : null) ?? 0xffffffff;
        const t = Math.min(1, Math.max(0, (args[3]?.num ?? 50) / 100));
        const lerp = (x: number, y: number) => Math.min(255, Math.max(0, roundHalfUp(x + (y - x) * t)));
        const out =
          ((lerp(a1, (c2 >>> 24) & 0xff) << 24) |
            (lerp(r1, (c2 >>> 16) & 0xff) << 16) |
            (lerp(g1, (c2 >>> 8) & 0xff) << 8) |
            lerp(b1, c2 & 0xff)) >>>
          0;
        return ofText(formatArgb(out));
      }
      case "contrast": {
        const lum = (0.2126 * r1 + 0.7152 * g1 + 0.0722 * b1) / 255;
        return ofText(lum > 0.55 ? "#FF000000" : "#FFFFFFFF");
      }
      case "lum":
        return ofNum((0.2126 * r1 + 0.7152 * g1 + 0.0722 * b1) / 2.55);
      default:
        return ofText(formatArgb(c1));
    }
  }

  private evalTc(args: KVal[]): KVal {
    const mode = args.length > 0 ? args[0].text.toLowerCase() : "";
    const txt = args.length > 1 ? args[1].text : "";
    const chars = Array.from(txt);
    const numArg = (i: number) => (i < args.length ? args[i].num : null);
    switch (mode) {
      case "up":
      case "upper":
        return ofText(txt.toUpperCase());
      case "low":
      case "lower":
        return ofText(txt.toLowerCase());
      case "cap":
      case "capitalize":
        return ofText(
          txt
            .split(" ")
            .map((w) => {
              const cs = Array.from(w);
              if (cs.length === 0) return "";
              return cs[0].toUpperCase() + cs.slice(1).join("").toLowerCase();
            })
            .join(" ")
        );
      case "len":
        return ofNum(chars.length);
      case "cut": {
        const n = Math.floor(Math.max(0, numArg(2) ?? chars.length));
        return ofText(chars.slice(0, n).join(""));
      }
      case "ell": {
        const max = Math.floor(Math.max(1, numArg(2) ?? chars.length));
        return chars.length > max ? ofText(chars.slice(0, max - 1).join("") + "…") : ofText(txt);
      }
      case "split": {
        const sep = args.length > 2 ? args[2].text : " ";
        const idx = Math.floor(Math.max(0, numArg(3) ?? 0));
        const parts = sep === "" ? chars : txt.split(sep);
        return ofText(idx < parts.length ? parts[idx] : "");
      }
      case "lpad":
      case "rpad": {
        const width = Math.floor(Math.min(64, Math.max(0, numArg(2) ?? 0)));
        const padArg = args.length > 3 ? Array.from(args[3].text)[0] : undefined;
        const pad = padArg ?? (mode === "lpad" ? "0" : " ");
        if (chars.length >= width) return ofText(txt);
        const fill = pad.repeat(width - chars.length);
        return ofText(mode === "lpad" ? fill + txt : txt + fill);
      }
      default:
        return ofText(txt);
    }
  }

  private evalMu(args: KVal[]): KVal {
    const op = args.length > 0 ? args[0].text.toLowerCase() : "";
    const nums: number[] = [];
    for (let i = 1; i < args.length; i++) {
      const n = args[i].num;
      if (n !== null) nums.push(n);
    }
    if (nums.length === 0) return ofNum(0);
    const x = nums[0];
    switch (op) {
      case "min":
        return ofNum(nums.reduce((a, b) => (b < a ? b : a), Infinity));
      case "max":
        return ofNum(nums.reduce((a, b) => (b > a ? b : a), -Infinity));
      case "abs":
        return ofNum(Math.abs(x));
      case "round":
        return ofNum(roundHalfUp(x));
      case "floor":
        return ofNum(Math.floor(x));
      case "ceil":
        return ofNum(Math.ceil(x));
      case "pow":
        return ofNum(Math.pow(x, nums.length > 1 ? nums[1] : 1));
      case "sqrt":
        return ofNum(Math.sqrt(Math.max(0, x)));
      case "sin":
        return ofNum(Math.sin(x * (Math.PI / 180)));
      case "cos":
        return ofNum(Math.cos(x * (Math.PI / 180)));
      case "clamp": {
        const lo = nums.length > 1 ? nums[1] : 0;
        const hi = nums.length > 2 ? nums[2] : Math.max(lo, 100);
        return ofNum(Math.min(Math.max(lo, hi), Math.max(Math.min(lo, hi), x)));
      }
      default:
        return ofNum(x);
    }
  }

  private evalFl(args: KVal[]): KVal {
    if (this.flDepth >= MAX_FL_DEPTH) return EMPTY;
    const int = (i: number, def: number) => {
      const n = i < args.length ? args[i].num : null;
      return roundHalfUp(Math.min(1e9, Math.max(-1e9, n ?? def)));
    };
    const start = int(0, 1);
    const end = int(1, 1);
    const step = Math.max(1, Math.abs(int(2, 1)));
    const body = args.length > 3 ? args[3].text : "";
    const sep = args.length > 4 ? args[4].text : "";
    const dir = end >= start ? 1 : -1;
    let out = "";
    let i = start;
    let count = 0;
    while (((dir > 0 && i <= end) || (dir < 0 && i >= end)) && count < MAX_FL_ITERS) {
      if (count > 0) out += sep;
      const locals = { ...this.locals, i: ofNum(i) };
      out += this.sub(body, locals, this.flDepth + 1).parseSeq(false, false).text;
      i += step * dir;
      count++;
    }
    return ofText(out);
  }
}

// ───────────────────────── API ─────────────────────────

/** Fecha `$`: próximo `$` fora de aspas. */
function findClosingDollar(s: string, start: number): number {
  let quote = "";
  let i = start;
  while (i < s.length) {
    const c = s[i];
    if (quote !== "") {
      if (c === "\\" && i + 1 < s.length) {
        i += 2;
        continue;
      }
      if (c === quote) quote = "";
    } else if (c === '"' || c === "'") {
      quote = c;
    } else if (c === "$") {
      return i;
    }
    i++;
  }
  return -1;
}

/** Avalia uma expressão inteira (sem `$`). */
export function evaluateFxExpressionRaw(expr: string, ctx: FxContext): string {
  return new Parser(expr, ctx, {}, 0, 0).parseSeq(false, false).text;
}

/** Avalia um template com trechos `$...$`. Texto fora dos trechos é copiado sem alteração. */
export function evaluateFxTemplate(template: string, ctx: FxContext): string {
  if (!template.includes("$")) return template;
  let out = "";
  let i = 0;
  while (i < template.length) {
    const start = template.indexOf("$", i);
    if (start < 0) break;
    out += template.slice(i, start);
    const close = findClosingDollar(template, start + 1);
    if (close < 0) {
      out += template.slice(start);
      i = template.length;
      break;
    }
    out += close === start + 1 ? "$" : evaluateFxExpressionRaw(template.slice(start + 1, close), ctx);
    i = close + 1;
  }
  out += template.slice(i);
  return out;
}

/** Igual a `FxEngine::eval_expr`: com `$` → template; senão expressão inteira. */
export function evaluateFxExpression(expr: string, ctx: FxContext): string {
  const t = trimWs(expr);
  if (t === "") return "";
  return t.includes("$") ? evaluateFxTemplate(t, ctx) : evaluateFxExpressionRaw(t, ctx);
}
