// Tipos, constantes, runtime Tauri (invokeCmd/resolveMediaUrl) e presets de vidro.
import { evaluateFxTemplate } from "../engine/fx/fxEngine";
import { SPRING } from "../engine/motion";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";


export type PackageType = "WALLPAPER" | "WIDGET";
export type LayerKind =
  | "VIDEO"
  | "IMAGE"
  | "SVG"
  | "TEXT"
  | "GLASS"
  | "GROUP"
  | "PROGRESS"
  | "SCENE_3D";
export type Scene3DEnvPreset = "STUDIO_DARK" | "CITY_NIGHT" | "SUNSET" | "DAYLIGHT";
export type Scene3DShowcaseVariant = "MONOLITH" | "GYROSCOPE" | "KINETIC_SLAB";
export type Scene3DObjectKind = "GLB" | "SHOWCASE";
/** TRANSPARENT (default) = alpha 0 behind the models; SOLID = backgroundColor; ENVIRONMENT = envPreset/HDRI as backdrop */
export type Scene3DBackgroundMode = "TRANSPARENT" | "SOLID" | "ENVIRONMENT";
export type Scene3DBindingSource =
  | "CURSOR_LOOK"
  | "HOVER_MESH"
  | "CLICK_MESH"
  | "AUDIO_BASS"
  | "AUDIO_MID"
  | "AUDIO_TREBLE"
  | "KEY_PULSE"
  | "MEDIA_PLAYING"
  | "CPU_USAGE";
export type Scene3DBindingTarget =
  | "NODE_LOOK_AT"
  | "PLAY_CLIP"
  | "EMISSIVE_GLOW"
  | "NODE_SCALE_PULSE"
  | "MORPH_WEIGHT"
  | "NODE_ROTATE_SPIN"
  /** Hinge/tilt proportional to the signal on the chosen axis (arm, wing, jaw, lid, VU needle) */
  | "NODE_ROTATE_SWING"
  /** Piston/jump offset proportional to the signal on the chosen axis (subwoofer cone, 3D key) */
  | "NODE_MOVE_OFFSET";
export type Scene3DAxis = "X" | "Y" | "Z";
/** "CAP" honours maxFps (5–200); "MONITOR" follows the display refresh rate (requestAnimationFrame) */
export type Scene3DFpsMode = "CAP" | "MONITOR";

/** Current Scene3DConfig schema; bump + extend normalizeScene3DConfig() when the shape changes */
export const SCENE3D_SCHEMA_VERSION = 3;
/** Pre-v3 default spin (rad/s); migrated to 0 because v3 models are static unless the user opts in */
export const LEGACY_DEFAULT_AUTO_ROTATE = 0.22;
export const SCENE3D_FPS_MIN = 5;
export const SCENE3D_FPS_MAX = 200;
export const SCENE3D_FPS_STEP = 5;

/** Snap any legacy/hand-edited FPS value to the 5–200 slider grid (step 5) */
export function clampScene3DFps(raw: unknown): number {
  const n = typeof raw === "number" && Number.isFinite(raw) ? raw : 60;
  const snapped = Math.round(n / SCENE3D_FPS_STEP) * SCENE3D_FPS_STEP;
  return Math.max(SCENE3D_FPS_MIN, Math.min(SCENE3D_FPS_MAX, snapped));
}

export interface ReactiveSignal {
  bass: number;
  mid: number;
  treble: number;
  level: number;
  key_pulse: number;
  typing_energy: number;
}

export const DEFAULT_REACTIVE_SIGNAL: ReactiveSignal = {
  bass: 0,
  mid: 0,
  treble: 0,
  level: 0,
  key_pulse: 0,
  typing_energy: 0,
};

export interface Scene3DDynamicTexture {
  id: string;
  materialName: string;
  source: "MEDIA_COVER" | "CUSTOM_IMAGE";
  customSrc?: string;
  emissiveMap?: boolean;
}

export interface ReactiveBinding3D {
  id: string;
  enabled: boolean;
  /** Target room object id (omitted/empty falls back to primary object) */
  objectId?: string;
  source: Scene3DBindingSource;
  /** Mesh name for HOVER_MESH / CLICK_MESH ("*" matches any mesh in the model) */
  sourceNode: string;
  targetType: Scene3DBindingTarget;
  /** Bone/Mesh name ("__ROOT__" = whole model), Clip name, Material name, or Morph name */
  targetName: string;
  /** Degrees for NODE_LOOK_AT / NODE_ROTATE_SWING, intensity for EMISSIVE_GLOW, delta ratio for NODE_SCALE_PULSE, rad/s for NODE_ROTATE_SPIN, 0..1 for MORPH_WEIGHT, model units for NODE_MOVE_OFFSET */
  strength: number;
  /** Spring damping (0.03..0.3) */
  smoothing: number;
  invertX?: boolean;
  invertY?: boolean;
  /** Axis for NODE_ROTATE_SWING, NODE_MOVE_OFFSET and NODE_ROTATE_SPIN (default "Y") */
  axis?: Scene3DAxis;
  /** Flips the direction along `axis` */
  invertAxis?: boolean;
}

export interface Scene3DObject {
  id: string;
  name: string;
  kind: Scene3DObjectKind;
  modelSrc: string;
  showcaseVariant?: Scene3DShowcaseVariant;
  position: [number, number, number];
  /** Euler degrees [rx, ry, rz] */
  rotation: [number, number, number];
  scale: [number, number, number];
  visible: boolean;
  autoRotateSpeed: number;
  playback: {
    clipName: string;
    speed: number;
    loop: boolean;
  };
  dynamicTextures?: Scene3DDynamicTexture[];
  /** Hierarchy: id of another object in the same room (absent = root); transform is local to the parent */
  parentId?: string;
}

export interface Scene3DCamera {
  fov: number;
  /** Camera position = target + spherical(distance, azimuthDeg, elevationDeg) */
  distance: number;
  azimuthDeg: number;
  elevationDeg: number;
  /** @deprecated use `target[1]`; kept in sync by normalizeScene3DConfig() */
  targetY: number;
  /** Orbit target (Sala 3D free camera); always filled by normalizeScene3DConfig() */
  target?: [number, number, number];
  /** Cursor parallax in degrees */
  orbitParallaxX: number;
  orbitParallaxY: number;
  damping: number;
}

export interface Scene3DConfig {
  /** Absent on projects saved before schema 2; filled by normalizeScene3DConfig() */
  schemaVersion?: number;
  objects: Scene3DObject[];
  backgroundMode: Scene3DBackgroundMode;
  /** Used when backgroundMode === "SOLID" */
  backgroundColor: string;
  /** @deprecated schema ≤2 single model; migrated into objects[0] and never saved again */
  modelSrc?: string;
  /** @deprecated schema ≤2; migrated into objects[0].showcaseVariant */
  showcaseVariant?: Scene3DShowcaseVariant;
  envPreset: Scene3DEnvPreset;
  envIntensity: number;
  exposure: number;
  renderScale: number;
  /** FPS ceiling, 5–200 in steps of 5 (see clampScene3DFps). Ignored when fpsMode === "MONITOR". */
  maxFps: number;
  fpsMode?: Scene3DFpsMode;
  /** @deprecated schema ≤2; migrated into objects[0].autoRotateSpeed (0.22 → 0) */
  autoRotateSpeed?: number;
  /** @deprecated schema ≤2; migrated into objects[0].scale */
  modelScale?: number;
  /** @deprecated schema ≤2; migrated into objects[0].position[1] */
  modelOffsetY?: number;
  autoDayNight?: boolean;
  /** Optional 0..24 override for previewing solar cycle in Studio; null/undefined = real system clock */
  dayNightHourOverride?: number | null;
  camera: Scene3DCamera;
  /** @deprecated schema ≤2; migrated into objects[0].playback */
  playback?: Scene3DObject["playback"];
  bindings?: ReactiveBinding3D[];
  /** @deprecated schema ≤2; migrated into objects[0].dynamicTextures */
  dynamicTextures?: Scene3DDynamicTexture[];
}

export type Scene3DAutoSuggestionKind = "BINDING" | "DYNAMIC_TEXTURE";

export type Scene3DAutoSuggestionRole =
  | "HEAD"
  | "NECK"
  /** Spine/torso: subtle cursor follow (low amplitude, soft smoothing) */
  | "BODY"
  | "EYE"
  | "JAW"
  | "WING"
  | "FAN"
  | "WHEEL"
  | "SPEAKER"
  | "SCREEN";

export interface Scene3DAutoSuggestion {
  id: string;
  role: Scene3DAutoSuggestionRole;
  kind: Scene3DAutoSuggestionKind;
  /** Short PT-BR chip label */
  label: string;
  /** Tooltip describing the detected part and action */
  detail: string;
  /** Matched node / morph / material name */
  targetName: string;
  binding?: Omit<ReactiveBinding3D, "id" | "objectId">;
  dynamicTexture?: Omit<Scene3DDynamicTexture, "id">;
}

export interface Scene3DPickedPart {
  objectId: string;
  /** Index into Scene3DObjectTree.flat (-1 if unavailable) */
  nodeId: number;
  /** Target name for bindings ("__ROOT__" if unnamed) */
  name: string;
  type: "bone" | "mesh" | "group";
  morphs: string[];
  materials: string[];
}

export interface Scene3DObjectInspectInfo {
  objectId: string;
  clipNames: string[];
  boneNames: string[];
  meshNames: string[];
  materialNames: string[];
  morphNames: string[];
  meshCount: number;
  triangleCount: number;
  textureCount: number;
  maxTextureSize: number;
  estimatedVramMb: number;
  isDefaultSculpture: boolean;
  error?: string | null;
  suggestions?: Scene3DAutoSuggestion[];
}

export interface Scene3DInspectInfo {
  clipNames: string[];
  boneNames: string[];
  meshNames: string[];
  materialNames: string[];
  morphNames: string[];
  meshCount: number;
  triangleCount: number;
  textureCount: number;
  maxTextureSize: number;
  estimatedVramMb: number;
  warnings: string[];
  isDefaultSculpture: boolean;
  error?: string | null;
  byObject?: Record<string, Scene3DObjectInspectInfo>;
}
export type ClickAction =
  | "NONE"
  | "MEDIA_PLAY_PAUSE"
  | "MEDIA_NEXT"
  | "MEDIA_PREV"
  // Executadas pelo motor nativo (toque no desktop): URL, app, variáveis globais.
  | "OPEN_URL"
  | "OPEN_APP"
  | "TOGGLE_GLOBAL"
  | "SET_GLOBAL"
  | "NEXT_GLOBAL_LIST";
export type BlendModeCss =
  | "normal"
  | "screen"
  | "multiply"
  | "overlay"
  | "lighten"
  | "darken"
  | "soft-light"
  | "difference"
  | "plus-lighter";

export interface GlobalVar {
  name: string;
  kind: "SWITCH" | "NUMBER" | "TEXT" | "COLOR";
  value: string;
  label: string;
  exposed: boolean;
}

export interface GlassEffect {
  enabled: boolean;
  blurPx: number;
  refractionScale: number;
  chromaticAberration: number;
  frostOpacity: number;
  tintColor: string;
  borderRadius: number;
  borderOpacity: number;
  shadowBlur: number;
  saturation: number;
  brightness?: number;
  contrast?: number;
  noiseOpacity?: number;
  bevelWidth?: number;
  specularIntensity?: number;
  textMode?: "GLYPH" | "PILL";
}

export interface SceneLayer {
  id: string;
  name: string;
  kind: LayerKind;
  x: number; // 0..1 normalized
  y: number; // 0..1 normalized
  width: number; // px in logical space (1920x1080 for wallpaper, widget w x h for widget)
  height: number;
  rotation: number;
  opacity: number;
  blendMode: BlendModeCss;
  hidden: boolean;
  locked: boolean;
  parallaxX: number; // -1.5..1.5
  parallaxY: number; // -1.5..1.5
  clickAction?: ClickAction;
  /** OPEN_URL */
  clickUrl?: string;
  /** OPEN_APP (caminho do executável/arquivo) */
  clickTarget?: string;
  /** Import de pacote: alvo OPEN_APP original, desativado até o usuário permitir. */
  clickTargetPending?: string;
  /** Import de pacote: ação OPEN_APP aguardando revisão (ver `utils/importReview`). */
  clickTargetNeedsReview?: boolean;
  clickLabel?: string;
  /** TOGGLE_GLOBAL / SET_GLOBAL / NEXT_GLOBAL_LIST */
  clickGlobal?: string;
  clickValue?: string;
  video?: {
    src: string;
    speed: number;
    muted: boolean;
    volume: number;
    objectFit: "cover" | "contain";
    clipRadius: number;
  };
  image?: {
    src: string;
    objectFit: "cover" | "contain";
    borderRadius: number;
    sourceMode?: "FILE" | "MEDIA_COVER";
  };
  svg?: {
    iconName: string;
    customSvg: string;
    color: string;
    colorAlpha?: number;
    strokeWidth: number;
    /** Preenchidos só ao aplicar no motor nativo (`withNativeAssets`): markup do ícone embutido. */
    resolvedSvg?: string;
    /** Ícone alternativo (Play/Pause: "pause" enquanto a mídia toca). */
    resolvedSvgAlt?: string;
  };
  text?: {
    content: string;
    fontFamily: string;
    fontPath: string;
    fontSize: number;
    fontWeight: number;
    letterSpacing: number;
    align: "left" | "center" | "right";
    color: string;
    colorAlpha?: number;
    stemStretch: number;
    shadowBlur: number;
    /** Largura máxima da caixa de texto em px lógicos (undefined/0 = largura automática sem quebrar linha) */
    maxWidth?: number;
  };
  progress?: {
    source: "MEDIA" | "CPU" | "RAM" | "BATTERY";
    fillColor: string;
    fillAlpha: number;
    trackColor: string;
    trackAlpha: number;
    borderRadius: number;
  };
  glass: GlassEffect;
  scene3d?: Scene3DConfig;
  children?: SceneLayer[];
  fx: Record<string, string>;
}

export interface PrismProject {
  id: string;
  title: string;
  packageType: PackageType;
  dim: number;
  backgroundColor: string;
  widget: {
    width: number;
    height: number;
    posX: number;
    posY: number;
    locked: boolean;
    /** Widget nativo: fica acima das janelas (HWND_TOPMOST); padrão = no desktop, atrás das janelas */
    alwaysOnTop?: boolean;
    /** Cliques atravessam para o desktop (toques voltam pelo motor). Padrão: igual a `locked` */
    clickThrough?: boolean;
  };
  globals: GlobalVar[];
  layers: SceneLayer[];
}

export interface LiveTelemetry {
  cpu_percent: number;
  ram_percent: number;
  ram_used_gb: number;
  ram_total_gb: number;
  hostname: string;
  battery_percent: number;
  battery_charging: boolean;
  media: {
    title: string;
    artist: string;
    album: string;
    playing: boolean;
    cover_path?: string;
    position_secs?: number;
    duration_secs?: number;
    updated_at_ms?: number;
  };
}

export interface MonitorInfoDto {
  id: string;
  device_name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  dpi_x: number;
  dpi_y: number;
  scale_factor: number;
  is_primary: boolean;
}

export interface RuntimeState {
  paused: boolean;
  desktop_visible: boolean;
  active_wallpaper_id: string | null;
  pinned_widget_ids: string[];
  locked_widget_ids: string[];
  monitors?: MonitorInfoDto[];
  /** Migração automática de wallpaper legado (WebView) falhou; precisa reaplicar no motor nativo. */
  legacy_migration_notice?: string | null;
  /** Avisos do último "Aplicar" (arquivo ausente etc.). */
  warnings?: string[];
}

/** Projeto que não pôde ser lido (`list_projects`). `restored`: o `.bak` entrou no lugar. */
export interface BrokenProjectDto {
  file: string;
  error: string;
  restored: boolean;
}

export interface ProjectListDto {
  projects: PrismProject[];
  broken: BrokenProjectDto[];
}

/** `import_prism_archive` sem `replace` quando o id do pacote já existe. */
export interface ImportConflictDto {
  conflict: true;
  existingId: string;
  existingTitle: string;
}

export function isImportConflict(v: unknown): v is ImportConflictDto {
  return typeof v === "object" && v !== null && (v as { conflict?: unknown }).conflict === true;
}

export interface PickedFileDto {
  path: string;
  file_name: string;
  ext: string;
  is_video: boolean;
  is_image: boolean;
  is_svg: boolean;
  is_font: boolean;
  is_model_3d?: boolean;
  text_content: string;
}

export const SPRING_TACTILE = SPRING.tactile;

export const BUILTIN_SVG_ICONS: Record<string, string> = {
  prism: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 22 20 2 20 12 2"/><line x1="12" y1="2" x2="12" y2="20"/></svg>`,
  play: `<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 4 20 12 6 20 6 4"/></svg>`,
  pause: `<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/></svg>`,
  skip_prev: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polygon points="19 20 9 12 19 4 19 20" fill="currentColor"/><line x1="5" y1="19" x2="5" y2="5"/></svg>`,
  skip_next: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 4 15 12 5 20 5 4" fill="currentColor"/><line x1="19" y1="5" x2="19" y2="19"/></svg>`,
  music: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>`,
  sun: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/></svg>`,
  moon: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>`,
  cpu: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="4" width="16" height="16" rx="2"/><rect x="9" y="9" width="6" height="6"/><path d="M9 1v3M15 1v3M9 20v3M15 20v3M20 9h3M20 14h3M1 9h3M1 14h3"/></svg>`,
  cloud: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/></svg>`,
  spark: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l1.9 5.8L20 10.7l-6.1 1.9L12 19l-1.9-6.4L4 10.7l6.1-1.9L12 3z"/></svg>`,
  battery: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="7" width="16" height="10" rx="2"/><line x1="22" y1="11" x2="22" y2="13"/></svg>`,
};

export const DEFAULT_TELEMETRY: LiveTelemetry = {
  cpu_percent: 0,
  ram_percent: 0,
  ram_used_gb: 0,
  ram_total_gb: 0,
  hostname: "Windows",
  battery_percent: 100,
  battery_charging: true,
  media: {
    title: "",
    artist: "",
    album: "",
    playing: false,
    cover_path: "",
    position_secs: 0,
    duration_secs: 0,
    updated_at_ms: 0,
  },
};

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function invokeCmd<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriRuntime()) {
    throw new Error(`Tauri offline (${cmd})`);
  }
  return invoke<T>(cmd, args);
}

export function resolveMediaUrl(src?: string): string {
  if (!src) return "";
  if (src.startsWith("data:") || src.startsWith("blob:") || src.startsWith("http://") || src.startsWith("https://")) {
    return src;
  }
  if (isTauriRuntime()) {
    return convertFileSrc(src);
  }
  return src;
}

export function defaultGlass(enabled = false): GlassEffect {
  return {
    enabled,
    blurPx: 22,
    refractionScale: 16,
    chromaticAberration: 0,
    frostOpacity: 0.08,
    tintColor: "#FFFFFF",
    borderRadius: 24,
    borderOpacity: 0.16,
    shadowBlur: 28,
    saturation: 1.2,
    brightness: 1.06,
    contrast: 1.04,
    noiseOpacity: 0.04,
    bevelWidth: 22,
    specularIntensity: 0.45,
    textMode: "GLYPH",
  };
}

export const GLASS_STYLE_PRESETS: Array<{ label: string; patch: Partial<GlassEffect> }> = [
  {
    label: "Cristal Líquido",
    patch: {
      enabled: true,
      blurPx: 0,
      refractionScale: 36,
      bevelWidth: 28,
      frostOpacity: 0.03,
      noiseOpacity: 0,
      tintColor: "#FFFFFF",
      borderOpacity: 0.26,
      specularIntensity: 0.78,
      brightness: 1.12,
      contrast: 1.08,
      saturation: 1.25,
      chromaticAberration: 0,
    },
  },
  {
    label: "Vidro Jateado",
    patch: {
      enabled: true,
      blurPx: 22,
      refractionScale: 16,
      bevelWidth: 22,
      frostOpacity: 0.12,
      noiseOpacity: 0.09,
      tintColor: "#FFFFFF",
      borderOpacity: 0.18,
      specularIntensity: 0.45,
      brightness: 1.06,
      contrast: 1.04,
      saturation: 1.18,
      chromaticAberration: 0,
    },
  },
  {
    label: "Vidro Fumê",
    patch: {
      enabled: true,
      blurPx: 26,
      refractionScale: 14,
      bevelWidth: 20,
      frostOpacity: 0.36,
      noiseOpacity: 0.06,
      tintColor: "#09090C",
      borderOpacity: 0.14,
      specularIntensity: 0.34,
      brightness: 0.85,
      contrast: 1.06,
      saturation: 1.1,
      chromaticAberration: 0,
    },
  },
  {
    label: "Fosco Suave",
    patch: {
      enabled: true,
      blurPx: 42,
      refractionScale: 0,
      bevelWidth: 16,
      frostOpacity: 0.08,
      noiseOpacity: 0.07,
      tintColor: "#FFFFFF",
      borderOpacity: 0.12,
      specularIntensity: 0.22,
      brightness: 1.02,
      contrast: 1.0,
      saturation: 1.08,
      chromaticAberration: 0,
    },
  },
];
