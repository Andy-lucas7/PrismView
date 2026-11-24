/**
 * Painel único do Estúdio (propriedades em cima, camadas embaixo): proporção da divisória e
 * estado aberto/recolhido, persistidos em `localStorage`. Funções puras (sem DOM) para testar.
 */

export const SPLIT_KEY = "prismview.studioPanelSplit.v1";
export const PANEL_OPEN_KEY = "prismview.studioPanelOpen.v1";

/** Fração da altura ocupada pelas propriedades. */
export const SPLIT_DEFAULT = 0.58;
export const SPLIT_MIN = 0.25;
export const SPLIT_MAX = 0.8;
/** Passo de teclado (setas) e de "Shift + setas". */
export const SPLIT_STEP = 0.03;
export const SPLIT_STEP_LARGE = 0.1;

/** Subconjunto de `Storage` usado aqui (permite injetar um falso nos testes). */
export interface KeyValueStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export function clampSplit(v: number): number {
  if (!Number.isFinite(v)) return SPLIT_DEFAULT;
  return Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, v));
}

/** Proporção guardada; valor ausente, inválido ou fora do intervalo cai no padrão/limite. */
export function readSplit(store: KeyValueStore | null): number {
  if (!store) return SPLIT_DEFAULT;
  try {
    const raw = store.getItem(SPLIT_KEY);
    if (raw === null) return SPLIT_DEFAULT;
    return clampSplit(Number.parseFloat(raw));
  } catch {
    return SPLIT_DEFAULT;
  }
}

export function writeSplit(store: KeyValueStore | null, v: number): void {
  if (!store) return;
  try {
    store.setItem(SPLIT_KEY, clampSplit(v).toFixed(4));
  } catch {
    /* sem storage (modo privado, cota): só não persiste */
  }
}

/** Painel aberto por padrão; só "0" o recolhe. */
export function readPanelOpen(store: KeyValueStore | null): boolean {
  if (!store) return true;
  try {
    return store.getItem(PANEL_OPEN_KEY) !== "0";
  } catch {
    return true;
  }
}

export function writePanelOpen(store: KeyValueStore | null, open: boolean): void {
  if (!store) return;
  try {
    store.setItem(PANEL_OPEN_KEY, open ? "1" : "0");
  } catch {
    /* ignora */
  }
}

/** Proporção para a posição vertical do ponteiro dentro de um contêiner (`top`, `height` em px). */
export function splitFromPointer(clientY: number, top: number, height: number): number {
  if (!(height > 0) || !Number.isFinite(clientY) || !Number.isFinite(top)) return SPLIT_DEFAULT;
  return clampSplit((clientY - top) / height);
}

/** Move a divisória `steps` passos (positivo = para baixo = propriedades maiores). */
export function nudgeSplit(current: number, steps: number, step = SPLIT_STEP): number {
  return clampSplit(clampSplit(current) + steps * step);
}

/**
 * Largura que o canvas deixa livre à direita: painel aberto = largura + margens; recolhido = só a
 * margem. Mantido aqui para o cálculo ficar num lugar só (e testável).
 */
export const PANEL_WIDTH_PX = 352;
export const PANEL_GAP_PX = 16;
export function stageRightInset(open: boolean): number {
  return open ? PANEL_WIDTH_PX + PANEL_GAP_PX * 2 : PANEL_GAP_PX;
}
