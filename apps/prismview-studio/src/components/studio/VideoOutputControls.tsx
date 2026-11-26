// FPS e resolução do vídeo no desktop (motor nativo). Mede o arquivo num <video> oculto
// (dimensões + FPS por `requestVideoFrameCallback`) e a altura do maior monitor.
import React, { useEffect, useState } from "react";
import { invokeCmd, isTauriRuntime, resolveMediaUrl } from "../../types/prism";
import type { MonitorInfoDto } from "../../types/prism";
import { Select } from "../ui/Controls";
import { InspectorSlider } from "../InspectorControls";
import { estimateFps, fpsLimitFor, heightOptions, maxFpsFor } from "../../utils/videoOutput";

interface VideoProbe {
  height: number;
  fps: number | null;
}

/** Quadros medidos para estimar o FPS. */
const FPS_SAMPLES = 12;
/** Cache por arquivo: trocar de camada não mede de novo. */
const probeCache = new Map<string, VideoProbe>();

type FrameMeta = { mediaTime: number };
type RvfcVideo = HTMLVideoElement & {
  requestVideoFrameCallback?: (cb: (now: number, meta: FrameMeta) => void) => number;
};

function probeVideo(src: string, signal: { cancelled: boolean }): Promise<VideoProbe | null> {
  const cached = probeCache.get(src);
  if (cached) return Promise.resolve(cached);
  return new Promise((resolve) => {
    const v = document.createElement("video") as RvfcVideo;
    v.muted = true;
    v.playsInline = true;
    v.preload = "auto";
    let height = 0;
    const deltas: number[] = [];
    let last: number | null = null;
    let done = false;
    const finish = (fps: number | null) => {
      if (done) return;
      done = true;
      v.pause();
      v.removeAttribute("src");
      v.load();
      const probe = height > 0 ? { height, fps } : null;
      if (probe && !signal.cancelled) probeCache.set(src, probe);
      resolve(probe);
    };
    const timeout = window.setTimeout(() => finish(estimateFps(deltas)), 4000);
    const onFrame = (_now: number, meta: FrameMeta) => {
      if (done || signal.cancelled) return finish(null);
      if (last !== null) deltas.push(meta.mediaTime - last);
      last = meta.mediaTime;
      if (deltas.length >= FPS_SAMPLES) {
        window.clearTimeout(timeout);
        finish(estimateFps(deltas));
      } else {
        v.requestVideoFrameCallback?.(onFrame);
      }
    };
    v.addEventListener("loadedmetadata", () => {
      height = v.videoHeight;
      if (!v.requestVideoFrameCallback) {
        window.clearTimeout(timeout);
        finish(null);
        return;
      }
      v.requestVideoFrameCallback(onFrame);
      void v.play().catch(() => finish(null));
    });
    v.addEventListener("error", () => {
      window.clearTimeout(timeout);
      finish(null);
    });
    v.src = resolveMediaUrl(src);
  });
}

let monitorHeightPromise: Promise<number | null> | null = null;
function maxMonitorHeight(): Promise<number | null> {
  monitorHeightPromise ??= (async () => {
    if (isTauriRuntime()) {
      try {
        const monitors = await invokeCmd<MonitorInfoDto[]>("list_monitors");
        const h = Math.max(0, ...monitors.map((m) => m.height));
        if (h > 0) return h;
      } catch {
        /* cai no navegador */
      }
    }
    const h = Math.round(window.screen.height * (window.devicePixelRatio || 1));
    return h > 0 ? h : null;
  })();
  return monitorHeightPromise;
}

interface Props {
  src: string;
  fpsLimit: number | undefined;
  decodeHeight: number | undefined;
  sliderHandlers: {
    onBeginTx: () => void;
    onCommitTx: () => void;
    onRecordStep: () => void;
    onFlushSave?: () => void;
  };
  onFpsChange: (fpsLimit: number | undefined) => void;
  onHeightChange: (decodeHeight: number | undefined) => void;
}

export const VideoOutputControls: React.FC<Props> = ({
  src,
  fpsLimit,
  decodeHeight,
  sliderHandlers,
  onFpsChange,
  onHeightChange,
}) => {
  const [probe, setProbe] = useState<VideoProbe | null>(() => probeCache.get(src) ?? null);
  const [monitorH, setMonitorH] = useState<number | null>(null);

  useEffect(() => {
    const signal = { cancelled: false };
    setProbe(probeCache.get(src) ?? null);
    if (src) void probeVideo(src, signal).then((p) => !signal.cancelled && setProbe(p));
    return () => {
      signal.cancelled = true;
    };
  }, [src]);

  useEffect(() => {
    let alive = true;
    void maxMonitorHeight().then((h) => alive && setMonitorH(h));
    return () => {
      alive = false;
    };
  }, []);

  const maxFps = maxFpsFor(probe?.fps ?? null);
  const fpsValue = Math.min(fpsLimit ?? maxFps, maxFps);
  const options = probe ? heightOptions(probe.height, monitorH) : [];
  const autoOption = options.find((o) => o.isAuto);
  const selected = decodeHeight ?? autoOption?.height ?? "";

  return (
    <div className="space-y-2 pt-2 border-t border-white/10">
      <span className="block text-[11px] font-medium uppercase tracking-wider text-[#A1A1AA]">
        No desktop
      </span>
      <InspectorSlider
        label={fpsValue === 0 ? "FPS do vídeo (imagem parada)" : `FPS do vídeo${probe?.fps ? ` · arquivo ${probe.fps}` : ""}`}
        value={fpsValue}
        min={0}
        max={maxFps}
        step={1}
        unit=" fps"
        {...sliderHandlers}
        onChange={(v) => onFpsChange(fpsLimitFor(v, maxFps))}
      />
      <div className="flex items-center justify-between gap-2 frosted-chrome-card px-3 py-2">
        <span className="text-xs text-[#A1A1AA]">Resolução</span>
        {options.length > 0 ? (
          <Select
            aria-label="Resolução de decodificação do vídeo"
            value={selected}
            onChange={(e) => {
              const h = Number(e.target.value);
              const opt = options.find((o) => o.height === h);
              onHeightChange(!opt || opt.isAuto ? undefined : h);
            }}
          >
            {options.map((o) => (
              <option key={o.height} value={o.height}>
                {o.label}
              </option>
            ))}
          </Select>
        ) : (
          <span className="text-[11px] text-[#71717A]">{src ? "lendo o vídeo…" : "sem vídeo"}</span>
        )}
      </div>
    </div>
  );
};
