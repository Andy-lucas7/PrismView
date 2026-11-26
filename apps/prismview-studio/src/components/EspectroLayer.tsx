// Camada SHADER (Modo Espectro) no Studio: desenha o efeito WGSL do usuário em WebGPU.
// Mesmo prelúdio e mesmo layout de uniforms que o motor nativo (`types/prismEspectro`).
// Loop de render sem alocação: buffers Float32Array reaproveitados; texturas (fundo, quadro
// anterior, imagens do usuário) só recriadas quando o tamanho/arquivo muda.
import React, { useEffect, useRef, useState } from "react";
import {
  ESPECTRO_GLOBALS_FLOATS,
  ESPECTRO_MAX_TEXTURES,
  packEspectroParams,
  parseHexColor,
  resolveMediaUrl,
  textureSrc,
  usesPrevious,
  ESPECTRO_STATE_FORMAT,
  ESPECTRO_PARTICLE_BYTES,
  type EspectroEffect,
  type ReactiveSignal,
} from "../types/prism";
import type { BackdropSourceInfo } from "../utils/opticalGlassWebGL";
import {
  compileEspectro,
  espectroOutput,
  getEspectroGpu,
  releaseEspectroOutput,
  webgpuAvailable,
  type EspectroCompiled,
} from "../engine/espectro/espectroGpu";
import { type PowerState, powerCapsFor } from "../engine/powerGovernor";

export interface EspectroLayerProps {
  layerId: string;
  /** Camada Espectro/3D logo abaixo (entrada de `sample_input`); null = usa o fundo. */
  inputLayerId?: string | null;
  /** Há vidro acima desta camada: mantém um espelho 2D legível pelo vidro (WebGL). */
  mirrorForGlass?: boolean;
  /** Canvases das camadas (3D e espelhos para o vidro) do compositor. */
  mediaElementsRef?: React.MutableRefObject<Map<string, HTMLVideoElement | HTMLImageElement | HTMLCanvasElement>>;
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
  mediaPlaying?: boolean;
  mediaProgress?: number;
  mediaHasSession?: boolean;
}

const TEX_USAGE = 0x04 | 0x02 | 0x10; // TEXTURE_BINDING | COPY_DST | RENDER_ATTACHMENT

/* eslint-disable @typescript-eslint/no-explicit-any */
const EspectroLayer: React.FC<EspectroLayerProps> = (props) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const propsRef = useRef(props);
  propsRef.current = props;
  const [compiled, setCompiled] = useState<EspectroCompiled | null>(null);
  const [status, setStatus] = useState<"loading" | "ok" | "nogpu" | "error">("loading");
  const [resumeTick, setResumeTick] = useState(0);

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

  // Consumidor da camada de baixo: pede ao produtor a cópia estável da saída dele.
  useEffect(() => {
    const id = props.inputLayerId;
    if (!id) return;
    const o = espectroOutput(id);
    o.consumers++;
    return () => {
      o.consumers--;
      if (o.consumers <= 0 && !o.canvas) releaseEspectroOutput(id);
    };
  }, [props.inputLayerId]);

  // Retoma o loop quando sai da pausa.
  useEffect(() => {
    if (!props.paused) setResumeTick((t) => t + 1);
  }, [props.paused]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !compiled?.pipeline) return;
    let disposed = false;
    let raf = 0;
    let ctx: any = null;
    let device: any = null;
    let gpuCtx: any = null;
    let globalsBuf: any = null;
    let paramsBuf: any = null;
    let bdTex: any = null;
    let bdW = 0;
    let bdH = 0;
    let bdKey = "";
    let prevTex: any = null;
    let inputTex: any = null;
    // Entrada vinda de outra camada Espectro (registro estável na GPU).
    let inputVersion = -1;
    let inputFromFx: any = null;
    // Espelho 2D para o vidro (só com vidro acima): o canvas WebGPU não é legível fora do quadro.
    let mirror: HTMLCanvasElement | null = null;
    let mirrorCtx: CanvasRenderingContext2D | null = null;
    // Pausado (miniaturas): continua desenhando até o fundo carregar, depois para.
    let bdReady = false;
    // Partículas (//@particles): buffer na VRAM, nunca lido de volta pela CPU.
    const particleCount = compiled.particleCompute ? compiled.parse.particles : 0;
    let particleBuf: any = null;
    let particleTex: any = null;
    let particleRw: any = null;
    let particleRo: any = null;
    let particleBindGroup: any = null;
    let inputW = 0;
    let inputH = 0;
    const inputRect = new Float32Array(4);
    let inputRectAge = 999;
    let whiteTex: any = null;
    let clearTex: any = null;
    const userTex: any[] = Array(ESPECTRO_MAX_TEXTURES).fill(null);
    const userSrc: string[] = Array(ESPECTRO_MAX_TEXTURES).fill("");
    let bindGroup: any = null;
    let simBindGroup: any = null;
    let stateTex: any = null;
    let statePrev: any = null;
    let bindDirty = true;
    const feedback = usesPrevious(propsRef.current.effect.code);
    const simulate = Boolean(compiled.simPipeline);
    const globals = new Float32Array(ESPECTRO_GLOBALS_FLOATS);
    const params = new Float32Array(compiled.parse.paramsSize / 4);
    const solid = new Uint8Array(4);
    const input = { x: 0.5, y: 0.5, down: 0, clickX: 0.5, clickY: 0.5, clickAt: -1 };
    const t0 = performance.now();
    let last = t0;
    let lastDraw = 0;
    let frameIndex = 0;

    const toLocal = (e: PointerEvent): [number, number] | null => {
      const r = canvas.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) return null;
      return [(e.clientX - r.left) / r.width, (e.clientY - r.top) / r.height];
    };
    const onMove = (e: PointerEvent) => {
      const p = toLocal(e);
      if (!p) return;
      input.x = p[0];
      input.y = p[1];
    };
    const onDown = (e: PointerEvent) => {
      const p = toLocal(e);
      input.down = 1;
      if (p && p[0] >= 0 && p[0] <= 1 && p[1] >= 0 && p[1] <= 1) {
        input.clickX = p[0];
        input.clickY = p[1];
        input.clickAt = performance.now();
      }
    };
    const onUp = () => {
      input.down = 0;
    };
    window.addEventListener("pointermove", onMove, { passive: true });
    window.addEventListener("pointerdown", onDown, { passive: true });
    window.addEventListener("pointerup", onUp, { passive: true });

    const loadUserTextures = () => {
      const fx = propsRef.current.effect;
      for (const def of compiled.parse.textures) {
        const src = textureSrc(fx, def.name);
        if (src === userSrc[def.slot]) continue;
        userSrc[def.slot] = src;
        userTex[def.slot]?.destroy();
        userTex[def.slot] = null;
        bindDirty = true;
        if (!src) continue;
        const slot = def.slot;
        const img = new Image();
        img.crossOrigin = "anonymous";
        img.src = resolveMediaUrl(src);
        img
          .decode()
          .then(() => createImageBitmap(img))
          .then((bmp) => {
            if (disposed || userSrc[slot] !== src) return;
            const tex = device.createTexture({ size: [bmp.width, bmp.height], format: "rgba8unorm", usage: TEX_USAGE });
            device.queue.copyExternalImageToTexture({ source: bmp }, { texture: tex }, [bmp.width, bmp.height]);
            bmp.close();
            userTex[slot]?.destroy();
            userTex[slot] = tex;
            bindDirty = true;
          })
          .catch(() => {
            /* arquivo ilegível: fica a textura branca */
          });
      }
    };

    // Entrada encadeada: canvas da camada de baixo copiado na GPU (sem passar pela CPU).
    const updateInputRect = (p: EspectroLayerProps, el: Element) => {
      const mine = canvas.getBoundingClientRect();
      const theirs = el.getBoundingClientRect();
      const fw = Math.max(1, p.frameW);
      const fh = Math.max(1, p.frameH);
      const sx = p.width > 0 ? mine.width / p.width : 1;
      const sy = p.height > 0 ? mine.height / p.height : 1;
      const rootLeft = mine.left - (p.centerX - p.width / 2) * sx;
      const rootTop = mine.top - (p.centerY - p.height / 2) * sy;
      inputRect[0] = (theirs.left - rootLeft) / sx / fw;
      inputRect[1] = (theirs.top - rootTop) / sy / fh;
      inputRect[2] = theirs.width / sx / fw;
      inputRect[3] = theirs.height / sy / fh;
    };

    const ensureInput = (p: EspectroLayerProps) => {
      // 1) Efeito Espectro abaixo: usa a cópia estável dele direto (sem cópia extra aqui).
      const fx = p.inputLayerId ? espectroOutput(p.inputLayerId) : null;
      if (fx && fx.canvas) {
        if (inputTex) {
          inputTex.destroy();
          inputTex = null;
        }
        if (fx.version !== inputVersion || inputFromFx !== fx.tex) {
          inputVersion = fx.version;
          inputFromFx = fx.tex;
          inputRectAge = 999;
          bindDirty = true;
        }
        if (fx.tex && ++inputRectAge > 30) {
          inputRectAge = 0;
          updateInputRect(p, fx.canvas);
        }
        return;
      }
      if (inputFromFx) {
        inputFromFx = null;
        bindDirty = true;
      }
      // 2) Cena 3D abaixo: canvas WebGL copiado para a GPU.
      const el = p.inputLayerId ? (p.mediaElementsRef?.current.get(p.inputLayerId) as HTMLCanvasElement | undefined) : undefined;
      if (!el || !(el instanceof HTMLCanvasElement) || el.width < 1 || el.height < 1) {
        if (inputTex) {
          inputTex.destroy();
          inputTex = null;
          bindDirty = true;
        }
        return;
      }
      if (!inputTex || inputW !== el.width || inputH !== el.height) {
        inputTex?.destroy();
        inputTex = device.createTexture({ size: [el.width, el.height], format: "rgba8unorm", usage: TEX_USAGE });
        inputW = el.width;
        inputH = el.height;
        inputRectAge = 999;
        bindDirty = true;
      }
      try {
        device.queue.copyExternalImageToTexture({ source: el }, { texture: inputTex }, [inputW, inputH]);
      } catch {
        /* quadro da entrada indisponível: mantém o anterior */
      }
      // Onde a entrada está no quadro: pelo DOM, relativo ao próprio canvas (a cada ~0,5 s).
      if (++inputRectAge > 30) {
        inputRectAge = 0;
        updateInputRect(p, el);
      }
    };

    const ensureBackdrop = (bd: BackdropSourceInfo | null) => {
      const el = bd?.element as any;
      const w = bd ? Math.max(1, Math.round((el?.videoWidth || el?.naturalWidth || el?.width || bd.intrinsicW) as number)) : 1;
      const h = bd ? Math.max(1, Math.round((el?.videoHeight || el?.naturalHeight || el?.height || bd.intrinsicH) as number)) : 1;
      if (!bdTex || bdW !== w || bdH !== h) {
        bdTex?.destroy();
        bdTex = device.createTexture({ size: [w, h], format: "rgba8unorm", usage: TEX_USAGE });
        bdW = w;
        bdH = h;
        bdKey = "";
        bindDirty = true;
      }
      const key = bd ? bd.key : `solid:${propsRef.current.fallbackColor}`;
      const live = bd ? bd.kind === "video" || bd.live === true : false;
      if (!live && key === bdKey) return;
      try {
        if (bd && el) {
          const ready = bd.kind !== "video" || (el as HTMLVideoElement).readyState >= 2;
          if (ready) {
            device.queue.copyExternalImageToTexture({ source: el }, { texture: bdTex }, [w, h]);
            bdReady = true;
          }
        } else {
          const fc = propsRef.current.fallbackColor;
          const c = parseHexColor(/^#[0-9a-fA-F]{6,8}$/.test(fc) ? fc : "#060608");
          solid[0] = c[0] * 255;
          solid[1] = c[1] * 255;
          solid[2] = c[2] * 255;
          solid[3] = 255;
          device.queue.writeTexture({ texture: bdTex }, solid, { bytesPerRow: 4 }, [1, 1]);
          bdReady = true;
        }
        bdKey = key;
      } catch {
        // Mídia ainda não decodificada / origem bloqueada: tenta de novo no próximo quadro.
      }
    };

    const frame = (now: number) => {
      raf = 0;
      if (disposed) return;
      const p = propsRef.current;
      const caps = powerCapsFor(p.powerState);
      const fpsCap = Math.min(p.isThumbnail ? 15 : 60, caps.fpsCap ?? 60);
      if (now - lastDraw >= 1000 / fpsCap - 1) {
        lastDraw = now;
        // `//@scale`: efeito em resolução menor; o canvas é ampliado pelo CSS.
        const dpr = Math.min(window.devicePixelRatio || 1, caps.dprCap) * caps.renderScaleCap * compiled.parse.scale;
        const cw = Math.max(1, Math.round(p.width * dpr));
        const ch = Math.max(1, Math.round(p.height * dpr));
        if (canvas.width !== cw || canvas.height !== ch) {
          canvas.width = cw;
          canvas.height = ch;
          if (feedback) {
            prevTex?.destroy();
            prevTex = device.createTexture({ size: [cw, ch], format: gpuCtx.format, usage: TEX_USAGE });
            bindDirty = true;
          }
          if (particleCount > 0) {
            particleTex?.destroy();
            particleTex = device.createTexture({ size: [cw, ch], format: "rgba8unorm", usage: TEX_USAGE });
            bindDirty = true;
          }
          if (simulate) {
            stateTex?.destroy();
            statePrev?.destroy();
            const usage = TEX_USAGE | 0x01; // + COPY_SRC
            stateTex = device.createTexture({ size: [cw, ch], format: ESPECTRO_STATE_FORMAT, usage });
            statePrev = device.createTexture({ size: [cw, ch], format: ESPECTRO_STATE_FORMAT, usage });
            bindDirty = true;
          }
          frameIndex = 0;
        }
        ensureBackdrop(p.backdrop);
        loadUserTextures();
        ensureInput(p);
        if (bindDirty || !bindGroup) {
          bindDirty = false;
          const view = (t: any) => (t ?? whiteTex).createView();
          const entries = (state: any, particles: any = particleTex) => [
              { binding: 0, resource: { buffer: globalsBuf } },
              { binding: 1, resource: bdTex.createView() },
              { binding: 2, resource: gpuCtx.sampler },
              { binding: 3, resource: { buffer: paramsBuf } },
              { binding: 4, resource: ((feedback ? prevTex : null) ?? clearTex).createView() },
              { binding: 5, resource: view(userTex[0]) },
              { binding: 6, resource: view(userTex[1]) },
              { binding: 7, resource: view(userTex[2]) },
              { binding: 8, resource: view(userTex[3]) },
              { binding: 9, resource: gpuCtx.repeatSampler },
              { binding: 10, resource: (state ?? clearTex).createView() },
              { binding: 11, resource: (inputFromFx ?? inputTex ?? bdTex).createView() },
              { binding: 12, resource: (particles ?? clearTex).createView() },
            ];
          // `effect` lê o estado novo; `simulate` lê o anterior.
          bindGroup = device.createBindGroup({ layout: gpuCtx.layout, entries: entries(stateTex) });
          simBindGroup = simulate ? device.createBindGroup({ layout: gpuCtx.layout, entries: entries(statePrev) }) : null;
          // O desenho das partículas escreve em particleTex: o grupo 0 dele não pode amostrá-la.
          particleBindGroup = particleCount > 0 ? device.createBindGroup({ layout: gpuCtx.layout, entries: entries(stateTex, null) }) : null;
        }
        const fw = Math.max(1, p.frameW);
        const fh = Math.max(1, p.frameH);
        const sig = caps.allowSignalBindings ? p.reactiveSignalRef?.current : undefined;
        const bd = p.backdrop;
        globals[0] = cw;
        globals[1] = ch;
        globals[2] = (now - t0) / 1000;
        globals[3] = Math.min(0.25, (now - last) / 1000);
        globals[4] = input.x;
        globals[5] = input.y;
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
        globals[16] = sig?.level ?? 0;
        globals[17] = sig?.bass ?? 0;
        globals[18] = sig?.mid ?? 0;
        globals[19] = sig?.treble ?? 0;
        globals[20] = 0; // batida: o Studio não tem detector (o motor tem)
        globals[21] = (p.batteryPercent ?? 100) / 100;
        globals[22] = bd ? 1 : 0;
        globals[23] = bd && bd.kind !== "canvas" ? 1 : 0;
        globals[24] = input.clickX;
        globals[25] = input.clickY;
        globals[26] = input.clickAt < 0 ? 1e6 : (now - input.clickAt) / 1000;
        globals[27] = frameIndex;
        globals[28] = input.down;
        globals[29] = sig?.key_pulse ?? 0;
        globals[30] = sig?.typing_energy ?? 0;
        globals[31] = 0;
        globals[32] = p.mediaPlaying ? 1 : 0;
        globals[33] = p.mediaProgress ?? 0;
        globals[34] = p.mediaHasSession ? 1 : 0;
        globals[35] = 0;
        if (inputTex || inputFromFx) {
          globals[36] = inputRect[0];
          globals[37] = inputRect[1];
          globals[38] = inputRect[2];
          globals[39] = inputRect[3];
        } else {
          globals[36] = globals[12];
          globals[37] = globals[13];
          globals[38] = globals[14];
          globals[39] = globals[15];
        }
        last = now;
        frameIndex++;
        packEspectroParams(compiled.parse, p.effect.params, params);
        device.queue.writeBuffer(globalsBuf, 0, globals);
        device.queue.writeBuffer(paramsBuf, 0, params);
        const enc = device.createCommandEncoder();
        if (simulate && simBindGroup) {
          const sim = enc.beginRenderPass({
            colorAttachments: [{ view: stateTex.createView(), loadOp: "clear", storeOp: "store", clearValue: { r: 0, g: 0, b: 0, a: 0 } }],
          });
          sim.setPipeline(compiled.simPipeline);
          sim.setBindGroup(0, simBindGroup);
          sim.draw(3);
          sim.end();
          enc.copyTextureToTexture({ texture: stateTex }, { texture: statePrev }, [cw, ch]);
        }
        if (particleCount > 0 && particleBindGroup) {
          const cp = enc.beginComputePass();
          cp.setPipeline(compiled.particleCompute);
          cp.setBindGroup(0, particleBindGroup);
          cp.setBindGroup(1, particleRw);
          cp.dispatchWorkgroups(Math.ceil(particleCount / 64));
          cp.end();
          const pp = enc.beginRenderPass({
            colorAttachments: [{ view: particleTex.createView(), loadOp: "clear", storeOp: "store", clearValue: { r: 0, g: 0, b: 0, a: 0 } }],
          });
          pp.setPipeline(compiled.particleRender);
          pp.setBindGroup(0, particleBindGroup);
          pp.setBindGroup(1, particleRo);
          pp.draw(6, particleCount);
          pp.end();
        }
        const out = ctx.getCurrentTexture();
        const pass = enc.beginRenderPass({
          colorAttachments: [
            { view: out.createView(), loadOp: "clear", storeOp: "store", clearValue: { r: 0, g: 0, b: 0, a: 0 } },
          ],
        });
        pass.setPipeline(compiled.pipeline);
        pass.setBindGroup(0, bindGroup);
        pass.draw(3);
        pass.end();
        // Cópia estável para o efeito de cima (só se alguém a usa).
        const outReg = espectroOutput(p.layerId);
        outReg.canvas = canvas;
        if (outReg.consumers > 0) {
          if (!outReg.tex || outReg.w !== cw || outReg.h !== ch) {
            outReg.tex?.destroy();
            outReg.tex = device.createTexture({ size: [cw, ch], format: gpuCtx.format, usage: TEX_USAGE });
            outReg.w = cw;
            outReg.h = ch;
            outReg.version++;
          }
          enc.copyTextureToTexture({ texture: out }, { texture: outReg.tex }, [cw, ch]);
        }
        // Memória entre quadros: o quadro de agora vira o `sample_previous` do próximo.
        if (feedback && prevTex) enc.copyTextureToTexture({ texture: out }, { texture: prevTex }, [cw, ch]);
        device.queue.submit([enc.finish()]);
        // Espelho 2D para o vidro, ainda dentro do quadro (o canvas WebGPU ainda é legível).
        if (p.mirrorForGlass) {
          if (!mirror) {
            mirror = document.createElement("canvas");
            mirrorCtx = mirror.getContext("2d");
            p.mediaElementsRef?.current.set(p.layerId, mirror);
          }
          if (mirror.width !== cw || mirror.height !== ch) {
            mirror.width = cw;
            mirror.height = ch;
          }
          mirrorCtx?.clearRect(0, 0, cw, ch);
          mirrorCtx?.drawImage(canvas, 0, 0);
        } else if (mirror) {
          p.mediaElementsRef?.current.delete(p.layerId);
          mirror = null;
          mirrorCtx = null;
        }
      }
      // Pausado (miniaturas do Hub): segue até o fundo carregar, para não congelar um quadro vazio.
      if ((!p.paused || !bdReady) && document.visibilityState === "visible") raf = requestAnimationFrame(frame);
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
      ctx.configure({ device, format: g.format, alphaMode: "premultiplied", usage: 0x10 | 0x01 }); // RENDER_ATTACHMENT | COPY_SRC
      globalsBuf = device.createBuffer({ size: ESPECTRO_GLOBALS_FLOATS * 4, usage: 0x40 | 0x08 }); // UNIFORM | COPY_DST
      if (particleCount > 0) {
        // Zerado na criação: life = 0 faz cada partícula nascer no 1º quadro.
        particleBuf = device.createBuffer({ size: particleCount * ESPECTRO_PARTICLE_BYTES, usage: 0x80 }); // STORAGE
        particleRw = device.createBindGroup({ layout: g.particleRwLayout, entries: [{ binding: 0, resource: { buffer: particleBuf } }] });
        particleRo = device.createBindGroup({ layout: g.particleRoLayout, entries: [{ binding: 1, resource: { buffer: particleBuf } }] });
      }
      paramsBuf = device.createBuffer({ size: compiled.parse.paramsSize, usage: 0x40 | 0x08 });
      whiteTex = device.createTexture({ size: [1, 1], format: "rgba8unorm", usage: TEX_USAGE });
      device.queue.writeTexture({ texture: whiteTex }, new Uint8Array([255, 255, 255, 255]), { bytesPerRow: 4 }, [1, 1]);
      clearTex = device.createTexture({ size: [1, 1], format: "rgba8unorm", usage: TEX_USAGE }); // zerada = transparente
      raf = requestAnimationFrame(frame);
    });

    return () => {
      disposed = true;
      if (raf) cancelAnimationFrame(raf);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("pointerup", onUp);
      document.removeEventListener("visibilitychange", onVis);
      globalsBuf?.destroy();
      paramsBuf?.destroy();
      bdTex?.destroy();
      prevTex?.destroy();
      inputTex?.destroy();
      particleTex?.destroy();
      particleBuf?.destroy();
      releaseEspectroOutput(propsRef.current.layerId);
      if (mirror) propsRef.current.mediaElementsRef?.current.delete(propsRef.current.layerId);
      stateTex?.destroy();
      statePrev?.destroy();
      whiteTex?.destroy();
      clearTex?.destroy();
      for (const t of userTex) t?.destroy();
      try {
        ctx?.unconfigure();
      } catch {
        /* contexto já perdido */
      }
    };
  }, [compiled, resumeTick]);

  return (
    <div style={{ width: `${props.width}px`, height: `${props.height}px`, position: "relative" }}>
      <canvas
        ref={canvasRef}
        style={{ width: "100%", height: "100%", display: "block" }}
      />
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
