import type {
  ReactiveBinding3D,
  Scene3DAutoSuggestion,
  Scene3DBindingSource,
  Scene3DBindingTarget,
  Scene3DDynamicTexture,
} from "../../types/prism";
import { BODY_FOLLOW_DEFAULTS, HEAD_FOLLOW_DEFAULTS } from "./partHeuristics.ts";

// "Ações Rápidas" (easy mode) of the 3D inspector. Pure + idempotent: they edit the SAME
// `config.bindings` / `object.dynamicTextures` the Pro editor shows (single source of truth),
// using only contracts the native engine already runs (NODE_LOOK_AT, NODE_SCALE_PULSE, MEDIA_COVER).

export type QuickActionKind = "HEAD" | "BODY" | "AUDIO";

interface QuickSpec {
  source: Scene3DBindingSource;
  targetType: Scene3DBindingTarget;
  strength: number;
  smoothing: number;
}

export const QUICK_SPECS: Record<QuickActionKind, QuickSpec> = {
  HEAD: { source: "CURSOR_LOOK", targetType: "NODE_LOOK_AT", ...HEAD_FOLLOW_DEFAULTS },
  BODY: { source: "CURSOR_LOOK", targetType: "NODE_LOOK_AT", ...BODY_FOLLOW_DEFAULTS },
  AUDIO: { source: "AUDIO_BASS", targetType: "NODE_SCALE_PULSE", strength: 0.08, smoothing: 0.2 },
};

/** Strength slider range per quick action (same units as ReactiveBinding3D.strength) */
export const QUICK_RANGES: Record<QuickActionKind, { min: number; max: number; step: number }> = {
  HEAD: { min: 4, max: 45, step: 1 },
  BODY: { min: 2, max: 20, step: 1 },
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
 * equivalent binding added by hand / by a suggestion chip (same object, source, action, target).
 * HEAD and BODY share CURSOR_LOOK→NODE_LOOK_AT, so for them the target name disambiguates.
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
  return bindings.filter(
    (b) =>
      b.id === id ||
      (targetName !== null &&
        b.targetName === targetName &&
        b.source === spec.source &&
        b.targetType === spec.targetType &&
        belongsTo(b, objectId, primaryObjectId))
  );
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

/** Target suggested by auto-detection for a quick action (HEAD falls back to NECK) */
export function suggestedQuickTarget(
  suggestions: readonly Scene3DAutoSuggestion[] | undefined,
  kind: QuickActionKind
): string | null {
  if (!suggestions) return null;
  if (kind === "AUDIO") {
    // Scale pulse only makes sense on a node: speaker if detected (never a morph/jaw), else caller uses __ROOT__
    return suggestions.find((s) => s.role === "SPEAKER")?.targetName ?? null;
  }
  const roles = kind === "HEAD" ? ["HEAD", "NECK"] : ["BODY"];
  for (const role of roles) {
    const hit = suggestions.find((s) => s.role === role);
    if (hit) return hit.targetName;
  }
  return null;
}

// ── Live album cover on a material ──────────────────────────────────────────────

export function quickCoverId(objectId: string): string {
  return `quick-cover-${objectId}`;
}

/** The dynamic texture driven by the quick "Capa da Música" card (quick id first, else the first entry) */
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
