// Camada SHADER (Modo Espectro) no Studio: desenha o efeito WGSL do usuário em WebGPU.
// Mesmo prelúdio e mesmo layout de uniforms que o motor nativo (`types/prismEspectro`).
// Loop de render sem alocação: buffers Float32Array reaproveitados, textura do fundo só
// recriada quando o tamanho da mídia muda.
import React, { useEffect, useRef, useState } from "react";
import {
  ESPECTRO_GLOBALS_FLOATS,
  packEspectroParams,
  parseHexColor,
  type EspectroEffect,
  type ReactiveSignal,
} from "../types/prism";
import type { BackdropSourceInfo } from "../utils/opticalGlassWebGL";
import { compileEspectro, getEspectroGpu, webgpuAvailable, type EspectroCompiled } from "../engine/espectro/espectroGpu";
import { type PowerState, powerCapsFor } from "../engine/powerGovernor";

export interface EspectroLayerProps {
  effect: EspectroEffect;
  /** Tamanho da camada em px CSS. */
  width: number;
  height: number;
  /** Centro da camada e tamanho do quadro, em px CSS do compositor (mesmo espaço do `backdrop`). */
  centerX: number;
  centerY: number;
  frameW: number;
  frameH: number;
  backdrop: BackdropSourceInfo | null;
  /** Cor de fundo do projeto quando não há mídia de fundo. */
  fallbackColor: string;
  paused?: boolean;
  isThumbnail?: boolean;
  powerState?: PowerState;
  reactiveSignalRef?: React.RefObject<ReactiveSignal>;
  batteryPercent?: number;
}

/* eslint-disable @typescript-eslint/no-explicit-any */
const EspectroLayer: React.FC<EspectroLayerProps> = (props) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const propsRef = useRef(props);
  propsRef.current = props;
  const [compiled, setCompiled] = useState<EspectroCompiled | null>(null);
  const [status, setStatus] = useState<"loading" | "ok" | "nogpu" | "error">("loading");

  useEffect(() => {
    let alive = true;
    if (!webgpuAvailable()) {
      setStatus("nogpu");
      return;
    }
    compileEspectro(props.effect.code).then((c) => {
      if (!alive) return;
      if (!c) {
        setStatus("nogpu");
        return;
      }
      // Erro de compilação mantém o último pipeline bom na tela.
      if (c.pipeline) {
        setCompiled(c);
        setStatus("ok");
      } else {
        setStatus((s) => (s === "ok" ? "ok" : "error"));
      }
    });
    return () => {
      alive = false;
    };
  }, [props.effect.code]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !compiled?.pipeline) return;
    let disposed = false;
    let raf = 0;
    let ctx: any = null;
    let device: any = null;
    let globalsBuf: any = null;
    let paramsBuf: any = null;
    let bdTex: any = null;
    let bdW = 0;
    let bdH = 0;
    let bdKey = "";
    let bindGroup: any = null;
    let gpuCtx: any = null;
    const globals = new Float32Array(ESPECTRO_GLOBALS_FLOATS);
    const params = new Float32Array(compiled.parse.paramsSize / 4);
    const solid = new Uint8Array(4);
    const cursor = { x: 0.5, y: 0.5 };
    const t0 = performance.now();
    let last = t0;
    let lastDraw = 0;

    const onMove = (e: PointerEvent) => {
      const r = canvas.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) return;
      cursor.x = (e.clientX - r.left) / r.width;
      cursor.y = (e.clientY - r.top) / r.height;
    };
    window.addEventListener("pointermove", onMove, { passive: true });

    const ensureBackdrop = (bd: BackdropSourceInfo | null): boolean => {
      const el = bd?.element as any;
      const w = bd ? Math.max(1, Math.round((el?.videoWidth || el?.naturalWidth || el?.width || bd.intrinsicW) as number)) : 1;
      const h = bd ? Math.max(1, Math.round((el?.videoHeight || el?.naturalHeight || el?.height || bd.intrinsicH) as number)) : 1;
      let recreated = false;
      if (!bdTex || bdW !== w || bdH !== h) {
        bdTex?.destroy();
        bdTex = device.createTexture({
          size: [w, h],
          format: "rgba8unorm",
          usage: 0x04 | 0x02 | 0x10, // TEXTURE_BINDING | COPY_DST | RENDER_ATTACHMENT
        });
        bdW = w;
        bdH = h;
        bdKey = "";
        recreated = true;
      }
      const key = bd ? bd.key : `solid:${propsRef.current.fallbackColor}`;
      const live = bd ? bd.kind === "video" || bd.live === true : false;
      if (live || key !== bdKey) {
        try {
          if (bd && el) {
            const ready = bd.kind !== "video" || (el as HTMLVideoElement).readyState >= 2;
            if (ready) device.queue.copyExternalImageToTexture({ source: el }, { texture: bdTex }, [w, h]);
          } else {
            const c = parseHexColor(/^#[0-9a-fA-F]{6,8}$/.test(propsRef.current.fallbackColor) ? propsRef.current.fallbackColor : "#060608");
            solid[0] = c[0] * 255;
            solid[1] = c[1] * 255;
            solid[2] = c[2] * 255;
            solid[3] = 255;
            device.queue.writeTexture({ texture: bdTex }, solid, { bytesPerRow: 4 }, [1, 1]);
          }
          bdKey = key;
        } catch {
          // Mídia ainda não decodificada / origem bloqueada: tenta de novo no próximo quadro.
        }
      }
      return recreated;
    };

    const frame = (now: number) => {
      raf = 0;
      if (disposed) return;
      const p = propsRef.current;
      const caps = powerCapsFor(p.powerState);
      const fpsCap = Math.min(p.isThumbnail ? 15 : 60, caps.fpsCap ?? 60);
      if (now - lastDraw >= 1000 / fpsCap - 1) {
        lastDraw = now;
        const dpr = Math.min(window.devicePixelRatio || 1, caps.dprCap) * caps.renderScaleCap;
        const cw = Math.max(1, Math.round(p.width * dpr));
        const ch = Math.max(1, Math.round(p.height * dpr));
        if (canvas.width !== cw || canvas.height !== ch) {
          canvas.width = cw;
          canvas.height = ch;
        }
        if (ensureBackdrop(p.backdrop) || !bindGroup) {
          bindGroup = device.createBindGroup({
            layout: gpuCtx.layout,
            entries: [
              { binding: 0, resource: { buffer: globalsBuf } },
              { binding: 1, resource: bdTex.createView() },
              { binding: 2, resource: gpuCtx.sampler },
              { binding: 3, resource: { buffer: paramsBuf } },
            ],
          });
        }
        const fw = Math.max(1, p.frameW);
        const fh = Math.max(1, p.frameH);
        const sig = p.reactiveSignalRef?.current;
        const bd = p.backdrop;
        globals[0] = cw;
        globals[1] = ch;
        globals[2] = (now - t0) / 1000;
        globals[3] = (now - last) / 1000;
        globals[4] = cursor.x;
        globals[5] = cursor.y;
        globals[6] = fw;
        globals[7] = fh;
        globals[8] = (p.centerX - p.width / 2) / fw;
        globals[9] = (p.centerY - p.height / 2) / fh;
        globals[10] = p.width / fw;
        globals[11] = p.height / fh;
        globals[12] = bd ? bd.drawLeft / fw : 0;
        globals[13] = bd ? bd.drawTop / fh : 0;
        globals[14] = bd ? bd.drawW / fw : 1;
        globals[15] = bd ? bd.drawH / fh : 1;
        globals[16] = caps.allowSignalBindings ? sig?.level ?? 0 : 0;
        globals[17] = caps.allowSignalBindings ? sig?.bass ?? 0 : 0;
        globals[18] = caps.allowSignalBindings ? sig?.mid ?? 0 : 0;
        globals[19] = caps.allowSignalBindings ? sig?.treble ?? 0 : 0;
        globals[20] = caps.allowSignalBindings ? sig?.key_pulse ?? 0 : 0;
        globals[21] = (p.batteryPercent ?? 100) / 100;
        globals[22] = bd ? 1 : 0;
        globals[23] = bd && bd.kind !== "canvas" ? 1 : 0;
        last = now;
        packEspectroParams(compiled.parse, p.effect.params, params);
        device.queue.writeBuffer(globalsBuf, 0, globals);
        device.queue.writeBuffer(paramsBuf, 0, params);
        const enc = device.createCommandEncoder();
        const pass = enc.beginRenderPass({
          colorAttachments: [
            { view: ctx.getCurrentTexture().createView(), loadOp: "clear", storeOp: "store", clearValue: { r: 0, g: 0, b: 0, a: 0 } },
          ],
        });
        pass.setPipeline(compiled.pipeline);
        pass.setBindGroup(0, bindGroup);
        pass.draw(3);
        pass.end();
        device.queue.submit([enc.finish()]);
      }
      if (!p.paused && document.visibilityState === "visible") raf = requestAnimationFrame(frame);
    };

    const onVis = () => {
      if (!raf && !disposed && document.visibilityState === "visible" && !propsRef.current.paused) {
        raf = requestAnimationFrame(frame);
      }
    };
    document.addEventListener("visibilitychange", onVis);

    getEspectroGpu().then((g) => {
      if (!g || disposed) return;
      gpuCtx = g;
      device = g.device;
      ctx = canvas.getContext("webgpu");
      if (!ctx) return;
      ctx.configure({ device, format: g.format, alphaMode: "premultiplied" });
      globalsBuf = device.createBuffer({ size: ESPECTRO_GLOBALS_FLOATS * 4, usage: 0x40 | 0x08 }); // UNIFORM | COPY_DST
      paramsBuf = device.createBuffer({ size: compiled.parse.paramsSize, usage: 0x40 | 0x08 });
      raf = requestAnimationFrame(frame);
    });

    return () => {
      disposed = true;
      if (raf) cancelAnimationFrame(raf);
      window.removeEventListener("pointermove", onMove);
      document.removeEventListener("visibilitychange", onVis);
      globalsBuf?.destroy();
      paramsBuf?.destroy();
      bdTex?.destroy();
      try {
        ctx?.unconfigure();
      } catch {
        /* contexto já perdido */
      }
    };
  }, [compiled]);

  // Retoma o loop quando sai da pausa (o efeito acima só agenda enquanto não pausado).
  useEffect(() => {
    if (!props.paused && compiled) setCompiled((c) => (c ? { ...c } : c));
  }, [props.paused]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div style={{ width: `${props.width}px`, height: `${props.height}px`, position: "relative" }}>
      <canvas ref={canvasRef} style={{ width: "100%", height: "100%", display: "block" }} />
      {status !== "ok" && !props.isThumbnail && (
        <div className="absolute inset-0 flex items-center justify-center text-[11px] text-[#A1A1AA] bg-black/30 pointer-events-none text-center px-4">
          {status === "loading"
            ? "Compilando efeito…"
            : status === "nogpu"
            ? "Prévia indisponível: WebGPU desligado neste WebView. O desktop desenha normalmente."
            : "Efeito com erro. Abra o Modo Espectro para ver."}
        </div>
      )}
    </div>
  );
};

export default EspectroLayer;
