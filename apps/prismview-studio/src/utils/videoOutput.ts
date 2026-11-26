// Opções de saída do vídeo no desktop (motor nativo): teto de FPS e altura de decodificação.
// Puro (sem DOM): usado pelo inspetor e testado em `tests/videoOutput.test.ts`.

/** Taxas comuns: a estimativa medida no navegador encaixa na mais próxima. */
const COMMON_FPS = [12, 15, 23.976, 24, 25, 29.97, 30, 48, 50, 59.94, 60, 72, 90, 100, 120, 144, 165, 240];

/** Alturas padrão oferecidas no seletor de resolução. */
const STANDARD_HEIGHTS = [4320, 2880, 2160, 1440, 1080, 720, 540, 480, 360];

/**
 * FPS do arquivo a partir de intervalos medidos entre quadros (segundos de mídia). Usa a mediana
 * (imune a quadros perdidos) e encaixa numa taxa comum se estiver a ≤ 3%.
 */
export function estimateFps(deltas: readonly number[]): number | null {
  const valid = deltas.filter((d) => Number.isFinite(d) && d > 0.001 && d < 1).sort((a, b) => a - b);
  if (valid.length < 3) return null;
  const median = valid[Math.floor(valid.length / 2)];
  const fps = 1 / median;
  let best = fps;
  let bestErr = Infinity;
  for (const c of COMMON_FPS) {
    const err = Math.abs(c - fps) / c;
    if (err < bestErr) {
      bestErr = err;
      best = c;
    }
  }
  return bestErr <= 0.03 ? best : Math.round(fps);
}

/** Maior FPS do slider (inteiro, arredondado para cima: 29,97 → 30). */
export function maxFpsFor(fileFps: number | null): number {
  return Math.max(1, Math.ceil(fileFps ?? 60));
}

/** Altura automática: a do monitor se o vídeo for mais alto que ele, senão a do arquivo. */
export function autoDecodeHeight(videoHeight: number, monitorHeight: number | null): number {
  if (monitorHeight && monitorHeight > 0 && videoHeight > monitorHeight) return monitorHeight;
  return videoHeight;
}

export interface HeightOption {
  height: number;
  label: string;
  /** Escolher esta = automático (salva sem `decodeHeight`). */
  isAuto: boolean;
}

/**
 * Opções do seletor: alturas padrão até a maior entre o arquivo e o monitor (dá para subir ou
 * descer), mais a do arquivo e a do monitor. Ordem decrescente, sem repetição.
 */
export function heightOptions(videoHeight: number, monitorHeight: number | null): HeightOption[] {
  const top = Math.max(videoHeight, monitorHeight ?? 0);
  const auto = autoDecodeHeight(videoHeight, monitorHeight);
  const set = new Set<number>([videoHeight, auto]);
  if (monitorHeight && monitorHeight > 0) set.add(monitorHeight);
  for (const h of STANDARD_HEIGHTS) if (h <= top) set.add(h);
  return [...set]
    .filter((h) => h >= 144)
    .sort((a, b) => b - a)
    .map((h) => {
      const tags: string[] = [];
      if (h === videoHeight) tags.push("original");
      if (monitorHeight && h === monitorHeight) tags.push("monitor");
      if (h === auto) tags.push("padrão");
      return { height: h, label: `${h}p${tags.length ? ` (${tags.join(", ")})` : ""}`, isAuto: h === auto };
    });
}

/** Valor salvo para o FPS escolhido: no máximo do arquivo = sem teto (`undefined`). */
export function fpsLimitFor(value: number, maxFps: number): number | undefined {
  const v = Math.max(0, Math.round(value));
  return v >= maxFps ? undefined : v;
}
