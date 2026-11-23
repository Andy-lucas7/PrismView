import type {
  ReactiveBinding3D,
  Scene3DAutoSuggestion,
  Scene3DBindingSource,
  Scene3DBindingTarget,
  Scene3DDynamicTexture,
} from "../../types/prism";

// "Ações Rápidas" (modo fácil) do inspetor 3D. Puras + idempotentes: editam exatamente o mesmo
// `config.bindings` / `object.dynamicTextures` do Modo Pro (fonte única da verdade), com ações
// universais que funcionam em qualquer modelo 3D (LOOK, CLICK, AUDIO, textura dinâmica).

export type QuickActionKind = "LOOK" | "CLICK" | "AUDIO";

interface QuickSpec {
  source: Scene3DBindingSource;
  targetType: Scene3DBindingTarget;
  strength: number;
  smoothing: number;
}

export const QUICK_SPECS: Record<QuickActionKind, QuickSpec> = {
  LOOK: { source: "CURSOR_LOOK", targetType: "NODE_LOOK_AT", strength: 18, smoothing: 0.1 },
  CLICK: { source: "CLICK_MESH", targetType: "NODE_SCALE_PULSE", strength: 0.14, smoothing: 0.16 },
  AUDIO: { source: "AUDIO_BASS", targetType: "NODE_SCALE_PULSE", strength: 0.08, smoothing: 0.2 },
};

/** Strength slider range per quick action (same units as ReactiveBinding3D.strength) */
export const QUICK_RANGES: Record<QuickActionKind, { min: number; max: number; step: number }> = {
  LOOK: { min: 4, max: 45, step: 1 },
  CLICK: { min: 0.04, max: 0.35, step: 0.01 },
  AUDIO: { min: 0.02, max: 0.3, step: 0.01 },
};

export function quickBindingId(kind: QuickActionKind, objectId: string): string {
  return `quick-${kind.toLowerCase()}-${objectId}`;
}

function belongsTo(b: ReactiveBinding3D, objectId: string, primaryObjectId: string): boolean {
  return (b.objectId || primaryObjectId) === objectId;
}

/**
 * Bindings that represent this quick action: the one with the deterministic quick id, plus an
 * equivalent binding on the same object/source/targetType (including preset defaults like
 * `b3d-default-look` / `b3d-default-click`). When `targetName` is provided, matches that target
 * or adopts an existing binding of the same quick kind on the object if no exact target exists.
 */
export function findQuickBindings(
  bindings: readonly ReactiveBinding3D[] | undefined,
  kind: QuickActionKind,
  objectId: string,
  primaryObjectId: string,
  targetName: string | null
): ReactiveBinding3D[] {
  if (!bindings) return [];
  const id = quickBindingId(kind, objectId);
  const spec = QUICK_SPECS[kind];
  const byId = bindings.filter((b) => b.id === id);
  const sameKind = bindings.filter(
    (b) =>
      b.id !== id &&
      b.source === spec.source &&
      b.targetType === spec.targetType &&
      belongsTo(b, objectId, primaryObjectId)
  );
  if (targetName !== null) {
    const exact = sameKind.filter((b) => b.targetName === targetName);
    if (exact.length > 0 || byId.length > 0) return [...byId, ...exact];
  }
  return [...byId, ...sameKind];
}

export interface QuickBindingRequest {
  kind: QuickActionKind;
  objectId: string;
  primaryObjectId: string;
  /** Bone/node name; "__ROOT__" = whole model */
  targetName: string;
  enabled: boolean;
  /** Keeps the current strength when omitted */
  strength?: number;
}

/**
 * Idempotent: toggling on twice never duplicates, toggling off removes every equivalent binding,
 * re-targeting updates the existing binding in place (keeps the user's Pro tuning).
 * Returns the SAME array reference when nothing changed (no history entry / re-render).
 */
export function applyQuickBinding(
  bindings: readonly ReactiveBinding3D[] | undefined,
  req: QuickBindingRequest
): ReactiveBinding3D[] {
  const list = bindings ? [...bindings] : [];
  const id = quickBindingId(req.kind, req.objectId);
  const spec = QUICK_SPECS[req.kind];
  const matches = findQuickBindings(list, req.kind, req.objectId, req.primaryObjectId, req.targetName);
  const byId = list.find((b) => b.id === id);
  if (byId && !matches.includes(byId)) matches.unshift(byId);

  if (!req.enabled) {
    if (matches.length === 0) return (bindings as ReactiveBinding3D[]) ?? list;
    const drop = new Set(matches);
    return list.filter((b) => !drop.has(b));
  }

  const keep = matches[0];
  if (!keep) {
    list.push({
      id,
      enabled: true,
      objectId: req.objectId,
      source: spec.source,
      sourceNode: "*",
      targetType: spec.targetType,
      targetName: req.targetName,
      strength: req.strength ?? spec.strength,
      smoothing: spec.smoothing,
      ...(spec.targetType === "NODE_LOOK_AT" ? { invertX: false, invertY: false } : {}),
    });
    return list;
  }

  const next: ReactiveBinding3D = {
    ...keep,
    enabled: true,
    objectId: keep.objectId || req.objectId,
    targetName: req.targetName,
    strength: req.strength ?? keep.strength,
  };
  const extras = new Set(matches.slice(1));
  const unchanged =
    extras.size === 0 &&
    keep.enabled &&
    keep.objectId === next.objectId &&
    keep.targetName === next.targetName &&
    keep.strength === next.strength;
  if (unchanged) return (bindings as ReactiveBinding3D[]) ?? list;
  return list.filter((b) => !extras.has(b)).map((b) => (b === keep ? next : b));
}

/** Optional default target from auto-detection; callers always fall back to "__ROOT__" */
export function suggestedQuickTarget(
  suggestions: readonly Scene3DAutoSuggestion[] | undefined,
  kind: QuickActionKind
): string | null {
  if (!suggestions) return null;
  if (kind === "AUDIO") {
    return suggestions.find((s) => s.role === "SPEAKER")?.targetName ?? null;
  }
  if (kind === "LOOK") {
    for (const role of ["HEAD", "NECK"]) {
      const hit = suggestions.find((s) => s.role === role);
      if (hit) return hit.targetName;
    }
  }
  return null;
}

/** Idempotent toggle for a detected part suggestion chip (only shown when the model has named parts). */
export function toggleSuggestionBinding(
  bindings: readonly ReactiveBinding3D[] | undefined,
  suggestion: Scene3DAutoSuggestion,
  objectId: string,
  primaryObjectId: string
): ReactiveBinding3D[] {
  const tpl = suggestion.binding;
  if (!tpl) return (bindings as ReactiveBinding3D[]) ?? [];
  const list = bindings ? [...bindings] : [];
  const matches = list.filter(
    (b) =>
      b.source === tpl.source &&
      b.targetType === tpl.targetType &&
      b.targetName === tpl.targetName &&
      belongsTo(b, objectId, primaryObjectId)
  );
  if (matches.length > 0 && matches.some((b) => b.enabled)) {
    const drop = new Set(matches);
    return list.filter((b) => !drop.has(b));
  }
  if (matches.length > 0) {
    const keep = matches[0];
    return list.map((b) => (b === keep ? { ...b, enabled: true } : b));
  }
  list.push({
    ...tpl,
    id: `sug-${suggestion.id}-${objectId}`,
    objectId,
    enabled: true,
  });
  return list;
}

// ── Live album cover / image on a material ───────────────────────────────────────

export function quickCoverId(objectId: string): string {
  return `quick-cover-${objectId}`;
}

/** The dynamic texture driven by the quick texture card (quick id first, else the first entry) */
export function findQuickCover(
  textures: readonly Scene3DDynamicTexture[] | undefined,
  objectId: string
): Scene3DDynamicTexture | null {
  if (!textures || textures.length === 0) return null;
  return textures.find((t) => t.id === quickCoverId(objectId)) ?? textures[0];
}

export interface QuickCoverRequest {
  objectId: string;
  enabled: boolean;
  materialName?: string;
  emissiveMap?: boolean;
  source?: Scene3DDynamicTexture["source"];
}

/** Idempotent add/update/remove of the quick cover entry; same reference when unchanged */
export function applyQuickCover(
  textures: readonly Scene3DDynamicTexture[] | undefined,
  req: QuickCoverRequest
): Scene3DDynamicTexture[] {
  const list = textures ? [...textures] : [];
  const cur = findQuickCover(list, req.objectId);
  if (!req.enabled) {
    if (!cur) return (textures as Scene3DDynamicTexture[]) ?? list;
    return list.filter((t) => t !== cur);
  }
  if (!cur) {
    list.push({
      id: quickCoverId(req.objectId),
      materialName: req.materialName ?? "*",
      source: req.source ?? "MEDIA_COVER",
      emissiveMap: req.emissiveMap ?? true,
    });
    return list;
  }
  const next: Scene3DDynamicTexture = {
    ...cur,
    materialName: req.materialName ?? cur.materialName,
    emissiveMap: req.emissiveMap ?? cur.emissiveMap,
    source: req.source ?? cur.source,
  };
  if (
    next.materialName === cur.materialName &&
    next.emissiveMap === cur.emissiveMap &&
    next.source === cur.source
  ) {
    return (textures as Scene3DDynamicTexture[]) ?? list;
  }
  return list.map((t) => (t === cur ? next : t));
}
