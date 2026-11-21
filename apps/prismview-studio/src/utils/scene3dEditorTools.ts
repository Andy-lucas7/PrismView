/**
 * Sala 3D editor tools (Plano Mestre — Fase 2). Loaded on demand ONLY inside the
 * Studio when the "Sala 3D" mode is active; the Desktop runtime never imports it.
 *
 * Controls
 *   RMB + drag ............ look around
 *   RMB + W A S D / Q E ... fly (Shift = faster)
 *   MMB drag / Alt + drag . pan
 *   Wheel ................. dolly forward / back
 *   LMB click ............. select object (gizmo attaches to it)
 *   Gizmo drag ............ move / rotate / scale; ONE commit on release
 */
import {
  Box3,
  Euler,
  GridHelper,
  Material,
  Object3D,
  PerspectiveCamera,
  Scene,
  Vector3,
} from "three";
import { TransformControls } from "three/examples/jsm/controls/TransformControls.js";

export type Scene3DGizmoMode = "translate" | "rotate" | "scale";

export interface Scene3DCameraCapture {
  target: [number, number, number];
  distance: number;
  azimuthDeg: number;
  elevationDeg: number;
  fov: number;
}

export interface Scene3DEditorView {
  position: [number, number, number];
  target: [number, number, number];
  fov: number;
}

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
  focusSelected: () => void;
  resetView: (view: Scene3DEditorView) => void;
  dispose: () => void;
}

const RAD2DEG = 180 / Math.PI;
const FLY_KEYS = new Set(["w", "a", "s", "d", "q", "e", "shift"]);
const COMMIT_HOLD_MS = 450;

function round(n: number, digits = 3): number {
  const f = 10 ** digits;
  return Math.round(n * f) / f;
}

export function createScene3DEditorSession(host: Scene3DEditorHost): Scene3DEditorSession {
  const { canvas, camera, scene } = host;

  // ── Free camera state ─────────────────────────────────────────────────────────
  const position = new Vector3(...host.initialView.position);
  const focusPoint = new Vector3(...host.initialView.target);
  let yaw = 0;
  let pitch = 0;
  let focusDistance = 1;
  const forward = new Vector3();
  const right = new Vector3();
  const up = new Vector3(0, 1, 0);
  const tmp = new Vector3();
  const euler = new Euler(0, 0, 0, "YXZ");

  const lookFrom = (from: Vector3, to: Vector3) => {
    tmp.copy(to).sub(from);
    const len = tmp.length();
    focusDistance = Math.max(0.3, len);
    if (len < 1e-6) return;
    tmp.divideScalar(len);
    yaw = Math.atan2(-tmp.x, -tmp.z);
    pitch = Math.asin(Math.max(-1, Math.min(1, tmp.y)));
  };
  lookFrom(position, focusPoint);

  const computeBasis = () => {
    forward.set(-Math.sin(yaw) * Math.cos(pitch), Math.sin(pitch), -Math.cos(yaw) * Math.cos(pitch));
    right.set(Math.cos(yaw), 0, -Math.sin(yaw));
  };

  // Smooth easing toward a focus target (F key / reset)
  let easeTarget: { pos: Vector3; look: Vector3 } | null = null;

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

  // ── Input ─────────────────────────────────────────────────────────────────────
  type DragKind = "look" | "pan" | "click" | null;
  let dragKind: DragKind = null;
  let lastX = 0;
  let lastY = 0;
  let downX = 0;
  let downY = 0;
  let activePointer: number | null = null;
  const keys = new Set<string>();
  let shiftHeld = false;

  const ndcFromEvent = (e: PointerEvent): [number, number] => {
    const rect = canvas.getBoundingClientRect();
    return [
      Math.max(0, Math.min(1, (e.clientX - rect.left) / Math.max(1, rect.width))),
      Math.max(0, Math.min(1, (e.clientY - rect.top) / Math.max(1, rect.height))),
    ];
  };

  const onPointerDown = (e: PointerEvent) => {
    if (activePointer !== null) return;
    // Gizmo handle under the cursor: TransformControls owns this drag
    if (e.button === 0 && !e.altKey && controls.axis !== null) return;
    activePointer = e.pointerId;
    lastX = downX = e.clientX;
    lastY = downY = e.clientY;
    if (e.button === 2) dragKind = "look";
    else if (e.button === 1 || (e.button === 0 && e.altKey)) dragKind = "pan";
    else if (e.button === 0) dragKind = "click";
    else dragKind = null;
    if (dragKind === "look" || dragKind === "pan") {
      try {
        canvas.setPointerCapture(e.pointerId);
      } catch {
        /* capture unsupported */
      }
      e.preventDefault();
    }
    easeTarget = null;
    host.wake();
  };

  const onPointerMove = (e: PointerEvent) => {
    if (activePointer !== e.pointerId || !dragKind) return;
    const dx = e.clientX - lastX;
    const dy = e.clientY - lastY;
    lastX = e.clientX;
    lastY = e.clientY;
    if (dragKind === "look") {
      yaw -= dx * 0.0042;
      pitch = Math.max(-1.52, Math.min(1.52, pitch - dy * 0.0042));
      host.wake();
    } else if (dragKind === "pan") {
      computeBasis();
      const rect = canvas.getBoundingClientRect();
      const worldPerPx =
        (2 * Math.tan((camera.fov * Math.PI) / 360) * focusDistance) / Math.max(1, rect.height);
      tmp.set(0, 0, 0).addScaledVector(right, -dx * worldPerPx);
      // camera-up = right × forward
      const camUp = new Vector3().crossVectors(right, forward).normalize();
      tmp.addScaledVector(camUp, dy * worldPerPx);
      position.add(tmp);
      host.wake();
    }
  };

  const endPointer = (e: PointerEvent) => {
    if (activePointer !== e.pointerId) return;
    const kind = dragKind;
    activePointer = null;
    dragKind = null;
    try {
      canvas.releasePointerCapture(e.pointerId);
    } catch {
      /* not captured */
    }
    if (kind === "look") keys.clear();
    if (kind === "click" && !dragging && controls.axis === null) {
      const moved = Math.hypot(e.clientX - downX, e.clientY - downY);
      if (moved < 4) {
        const [nx, ny] = ndcFromEvent(e);
        const hitId = host.pickObjectAt(nx, ny);
        host.onSelectObject(hitId);
      }
    }
    host.wake();
  };

  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    computeBasis();
    const step = Math.max(0.08, focusDistance * 0.12) * Math.sign(e.deltaY) * -1;
    position.addScaledVector(forward, step);
    focusDistance = Math.max(0.3, focusDistance - step);
    easeTarget = null;
    host.wake();
  };

  const onContextMenu = (e: Event) => e.preventDefault();

  // Capture phase on window: while RMB is held, fly keys never reach Studio shortcuts
  const onKeyDown = (e: KeyboardEvent) => {
    const k = e.key.toLowerCase();
    if (dragKind !== "look" || !FLY_KEYS.has(k)) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    keys.add(k);
    shiftHeld = e.shiftKey;
    e.preventDefault();
    e.stopPropagation();
    host.wake();
  };
  const onKeyUp = (e: KeyboardEvent) => {
    const k = e.key.toLowerCase();
    shiftHeld = e.shiftKey;
    if (keys.delete(k)) {
      e.preventDefault();
      e.stopPropagation();
    }
  };
  const onBlur = () => {
    keys.clear();
    dragKind = null;
    activePointer = null;
  };

  canvas.addEventListener("pointerdown", onPointerDown);
  canvas.addEventListener("pointermove", onPointerMove);
  canvas.addEventListener("pointerup", endPointer);
  canvas.addEventListener("pointercancel", endPointer);
  canvas.addEventListener("wheel", onWheel, { passive: false });
  canvas.addEventListener("contextmenu", onContextMenu);
  window.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("keyup", onKeyUp, true);
  window.addEventListener("blur", onBlur);

  // ── Per-frame ─────────────────────────────────────────────────────────────────
  const update = (dt: number) => {
    computeBasis();

    if (keys.size > 0) {
      const speed = Math.max(1.2, focusDistance * 0.9) * (shiftHeld || keys.has("shift") ? 3 : 1);
      tmp.set(0, 0, 0);
      if (keys.has("w")) tmp.add(forward);
      if (keys.has("s")) tmp.sub(forward);
      if (keys.has("d")) tmp.add(right);
      if (keys.has("a")) tmp.sub(right);
      if (keys.has("e")) tmp.add(up);
      if (keys.has("q")) tmp.sub(up);
      if (tmp.lengthSq() > 0) position.addScaledVector(tmp.normalize(), speed * dt);
    }

    if (easeTarget) {
      const a = 1 - Math.exp(-10 * dt);
      position.lerp(easeTarget.pos, a);
      lookFrom(position, easeTarget.look);
      computeBasis();
      if (position.distanceToSquared(easeTarget.pos) < 1e-5) easeTarget = null;
    }

    camera.position.copy(position);
    euler.set(pitch, yaw, 0, "YXZ");
    camera.quaternion.setFromEuler(euler);
    camera.updateMatrixWorld();

    // Selection may point at a pivot that was (re)created after a model reload
    attachSelected();
  };

  const captureCameraState = (): Scene3DCameraCapture => {
    computeBasis();
    const dist = Math.max(0.3, focusDistance);
    const target = position.clone().addScaledVector(forward, dist);
    const offset = position.clone().sub(target);
    const el = Math.asin(Math.max(-1, Math.min(1, offset.y / dist)));
    const az = Math.atan2(offset.x, offset.z);
    return {
      target: [round(target.x), round(target.y), round(target.z)],
      distance: round(dist, 2),
      azimuthDeg: round(az * RAD2DEG, 1),
      elevationDeg: round(el * RAD2DEG, 1),
      fov: round(camera.fov, 1),
    };
  };

  const focusSelected = () => {
    const pivot = selectedId ? host.getPivot(selectedId) : null;
    if (!pivot) return;
    const box = new Box3().setFromObject(pivot);
    if (box.isEmpty()) return;
    const center = box.getCenter(new Vector3());
    const size = box.getSize(new Vector3());
    const radius = Math.max(0.4, size.length() * 0.5);
    const dist = radius / Math.sin((camera.fov * Math.PI) / 360) * 1.15;
    computeBasis();
    const pos = center.clone().addScaledVector(forward, -dist);
    easeTarget = { pos, look: center };
    host.wake();
  };

  const resetView = (view: Scene3DEditorView) => {
    easeTarget = { pos: new Vector3(...view.position), look: new Vector3(...view.target) };
    host.wake();
  };

  let disposed = false;
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    canvas.removeEventListener("pointerdown", onPointerDown);
    canvas.removeEventListener("pointermove", onPointerMove);
    canvas.removeEventListener("pointerup", endPointer);
    canvas.removeEventListener("pointercancel", endPointer);
    canvas.removeEventListener("wheel", onWheel);
    canvas.removeEventListener("contextmenu", onContextMenu);
    window.removeEventListener("keydown", onKeyDown, true);
    window.removeEventListener("keyup", onKeyUp, true);
    window.removeEventListener("blur", onBlur);
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
      dragging ||
      keys.size > 0 ||
      easeTarget !== null ||
      dragKind === "look" ||
      dragKind === "pan" ||
      (holdId !== null && performance.now() < holdUntil),
    update,
    captureCameraState,
    focusSelected,
    resetView,
    dispose,
  };
}
