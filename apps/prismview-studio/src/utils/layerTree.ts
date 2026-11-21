import { createLayer, SceneLayer } from "../types/prism";

export interface LayerLocation {
  layer: SceneLayer;
  parent: SceneLayer | null;
  index: number;
  siblings: SceneLayer[];
}

export type LayerDropPlacement = "above" | "below" | "inside" | "root-top" | "root-bottom";

export function findLayerById(layers: SceneLayer[], id: string | null): SceneLayer | null {
  if (!id) return null;
  for (const layer of layers) {
    if (layer.id === id) return layer;
    if (layer.children?.length) {
      const nested = findLayerById(layer.children, id);
      if (nested) return nested;
    }
  }
  return null;
}

export function findLayerLocation(
  layers: SceneLayer[],
  id: string | null,
  parent: SceneLayer | null = null
): LayerLocation | null {
  if (!id) return null;
  for (let i = 0; i < layers.length; i++) {
    const layer = layers[i];
    if (layer.id === id) {
      return { layer, parent, index: i, siblings: layers };
    }
    if (layer.children?.length) {
      const nested = findLayerLocation(layer.children, id, layer);
      if (nested) return nested;
    }
  }
  return null;
}

export function collectAllLayerIds(layers: SceneLayer[]): string[] {
  const out: string[] = [];
  for (const layer of layers) {
    out.push(layer.id);
    if (layer.children?.length) {
      out.push(...collectAllLayerIds(layer.children));
    }
  }
  return out;
}

export function isDescendantOf(
  layers: SceneLayer[],
  ancestorId: string,
  candidateId: string
): boolean {
  if (ancestorId === candidateId) return true;
  const ancestor = findLayerById(layers, ancestorId);
  if (!ancestor?.children?.length) return false;
  return findLayerById(ancestor.children, candidateId) !== null;
}

/**
 * Removes IDs whose ancestor is already selected so group moves/copies/deletes never double-act on children.
 */
export function pruneNestedSelection(layers: SceneLayer[], ids: string[]): string[] {
  const set = new Set(ids);
  return ids.filter((id) => {
    let cur = findLayerLocation(layers, id);
    while (cur?.parent) {
      if (set.has(cur.parent.id)) return false;
      cur = findLayerLocation(layers, cur.parent.id);
    }
    return true;
  });
}

export function updateLayerById(
  layers: SceneLayer[],
  id: string,
  fn: (layer: SceneLayer) => SceneLayer
): SceneLayer[] {
  let changed = false;
  const next = layers.map((layer) => {
    if (layer.id === id) {
      changed = true;
      return fn(layer);
    }
    if (layer.children?.length) {
      const updatedChildren = updateLayerById(layer.children, id, fn);
      if (updatedChildren !== layer.children) {
        changed = true;
        return { ...layer, children: updatedChildren };
      }
    }
    return layer;
  });
  return changed ? next : layers;
}

export function updateLayersByIds(
  layers: SceneLayer[],
  ids: Set<string>,
  fn: (layer: SceneLayer) => SceneLayer
): SceneLayer[] {
  if (ids.size === 0) return layers;
  let changed = false;
  const next = layers.map((layer) => {
    let cur = layer;
    if (ids.has(layer.id)) {
      cur = fn(layer);
      changed = true;
    }
    if (cur.children?.length) {
      const updatedChildren = updateLayersByIds(cur.children, ids, fn);
      if (updatedChildren !== cur.children) {
        cur = { ...cur, children: updatedChildren };
        changed = true;
      }
    }
    return cur;
  });
  return changed ? next : layers;
}

export function removeLayersByIds(layers: SceneLayer[], ids: Set<string>): SceneLayer[] {
  if (ids.size === 0) return layers;
  let changed = false;
  const out: SceneLayer[] = [];
  for (const layer of layers) {
    if (ids.has(layer.id)) {
      changed = true;
      continue;
    }
    if (layer.children?.length) {
      const nextChildren = removeLayersByIds(layer.children, ids);
      if (nextChildren !== layer.children) {
        changed = true;
        out.push({ ...layer, children: nextChildren });
        continue;
      }
    }
    out.push(layer);
  }
  return changed ? out : layers;
}

export function moveLayersZByIds(
  layers: SceneLayer[],
  ids: Set<string>,
  delta: number
): SceneLayer[] {
  if (ids.size === 0 || delta === 0) return layers;
  const copy = [...layers];
  let changed = false;

  if (delta > 0) {
    for (let i = copy.length - 2; i >= 0; i--) {
      if (ids.has(copy[i].id) && !ids.has(copy[i + 1].id)) {
        const tmp = copy[i];
        copy[i] = copy[i + 1];
        copy[i + 1] = tmp;
        changed = true;
      }
    }
  } else {
    for (let i = 1; i < copy.length; i++) {
      if (ids.has(copy[i].id) && !ids.has(copy[i - 1].id)) {
        const tmp = copy[i];
        copy[i] = copy[i - 1];
        copy[i - 1] = tmp;
        changed = true;
      }
    }
  }

  for (let i = 0; i < copy.length; i++) {
    const layer = copy[i];
    if (layer.children?.length) {
      const movedChildren = moveLayersZByIds(layer.children, ids, delta);
      if (movedChildren !== layer.children) {
        copy[i] = { ...layer, children: movedChildren };
        changed = true;
      }
    }
  }

  return changed ? copy : layers;
}

function reassignIds(layer: SceneLayer, isRoot: boolean, suffixCopy: boolean): SceneLayer {
  const freshId = `l-${Math.random().toString(36).slice(2, 9)}`;
  return {
    ...layer,
    id: freshId,
    name: isRoot && suffixCopy ? `${layer.name} (Cópia)` : layer.name,
    children: layer.children?.map((c) => reassignIds(c, false, false)),
  };
}

export function cloneLayerWithFreshIds(layer: SceneLayer, suffixCopy = true): SceneLayer {
  const deep = JSON.parse(JSON.stringify(layer)) as SceneLayer;
  return reassignIds(deep, true, suffixCopy);
}

function duplicateLayerById(
  layers: SceneLayer[],
  id: string
): { layers: SceneLayer[]; newId: string | null } {
  const idx = layers.findIndex((l) => l.id === id);
  if (idx !== -1) {
    const clone = cloneLayerWithFreshIds(layers[idx], true);
    const copy = [...layers];
    copy.splice(idx + 1, 0, clone);
    return { layers: copy, newId: clone.id };
  }
  for (let i = 0; i < layers.length; i++) {
    const layer = layers[i];
    if (layer.children?.length) {
      const res = duplicateLayerById(layer.children, id);
      if (res.newId) {
        const copy = [...layers];
        copy[i] = { ...layer, children: res.layers };
        return { layers: copy, newId: res.newId };
      }
    }
  }
  return { layers, newId: null };
}

export function duplicateLayersByIds(
  layers: SceneLayer[],
  ids: string[],
  offsetX = 0.015,
  offsetY = 0.015
): { layers: SceneLayer[]; newIds: string[] } {
  const topIds = pruneNestedSelection(layers, ids);
  const newIds: string[] = [];
  let current = layers;
  for (const id of topIds) {
    const res = duplicateLayerById(current, id);
    if (res.newId) {
      newIds.push(res.newId);
      current = updateLayerById(res.layers, res.newId, (l) => ({
        ...l,
        x: Math.max(0, Math.min(1, Number((l.x + offsetX).toFixed(4)))),
        y: Math.max(0, Math.min(1, Number((l.y + offsetY).toFixed(4)))),
      }));
    }
  }
  return { layers: current, newIds };
}

function getParentBoxInCanvas(
  layers: SceneLayer[],
  parentId: string | null,
  canvasW: number,
  canvasH: number
): { left: number; top: number; width: number; height: number } {
  if (!parentId) {
    return { left: 0, top: 0, width: canvasW, height: canvasH };
  }
  const chain: SceneLayer[] = [];
  let cur = findLayerLocation(layers, parentId);
  while (cur) {
    chain.unshift(cur.layer);
    cur = cur.parent ? findLayerLocation(layers, cur.parent.id) : null;
  }
  let box = { left: 0, top: 0, width: canvasW, height: canvasH };
  for (const node of chain) {
    const cx = box.left + node.x * box.width;
    const cy = box.top + node.y * box.height;
    const w = Math.max(1, node.width);
    const h = Math.max(1, node.height);
    box = { left: cx - w * 0.5, top: cy - h * 0.5, width: w, height: h };
  }
  return box;
}

function convertLayerPosition(
  layer: SceneLayer,
  fromBox: { left: number; top: number; width: number; height: number },
  toBox: { left: number; top: number; width: number; height: number }
): SceneLayer {
  if (
    fromBox.left === toBox.left &&
    fromBox.top === toBox.top &&
    fromBox.width === toBox.width &&
    fromBox.height === toBox.height
  ) {
    return layer;
  }
  const absX = fromBox.left + layer.x * fromBox.width;
  const absY = fromBox.top + layer.y * fromBox.height;
  const nx = Math.max(0, Math.min(1, Number(((absX - toBox.left) / Math.max(1, toBox.width)).toFixed(4))));
  const ny = Math.max(0, Math.min(1, Number(((absY - toBox.top) / Math.max(1, toBox.height)).toFixed(4))));
  return { ...layer, x: nx, y: ny };
}

function insertIntoTree(
  layers: SceneLayer[],
  targetId: string,
  placement: "above" | "below" | "inside",
  items: SceneLayer[]
): SceneLayer[] {
  if (placement === "inside") {
    return updateLayerById(layers, targetId, (g) => ({
      ...g,
      children: [...(g.children ?? []), ...items],
    }));
  }

  const idx = layers.findIndex((l) => l.id === targetId);
  if (idx !== -1) {
    const copy = [...layers];
    // Visual list is reversed: "above" in panel = higher Z = index + 1; "below" = lower Z = index
    const insertAt = placement === "above" ? idx + 1 : idx;
    copy.splice(insertAt, 0, ...items);
    return copy;
  }

  let changed = false;
  const next = layers.map((layer) => {
    if (layer.children?.length) {
      const updated = insertIntoTree(layer.children, targetId, placement, items);
      if (updated !== layer.children) {
        changed = true;
        return { ...layer, children: updated };
      }
    }
    return layer;
  });
  return changed ? next : layers;
}

/**
 * Reorders or reparents one or more dragged layers relative to a drop target,
 * preserving visual screen coordinates when moving across GROUP boundaries.
 */
export function moveLayersByDrop(
  layers: SceneLayer[],
  dragIds: string[],
  targetId: string | null,
  placement: LayerDropPlacement,
  canvasW: number,
  canvasH: number
): SceneLayer[] {
  const cleanIds = pruneNestedSelection(layers, dragIds).filter((id) => {
    if (!targetId) return true;
    if (id === targetId) return false;
    return !isDescendantOf(layers, id, targetId);
  });
  if (cleanIds.length === 0) return layers;

  // Determine destination parent
  let destParentId: string | null = null;
  if (placement === "root-top" || placement === "root-bottom") {
    destParentId = null;
  } else if (targetId) {
    const targetLoc = findLayerLocation(layers, targetId);
    if (!targetLoc) return layers;
    if (placement === "inside") {
      if (targetLoc.layer.kind !== "GROUP") return layers;
      destParentId = targetLoc.layer.id;
    } else {
      destParentId = targetLoc.parent?.id ?? null;
    }
  } else {
    return layers;
  }

  const destBox = getParentBoxInCanvas(layers, destParentId, canvasW, canvasH);

  // Collect extracted layers in ascending tree order (so relative Z is preserved)
  const idSet = new Set(cleanIds);
  const extracted: SceneLayer[] = [];
  const collectInOrder = (list: SceneLayer[], parentId: string | null) => {
    for (const item of list) {
      if (idSet.has(item.id)) {
        const fromBox = getParentBoxInCanvas(layers, parentId, canvasW, canvasH);
        extracted.push(convertLayerPosition(item, fromBox, destBox));
      } else if (item.children?.length) {
        collectInOrder(item.children, item.id);
      }
    }
  };
  collectInOrder(layers, null);
  if (extracted.length === 0) return layers;

  const stripped = removeLayersByIds(layers, idSet);

  if (placement === "root-top") {
    return [...stripped, ...extracted];
  }
  if (placement === "root-bottom") {
    return [...extracted, ...stripped];
  }
  if (!targetId) return layers;
  return insertIntoTree(stripped, targetId, placement, extracted);
}

/**
 * Wraps 1+ selected layers into a new GROUP whose bounding box tightly encloses them,
 * keeping every layer in its exact visual canvas position.
 */
export function groupLayersByIds(
  layers: SceneLayer[],
  ids: string[],
  canvasW: number,
  canvasH: number
): { layers: SceneLayer[]; groupId: string | null } {
  const cleanIds = pruneNestedSelection(layers, ids);
  if (cleanIds.length === 0) return { layers, groupId: null };

  const firstLoc = findLayerLocation(layers, cleanIds[0]);
  if (!firstLoc) return { layers, groupId: null };
  const targetParentId = firstLoc.parent?.id ?? null;
  const parentBox = getParentBoxInCanvas(layers, targetParentId, canvasW, canvasH);

  const idSet = new Set(cleanIds);
  const members: Array<{ layer: SceneLayer; absX: number; absY: number; w: number; h: number }> = [];
  let highestIndexInParent = -1;

  const walk = (list: SceneLayer[], parentId: string | null) => {
    list.forEach((item, idx) => {
      if (idSet.has(item.id)) {
        const fromBox = getParentBoxInCanvas(layers, parentId, canvasW, canvasH);
        const absX = fromBox.left + item.x * fromBox.width;
        const absY = fromBox.top + item.y * fromBox.height;
        const w = Math.max(40, item.kind === "TEXT" ? (item.text?.fontSize ?? 72) * 3.2 : item.width);
        const h = Math.max(40, item.kind === "TEXT" ? (item.text?.fontSize ?? 72) * 1.4 : item.height);
        members.push({ layer: item, absX, absY, w, h });
        if (parentId === targetParentId && idx > highestIndexInParent) {
          highestIndexInParent = idx;
        }
      } else if (item.children?.length) {
        walk(item.children, item.id);
      }
    });
  };
  walk(layers, null);
  if (members.length === 0) return { layers, groupId: null };

  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const m of members) {
    minX = Math.min(minX, m.absX - m.w * 0.5);
    minY = Math.min(minY, m.absY - m.h * 0.5);
    maxX = Math.max(maxX, m.absX + m.w * 0.5);
    maxY = Math.max(maxY, m.absY + m.h * 0.5);
  }
  const pad = 24;
  const gw = Math.max(180, Math.min(canvasW, Math.round(maxX - minX + pad * 2)));
  const gh = Math.max(120, Math.min(canvasH, Math.round(maxY - minY + pad * 2)));
  const gcx = (minX + maxX) * 0.5;
  const gcy = (minY + maxY) * 0.5;
  const groupBox = { left: gcx - gw * 0.5, top: gcy - gh * 0.5, width: gw, height: gh };

  const groupChildren = members.map((m) => ({
    ...m.layer,
    x: Math.max(0, Math.min(1, Number(((m.absX - groupBox.left) / gw).toFixed(4)))),
    y: Math.max(0, Math.min(1, Number(((m.absY - groupBox.top) / gh).toFixed(4)))),
  }));

  const group: SceneLayer = {
    ...createLayer("GROUP", `Grupo (${groupChildren.length})`),
    x: Math.max(0, Math.min(1, Number(((gcx - parentBox.left) / Math.max(1, parentBox.width)).toFixed(4)))),
    y: Math.max(0, Math.min(1, Number(((gcy - parentBox.top) / Math.max(1, parentBox.height)).toFixed(4)))),
    width: gw,
    height: gh,
    children: groupChildren,
  };

  // Count how many members were before highestIndexInParent inside targetParent to keep exact Z slot
  const siblingsBefore = targetParentId
    ? findLayerById(layers, targetParentId)?.children ?? []
    : layers;
  let insertIdx = 0;
  for (let i = 0; i <= highestIndexInParent && i < siblingsBefore.length; i++) {
    if (!idSet.has(siblingsBefore[i].id)) {
      insertIdx += 1;
    }
  }

  const stripped = removeLayersByIds(layers, idSet);
  const nextLayers =
    targetParentId === null
      ? [...stripped.slice(0, insertIdx), group, ...stripped.slice(insertIdx)]
      : updateLayerById(stripped, targetParentId, (p) => {
          const ch = p.children ?? [];
          const clamped = Math.max(0, Math.min(ch.length, insertIdx));
          return {
            ...p,
            children: [...ch.slice(0, clamped), group, ...ch.slice(clamped)],
          };
        });

  return { layers: nextLayers, groupId: group.id };
}

/**
 * Unpacks a GROUP's children into its parent at the group's exact z-position,
 * converting coordinates back to parent space so nothing shifts visually.
 */
export function ungroupLayerById(
  layers: SceneLayer[],
  groupId: string,
  canvasW: number,
  canvasH: number
): { layers: SceneLayer[]; childIds: string[] } {
  const loc = findLayerLocation(layers, groupId);
  if (!loc || loc.layer.kind !== "GROUP") {
    return { layers, childIds: [] };
  }
  const children = loc.layer.children ?? [];
  const groupBox = getParentBoxInCanvas(layers, groupId, canvasW, canvasH);
  const parentBox = getParentBoxInCanvas(layers, loc.parent?.id ?? null, canvasW, canvasH);
  const unpacked = children.map((c) => convertLayerPosition(c, groupBox, parentBox));
  const childIds = unpacked.map((c) => c.id);

  const replaceIn = (list: SceneLayer[]): SceneLayer[] => {
    const idx = list.findIndex((l) => l.id === groupId);
    if (idx !== -1) {
      const copy = [...list];
      copy.splice(idx, 1, ...unpacked);
      return copy;
    }
    let changed = false;
    const next = list.map((l) => {
      if (l.children?.length) {
        const r = replaceIn(l.children);
        if (r !== l.children) {
          changed = true;
          return { ...l, children: r };
        }
      }
      return l;
    });
    return changed ? next : list;
  };

  return { layers: replaceIn(layers), childIds };
}

export function countAllLayers(layers: SceneLayer[]): number {
  let total = 0;
  for (const layer of layers) {
    total += 1;
    if (layer.children?.length) {
      total += countAllLayers(layer.children);
    }
  }
  return total;
}
