/**
 * Ponte para `crates/prism-vstem-wasm`: o alongamento vetorial por haste do motor nativo
 * (`prism-core::vstem`) dentro do Estúdio. Sem DOM: roda também no Node (testes).
 *
 * ABI C (sem wasm-bindgen): os bytes da fonte e o texto vão para a memória do módulo, `vs_layout`
 * devolve `[largura, altura, nº de linhas, ...comandos]` e os comandos (`0 = M x y`, `1 = L x y`,
 * `2 = Z`) viram um `Path2D`/contexto. Arquivo `public/wasm/prism_vstem.wasm` (npm run build:vstem).
 */

/** Versão da ABI que este arquivo entende; o módulo precisa devolver o mesmo número. */
export const VSTEM_ABI = 1;

export const OP_MOVE = 0;
export const OP_LINE = 1;
export const OP_CLOSE = 2;

interface VstemExports {
  memory: WebAssembly.Memory;
  vs_abi_version(): number;
  vs_alloc(len: number): number;
  vs_free(ptr: number, len: number): void;
  vs_font_add(ptr: number, len: number, faceIndex: number): number;
  vs_font_remove(id: number): void;
  vs_font_clear_caches(id: number): void;
  vs_layout(
    fontId: number,
    textPtr: number,
    textLen: number,
    size: number,
    weight: number,
    letterSpacingPx: number,
    align: number,
    maxWidth: number,
    stretch: number,
    compensation: number,
    deviceScale: number
  ): number;
  vs_out_ptr(): number;
  vs_out_len(): number;
}

export type StemAlign = "left" | "center" | "right";

export interface StemLayoutOptions {
  /** Corpo em px CSS. */
  size: number;
  /** Peso (eixo wght das fontes variáveis; fontes estáticas ignoram). */
  weight: number;
  letterSpacingPx: number;
  align: StemAlign;
  /** 0 = largura automática. */
  maxWidth: number;
  /** ≥ 1; 1 devolve só a caixa (sem comandos). */
  stretch: number;
  /** Compensação de espessura 0..1,5 (padrão 1). */
  compensation?: number;
  /** Densidade de pixels da tela (tolerância ao achatar curvas). */
  deviceScale: number;
}

export interface StemLayoutResult {
  /** Caixa de layout em px CSS (a altura já inclui o alongamento). */
  width: number;
  height: number;
  lines: number;
  /** Comandos de caminho (cópia: segura depois de novas chamadas). */
  cmds: Float32Array;
}

/** Superfície mínima de `CanvasRenderingContext2D`/`Path2D` usada para desenhar os comandos. */
export interface PathSink {
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
  closePath(): void;
}

/** Desenha os comandos em `sink`, deslocados em `(ox, oy)` e multiplicados por `scale`. */
export function traceCommands(sink: PathSink, cmds: Float32Array, ox = 0, oy = 0, scale = 1): void {
  let i = 0;
  while (i < cmds.length) {
    const op = cmds[i];
    if (op === OP_CLOSE) {
      sink.closePath();
      i += 1;
    } else if (op === OP_MOVE) {
      sink.moveTo(ox + cmds[i + 1] * scale, oy + cmds[i + 2] * scale);
      i += 3;
    } else if (op === OP_LINE) {
      sink.lineTo(ox + cmds[i + 1] * scale, oy + cmds[i + 2] * scale);
      i += 3;
    } else {
      return; // comando desconhecido: corta aqui em vez de desenhar lixo
    }
  }
}

const ALIGN: Record<StemAlign, number> = { left: 0, center: 1, right: 2 };

export class VstemModule {
  private readonly ex: VstemExports;
  private readonly encoder = new TextEncoder();

  private constructor(ex: VstemExports) {
    this.ex = ex;
  }

  /** Instancia o módulo a partir dos bytes do .wasm; rejeita se faltam exports ou a ABI difere. */
  static async instantiate(wasm: BufferSource): Promise<VstemModule> {
    const { instance } = await WebAssembly.instantiate(wasm, {});
    const ex = instance.exports as unknown as Partial<VstemExports>;
    for (const name of ["memory", "vs_abi_version", "vs_alloc", "vs_free", "vs_font_add", "vs_layout", "vs_out_ptr", "vs_out_len"] as const) {
      if (!(name in ex)) throw new Error(`vstem.wasm sem o export ${name}`);
    }
    const abi = (ex as VstemExports).vs_abi_version();
    if (abi !== VSTEM_ABI) throw new Error(`vstem.wasm com ABI ${abi}; esperado ${VSTEM_ABI} (rode npm run build:vstem)`);
    return new VstemModule(ex as VstemExports);
  }

  private write(bytes: Uint8Array): number {
    const ptr = this.ex.vs_alloc(bytes.length);
    if (ptr === 0) return 0;
    // A memória pode crescer em vs_alloc: o buffer é lido depois.
    new Uint8Array(this.ex.memory.buffer, ptr, bytes.length).set(bytes);
    return ptr;
  }

  /** Registra uma fonte. Devolve o id (> 0) ou 0 se os bytes não são uma fonte válida. */
  addFont(bytes: Uint8Array, faceIndex = 0): number {
    if (bytes.length === 0) return 0;
    const ptr = this.write(bytes);
    if (ptr === 0) return 0;
    try {
      return this.ex.vs_font_add(ptr, bytes.length, faceIndex);
    } finally {
      this.ex.vs_free(ptr, bytes.length);
    }
  }

  removeFont(id: number): void {
    this.ex.vs_font_remove(id);
  }

  clearFontCaches(id: number): void {
    this.ex.vs_font_clear_caches(id);
  }

  /** Layout + alongamento. `null` se a fonte é desconhecida ou ilegível. */
  layout(fontId: number, text: string, o: StemLayoutOptions): StemLayoutResult | null {
    const bytes = this.encoder.encode(text);
    const ptr = bytes.length > 0 ? this.write(bytes) : 0;
    if (bytes.length > 0 && ptr === 0) return null;
    let n = 0;
    try {
      n = this.ex.vs_layout(
        fontId,
        ptr,
        bytes.length,
        o.size,
        o.weight,
        o.letterSpacingPx,
        ALIGN[o.align],
        o.maxWidth,
        o.stretch,
        o.compensation ?? 1,
        o.deviceScale
      );
    } finally {
      if (ptr !== 0) this.ex.vs_free(ptr, bytes.length);
    }
    if (n < 3) return null;
    const out = new Float32Array(this.ex.memory.buffer, this.ex.vs_out_ptr(), this.ex.vs_out_len());
    return { width: out[0], height: out[1], lines: out[2], cmds: out.slice(3) };
  }
}

/** Monta um `Path2D` (só no navegador) a partir dos comandos. */
export function pathFromCommands(cmds: Float32Array, ox = 0, oy = 0, scale = 1): Path2D {
  const p = new Path2D();
  traceCommands(p, cmds, ox, oy, scale);
  return p;
}
