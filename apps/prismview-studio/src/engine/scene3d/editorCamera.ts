/**
 * Sala 3D free camera (Studio only). Lives in the `three-editor` chunk: imported
 * exclusively through the dynamic `scene3dEditorTools` import, never by desktop/widget.
 *
 * Controls
 *   RMB + drag ............ look around (Alt + RMB = orbit around the focus point)
 *   RMB + W A S D / Q E ... fly (Shift = faster)
 *   MMB drag / Alt + drag . pan
 *   Wheel ................. dolly forward / back
 *
 * Render on demand: input calls `wake()`; `needsFrames()` is true only while
 * something keeps moving without input (fly keys held, focus easing).
 */
import { Box3, Euler, PerspectiveCamera, Vector3 } from "three";

/** Orbit-style camera snapshot (same convention as `computeConfigCameraView`) */
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

export interface EditorCameraHost {
  canvas: HTMLCanvasElement;
  camera: PerspectiveCamera;
  initialView: Scene3DEditorView;
  wake: () => void;
  /** Fires when a camera drag (look/orbit/pan) starts or ends */
  onNavigate?: (active: boolean) => void;
}

export interface EditorCameraController {
  /** Disables every camera input (drag, wheel, fly keys); aborts an active drag */
  setEnabled: (enabled: boolean) => void;
  /** Integrates fly/easing and writes the camera transform. No allocations. */
  update: (dt: number) => void;
  /** Continuous frames needed (fly keys held or easing in progress) */
  needsFrames: () => boolean;
  /** True while a camera drag (look/orbit/pan) owns the pointer */
  isNavigating: () => boolean;
  getState: () => Scene3DCameraCapture;
  /** Eases to an orbit state. `fov` is ignored: the config FOV owns the projection. */
  setState: (state: Scene3DCameraCapture, animate?: boolean) => void;
  setView: (view: Scene3DEditorView, animate?: boolean) => void;
  /** Frames a world-space box keeping the current view direction */
  focusBox: (box: Box3) => boolean;
  dispose: () => void;
}

const RAD2DEG = 180 / Math.PI;
const DEG2RAD = Math.PI / 180;
const LOOK_SPEED = 0.0042;
const PITCH_LIMIT = 1.52;
const MIN_DISTANCE = 0.3;

function round(n: number, digits = 3): number {
  const f = 10 ** digits;
  return Math.round(n * f) / f;
}

export function createEditorCamera(host: EditorCameraHost): EditorCameraController {
  const { canvas, camera } = host;

  // ── State (all vectors pre-allocated) ─────────────────────────────────────────
  const position = new Vector3();
  let yaw = 0;
  let pitch = 0;
  let focusDistance = 1;
  const forward = new Vector3();
  const right = new Vector3();
  const camUp = new Vector3();
  const worldUp = new Vector3(0, 1, 0);
  const tmp = new Vector3();
  const pivot = new Vector3();
  const euler = new Euler(0, 0, 0, "YXZ");
  const easePos = new Vector3();
  const easeLook = new Vector3();
  const boxCenter = new Vector3();
  const boxSize = new Vector3();
  let easing = false;

  const lookFrom = (from: Vector3, to: Vector3) => {
    tmp.copy(to).sub(from);
    const len = tmp.length();
    focusDistance = Math.max(MIN_DISTANCE, len);
    if (len < 1e-6) return;
    tmp.divideScalar(len);
    yaw = Math.atan2(-tmp.x, -tmp.z);
    pitch = Math.asin(Math.max(-1, Math.min(1, tmp.y)));
  };

  const computeBasis = () => {
    const cp = Math.cos(pitch);
    forward.set(-Math.sin(yaw) * cp, Math.sin(pitch), -Math.cos(yaw) * cp);
    right.set(Math.cos(yaw), 0, -Math.sin(yaw));
    camUp.crossVectors(right, forward).normalize();
  };

  const applyView = (view: Scene3DEditorView, animate: boolean) => {
    if (animate) {
      easePos.set(view.position[0], view.position[1], view.position[2]);
      easeLook.set(view.target[0], view.target[1], view.target[2]);
      easing = true;
    } else {
      position.set(view.position[0], view.position[1], view.position[2]);
      pivot.set(view.target[0], view.target[1], view.target[2]);
      lookFrom(position, pivot);
      easing = false;
    }
    host.wake();
  };

  applyView(host.initialView, false);

  // ── Input ─────────────────────────────────────────────────────────────────────
  type DragKind = "look" | "orbit" | "pan" | null;
  let dragKind: DragKind = null;
  let activePointer: number | null = null;
  let lastX = 0;
  let lastY = 0;
  let kW = false;
  let kA = false;
  let kS = false;
  let kD = false;
  let kQ = false;
  let kE = false;
  let shiftHeld = false;
  let enabled = true;

  const clearKeys = () => {
    kW = kA = kS = kD = kQ = kE = false;
    shiftHeld = false;
  };
  const anyFlyKey = () => kW || kA || kS || kD || kQ || kE;

  const onPointerDown = (e: PointerEvent) => {
    if (!enabled || activePointer !== null) return;
    let kind: DragKind = null;
    if (e.button === 2) kind = e.altKey ? "orbit" : "look";
    else if (e.button === 1 || (e.button === 0 && e.altKey)) kind = "pan";
    if (!kind) return; // plain LMB belongs to selection / gizmo
    dragKind = kind;
    activePointer = e.pointerId;
    lastX = e.clientX;
    lastY = e.clientY;
    try {
      canvas.setPointerCapture(e.pointerId);
    } catch {
      /* capture unsupported */
    }
    e.preventDefault();
    easing = false;
    host.onNavigate?.(true);
    host.wake();
  };

  const onPointerMove = (e: PointerEvent) => {
    if (activePointer !== e.pointerId || !dragKind) return;
    const dx = e.clientX - lastX;
    const dy = e.clientY - lastY;
    lastX = e.clientX;
    lastY = e.clientY;
    if (dx === 0 && dy === 0) return;
    if (dragKind === "look") {
      yaw -= dx * LOOK_SPEED;
      pitch = Math.max(-PITCH_LIMIT, Math.min(PITCH_LIMIT, pitch - dy * LOOK_SPEED));
    } else if (dragKind === "orbit") {
      computeBasis();
      pivot.copy(position).addScaledVector(forward, focusDistance);
      yaw -= dx * LOOK_SPEED;
      pitch = Math.max(-PITCH_LIMIT, Math.min(PITCH_LIMIT, pitch - dy * LOOK_SPEED));
      computeBasis();
      position.copy(pivot).addScaledVector(forward, -focusDistance);
    } else {
      computeBasis();
      const worldPerPx =
        (2 * Math.tan((camera.fov * DEG2RAD) / 2) * focusDistance) / Math.max(1, canvas.clientHeight);
      position.addScaledVector(right, -dx * worldPerPx).addScaledVector(camUp, dy * worldPerPx);
    }
    host.wake();
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
    if (kind === "look" || kind === "orbit") clearKeys();
    host.onNavigate?.(false);
    host.wake();
  };

  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    if (!enabled || e.deltaY === 0) return;
    computeBasis();
    const step = Math.max(0.08, focusDistance * 0.12) * -Math.sign(e.deltaY);
    // Dolly: never push through the focus point
    const applied = step > 0 ? Math.min(step, focusDistance - MIN_DISTANCE) : step;
    position.addScaledVector(forward, applied);
    focusDistance = Math.max(MIN_DISTANCE, focusDistance - applied);
    easing = false;
    host.wake();
  };

  const onContextMenu = (e: Event) => e.preventDefault();

  const setKey = (key: string, down: boolean): boolean => {
    switch (key) {
      case "w":
        kW = down;
        return true;
      case "a":
        kA = down;
        return true;
      case "s":
        kS = down;
        return true;
      case "d":
        kD = down;
        return true;
      case "q":
        kQ = down;
        return true;
      case "e":
        kE = down;
        return true;
      case "shift":
        shiftHeld = down;
        return true;
      default:
        return false;
    }
  };

  // Capture phase on window: while RMB is held, fly keys never reach Studio shortcuts
  const onKeyDown = (e: KeyboardEvent) => {
    if (!enabled || (dragKind !== "look" && dragKind !== "orbit")) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (!setKey(e.key.toLowerCase(), true)) return;
    shiftHeld = e.shiftKey;
    e.preventDefault();
    e.stopPropagation();
    host.wake();
  };
  const onKeyUp = (e: KeyboardEvent) => {
    const k = e.key.toLowerCase();
    const wasHeld =
      (k === "w" && kW) ||
      (k === "a" && kA) ||
      (k === "s" && kS) ||
      (k === "d" && kD) ||
      (k === "q" && kQ) ||
      (k === "e" && kE);
    setKey(k, false);
    shiftHeld = e.shiftKey;
    if (wasHeld) {
      e.preventDefault();
      e.stopPropagation();
    }
  };
  const releaseNavigation = () => {
    const wasNavigating = dragKind !== null;
    if (activePointer !== null) {
      try {
        canvas.releasePointerCapture(activePointer);
      } catch {
        /* not captured */
      }
    }
    clearKeys();
    dragKind = null;
    activePointer = null;
    if (wasNavigating) host.onNavigate?.(false);
  };
  const onBlur = releaseNavigation;
  const setEnabled = (on: boolean) => {
    if (enabled === on) return;
    enabled = on;
    if (!on) releaseNavigation();
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
    const step = Math.min(0.1, Math.max(0, dt));
    computeBasis();

    if (anyFlyKey()) {
      const speed = Math.max(1.2, focusDistance * 0.9) * (shiftHeld ? 3 : 1);
      tmp.set(0, 0, 0);
      if (kW) tmp.add(forward);
      if (kS) tmp.sub(forward);
      if (kD) tmp.add(right);
      if (kA) tmp.sub(right);
      if (kE) tmp.add(worldUp);
      if (kQ) tmp.sub(worldUp);
      if (tmp.lengthSq() > 0) position.addScaledVector(tmp.normalize(), speed * step);
    }

    if (easing) {
      const a = 1 - Math.exp(-10 * step);
      position.lerp(easePos, a);
      lookFrom(position, easeLook);
      computeBasis();
      if (position.distanceToSquared(easePos) < 1e-5) {
        position.copy(easePos);
        lookFrom(position, easeLook);
        easing = false;
      }
    }

    camera.position.copy(position);
    euler.set(pitch, yaw, 0, "YXZ");
    camera.quaternion.setFromEuler(euler);
    camera.updateMatrixWorld();
  };

  const getState = (): Scene3DCameraCapture => {
    computeBasis();
    const dist = Math.max(MIN_DISTANCE, focusDistance);
    pivot.copy(position).addScaledVector(forward, dist);
    tmp.copy(position).sub(pivot);
    const el = Math.asin(Math.max(-1, Math.min(1, tmp.y / dist)));
    const az = Math.atan2(tmp.x, tmp.z);
    return {
      target: [round(pivot.x), round(pivot.y), round(pivot.z)],
      distance: round(dist, 2),
      azimuthDeg: round(az * RAD2DEG, 1),
      elevationDeg: round(el * RAD2DEG, 1),
      fov: round(camera.fov, 1),
    };
  };

  const setState = (state: Scene3DCameraCapture, animate = true) => {
    const tx = Number.isFinite(state.target[0]) ? state.target[0] : 0;
    const ty = Number.isFinite(state.target[1]) ? state.target[1] : 0;
    const tz = Number.isFinite(state.target[2]) ? state.target[2] : 0;
    const dist = Math.max(MIN_DISTANCE, Number.isFinite(state.distance) ? state.distance : 4.8);
    const az = (Number.isFinite(state.azimuthDeg) ? state.azimuthDeg : 0) * DEG2RAD;
    const el = Math.max(-80, Math.min(80, Number.isFinite(state.elevationDeg) ? state.elevationDeg : 0)) * DEG2RAD;
    applyView(
      {
        position: [
          tx + Math.sin(az) * Math.cos(el) * dist,
          ty + Math.sin(el) * dist,
          tz + Math.cos(az) * Math.cos(el) * dist,
        ],
        target: [tx, ty, tz],
        fov: camera.fov,
      },
      animate
    );
  };

  const focusBox = (box: Box3): boolean => {
    if (box.isEmpty()) return false;
    box.getCenter(boxCenter);
    box.getSize(boxSize);
    const radius = Math.max(0.4, boxSize.length() * 0.5);
    const dist = (radius / Math.sin((camera.fov * DEG2RAD) / 2)) * 1.15;
    computeBasis();
    easePos.copy(boxCenter).addScaledVector(forward, -dist);
    easeLook.copy(boxCenter);
    easing = true;
    host.wake();
    return true;
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
    if (activePointer !== null) {
      try {
        canvas.releasePointerCapture(activePointer);
      } catch {
        /* not captured */
      }
    }
    activePointer = null;
    dragKind = null;
    clearKeys();
  };

  return {
    setEnabled,
    update,
    needsFrames: () => easing || anyFlyKey(),
    isNavigating: () => dragKind !== null,
    getState,
    setState,
    setView: (view, animate = true) => applyView(view, animate),
    focusBox,
    dispose,
  };
}
