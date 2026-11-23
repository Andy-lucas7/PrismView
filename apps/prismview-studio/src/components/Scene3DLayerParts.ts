// Peças do Scene3DLayer (PLANO_CORRECAO 5.1): alvos de binding, dia/noite, inspeção agregada e cache de miniaturas.
import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  ACESFilmicToneMapping,
  AmbientLight,
  AnimationAction,
  AnimationClip,
  AnimationMixer,
  Box3,
  Color,
  DirectionalLight,
  Euler,
  Group,
  LoopOnce,
  LoopRepeat,
  Object3D,
  PerspectiveCamera,
  PMREMGenerator,
  PointLight,
  Quaternion,
  Raycaster,
  Scene,
  SRGBColorSpace,
  Texture,
  TextureLoader,
  Vector2,
  Vector3,
  WebGLRenderer,
  WebGLRenderTarget,
} from "three";
import {
  getSharedGltfLoader,
  releaseSharedLoaders,
  retainSharedLoaders,
} from "../engine/scene3d/loaders";
import {
  clampScene3DFps,
  DEFAULT_REACTIVE_SIGNAL,
  normalizeScene3DObjects,
  ReactiveBinding3D,
  ReactiveSignal,
  resolveMediaUrl,
  Scene3DAxis,
  Scene3DConfig,
  Scene3DEnvPreset,
  Scene3DInspectInfo,
  Scene3DObject,
  Scene3DObjectInspectInfo,
  Scene3DPickedPart,
} from "../types/prism";
import {
  applyConfigCamera,
  applyDayNightLighting,
  applySceneBackground,
  buildProceduralEnvironment,
  computeConfigCameraView,
  ENV_SPECS,
  sampleDayNightBackground,
} from "../engine/scene3d/environment";
import { buildShowcaseSculpture, type DefaultSculptureRefs } from "../engine/scene3d/sculptures";
import {
  buildModelIndex,
  disposeObject3D,
  restoreBaseMaps,
  type EmissiveMatEntry,
  matchesMeshTarget,
  type ModelSceneIndex,
  nodeUnitScale,
  resolvePickedPartNode,
} from "../engine/scene3d/modelIndex";
import { type PowerState, powerCapsFor } from "../engine/powerGovernor";
import type { Scene3DObjectTree } from "../engine/scene3d/modelTree";
import type {
  Scene3DCameraCapture,
  Scene3DEditorSession,
  Scene3DGizmoMode,
} from "../utils/scene3dEditorTools";


export const AXIS_VECTORS: Record<Scene3DAxis, Vector3> = {
  X: new Vector3(1, 0, 0),
  Y: new Vector3(0, 1, 0),
  Z: new Vector3(0, 0, 1),
};

export interface LoadedObjectRuntime {
  id: string;
  sourceKey: string;
  /** Key (`kind|modelSrc|variant`) of the load currently in flight; "" when idle (AUDITORIA 1.6). */
  loadKey: string;
  loadToken: number;
  pivot: Group;
  modelRoot: Object3D | null;
  modelIndex: ModelSceneIndex | null;
  defaultRefs: DefaultSculptureRefs | null;
  mixer: AnimationMixer | null;
  clips: AnimationClip[];
  baseAction: AnimationAction | null;
  overrideAction: AnimationAction | null;
  overrideClipName: string | null;
  overrideHoldHoverId: string | null;
  activeClipKey: string;
  spinAngle: number;
  error: string | null;
}

/**
 * Binding target resolved by `objectId` + `targetName` against the owning object's index.
 * Rebuilt only when the config or a model index changes (`targetsRev`), never per frame.
 * `rt === null` ⇒ owner object not loaded yet: the binding is skipped (never falls onto another object).
 */
export interface ResolvedBindingTarget {
  rt: LoadedObjectRuntime | null;
  node: Object3D | null;
  mats: readonly EmissiveMatEntry[];
}

export const EMPTY_MATS: readonly EmissiveMatEntry[] = [];
export const UNRESOLVED_TARGET: ResolvedBindingTarget = { rt: null, node: null, mats: EMPTY_MATS };

/** Owner of a binding: its `objectId` when that object exists, otherwise the first object (≤v2 compat). */
export function bindingOwnerId(objs: readonly Scene3DObject[], objectId: string | undefined): string | null {
  if (objectId) {
    for (const o of objs) if (o.id === objectId) return objectId;
  }
  return objs[0]?.id ?? null;
}

export function dayNightHourFor(cfg: Scene3DConfig): number {
  if (typeof cfg.dayNightHourOverride === "number" && Number.isFinite(cfg.dayNightHourOverride)) {
    return cfg.dayNightHourOverride;
  }
  const d = new Date();
  return d.getHours() + d.getMinutes() / 60;
}

/** (Re)applies the canvas background for `cfg` (mode/colour/day-night). Never called per frame. */
export function syncSceneBackground(scene: Scene, cfg: Scene3DConfig): void {
  applySceneBackground(scene, cfg, cfg.autoDayNight ? sampleDayNightBackground(dayNightHourFor(cfg)) : undefined);
}

export function buildAggregatedInspectInfo(
  objectsMap: Map<string, LoadedObjectRuntime>,
  orderedObjs: Scene3DObject[]
): Scene3DInspectInfo {
  const byObject: Record<string, Scene3DObjectInspectInfo> = {};
  let meshCount = 0;
  let triangleCount = 0;
  let textureCount = 0;
  let maxTextureSize = 0;
  let estimatedVramMb = 0;
  let hasExternal = false;
  let primaryInfo: Scene3DObjectInspectInfo | null = null;
  let firstError: string | null = null;

  for (const obj of orderedObjs) {
    const rt = objectsMap.get(obj.id);
    if (!rt?.modelIndex) continue;
    const objInfo: Scene3DObjectInspectInfo = {
      ...rt.modelIndex.info,
      objectId: obj.id,
      error: rt.error ?? rt.modelIndex.info.error ?? null,
    };
    byObject[obj.id] = objInfo;
    if (!primaryInfo) primaryInfo = objInfo;
    if (!firstError && objInfo.error) firstError = objInfo.error;
    meshCount += objInfo.meshCount;
    triangleCount += objInfo.triangleCount;
    textureCount += objInfo.textureCount;
    if (objInfo.maxTextureSize > maxTextureSize) maxTextureSize = objInfo.maxTextureSize;
    estimatedVramMb += objInfo.estimatedVramMb;
    if (!objInfo.isDefaultSculpture) hasExternal = true;
  }

  estimatedVramMb = Math.round(estimatedVramMb * 10) / 10;

  const warnings: string[] = [];
  if (hasExternal) {
    if (triangleCount > 80_000) {
      warnings.push(
        `Geometria densa na sala (${(triangleCount / 1000).toFixed(0)}k tris) — ideal ≤ 60k tris ou Decimate no Blender.`
      );
    }
    if (maxTextureSize > 2048) {
      warnings.push(
        `Textura ${maxTextureSize}px detectada — reduza para ≤ 2048px (WebP/KTX2) para economizar VRAM.`
      );
    }
    if (textureCount > 12) {
      warnings.push(
        `${textureCount} texturas na sala — combine materiais em Atlas para reduzir draw calls.`
      );
    }
    if (estimatedVramMb > 160) {
      warnings.push(
        `VRAM estimada ~${Math.round(estimatedVramMb)} MB — recomendado ativar Modo Leve (70% · 30 FPS).`
      );
    }
  }

  return {
    clipNames: primaryInfo?.clipNames ?? [],
    boneNames: primaryInfo?.boneNames ?? [],
    meshNames: primaryInfo?.meshNames ?? [],
    materialNames: primaryInfo?.materialNames ?? [],
    morphNames: primaryInfo?.morphNames ?? [],
    meshCount,
    triangleCount,
    textureCount,
    maxTextureSize,
    estimatedVramMb,
    warnings,
    isDefaultSculpture: primaryInfo?.isDefaultSculpture ?? true,
    error: firstError,
    byObject,
  };
}

export interface BindingRuntimeState {
  yaw: number;
  pitch: number;
  spin: number;
  weight: number;
  pulse: number;
  toggled: boolean;
  wasHovered: boolean;
}

export const DEEP_SLEEP_MS = 60_000;

// ── 2.1 Hub thumbnails: ONE live engine at a time, then a static image ─────────────────────────
// Each thumbnail used to keep a full WebGL renderer (+PMREM, model, textures) alive forever.
// Now thumbnails take a turn on a serial queue, render once, snapshot to a blob URL, release the
// GPU context, and display an <img>. Snapshots are cached by config hash + size.
const THUMB_CACHE_MAX = 64;
export const thumbCache = new Map<string, string>();
let thumbQueueTail: Promise<void> = Promise.resolve();

export function acquireThumbSlot(): { granted: Promise<void>; release: () => void } {
  let release!: () => void;
  const mine = new Promise<void>((res) => {
    release = res;
  });
  const granted = thumbQueueTail;
  thumbQueueTail = thumbQueueTail.then(() => mine);
  return { granted, release };
}

export function hashString(str: string): string {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (let i = 0; i < str.length; i++) {
    const c = str.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 2654435761);
    h2 = Math.imul(h2 ^ c, 1597334677);
  }
  return `${(h1 >>> 0).toString(36)}${(h2 >>> 0).toString(36)}`;
}

export function storeThumb(key: string, url: string) {
  const old = thumbCache.get(key);
  if (old) URL.revokeObjectURL(old);
  thumbCache.delete(key);
  thumbCache.set(key, url);
  if (thumbCache.size > THUMB_CACHE_MAX) {
    const oldest = thumbCache.keys().next().value;
    if (oldest !== undefined) {
      const u = thumbCache.get(oldest);
      if (u) URL.revokeObjectURL(u);
      thumbCache.delete(oldest);
    }
  }
}
