import {
  AmbientLight,
  Color,
  DirectionalLight,
  DoubleSide,
  Material,
  Mesh,
  MeshBasicMaterial,
  PlaneGeometry,
  PMREMGenerator,
  PointLight,
  Scene,
  WebGLRenderTarget,
} from "three";
import type { Scene3DConfig, Scene3DEnvPreset } from "../../types/prism";

// Procedural IBL presets, background, real-clock day/night lighting and the
// wallpaper camera orbit. Pure functions: no React, shared by runtime & editor.

export interface EnvPresetSpec {
  bgHex: string;
  ambientHex: string;
  ambientIntensity: number;
  keyHex: string;
  keyIntensity: number;
  keyPos: [number, number, number];
  rimHex: string;
  rimIntensity: number;
  rimPos: [number, number, number];
  cards: Array<{
    color: string;
    intensity: number;
    pos: [number, number, number];
    rot: [number, number, number];
    scale: [number, number];
  }>;
}

export const ENV_SPECS: Record<Scene3DEnvPreset, EnvPresetSpec> = {
  STUDIO_DARK: {
    bgHex: "#07080b",
    ambientHex: "#b8c0d0",
    ambientIntensity: 0.4,
    keyHex: "#ffffff",
    keyIntensity: 2.2,
    keyPos: [4.5, 5.5, 4.5],
    rimHex: "#c8d4ea",
    rimIntensity: 1.4,
    rimPos: [-4.5, 2.2, -3.5],
    cards: [
      { color: "#ffffff", intensity: 4.0, pos: [0, 6.5, 1.5], rot: [Math.PI / 2, 0, 0], scale: [9, 5] },
      { color: "#d6e0f0", intensity: 2.4, pos: [-7, 2.5, 2], rot: [0, Math.PI / 2, 0], scale: [6, 5] },
      { color: "#f4ede4", intensity: 2.0, pos: [7, 1.5, -2], rot: [0, -Math.PI / 2, 0], scale: [5, 5] },
      { color: "#1d2028", intensity: 0.8, pos: [0, -5, 0], rot: [-Math.PI / 2, 0, 0], scale: [14, 14] },
    ],
  },
  CITY_NIGHT: {
    bgHex: "#05070e",
    ambientHex: "#7b92bc",
    ambientIntensity: 0.35,
    keyHex: "#b9d7ff",
    keyIntensity: 2.1,
    keyPos: [3.8, 5.0, 4.2],
    rimHex: "#9aa8ff",
    rimIntensity: 2.0,
    rimPos: [-5.0, 2.0, -3.2],
    cards: [
      { color: "#9ac7ff", intensity: 4.2, pos: [-6.5, 3, 2], rot: [0, Math.PI / 2, 0], scale: [6, 6] },
      { color: "#c9d4ff", intensity: 3.0, pos: [6.5, 2, -2], rot: [0, -Math.PI / 2, 0], scale: [5, 6] },
      { color: "#4d6796", intensity: 1.5, pos: [0, 7, 0], rot: [Math.PI / 2, 0, 0], scale: [10, 10] },
      { color: "#131926", intensity: 0.7, pos: [0, -5, 0], rot: [-Math.PI / 2, 0, 0], scale: [14, 14] },
    ],
  },
  SUNSET: {
    bgHex: "#100a0b",
    ambientHex: "#e6b8a2",
    ambientIntensity: 0.45,
    keyHex: "#ffbe85",
    keyIntensity: 2.6,
    keyPos: [5.5, 3.2, 3.8],
    rimHex: "#b79ad8",
    rimIntensity: 1.5,
    rimPos: [-4.5, 3.4, -4.0],
    cards: [
      { color: "#ffb06e", intensity: 4.5, pos: [7, 2.2, 2], rot: [0, -Math.PI / 2, 0], scale: [7, 4.5] },
      { color: "#a588c9", intensity: 2.2, pos: [-7, 4.0, -1], rot: [0, Math.PI / 2, 0], scale: [7, 5] },
      { color: "#ffe0bd", intensity: 2.4, pos: [0, 6.8, 2], rot: [Math.PI / 2, 0, 0], scale: [9, 4] },
      { color: "#221719", intensity: 0.7, pos: [0, -5, 0], rot: [-Math.PI / 2, 0, 0], scale: [14, 14] },
    ],
  },
  DAYLIGHT: {
    bgHex: "#15181e",
    ambientHex: "#eef2f8",
    ambientIntensity: 0.65,
    keyHex: "#fffaf2",
    keyIntensity: 2.6,
    keyPos: [4.5, 6.5, 5.0],
    rimHex: "#dce8f8",
    rimIntensity: 1.3,
    rimPos: [-5.0, 3.5, -3.5],
    cards: [
      { color: "#ffffff", intensity: 4.6, pos: [0, 7, 1], rot: [Math.PI / 2, 0, 0], scale: [12, 8] },
      { color: "#eef4ff", intensity: 2.8, pos: [-7, 2.5, 1], rot: [0, Math.PI / 2, 0], scale: [8, 6] },
      { color: "#fff6ea", intensity: 2.6, pos: [7, 2.5, 1], rot: [0, -Math.PI / 2, 0], scale: [8, 6] },
      { color: "#4d535e", intensity: 1.2, pos: [0, -5, 0], rot: [-Math.PI / 2, 0, 0], scale: [14, 14] },
    ],
  },
};

export function buildProceduralEnvironment(
  pmrem: PMREMGenerator,
  preset: Scene3DEnvPreset
): WebGLRenderTarget {
  const envScene = new Scene();
  const spec = ENV_SPECS[preset] ?? ENV_SPECS.STUDIO_DARK;
  envScene.background = new Color(spec.bgHex);

  const planeGeo = new PlaneGeometry(1, 1);
  const mats: Material[] = [];
  for (const card of spec.cards) {
    const c = new Color(card.color).multiplyScalar(card.intensity);
    const mat = new MeshBasicMaterial({ color: c, side: DoubleSide });
    mats.push(mat);
    const mesh = new Mesh(planeGeo, mat);
    mesh.position.set(card.pos[0], card.pos[1], card.pos[2]);
    mesh.rotation.set(card.rot[0], card.rot[1], card.rot[2]);
    mesh.scale.set(card.scale[0], card.scale[1], 1);
    envScene.add(mesh);
  }

  const target = pmrem.fromScene(envScene, 0.04);
  planeGeo.dispose();
  mats.forEach((m) => m.dispose());
  return target;
}

interface DayNightKeyframe {
  hour: number;
  bg: Color;
  amb: Color;
  ambInt: number;
  key: Color;
  keyInt: number;
  rim: Color;
  rimInt: number;
}

const DAY_NIGHT_KEYS: DayNightKeyframe[] = [
  { hour: 0, bg: new Color("#05070e"), amb: new Color("#6e84b0"), ambInt: 0.3, key: new Color("#8baae6"), keyInt: 1.4, rim: new Color("#5a6c9e"), rimInt: 1.5 },
  { hour: 5.5, bg: new Color("#0d0c14"), amb: new Color("#9f8ba8"), ambInt: 0.38, key: new Color("#ffa876"), keyInt: 2.0, rim: new Color("#7f89c4"), rimInt: 1.4 },
  { hour: 7.5, bg: new Color("#16151c"), amb: new Color("#e6c7b2"), ambInt: 0.52, key: new Color("#ffd4a3"), keyInt: 2.5, rim: new Color("#b8c8e8"), rimInt: 1.3 },
  { hour: 13.0, bg: new Color("#161a22"), amb: new Color("#eef3fa"), ambInt: 0.68, key: new Color("#fffaf0"), keyInt: 2.8, rim: new Color("#d8e6fa"), rimInt: 1.2 },
  { hour: 18.0, bg: new Color("#140b0e"), amb: new Color("#df9e82"), ambInt: 0.48, key: new Color("#ff9854"), keyInt: 2.7, rim: new Color("#a482cf"), rimInt: 1.6 },
  { hour: 20.5, bg: new Color("#070812"), amb: new Color("#788db8"), ambInt: 0.34, key: new Color("#9ab4e8"), keyInt: 1.6, rim: new Color("#6d7db8"), rimInt: 1.6 },
  { hour: 24.0, bg: new Color("#05070e"), amb: new Color("#6e84b0"), ambInt: 0.3, key: new Color("#8baae6"), keyInt: 1.4, rim: new Color("#5a6c9e"), rimInt: 1.5 },
];

const dnAmbTmp = new Color();
const dnKeyTmp = new Color();
const dnRimTmp = new Color();
const solidBgTmp = new Color();
const envBgTmp = new Color();
const dnBgTmp = new Color();

function dayNightSegment(hourRaw: number): { k0: DayNightKeyframe; k1: DayNightKeyframe; ease: number } {
  const h = ((hourRaw % 24) + 24) % 24;
  let k0 = DAY_NIGHT_KEYS[0];
  let k1 = DAY_NIGHT_KEYS[DAY_NIGHT_KEYS.length - 1];
  for (let i = 0; i < DAY_NIGHT_KEYS.length - 1; i++) {
    if (h >= DAY_NIGHT_KEYS[i].hour && h <= DAY_NIGHT_KEYS[i + 1].hour) {
      k0 = DAY_NIGHT_KEYS[i];
      k1 = DAY_NIGHT_KEYS[i + 1];
      break;
    }
  }
  const span = Math.max(0.001, k1.hour - k0.hour);
  const t = Math.max(0, Math.min(1, (h - k0.hour) / span));
  return { k0, k1, ease: t * t * (3 - 2 * t) };
}

/** Interpolated day/night sky colour for `hour`, written into a shared scratch Color (do not retain). */
export function sampleDayNightBackground(hour: number): Color {
  const { k0, k1, ease } = dayNightSegment(hour);
  return dnBgTmp.copy(k0.bg).lerp(k1.bg, ease);
}

/**
 * Canvas background per `cfg.backgroundMode`.
 * TRANSPARENT → no background (cleared with alpha 0); SOLID → `backgroundColor`;
 * ENVIRONMENT → preset colour, or the day/night sky colour when `dayNightBg` is given.
 * `scene.environment` (IBL) is untouched. Call only when mode/colour/hour change.
 */
export function applySceneBackground(scene: Scene, cfg: Scene3DConfig, dayNightBg?: Color): void {
  if (cfg.backgroundMode === "SOLID") {
    solidBgTmp.set(cfg.backgroundColor || "#07080b");
    scene.background = solidBgTmp;
  } else if (cfg.backgroundMode === "ENVIRONMENT") {
    if (cfg.autoDayNight && dayNightBg) {
      envBgTmp.copy(dayNightBg);
    } else {
      envBgTmp.set((ENV_SPECS[cfg.envPreset] ?? ENV_SPECS.STUDIO_DARK).bgHex);
    }
    scene.background = envBgTmp;
  } else {
    scene.background = null;
  }
}

export function applyDayNightLighting(
  hourRaw: number,
  scene: Scene,
  ambientLight: AmbientLight,
  keyLight: DirectionalLight,
  rimLight: PointLight,
  cfg: Scene3DConfig
): void {
  const h = ((hourRaw % 24) + 24) % 24;
  const { k0, k1, ease } = dayNightSegment(h);

  dnAmbTmp.copy(k0.amb).lerp(k1.amb, ease);
  dnKeyTmp.copy(k0.key).lerp(k1.key, ease);
  dnRimTmp.copy(k0.rim).lerp(k1.rim, ease);

  applySceneBackground(scene, cfg, sampleDayNightBackground(h));
  ambientLight.color.copy(dnAmbTmp);
  ambientLight.intensity = k0.ambInt + (k1.ambInt - k0.ambInt) * ease;
  keyLight.color.copy(dnKeyTmp);
  keyLight.intensity = k0.keyInt + (k1.keyInt - k0.keyInt) * ease;
  rimLight.color.copy(dnRimTmp);
  rimLight.intensity = k0.rimInt + (k1.rimInt - k0.rimInt) * ease;

  // Smooth solar arc across sky (rises east at 6h, zenith at 12h, sets west at 18h; moon overhead at midnight)
  const sunAngle = ((h - 6) / 12) * Math.PI;
  const isDay = h >= 6 && h <= 18;
  const arcX = isDay ? Math.cos(sunAngle) * 6.5 : -Math.cos(sunAngle) * 5.0;
  const arcY = isDay ? Math.max(1.2, Math.sin(sunAngle) * 7.2) : 4.2;
  const arcZ = isDay ? 4.2 : -3.8;
  keyLight.position.set(arcX, arcY, arcZ);
  rimLight.position.set(-arcX * 0.7, 2.8, -arcZ * 0.8);
}

/**
 * Wallpaper camera from config: spherical orbit around `camera.target`
 * (or the legacy [0, targetY, 0]) plus the live cursor-orbit offsets in degrees.
 * Shared by the renderer and by the Sala 3D editor (initial / reset view).
 */
export function computeConfigCameraView(
  cfg: Scene3DConfig,
  orbitAzDeg: number,
  orbitElDeg: number
): { position: [number, number, number]; target: [number, number, number]; fov: number } {
  const cam = cfg.camera;
  const t = cam.target;
  const tx = t && Number.isFinite(t[0]) ? t[0] : 0;
  const ty = t && Number.isFinite(t[1]) ? t[1] : cam.targetY || 0;
  const tz = t && Number.isFinite(t[2]) ? t[2] : 0;
  const azRad = ((cam.azimuthDeg + orbitAzDeg) * Math.PI) / 180;
  const elRad = (Math.max(-80, Math.min(80, cam.elevationDeg + orbitElDeg)) * Math.PI) / 180;
  const dist = Math.max(0.3, cam.distance || 4.8);
  return {
    position: [
      tx + Math.sin(azRad) * Math.cos(elRad) * dist,
      ty + Math.sin(elRad) * dist,
      tz + Math.cos(azRad) * Math.cos(elRad) * dist,
    ],
    target: [tx, ty, tz],
    fov: Math.max(20, Math.min(85, cam.fov || 42)),
  };
}
