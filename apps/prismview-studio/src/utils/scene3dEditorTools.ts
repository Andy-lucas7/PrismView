/**
 * Sala 3D editor tools (Plano Mestre — Fase 2). Loaded on demand ONLY inside the
 * Studio when the "Sala 3D" mode is active; the Desktop runtime never imports it.
 *
 * Controls
 *   Camera (look / orbit / fly / pan / dolly) ... see `engine/scene3d/editorCamera`
 *   LMB click ............. select object (gizmo attaches to it)
 *   Gizmo drag ............ move / rotate / scale; ONE commit on release
 */
import { Box3, GridHelper, Material, Object3D, PerspectiveCamera, Scene } from "three";
import { TransformControls } from "three/examples/jsm/controls/TransformControls.js";
import {
  createEditorCamera,
  type Scene3DCameraCapture,
  type Scene3DEditorView,
} from "../engine/scene3d/editorCamera";

export type { Scene3DCameraCapture, Scene3DEditorView };

export type Scene3DGizmoMode = "translate" | "rotate" | "scale";

export interface Scene3DEditorHost {
  canvas: HTMLCanvasElement;
  camera: PerspectiveCamera;
  scene: Scene;
  initialView: Scene3DEditorView;
  getPivot: (objectId: string) => Object3D | null;
  pickObjectAt: (nx: number, ny: number) => string | null;
  wake: () => void;
  onSelectObject: (objectId: string | null) => void;
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

  let selectedId: string | null = null;
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
  };

  const onDraggingChanged = (ev: { value: boolean }) => {
    dragging = Boolean(ev.value);
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
    scene.remove(grid);
    grid.geometry.dispose();
    for (const m of gridMats) m.dispose();
  };

  return {
    setSelected: (id) => {
      selectedId = id;
      attachSelected();
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
