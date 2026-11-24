// Funções puras de alinhamento, distribuição, tamanho, rotação, espelhamento e guias inteligentes (Etapa 8).
// Zero dependência de React/DOM e zero alocação no loop quente de arraste (`snapAxisInto`).
import type { SceneLayer } from "../types/prismBase";

interface Loc {
  layer: SceneLayer;
  parent: SceneLayer | null;
}

function findLoc(
  layers: readonly SceneLayer[],
  id: string,
  parent: SceneLayer | null = null
): Loc | null {
  for (const layer of layers) {
    if (layer.id === id) return { layer, parent };
    if (layer.children?.length) {
      const nested = findLoc(layer.children, id, layer);
      if (nested) return nested;
    }
  }
  return null;
}

function pruneNested(layers: readonly SceneLayer[], ids: readonly string[]): string[] {
  const set = new Set(ids);
  return ids.filter((id) => {
    let cur = findLoc(layers, id);
    while (cur?.parent) {
      if (set.has(cur.parent.id)) return false;
      cur = findLoc(layers, cur.parent.id);
    }
    return true;
  });
}

function updateMany(
  layers: SceneLayer[],
  ids: ReadonlySet<string>,
  fn: (layer: SceneLayer) => SceneLayer
): SceneLayer[] {
  if (ids.size === 0) return layers;
  let changed = false;
  const next = layers.map((layer) => {
    let cur = layer;
    if (ids.has(layer.id)) {
      const patched = fn(layer);
      if (patched !== layer) {
        cur = patched;
        changed = true;
      }
    }
    if (cur.children?.length) {
      const ch = updateMany(cur.children, ids, fn);
      if (ch !== cur.children) {
        cur = { ...cur, children: ch };
        changed = true;
      }
    }
    return cur;
  });
  return changed ? next : layers;
}

export function collectLayerIdsByKindPure(
  layers: readonly SceneLayer[],
  kind: SceneLayer["kind"]
): string[] {
  const out: string[] = [];
  for (const layer of layers) {
    if (layer.kind === kind) out.push(layer.id);
    if (layer.children?.length) {
      out.push(...collectLayerIdsByKindPure(layer.children, kind));
    }
  }
  return out;
}

export function moveLayersZExtremePure(
  layers: SceneLayer[],
  ids: ReadonlySet<string>,
  toFront: boolean
): SceneLayer[] {
  if (ids.size === 0) return layers;
  const keep: SceneLayer[] = [];
  const picked: SceneLayer[] = [];
  for (const layer of layers) {
    if (ids.has(layer.id)) picked.push(layer);
    else keep.push(layer);
  }
  let next = toFront ? [...keep, ...picked] : [...picked, ...keep];
  let changed = next.some((item, idx) => item !== layers[idx]);

  next = next.map((layer) => {
    if (!layer.children?.length) return layer;
    const movedChildren = moveLayersZExtremePure(layer.children, ids, toFront);
    if (movedChildren !== layer.children) {
      changed = true;
      return { ...layer, children: movedChildren };
    }
    return layer;
  });

  return changed ? next : layers;
}

export type AlignMode = "left" | "centerX" | "right" | "top" | "centerY" | "bottom";
export type AlignReference = "screen" | "selection";
export type DistributeAxis = "horizontal" | "vertical";
export type MatchSizeMode = "width" | "height" | "both";
export type FlipAxis = "horizontal" | "vertical";

export interface LogicalSize {
  w: number;
  h: number;
}

export interface SnapAxisBuffer {
  snapped: number;
  guide: number | null;
}

/**
 * Estima o tamanho lógico `(w, h)` de uma camada (usando medições reais do DOM quando disponíveis,
 * ou heurística tipográfica fiel para `TEXT`).
 */
export function estimateLayerLogicalSize(
  layer: SceneLayer,
  measured?: ReadonlyMap<string, LogicalSize>
): LogicalSize {
  const m = measured?.get(layer.id);
  if (m && m.w > 0 && m.h > 0) return m;
  if (layer.kind === "TEXT" && layer.text) {
    const fs = Math.max(10, layer.text.fontSize || 96);
    const lines = Math.max(1, (layer.text.content || " ").split("\n").length);
    const longest = Math.max(
      1,
      ...(layer.text.content || " ").split("\n").map((ln) => ln.length)
    );
    const estNaturalW = Math.max(fs * 0.8, longest * fs * (0.56 + Math.max(0, layer.text.letterSpacing)));
    const w = layer.text.maxWidth && layer.text.maxWidth > 0 ? layer.text.maxWidth : estNaturalW;
    const stretch = Math.max(1, layer.text.stemStretch || 1);
    const h = lines * fs * 1.06 * stretch;
    return { w, h };
  }
  return {
    w: Math.max(1, layer.width || 1),
    h: Math.max(1, layer.height || 1),
  };
}

function clamp01(n: number): number {
  return Math.max(0, Math.min(1, Number(n.toFixed(4))));
}

function parentSizeFor(
  parent: SceneLayer | null,
  canvasW: number,
  canvasH: number
): { w: number; h: number } {
  if (parent) {
    return { w: Math.max(1, parent.width), h: Math.max(1, parent.height) };
  }
  return { w: Math.max(1, canvasW), h: Math.max(1, canvasH) };
}

/**
 * Alinha 1+ camadas à tela/contêiner (`reference = "screen"`, ou sempre que há apenas 1 item)
 * ou entre os próprios selecionados (`reference = "selection"` com 2+ itens).
 */
export function alignLayers(
  layers: SceneLayer[],
  ids: readonly string[],
  mode: AlignMode,
  reference: AlignReference,
  canvasW: number,
  canvasH: number,
  measured?: ReadonlyMap<string, LogicalSize>
): SceneLayer[] {
  const cleanIds = pruneNested(layers, ids);
  const entries: Array<{
    layer: SceneLayer;
    parent: SceneLayer | null;
    pw: number;
    ph: number;
    halfWNorm: number;
    halfHNorm: number;
  }> = [];

  for (const id of cleanIds) {
    const loc = findLoc(layers, id);
    if (!loc || loc.layer.locked) continue;
    const { w: pw, h: ph } = parentSizeFor(loc.parent, canvasW, canvasH);
    const sz = estimateLayerLogicalSize(loc.layer, measured);
    entries.push({
      layer: loc.layer,
      parent: loc.parent,
      pw,
      ph,
      halfWNorm: Math.min(0.5, sz.w / pw / 2),
      halfHNorm: Math.min(0.5, sz.h / ph / 2),
    });
  }
  if (entries.length === 0) return layers;

  const useSelection = reference === "selection" && entries.length >= 2;

  let minLeft = Infinity;
  let maxRight = -Infinity;
  let minTop = Infinity;
  let maxBottom = -Infinity;
  if (useSelection) {
    for (const e of entries) {
      minLeft = Math.min(minLeft, e.layer.x - e.halfWNorm);
      maxRight = Math.max(maxRight, e.layer.x + e.halfWNorm);
      minTop = Math.min(minTop, e.layer.y - e.halfHNorm);
      maxBottom = Math.max(maxBottom, e.layer.y + e.halfHNorm);
    }
  } else {
    minLeft = 0;
    maxRight = 1;
    minTop = 0;
    maxBottom = 1;
  }
  const midX = (minLeft + maxRight) * 0.5;
  const midY = (minTop + maxBottom) * 0.5;

  const updates = new Map<string, { x: number; y: number }>();
  for (const e of entries) {
    let nx = e.layer.x;
    let ny = e.layer.y;
    switch (mode) {
      case "left":
        nx = minLeft + e.halfWNorm;
        break;
      case "centerX":
        nx = midX;
        break;
      case "right":
        nx = maxRight - e.halfWNorm;
        break;
      case "top":
        ny = minTop + e.halfHNorm;
        break;
      case "centerY":
        ny = midY;
        break;
      case "bottom":
        ny = maxBottom - e.halfHNorm;
        break;
    }
    updates.set(e.layer.id, { x: clamp01(nx), y: clamp01(ny) });
  }

  return updateMany(layers, new Set(updates.keys()), (l) => {
    const u = updates.get(l.id);
    if (!u || (u.x === l.x && u.y === l.y)) return l;
    return { ...l, x: u.x, y: u.y };
  });
}

/**
 * Centraliza na tela `(0.5, 0.5)`:
 * - 1 camada: posiciona em `(0.5, 0.5)`.
 * - 2+ camadas: translada o conjunto em bloco rígido para que o centro da caixa envolvente fique em `(0.5, 0.5)`.
 */
export function centerLayersOnScreen(
  layers: SceneLayer[],
  ids: readonly string[],
  canvasW = 1920,
  canvasH = 1080,
  measured?: ReadonlyMap<string, LogicalSize>
): SceneLayer[] {
  const cleanIds = pruneNested(layers, ids);
  const entries = cleanIds
    .map((id) => findLoc(layers, id))
    .filter((loc): loc is NonNullable<typeof loc> => Boolean(loc && !loc.layer.locked));
  if (entries.length === 0) return layers;

  if (entries.length === 1) {
    const id = entries[0].layer.id;
    return updateMany(layers, new Set([id]), (l) =>
      l.x === 0.5 && l.y === 0.5 ? l : { ...l, x: 0.5, y: 0.5 }
    );
  }

  let minLeft = Infinity;
  let maxRight = -Infinity;
  let minTop = Infinity;
  let maxBottom = -Infinity;
  for (const loc of entries) {
    const { w: pw, h: ph } = parentSizeFor(loc.parent, canvasW, canvasH);
    const sz = estimateLayerLogicalSize(loc.layer, measured);
    const hw = Math.min(0.5, sz.w / pw / 2);
    const hh = Math.min(0.5, sz.h / ph / 2);
    minLeft = Math.min(minLeft, loc.layer.x - hw);
    maxRight = Math.max(maxRight, loc.layer.x + hw);
    minTop = Math.min(minTop, loc.layer.y - hh);
    maxBottom = Math.max(maxBottom, loc.layer.y + hh);
  }
  const dx = 0.5 - (minLeft + maxRight) * 0.5;
  const dy = 0.5 - (minTop + maxBottom) * 0.5;
  if (Math.abs(dx) < 1e-5 && Math.abs(dy) < 1e-5) return layers;

  const targetIds = new Set(entries.map((e) => e.layer.id));
  return updateMany(layers, targetIds, (l) => ({
    ...l,
    x: clamp01(l.x + dx),
    y: clamp01(l.y + dy),
  }));
}

/**
 * Distribui 3+ camadas uniformemente ao longo do eixo horizontal ou vertical, mantendo fixas as
 * duas extremas e espaçando igualmente os vãos livres (ou centros caso os itens se sobreponham).
 */
export function distributeLayers(
  layers: SceneLayer[],
  ids: readonly string[],
  axis: DistributeAxis,
  canvasW = 1920,
  canvasH = 1080,
  measured?: ReadonlyMap<string, LogicalSize>
): SceneLayer[] {
  const cleanIds = pruneNested(layers, ids);
  const items = cleanIds
    .map((id) => findLoc(layers, id))
    .filter((loc): loc is NonNullable<typeof loc> => Boolean(loc && !loc.layer.locked))
    .map((loc) => {
      const { w: pw, h: ph } = parentSizeFor(loc.parent, canvasW, canvasH);
      const sz = estimateLayerLogicalSize(loc.layer, measured);
      const extent = axis === "horizontal" ? sz.w / pw : sz.h / ph;
      const pos = axis === "horizontal" ? loc.layer.x : loc.layer.y;
      return { id: loc.layer.id, pos, extent };
    });

  if (items.length < 3) return layers;

  items.sort((a, b) => a.pos - b.pos);
  const first = items[0];
  const last = items[items.length - 1];

  const outerMin = first.pos - first.extent * 0.5;
  const outerMax = last.pos + last.extent * 0.5;
  const totalExtent = items.reduce((acc, it) => acc + it.extent, 0);
  const outerSpan = outerMax - outerMin;

  const nextPosById = new Map<string, number>();
  if (outerSpan > totalExtent + 1e-4) {
    // Espaçamento uniforme pelas bordas (gaps iguais entre itens)
    const gap = (outerSpan - totalExtent) / (items.length - 1);
    let cursor = outerMin;
    for (let i = 0; i < items.length; i++) {
      const center = cursor + items[i].extent * 0.5;
      if (i > 0 && i < items.length - 1) {
        nextPosById.set(items[i].id, clamp01(center));
      }
      cursor += items[i].extent + gap;
    }
  } else {
    // Fallback quando as caixas são maiores que o vão total: distribui centros igualmente
    const step = (last.pos - first.pos) / (items.length - 1);
    for (let i = 1; i < items.length - 1; i++) {
      nextPosById.set(items[i].id, clamp01(first.pos + step * i));
    }
  }

  if (nextPosById.size === 0) return layers;
  return updateMany(layers, new Set(nextPosById.keys()), (l) => {
    const v = nextPosById.get(l.id);
    if (v === undefined) return l;
    return axis === "horizontal" ? { ...l, x: v } : { ...l, y: v };
  });
}

/**
 * Iguala a largura e/ou altura das camadas selecionadas usando como referência o item primário
 * (último ID de `ids` destravado ou último da seleção). Para camadas `TEXT`, iguala `fontSize`
 * quando igualando altura/ambos e `maxWidth` quando igualando largura.
 */
export function matchLayersSize(
  layers: SceneLayer[],
  ids: readonly string[],
  mode: MatchSizeMode
): SceneLayer[] {
  const cleanIds = pruneNested(layers, ids);
  if (cleanIds.length < 2) return layers;

  const refLoc = findLoc(layers, cleanIds[cleanIds.length - 1]);
  if (!refLoc) return layers;
  const ref = refLoc.layer;
  const targetW = ref.width;
  const targetH = ref.height;
  const targetFont = ref.text?.fontSize;
  const targetMaxW = ref.text?.maxWidth;

  const targetIds = new Set(cleanIds.filter((id) => id !== ref.id));
  return updateMany(layers, targetIds, (l) => {
    if (l.locked) return l;
    if (l.kind === "TEXT" && l.text && ref.kind === "TEXT") {
      const nextText = { ...l.text };
      if ((mode === "height" || mode === "both") && targetFont !== undefined) {
        nextText.fontSize = targetFont;
      }
      if (mode === "width" || mode === "both") {
        if (targetMaxW && targetMaxW > 0) nextText.maxWidth = targetMaxW;
        else delete nextText.maxWidth;
      }
      return { ...l, text: nextText };
    }
    if (l.kind === "TEXT") return l;
    return {
      ...l,
      width: mode === "width" || mode === "both" ? targetW : l.width,
      height: mode === "height" || mode === "both" ? targetH : l.height,
    };
  });
}

function normalizeDeg(deg: number): number {
  let d = ((deg + 180) % 360 + 360) % 360 - 180;
  if (d === -180 && deg > 0) d = 180;
  return Number(d.toFixed(2));
}

/** Gira as camadas selecionadas em +90° (`dir = 1`) ou -90° (`dir = -1`). */
export function rotateLayersBy90(
  layers: SceneLayer[],
  ids: readonly string[],
  dir: 1 | -1
): SceneLayer[] {
  const cleanIds = pruneNested(layers, ids);
  if (cleanIds.length === 0) return layers;
  return updateMany(layers, new Set(cleanIds), (l) => {
    if (l.locked) return l;
    return { ...l, rotation: normalizeDeg(l.rotation + dir * 90) };
  });
}

/**
 * Espelha horizontalmente (`flipX`) ou verticalmente (`flipY`) as camadas selecionadas.
 * Quando há 2+ camadas selecionadas, também espelha suas posições em relação ao centro da seleção.
 */
export function flipLayers(
  layers: SceneLayer[],
  ids: readonly string[],
  axis: FlipAxis
): SceneLayer[] {
  const cleanIds = pruneNested(layers, ids);
  const unlocked = cleanIds
    .map((id) => findLoc(layers, id)?.layer)
    .filter((l): l is SceneLayer => Boolean(l && !l.locked));
  if (unlocked.length === 0) return layers;

  let pivot = 0.5;
  if (unlocked.length >= 2) {
    let minP = Infinity;
    let maxP = -Infinity;
    for (const l of unlocked) {
      const p = axis === "horizontal" ? l.x : l.y;
      minP = Math.min(minP, p);
      maxP = Math.max(maxP, p);
    }
    pivot = (minP + maxP) * 0.5;
  }

  const targetIds = new Set(unlocked.map((l) => l.id));
  const multi = unlocked.length >= 2;
  return updateMany(layers, targetIds, (l) => {
    if (axis === "horizontal") {
      return {
        ...l,
        flipX: !l.flipX,
        x: multi ? clamp01(2 * pivot - l.x) : l.x,
      };
    }
    return {
      ...l,
      flipY: !l.flipY,
      y: multi ? clamp01(2 * pivot - l.y) : l.y,
    };
  });
}

// ───────────────────── Guias Inteligentes Zero-Alloc (60–144 FPS) ─────────────────────

/**
 * Calcula o encaixe magnético de um eixo (centro + bordas esquerda/direita ou topo/base) contra
 * `targets`, escrevendo direto em `out` sem alocar objetos nem arrays por frame.
 */
export function snapAxisInto(
  out: SnapAxisBuffer,
  rawCenter: number,
  halfExtent: number,
  targets: readonly number[],
  threshold: number
): SnapAxisBuffer {
  if (threshold <= 0 || targets.length === 0) {
    out.snapped = Math.max(0, Math.min(1, rawCenter));
    out.guide = null;
    return out;
  }

  let bestDist = threshold;
  let bestCenter = rawCenter;
  let bestGuide: number | null = null;
  const hasEdges = halfExtent > 0.002;
  const leftVal = rawCenter - halfExtent;
  const rightVal = rawCenter + halfExtent;

  for (let i = 0; i < targets.length; i++) {
    const target = targets[i];

    // 1. Centro do item arrastado contra o alvo (centro ou borda de outro item / tela)
    const dCenter = Math.abs(rawCenter - target);
    if (dCenter < bestDist) {
      bestDist = dCenter;
      bestCenter = target;
      bestGuide = target;
    }

    if (hasEdges) {
      // 2. Borda inicial (esquerda / topo)
      const dStart = Math.abs(leftVal - target);
      if (dStart < bestDist) {
        bestDist = dStart;
        bestCenter = target + halfExtent;
        bestGuide = target;
      }

      // 3. Borda final (direita / base)
      const dEnd = Math.abs(rightVal - target);
      if (dEnd < bestDist) {
        bestDist = dEnd;
        bestCenter = target - halfExtent;
        bestGuide = target;
      }
    }
  }

  out.snapped = Math.max(0, Math.min(1, bestCenter));
  out.guide = bestGuide;
  return out;
}
