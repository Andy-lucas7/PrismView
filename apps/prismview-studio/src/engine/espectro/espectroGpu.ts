// WebGPU do Modo Espectro no Studio: um device compartilhado + cache de pipelines por código.
// O TS 5.9 não traz os tipos do WebGPU e o projeto não depende de @webgpu/types, então a API
// é usada por uma fachada mínima (`Gpu*`) só com o que este módulo toca.
import { buildEspectroShader, type EspectroParseResult } from "../../types/prismEspectro";

/* eslint-disable @typescript-eslint/no-explicit-any */
export type GpuDevice = any;
export type GpuPipeline = any;
export type GpuBindGroupLayout = any;

export interface EspectroDiagnostic {
  /** Linha no código do usuário (1-based); 0 = fora do código do usuário (prelúdio/entrada). */
  line: number;
  column: number;
  message: string;
  severity: "error" | "warning";
}

export interface EspectroCompiled {
  pipeline: GpuPipeline | null;
  diagnostics: EspectroDiagnostic[];
  parse: EspectroParseResult;
}

interface GpuContext {
  device: GpuDevice;
  format: string;
  layout: GpuBindGroupLayout;
  pipelineLayout: any;
  sampler: any;
}

let ctxPromise: Promise<GpuContext | null> | null = null;
const cache = new Map<string, Promise<EspectroCompiled>>();
const CACHE_MAX = 24;

export function webgpuAvailable(): boolean {
  return typeof navigator !== "undefined" && Boolean((navigator as any).gpu);
}

/** Device único do Studio para todos os efeitos (null = sem WebGPU). */
export function getEspectroGpu(): Promise<GpuContext | null> {
  if (ctxPromise) return ctxPromise;
  ctxPromise = (async () => {
    if (!webgpuAvailable()) return null;
    const gpu = (navigator as any).gpu;
    const adapter = await gpu.requestAdapter({ powerPreference: "low-power" });
    if (!adapter) return null;
    const device = await adapter.requestDevice();
    device.lost.then(() => {
      ctxPromise = null;
      cache.clear();
    });
    const STAGE = 0x1 | 0x2; // VERTEX | FRAGMENT
    const layout = device.createBindGroupLayout({
      entries: [
        { binding: 0, visibility: STAGE, buffer: { type: "uniform" } },
        { binding: 1, visibility: STAGE, texture: { sampleType: "float" } },
        { binding: 2, visibility: STAGE, sampler: { type: "filtering" } },
        { binding: 3, visibility: STAGE, buffer: { type: "uniform" } },
      ],
    });
    const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout] });
    const sampler = device.createSampler({ magFilter: "linear", minFilter: "linear", addressModeU: "clamp-to-edge", addressModeV: "clamp-to-edge" });
    return { device, format: gpu.getPreferredCanvasFormat(), layout, pipelineLayout, sampler } as GpuContext;
  })().catch(() => {
    ctxPromise = null;
    return null;
  });
  return ctxPromise;
}

async function compileUncached(ctx: GpuContext, code: string): Promise<EspectroCompiled> {
  const build = buildEspectroShader(code);
  const diagnostics: EspectroDiagnostic[] = build.parse.errors.map((e) => ({
    line: e.line,
    column: 1,
    message: e.message,
    severity: "error" as const,
  }));
  const { device } = ctx;
  device.pushErrorScope("validation");
  const module = device.createShaderModule({ code: build.wgsl });
  const info = await module.getCompilationInfo();
  for (const m of info.messages as any[]) {
    const userLine = m.lineNum - build.userLineStart + 1;
    diagnostics.push({
      line: userLine >= 1 ? userLine : 0,
      column: m.linePos ?? 1,
      message: m.message,
      severity: m.type === "error" ? "error" : "warning",
    });
  }
  let pipeline: GpuPipeline | null = null;
  if (!diagnostics.some((d) => d.severity === "error")) {
    try {
      pipeline = await device.createRenderPipelineAsync({
        layout: ctx.pipelineLayout,
        vertex: { module, entryPoint: "prism_vs" },
        fragment: {
          module,
          entryPoint: "prism_fs",
          targets: [
            {
              format: ctx.format,
              blend: {
                color: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
                alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
              },
            },
          ],
        },
        primitive: { topology: "triangle-list" },
      });
    } catch (err: any) {
      diagnostics.push({ line: 0, column: 1, message: String(err?.message ?? err), severity: "error" });
    }
  }
  const scopeErr = await device.popErrorScope();
  if (scopeErr && !diagnostics.some((d) => d.severity === "error")) {
    diagnostics.push({ line: 0, column: 1, message: scopeErr.message, severity: "error" });
    pipeline = null;
  }
  if (!diagnostics.some((d) => d.message.includes("effect")) && !/\bfn\s+effect\s*\(/.test(code)) {
    diagnostics.unshift({ line: 1, column: 1, message: "Defina fn effect(uv: vec2f) -> vec4f.", severity: "error" });
  }
  return { pipeline, diagnostics, parse: build.parse };
}

/** Compila (ou devolve do cache) o efeito. Nunca lança; erros vêm em `diagnostics`. */
export async function compileEspectro(code: string): Promise<EspectroCompiled | null> {
  const ctx = await getEspectroGpu();
  if (!ctx) return null;
  let hit = cache.get(code);
  if (!hit) {
    hit = compileUncached(ctx, code);
    cache.set(code, hit);
    if (cache.size > CACHE_MAX) {
      const first = cache.keys().next().value;
      if (first !== undefined) cache.delete(first);
    }
  }
  return hit;
}
