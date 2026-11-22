/**
 * Sala 3D editor tools (Plano Mestre — Fase 2). Loaded on demand ONLY inside the
 * Studio when the "Sala 3D" mode is active; the Desktop runtime never imports it.
 *
 * Controls
 *   Camera (look / orbit / fly / pan / dolly) ... see `engine/scene3d/editorCamera`
 *   LMB click ............. select object (gizmo attaches to it)
 *   Gizmo drag ............ move / rotate / scale; ONE commit on release
 */
import {
  Box3,
  Box3Helper,
  BoxHelper,
  GridHelper,
  LineBasicMaterial,
  Material,
  Object3D,
  PerspectiveCamera,
  Scene,
  SkeletonHelper,
  Vector3,
} from "three";
import { TransformControls } from "three/examples/jsm/controls/TransformControls.js";
import {
  createEditorCamera,
  type Scene3DCameraCapture,
  type Scene3DEditorView,
} from "../engine/scene3d/editorCamera";
import type { ModelSceneIndex } from "../engine/scene3d/modelIndex";
import {
  getModelTree,
  getTreeNodeId,
  getTreeNodeObject,
  type Scene3DObjectTree,
} from "../engine/scene3d/modelTree";
import type { Scene3DPickedPart } from "../types/prism";

export type { Scene3DCameraCapture, Scene3DEditorView };

export type Scene3DGizmoMode = "translate" | "rotate" | "scale";

export interface Scene3DEditorHost {
  canvas: HTMLCanvasElement;
  camera: PerspectiveCamera;
  scene: Scene;
  initialView: Scene3DEditorView;
  getPivot: (objectId: string) => Object3D | null;
  /** Index of the loaded model of an object (null while loading / unknown) */
  getModelIndex: (objectId: string) => ModelSceneIndex | null;
  pickObjectAt: (nx: number, ny: number) => string | null;
  /** Raycast for Alt+Click part picking: resolves the most useful named bone/group/mesh under cursor */
  pickPartAt: (nx: number, ny: number) => { objectId: string; node: Object3D } | null;
  wake: () => void;
  onSelectObject: (objectId: string | null) => void;
  onPickPart: (part: Scene3DPickedPart | null) => void;
  onCommitTransform: (
    objectId: string,
    position: [number, number, number],
    rotationDeg: [number, number, number],
    scale: [number, number, number]
  ) => void;
}

export interface Scene3DEditorSession {
  setSelected: (objectId: string | null) => void;
  setMode: (mode: Scene3DGizmoMode) => void;
  setGrid: (visible: boolean) => void;
  setSnap: (enabled: boolean) => void;
  /** SkeletonHelper of the selected object's model (built lazily, disposed when hidden / deselected / on exit) */
  setSkeleton: (visible: boolean) => void;
  /** Outliner tree of an object's model (cached per model); null while the model is not loaded */
  getObjectTree: (objectId: string) => Scene3DObjectTree | null;
  /** Highlights one part (tree node id) of an object's model in the viewport; null clears it */
  highlightNode: (objectId: string, nodeId: number | null) => void;
  /** True while the gizmo drags the object (and briefly after release, until the commit lands) */
  isDragging: (objectId: string) => boolean;
  /** Keep the render loop alive (flying, dragging, easing) */
  needsFrames: () => boolean;
  update: (dt: number) => void;
  captureCameraState: () => Scene3DCameraCapture;
  setCameraState: (state: Scene3DCameraCapture, animate?: boolean) => void;
  focusOn: (objectId: string) => boolean;
  focusSelected: () => void;
  resetView: (view: Scene3DEditorView) => void;
  dispose: () => void;
}

const RAD2DEG = 180 / Math.PI;
const COMMIT_HOLD_MS = 450;

function round(n: number, digits = 3): number {
  const f = 10 ** digits;
  return Math.round(n * f) / f;
}

export function createScene3DEditorSession(host: Scene3DEditorHost): Scene3DEditorSession {
  const { canvas, camera, scene } = host;

  // ── Free camera (look / orbit / fly / pan / dolly) ───────────────────────────────
  const cam = createEditorCamera({
    canvas,
    camera,
    initialView: host.initialView,
    wake: host.wake,
    // Camera drag (Alt+LMB pan, RMB...) owns the pointer: the gizmo must not start a drag too
    onNavigate: (active) => {
      controls.enabled = !active;
    },
    // Alt+LMB without drag: pick the bone/part under cursor without conflicting with Alt+drag pan
    onAltClick: (clientX, clientY) => {
      const rect = canvas.getBoundingClientRect();
      const nx = Math.max(0, Math.min(1, (clientX - rect.left) / Math.max(1, rect.width)));
      const ny = Math.max(0, Math.min(1, (clientY - rect.top) / Math.max(1, rect.height)));
      const hit = host.pickPartAt(nx, ny);
      if (!hit) {
        setPartHighlight(null, null);
        host.onPickPart(null);
        return;
      }
      const idx = host.getModelIndex(hit.objectId);
      const tree = idx ? getModelTree(idx) : null;
      const nodeId = idx ? getTreeNodeId(idx, hit.node) : null;
      const treeNode = tree && nodeId !== null ? tree.flat[nodeId] ?? null : null;
      const rawName = hit.node.name || treeNode?.name || "__ROOT__";
      const name = rawName === "(sem nome)" ? "__ROOT__" : rawName;
      const type: Scene3DPickedPart["type"] =
        treeNode?.type ??
        ((hit.node as Object3D & { isBone?: boolean }).isBone
          ? "bone"
          : (hit.node as Object3D & { isMesh?: boolean }).isMesh
          ? "mesh"
          : "group");
      setPartHighlight(hit.node, hit.objectId);
      if (hit.objectId !== selectedId) {
        host.onSelectObject(hit.objectId);
      }
      host.onPickPart({
        objectId: hit.objectId,
        nodeId: nodeId ?? -1,
        name,
        type,
        morphs: treeNode?.morphs ?? [],
        materials: treeNode?.materials ?? [],
      });
    },
  });
  const focusBox = new Box3();

  // ── Gizmo & grid ──────────────────────────────────────────────────────────────
  const controls = new TransformControls(camera, canvas);
  controls.setSize(0.85);
  const gizmoHelper = controls.getHelper();
  scene.add(gizmoHelper);

  const grid = new GridHelper(24, 48, 0x8a8f9c, 0x30333b);
  grid.position.y = -1.2; // normalized models are 2.4 units tall around their pivot
  const gridMats = (Array.isArray(grid.material) ? grid.material : [grid.material]) as Material[];
  for (const m of gridMats) {
    m.transparent = true;
    m.opacity = 0.45;
    m.depthWrite = false;
  }
  scene.add(grid);

  // Selection highlight: one BoxHelper reused for every selection (retargeted, never recreated)
  const highlight = new BoxHelper(new Object3D(), 0x7cc4ff);
  highlight.visible = false;
  highlight.renderOrder = 998;
  const highlightMat = highlight.material as LineBasicMaterial;
  highlightMat.transparent = true;
  highlightMat.opacity = 0.85;
  highlightMat.depthTest = false;
  highlightMat.toneMapped = false;
  scene.add(highlight);

  let selectedId: string | null = null;

  // ── Skeleton helper (only while toggled on) ───────────────────────────────────
  let skeletonWanted = false;
  let skeleton: SkeletonHelper | null = null;
  let skeletonRoot: Object3D | null = null;

  const disposeSkeleton = () => {
    if (skeleton) {
      scene.remove(skeleton);
      skeleton.dispose();
      skeleton = null;
    }
    skeletonRoot = null;
  };

  /** Keeps the helper bound to the selected object's CURRENT model root (reloads swap the root) */
  const syncSkeleton = () => {
    const root = skeletonWanted && selectedId ? host.getModelIndex(selectedId)?.holder ?? null : null;
    if (root === skeletonRoot) return;
    disposeSkeleton();
    if (!root) return;
    skeletonRoot = root; // remembered even without bones, so a bone-less model isn't rescanned every frame
    const helper = new SkeletonHelper(root);
    if (helper.bones.length === 0) {
      helper.dispose();
      return;
    }
    helper.renderOrder = 997;
    skeleton = helper;
    scene.add(helper);
  };

  // ── Part highlight (outliner / Alt+Click): one Box3Helper reused for every part ──
  const partBox = new Box3();
  const partPos = new Vector3();
  const partSize = new Vector3(0.08, 0.08, 0.08);
  let partHelper: Box3Helper | null = null;
  let partObject: Object3D | null = null;
  let partObjectId: string | null = null;

  const disposePartHelper = () => {
    if (!partHelper) return;
    scene.remove(partHelper);
    partHelper.geometry.dispose();
    (partHelper.material as Material).dispose();
    partHelper = null;
  };

  const updatePartBox = () => {
    if (!partHelper || !partObject) return;
    partBox.setFromObject(partObject);
    if (partBox.isEmpty()) {
      // Bones / empty groups have no geometry: mark the joint with a small cube
      partObject.getWorldPosition(partPos);
      partBox.setFromCenterAndSize(partPos, partSize);
    }
    partHelper.box.copy(partBox);
  };

  const setPartHighlight = (obj: Object3D | null, ownerId: string | null) => {
    if (!obj) {
      partObject = null;
      partObjectId = null;
      disposePartHelper();
      host.wake();
      return;
    }
    if (!partHelper) {
      partHelper = new Box3Helper(partBox, 0xffb454);
      partHelper.renderOrder = 999;
      const mat = partHelper.material as LineBasicMaterial;
      mat.transparent = true;
      mat.opacity = 0.95;
      mat.depthTest = false;
      mat.toneMapped = false;
      scene.add(partHelper);
    }
    partObject = obj;
    partObjectId = ownerId;
    updatePartBox();
    host.wake();
  };

  let attachedPivot: Object3D | null = null;
  let dragging = false;
  let holdId: string | null = null;
  let holdUntil = 0;

  const attachSelected = () => {
    const pivot = selectedId ? host.getPivot(selectedId) : null;
    if (pivot === attachedPivot) return;
    if (pivot) controls.attach(pivot);
    else controls.detach();
    attachedPivot = pivot;
    if (pivot) highlight.setFromObject(pivot);
    highlight.visible = pivot !== null;
  };

  const onDraggingChanged = (ev: { value: boolean }) => {
    dragging = Boolean(ev.value);
    // Free camera is off while the gizmo owns the pointer
    cam.setEnabled(!dragging);
    if (!dragging && selectedId && attachedPivot) {
      const p = attachedPivot.position;
      attachedPivot.rotation.reorder("YXZ");
      const r = attachedPivot.rotation;
      const s = attachedPivot.scale;
      holdId = selectedId;
      holdUntil = performance.now() + COMMIT_HOLD_MS;
      host.onCommitTransform(
        selectedId,
        [round(p.x), round(p.y), round(p.z)],
        [round(r.x * RAD2DEG, 2), round(r.y * RAD2DEG, 2), round(r.z * RAD2DEG, 2)],
        [round(Math.max(0.05, s.x)), round(Math.max(0.05, s.y)), round(Math.max(0.05, s.z))]
      );
    }
    host.wake();
  };
  const onGizmoChange = () => host.wake();
  controls.addEventListener("dragging-changed", onDraggingChanged as never);
  controls.addEventListener("change", onGizmoChange as never);

  // ── Selection input (plain LMB click; camera buttons belong to `cam`) ─────────
  let clickPointer: number | null = null;
  let downX = 0;
  let downY = 0;

  const onPointerDown = (e: PointerEvent) => {
    if (clickPointer !== null || e.button !== 0 || e.altKey || cam.isNavigating()) return;
    // Gizmo handle under the cursor: TransformControls owns this drag
    if (controls.axis !== null) return;
    clickPointer = e.pointerId;
    downX = e.clientX;
    downY = e.clientY;
  };

  const onPointerUp = (e: PointerEvent) => {
    if (clickPointer !== e.pointerId) return;
    clickPointer = null;
    if (dragging || controls.axis !== null) return;
    if (Math.hypot(e.clientX - downX, e.clientY - downY) >= 4) return;
    const rect = canvas.getBoundingClientRect();
    const nx = Math.max(0, Math.min(1, (e.clientX - rect.left) / Math.max(1, rect.width)));
    const ny = Math.max(0, Math.min(1, (e.clientY - rect.top) / Math.max(1, rect.height)));
    host.onSelectObject(host.pickObjectAt(nx, ny));
    host.wake();
  };

  const onPointerCancel = (e: PointerEvent) => {
    if (clickPointer === e.pointerId) clickPointer = null;
  };

  canvas.addEventListener("pointerdown", onPointerDown);
  canvas.addEventListener("pointerup", onPointerUp);
  canvas.addEventListener("pointercancel", onPointerCancel);

  // ── Per-frame ─────────────────────────────────────────────────────────────────
  const update = (dt: number) => {
    cam.update(dt);
    // Selection may point at a pivot that was (re)created after a model reload
    attachSelected();
    // Follows the object (gizmo drag, bindings, spin, model load); BoxHelper.update reuses a module Box3
    if (highlight.visible) highlight.update();
    if (skeletonWanted) syncSkeleton();
    if (partObject) updatePartBox();
  };

  const focusOn = (objectId: string): boolean => {
    const pivot = host.getPivot(objectId);
    if (!pivot) return false;
    focusBox.setFromObject(pivot);
    return cam.focusBox(focusBox);
  };

  const focusSelected = () => {
    if (selectedId) focusOn(selectedId);
  };

  const resetView = (view: Scene3DEditorView) => cam.setView(view, true);

  let disposed = false;
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    canvas.removeEventListener("pointerdown", onPointerDown);
    canvas.removeEventListener("pointerup", onPointerUp);
    canvas.removeEventListener("pointercancel", onPointerCancel);
    cam.dispose();
    controls.removeEventListener("dragging-changed", onDraggingChanged as never);
    controls.removeEventListener("change", onGizmoChange as never);
    controls.detach();
    scene.remove(gizmoHelper);
    controls.dispose();
    disposeSkeleton();
    disposePartHelper();
    scene.remove(highlight);
    highlight.geometry.dispose();
    highlightMat.dispose();
    scene.remove(grid);
    grid.geometry.dispose();
    for (const m of gridMats) m.dispose();
  };

  return {
    setSelected: (id) => {
      selectedId = id;
      if (partObjectId && partObjectId !== id) {
        setPartHighlight(null, null);
      }
      attachSelected();
      if (skeletonWanted) syncSkeleton();
    },
    setMode: (mode) => controls.setMode(mode),
    setGrid: (visible) => {
      grid.visible = visible;
    },
    setSnap: (enabled) => {
      controls.setTranslationSnap(enabled ? 0.25 : null);
      controls.setRotationSnap(enabled ? Math.PI / 12 : null);
      controls.setScaleSnap(enabled ? 0.1 : null);
    },
    setSkeleton: (visible) => {
      if (skeletonWanted === visible) return;
      skeletonWanted = visible;
      if (visible) syncSkeleton();
      else disposeSkeleton();
      host.wake();
    },
    getObjectTree: (id) => {
      const idx = host.getModelIndex(id);
      return idx ? getModelTree(idx) : null;
    },
    highlightNode: (id, nodeId) => {
      if (nodeId === null) {
        // Ignore stale unmount cleanup from a previously selected object's outliner
        if (partObjectId && id && partObjectId !== id) return;
        setPartHighlight(null, null);
        return;
      }
      const idx = host.getModelIndex(id);
      const obj = idx ? getTreeNodeObject(idx, nodeId) : null;
      setPartHighlight(obj, obj ? id : null);
    },
    isDragging: (id) =>
      (dragging && id === selectedId) || (id === holdId && performance.now() < holdUntil),
    needsFrames: () =>
      dragging || cam.needsFrames() || (holdId !== null && performance.now() < holdUntil),
    update,
    captureCameraState: cam.getState,
    setCameraState: cam.setState,
    focusOn,
    focusSelected,
    resetView,
    dispose,
  };
}
