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
export type Scene3DBackgroundMode = "TRANSPARENT" | "SOLID";
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
export const SCENE3D_SCHEMA_VERSION = 2;
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
}

export interface Scene3DConfig {
  /** Absent on projects saved before schema 2; filled by normalizeScene3DConfig() */
  schemaVersion?: number;
  modelSrc: string;
  showcaseVariant?: Scene3DShowcaseVariant;
  objects?: Scene3DObject[];
  backgroundMode?: Scene3DBackgroundMode;
  backgroundColor?: string;
  envPreset: Scene3DEnvPreset;
  envIntensity: number;
  exposure: number;
  renderScale: number;
  /** FPS ceiling, 5–200 in steps of 5 (see clampScene3DFps). Ignored when fpsMode === "MONITOR". */
  maxFps: number;
  fpsMode?: Scene3DFpsMode;
  autoRotateSpeed: number;
  modelScale: number;
  modelOffsetY: number;
  autoDayNight?: boolean;
  /** Optional 0..24 override for previewing solar cycle in Studio; null/undefined = real system clock */
  dayNightHourOverride?: number | null;
  camera: {
    fov: number;
    distance: number;
    azimuthDeg: number;
    elevationDeg: number;
    targetY: number;
    /** Orbit target captured from the Sala 3D free camera; absent = [0, targetY, 0] */
    target?: [number, number, number];
    orbitParallaxX: number;
    orbitParallaxY: number;
    damping: number;
  };
  playback: {
    clipName: string;
    speed: number;
    loop: boolean;
  };
  bindings?: ReactiveBinding3D[];
  dynamicTextures?: Scene3DDynamicTexture[];
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
export type ClickAction = "NONE" | "MEDIA_PLAY_PAUSE" | "MEDIA_NEXT" | "MEDIA_PREV";
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
  monitor_wallpapers?: Record<string, string>;
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

export const SPRING_TACTILE = {
  type: "spring" as const,
  stiffness: 400,
  damping: 32,
  mass: 0.9,
};

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

export function createBinding3D(
  preset:
    | "LOOK_AT"
    | "HOVER_GLOW"
    | "CLICK_CLIP"
    | "HOVER_PULSE"
    | "AUDIO_BASS"
    | "KEY_PULSE"
    | "AUDIO_SWING"
    | "AUDIO_PISTON",
  info?: Scene3DObjectInspectInfo | Scene3DInspectInfo | null,
  objectId?: string
): ReactiveBinding3D {
  const id = `b3d-${Math.random().toString(36).slice(2, 9)}`;
  const objPatch = objectId ? { objectId } : {};
  if (preset === "LOOK_AT") {
    const headBone =
      info?.boneNames.find((b) => /head|neck|eye|cabeca|pescoco/i.test(b)) ??
      info?.boneNames[0] ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "CURSOR_LOOK",
      sourceNode: "*",
      targetType: "NODE_LOOK_AT",
      targetName: headBone,
      strength: 24,
      smoothing: 0.12,
      invertX: false,
      invertY: false,
    };
  }
  if (preset === "HOVER_GLOW") {
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "HOVER_MESH",
      sourceNode: "*",
      targetType: "EMISSIVE_GLOW",
      targetName: info?.materialNames[0] ?? "*",
      strength: 1.4,
      smoothing: 0.14,
    };
  }
  if (preset === "AUDIO_BASS") {
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType: "NODE_SCALE_PULSE",
      targetName: "__ROOT__",
      strength: 0.18,
      smoothing: 0.22,
    };
  }
  if (preset === "AUDIO_SWING") {
    // Hinge-like parts first (wing, jaw, arm, lid, needle…), whole model as fallback
    const names = [...(info?.boneNames ?? []), ...(info?.meshNames ?? [])];
    const hinge =
      names.find((n) => /wing|asa|jaw|mandib|arm|braco|lid|tampa|needle|agulha|tail|cauda/i.test(n)) ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType: "NODE_ROTATE_SWING",
      targetName: hinge,
      strength: 22,
      smoothing: 0.2,
      axis: "Z",
    };
  }
  if (preset === "AUDIO_PISTON") {
    const names = [...(info?.meshNames ?? []), ...(info?.boneNames ?? [])];
    const cone =
      names.find((n) => /speaker|woofer|cone|caixa|alto.?falante|key|tecla|piston/i.test(n)) ??
      "__ROOT__";
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "AUDIO_BASS",
      sourceNode: "*",
      targetType: "NODE_MOVE_OFFSET",
      targetName: cone,
      strength: 0.12,
      smoothing: 0.26,
      axis: "Z",
    };
  }
  if (preset === "KEY_PULSE") {
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "KEY_PULSE",
      sourceNode: "*",
      targetType: "EMISSIVE_GLOW",
      targetName: info?.materialNames[0] ?? "*",
      strength: 1.6,
      smoothing: 0.22,
    };
  }
  if (preset === "CLICK_CLIP") {
    const hasClips = Boolean(info && info.clipNames.length > 0);
    return {
      id,
      enabled: true,
      ...objPatch,
      source: "CLICK_MESH",
      sourceNode: "*",
      targetType: hasClips ? "PLAY_CLIP" : "NODE_SCALE_PULSE",
      targetName: hasClips ? info!.clipNames[Math.min(1, info!.clipNames.length - 1)] : "__ROOT__",
      strength: hasClips ? 1 : 0.16,
      smoothing: 0.15,
    };
  }
  return {
    id,
    enabled: true,
    ...objPatch,
    source: "HOVER_MESH",
    sourceNode: "*",
    targetType: "NODE_SCALE_PULSE",
    targetName: "__ROOT__",
    strength: 0.1,
    smoothing: 0.14,
  };
}

function defaultShowcaseLabel(variant?: Scene3DShowcaseVariant): string {
  if (variant === "GYROSCOPE") return "Astrolábio Solar";
  if (variant === "KINETIC_SLAB") return "Display Slab 3D";
  return "Obsidian Monolith";
}

function fileBaseName(pathStr: string): string {
  const leaf = pathStr.split(/[\\/]/).pop() || "";
  return leaf.replace(/\.[^.]+$/, "") || "Modelo .glb";
}

export function createScene3DObject(
  kind: Scene3DObjectKind,
  opts?: {
    id?: string;
    name?: string;
    modelSrc?: string;
    showcaseVariant?: Scene3DShowcaseVariant;
    position?: [number, number, number];
    rotation?: [number, number, number];
    scale?: [number, number, number];
    autoRotateSpeed?: number;
    dynamicTextures?: Scene3DDynamicTexture[];
  }
): Scene3DObject {
  const modelSrc = (opts?.modelSrc ?? "").trim();
  const variant = opts?.showcaseVariant ?? "MONOLITH";
  return {
    id: opts?.id ?? `obj3d-${Math.random().toString(36).slice(2, 9)}`,
    name:
      opts?.name ||
      (kind === "GLB" && modelSrc ? fileBaseName(modelSrc) : defaultShowcaseLabel(variant)),
    kind,
    modelSrc,
    showcaseVariant: kind === "SHOWCASE" ? variant : undefined,
    position: opts?.position ?? [0, 0, 0],
    rotation: opts?.rotation ?? [0, 0, 0],
    scale: opts?.scale ?? [1, 1, 1],
    visible: true,
    autoRotateSpeed: opts?.autoRotateSpeed ?? (kind === "GLB" ? 0 : 0.22),
    playback: {
      clipName: "",
      speed: 1,
      loop: true,
    },
    dynamicTextures: opts?.dynamicTextures ?? [],
  };
}

export function normalizeScene3DObjects(cfg: Scene3DConfig): Scene3DObject[] {
  if (Array.isArray(cfg.objects) && cfg.objects.length > 0) {
    return cfg.objects;
  }
  const hasModel = Boolean(cfg.modelSrc?.trim());
  const variant = cfg.showcaseVariant ?? "MONOLITH";
  const sc = Number.isFinite(cfg.modelScale) && cfg.modelScale > 0 ? cfg.modelScale : 1;
  return [
    {
      id: "obj-primary",
      name: hasModel ? fileBaseName(cfg.modelSrc) : defaultShowcaseLabel(variant),
      kind: hasModel ? "GLB" : "SHOWCASE",
      modelSrc: cfg.modelSrc ?? "",
      showcaseVariant: hasModel ? undefined : variant,
      position: [0, Number.isFinite(cfg.modelOffsetY) ? cfg.modelOffsetY : 0, 0],
      rotation: [0, 0, 0],
      scale: [sc, sc, sc],
      visible: true,
      autoRotateSpeed: Number.isFinite(cfg.autoRotateSpeed)
        ? cfg.autoRotateSpeed
        : hasModel
        ? 0
        : 0.22,
      playback: cfg.playback ?? { clipName: "", speed: 1, loop: true },
      dynamicTextures: cfg.dynamicTextures ?? [],
    },
  ];
}

export function defaultScene3D(
  modelSrc = "",
  showcaseVariant: Scene3DShowcaseVariant = "MONOLITH"
): Scene3DConfig {
  const hasExternalModel = Boolean(modelSrc.trim());
  const primary = createScene3DObject(hasExternalModel ? "GLB" : "SHOWCASE", {
    id: "obj-primary",
    modelSrc,
    showcaseVariant,
    autoRotateSpeed: hasExternalModel ? 0 : 0.22,
  });
  return {
    modelSrc,
    showcaseVariant,
    objects: [primary],
    backgroundMode: "TRANSPARENT",
    backgroundColor: "#07080b",
    envPreset: "STUDIO_DARK",
    envIntensity: 1.25,
    exposure: 1.08,
    renderScale: 0.85,
    maxFps: 60,
    autoRotateSpeed: hasExternalModel ? 0 : 0.22,
    modelScale: 1,
    modelOffsetY: 0,
    camera: {
      fov: 42,
      distance: 4.8,
      azimuthDeg: hasExternalModel ? 0 : 22,
      elevationDeg: hasExternalModel ? 8 : 14,
      targetY: 0,
      orbitParallaxX: 12,
      orbitParallaxY: 7,
      damping: 0.08,
    },
    playback: {
      clipName: "",
      speed: 1,
      loop: true,
    },
    bindings: [
      {
        id: "b3d-default-look",
        enabled: true,
        objectId: "obj-primary",
        source: "CURSOR_LOOK",
        sourceNode: "*",
        targetType: "NODE_LOOK_AT",
        targetName: "__ROOT__",
        strength: 18,
        smoothing: 0.1,
      },
      {
        id: "b3d-default-click",
        enabled: true,
        objectId: "obj-primary",
        source: "CLICK_MESH",
        sourceNode: "*",
        targetType: "NODE_SCALE_PULSE",
        targetName: "__ROOT__",
        strength: 0.14,
        smoothing: 0.16,
      },
    ],
    schemaVersion: SCENE3D_SCHEMA_VERSION,
    fpsMode: "CAP",
  };
}

function finiteOr(n: unknown, fallback: number): number {
  return typeof n === "number" && Number.isFinite(n) ? n : fallback;
}

/**
 * Single migration point for Scene3DConfig (call it at the borders: project load,
 * .prism import, scene broadcast to desktop windows). Idempotent and cheap: returns
 * the same object when it is already current, so React memoization keeps working.
 */
export function normalizeScene3DConfig(raw: Scene3DConfig | undefined | null): Scene3DConfig {
  const base = defaultScene3D();
  if (!raw || typeof raw !== "object") return base;
  const fps = clampScene3DFps(raw.maxFps);
  const fpsMode: Scene3DFpsMode = raw.fpsMode === "MONITOR" ? "MONITOR" : "CAP";
  if (
    raw.schemaVersion === SCENE3D_SCHEMA_VERSION &&
    raw.maxFps === fps &&
    raw.fpsMode === fpsMode &&
    Array.isArray(raw.objects) &&
    raw.objects.length > 0 &&
    raw.camera
  ) {
    return raw;
  }
  const cam = { ...base.camera, ...(raw.camera ?? {}) };
  // `objects: raw.objects` keeps legacy single-modelSrc projects from inheriting the default sculpture
  const objects = normalizeScene3DObjects({ ...base, ...raw, objects: raw.objects });
  return {
    ...base,
    ...raw,
    schemaVersion: SCENE3D_SCHEMA_VERSION,
    objects,
    modelSrc: objects[0]?.modelSrc ?? raw.modelSrc ?? "",
    maxFps: fps,
    fpsMode,
    renderScale: Math.max(0.5, Math.min(1, finiteOr(raw.renderScale, base.renderScale))),
    camera: cam,
    bindings: Array.isArray(raw.bindings) ? raw.bindings : base.bindings,
  };
}

function normalizeLayersScene3D(layers: SceneLayer[]): SceneLayer[] {
  let changed = false;
  const next = layers.map((l) => {
    let out = l;
    if (l.kind === "SCENE_3D") {
      const s3d = normalizeScene3DConfig(l.scene3d);
      if (s3d !== l.scene3d) out = { ...out, scene3d: s3d };
    }
    if (l.children?.length) {
      const kids = normalizeLayersScene3D(l.children);
      if (kids !== l.children) out = { ...out, children: kids };
    }
    if (out !== l) changed = true;
    return out;
  });
  return changed ? next : layers;
}

/** Sala 3D gizmo commit: writes one object's transform (used by the Studio editor) */
export function setScene3DObjectTransform(
  cfg: Scene3DConfig,
  objectId: string,
  position: [number, number, number],
  rotation: [number, number, number],
  scale: [number, number, number]
): Scene3DConfig {
  const objs = normalizeScene3DObjects(cfg);
  const nextObjs = objs.map((o) => (o.id === objectId ? { ...o, position, rotation, scale } : o));
  return { ...cfg, objects: nextObjs, modelSrc: nextObjs[0]?.modelSrc ?? cfg.modelSrc };
}

export function duplicateScene3DObject(
  cfg: Scene3DConfig,
  objectId: string
): { config: Scene3DConfig; newId: string | null } {
  const objs = normalizeScene3DObjects(cfg);
  const src = objs.find((o) => o.id === objectId);
  if (!src) return { config: cfg, newId: null };
  const newId = `obj3d-${Math.random().toString(36).slice(2, 9)}`;
  const clone: Scene3DObject = {
    ...(JSON.parse(JSON.stringify(src)) as Scene3DObject),
    id: newId,
    name: `${src.name} (Cópia)`,
    position: [Number(((src.position?.[0] ?? 0) + 0.8).toFixed(2)), src.position?.[1] ?? 0, src.position?.[2] ?? 0],
  };
  const nextObjs = [...objs, clone];
  return { config: { ...cfg, objects: nextObjs, modelSrc: nextObjs[0]?.modelSrc ?? "" }, newId };
}

/** Removes an object (a room always keeps at least one) and its bindings */
export function removeScene3DObject(cfg: Scene3DConfig, objectId: string): Scene3DConfig {
  const objs = normalizeScene3DObjects(cfg);
  if (objs.length <= 1) return cfg;
  const nextObjs = objs.filter((o) => o.id !== objectId);
  if (nextObjs.length === objs.length) return cfg;
  return {
    ...cfg,
    objects: nextObjs,
    modelSrc: nextObjs[0]?.modelSrc ?? "",
    bindings: (cfg.bindings ?? []).filter((b) => b.objectId !== objectId),
  };
}

/** Project-level border migration (keeps the same reference when nothing changed) */
export function normalizeProjectScene3D<T extends PrismProject>(project: T): T {
  if (!project || !Array.isArray(project.layers)) return project;
  const layers = normalizeLayersScene3D(project.layers);
  return layers === project.layers ? project : { ...project, layers };
}

export function createLayer(kind: LayerKind, name?: string): SceneLayer {
  const id = `l-${Math.random().toString(36).slice(2, 9)}`;
  const base: SceneLayer = {
    id,
    name:
      name ??
      (kind === "VIDEO"
        ? "Camada de Vídeo"
        : kind === "SCENE_3D"
        ? "Cena 3D (.glb)"
        : kind === "IMAGE"
        ? "Imagem / Capa"
        : kind === "SVG"
        ? "Ícone / Botão"
        : kind === "TEXT"
        ? "Relógio / Texto"
        : kind === "GLASS"
        ? "Placa de Vidro"
        : kind === "PROGRESS"
        ? "Barra de Progresso"
        : "Grupo / Widget"),
    kind,
    x: 0.5,
    y: 0.5,
    width:
      kind === "VIDEO" || kind === "SCENE_3D"
        ? 1920
        : kind === "GLASS"
        ? 540
        : kind === "IMAGE"
        ? 480
        : kind === "PROGRESS"
        ? 320
        : 160,
    height:
      kind === "VIDEO" || kind === "SCENE_3D"
        ? 1080
        : kind === "GLASS"
        ? 220
        : kind === "IMAGE"
        ? 360
        : kind === "PROGRESS"
        ? 6
        : 160,
    rotation: 0,
    opacity: 1,
    blendMode: "normal",
    hidden: false,
    locked: false,
    parallaxX: 0,
    parallaxY: 0,
    clickAction: "NONE",
    glass: defaultGlass(kind === "GLASS"),
    fx: {},
  };

  if (kind === "VIDEO") {
    base.video = { src: "", speed: 1, muted: true, volume: 0, objectFit: "cover", clipRadius: 0 };
  } else if (kind === "IMAGE") {
    base.image = { src: "", objectFit: "contain", borderRadius: 0, sourceMode: "FILE" };
  } else if (kind === "SVG") {
    base.svg = { iconName: "prism", customSvg: "", color: "#FFFFFF", colorAlpha: 1, strokeWidth: 1.6 };
  } else if (kind === "TEXT") {
    base.text = {
      content: "$df(HH:mm)$",
      fontFamily: "Outfit",
      fontPath: "",
      fontSize: 132,
      fontWeight: 300,
      letterSpacing: -0.03,
      align: "center",
      color: "#F8F8FA",
      colorAlpha: 1,
      stemStretch: 1.0,
      shadowBlur: 12,
    };
  } else if (kind === "PROGRESS") {
    base.progress = {
      source: "MEDIA",
      fillColor: "#FFFFFF",
      fillAlpha: 0.92,
      trackColor: "#FFFFFF",
      trackAlpha: 0.16,
      borderRadius: 999,
    };
  } else if (kind === "SCENE_3D") {
    base.scene3d = defaultScene3D();
  } else if (kind === "GROUP") {
    base.width = 520;
    base.height = 240;
    base.children = [];
  }
  return base;
}

export function createPure3DProject(title: string, modelPath = "", id?: string): PrismProject {
  const stage = createLayer("SCENE_3D", title || "Cena 3D");
  stage.scene3d = defaultScene3D(modelPath);

  const clock = createLayer("TEXT", "Relógio Cristal 3D");
  clock.x = 0.5;
  clock.y = 0.22;
  clock.text = {
    content: "$df(HH:mm)$",
    fontFamily: "Outfit",
    fontPath: "",
    fontSize: 136,
    fontWeight: 600,
    letterSpacing: -0.03,
    align: "center",
    color: "#FFFFFF",
    colorAlpha: 0.14,
    stemStretch: 1.0,
    shadowBlur: 10,
  };
  clock.glass = {
    ...defaultGlass(true),
    blurPx: 8,
    refractionScale: 28,
    bevelWidth: 24,
    frostOpacity: 0.04,
    noiseOpacity: 0,
    borderOpacity: 0.22,
    specularIntensity: 0.72,
    textMode: "GLYPH",
  };

  return {
    id: id ?? `scene3d-${Date.now().toString(36)}`,
    title: title || "Cena 3D",
    packageType: "WALLPAPER",
    dim: 0,
    backgroundColor: "#060608",
    widget: { width: 480, height: 240, posX: 120, posY: 120, locked: false },
    globals: [],
    layers: [stage, clock],
  };
}

export interface Scene3DTemplateEntry {
  project: PrismProject;
  subtitle: string;
  badges: string[];
}

export function createSolarAtelier3DProject(id = "preset-solar-atelier-3d"): PrismProject {
  const stage = createLayer("SCENE_3D", "Astrolábio Solar 3D");
  stage.scene3d = {
    ...defaultScene3D("", "GYROSCOPE"),
    envPreset: "SUNSET",
    envIntensity: 1.35,
    exposure: 1.12,
    autoRotateSpeed: 0.28,
    objects: [
      createScene3DObject("SHOWCASE", {
        id: "obj-primary",
        showcaseVariant: "GYROSCOPE",
        autoRotateSpeed: 0.28,
      }),
    ],
    autoDayNight: true,
    dayNightHourOverride: null,
    camera: {
      fov: 40,
      distance: 4.9,
      azimuthDeg: 18,
      elevationDeg: 18,
      targetY: 0,
      orbitParallaxX: 16,
      orbitParallaxY: 9,
      damping: 0.08,
    },
    bindings: [
      {
        id: "b3d-solar-look",
        enabled: true,
        objectId: "obj-primary",
        source: "CURSOR_LOOK",
        sourceNode: "*",
        targetType: "NODE_LOOK_AT",
        targetName: "AstroCore",
        strength: 24,
        smoothing: 0.1,
      },
      {
        id: "b3d-solar-click",
        enabled: true,
        objectId: "obj-primary",
        source: "CLICK_MESH",
        sourceNode: "*",
        targetType: "NODE_SCALE_PULSE",
        targetName: "__ROOT__",
        strength: 0.12,
        smoothing: 0.15,
      },
    ],
  };

  const dateLine = createLayer("TEXT", "Data Solar");
  dateLine.x = 0.5;
  dateLine.y = 0.13;
  dateLine.text = {
    content: "$tc(up, df(EEE · dd MMM))$",
    fontFamily: "Inter",
    fontPath: "",
    fontSize: 15,
    fontWeight: 500,
    letterSpacing: 0.28,
    align: "center",
    color: "#E4E4E7",
    colorAlpha: 0.72,
    stemStretch: 1.0,
    shadowBlur: 0,
  };

  const clock = createLayer("TEXT", "Relógio Solar");
  clock.x = 0.5;
  clock.y = 0.24;
  clock.text = {
    content: "$df(HH:mm)$",
    fontFamily: "Outfit",
    fontPath: "",
    fontSize: 126,
    fontWeight: 300,
    letterSpacing: -0.04,
    align: "center",
    color: "#FFFFFF",
    colorAlpha: 0.92,
    stemStretch: 1.0,
    shadowBlur: 12,
  };

  return {
    id,
    title: "Solar Atelier 3D",
    packageType: "WALLPAPER",
    dim: 0,
    backgroundColor: "#06070B",
    widget: { width: 480, height: 240, posX: 120, posY: 120, locked: false },
    globals: [],
    layers: [stage, dateLine, clock],
  };
}

export function createPulseReactive3DProject(id = "preset-pulse-reactive-3d"): PrismProject {
  const stage = createLayer("SCENE_3D", "Palco Reativo 3D");
  const coverTex: Scene3DDynamicTexture = {
    id: "dtex-pulse-cover",
    materialName: "DisplayScreen",
    source: "MEDIA_COVER",
    emissiveMap: true,
  };
  stage.scene3d = {
    ...defaultScene3D("", "KINETIC_SLAB"),
    envPreset: "CITY_NIGHT",
    envIntensity: 1.3,
    exposure: 1.1,
    autoRotateSpeed: 0.12,
    objects: [
      createScene3DObject("SHOWCASE", {
        id: "obj-primary",
        showcaseVariant: "KINETIC_SLAB",
        autoRotateSpeed: 0.12,
        dynamicTextures: [coverTex],
      }),
    ],
    camera: {
      fov: 40,
      distance: 4.5,
      azimuthDeg: 14,
      elevationDeg: 10,
      targetY: 0,
      orbitParallaxX: 15,
      orbitParallaxY: 8,
      damping: 0.08,
    },
    bindings: [
      {
        id: "b3d-pulse-look",
        enabled: true,
        objectId: "obj-primary",
        source: "CURSOR_LOOK",
        sourceNode: "*",
        targetType: "NODE_LOOK_AT",
        targetName: "KineticFrame",
        strength: 18,
        smoothing: 0.1,
      },
      {
        id: "b3d-pulse-bass-scale",
        enabled: true,
        objectId: "obj-primary",
        source: "AUDIO_BASS",
        sourceNode: "*",
        targetType: "NODE_SCALE_PULSE",
        targetName: "KineticFrame",
        strength: 0.16,
        smoothing: 0.24,
      },
      {
        id: "b3d-pulse-bass-glow",
        enabled: true,
        objectId: "obj-primary",
        source: "AUDIO_BASS",
        sourceNode: "*",
        targetType: "EMISSIVE_GLOW",
        targetName: "KineticAccent",
        strength: 2.2,
        smoothing: 0.24,
      },
      {
        id: "b3d-pulse-keys",
        enabled: true,
        objectId: "obj-primary",
        source: "KEY_PULSE",
        sourceNode: "*",
        targetType: "NODE_ROTATE_SPIN",
        targetName: "PulseHalo",
        strength: 1.8,
        smoothing: 0.2,
      },
    ],
    dynamicTextures: [coverTex],
  };

  const trackTitle = createLayer("TEXT", "Faixa & Hora");
  trackTitle.x = 0.5;
  trackTitle.y = 0.87;
  trackTitle.text = {
    content: "$df(HH:mm)$  ·  $mi(title)$",
    fontFamily: "JetBrains Mono",
    fontPath: "",
    fontSize: 15,
    fontWeight: 500,
    letterSpacing: 0.1,
    align: "center",
    color: "#E4E4E7",
    colorAlpha: 0.82,
    stemStretch: 1.0,
    shadowBlur: 6,
  };

  return {
    id,
    title: "Pulse Reactive 3D",
    packageType: "WALLPAPER",
    dim: 0,
    backgroundColor: "#05060A",
    widget: { width: 480, height: 240, posX: 120, posY: 120, locked: false },
    globals: [],
    layers: [stage, trackTitle],
  };
}

export function create3DTemplateProjects(): Scene3DTemplateEntry[] {
  return [
    {
      project: createPure3DProject("Obsidian 3D Studio", "", "preset-obsidian-3d-studio"),
      subtitle: "Monólito arquitetônico com relógio em Cristal Líquido refratando a cena 3D ao vivo.",
      badges: ["Mouse Look-At", "Cristal 3D", "Clique Pulso"],
    },
    {
      project: createSolarAtelier3DProject("preset-solar-atelier-3d"),
      subtitle: "Astrolábio cinético de titânio sincronizado ao ciclo solar real de 24h do Windows.",
      badges: ["Ciclo Dia/Noite", "Órbita Mouse", "IBL PBR"],
    },
    {
      project: createPulseReactive3DProject("preset-pulse-reactive-3d"),
      subtitle: "Quadro flutuante 3D que projeta a capa da música atual e pulsa com graves e teclado.",
      badges: ["Áudio WASAPI", "Capa 3D ao Vivo", "Pulso Teclado"],
    },
  ];
}

export function createPureVideoProject(title: string, videoPath: string, id?: string): PrismProject {
  const videoLayer = createLayer("VIDEO", title);
  videoLayer.video = {
    src: videoPath,
    speed: 1,
    muted: true,
    volume: 0,
    objectFit: "cover",
    clipRadius: 0,
  };
  videoLayer.parallaxX = 0;
  videoLayer.parallaxY = 0;

  return {
    id: id ?? `video-${Date.now().toString(36)}`,
    title,
    packageType: "WALLPAPER",
    dim: 0,
    backgroundColor: "#000000",
    widget: { width: 480, height: 240, posX: 120, posY: 120, locked: false },
    globals: [],
    layers: [videoLayer],
  };
}

export function createDefaultProjects(): PrismProject[] {
  // 1. Wallpaper: Obsidian Editorial (Ultra-clean architectural typography, zero RGB clutter)
  const obsidianDate = createLayer("TEXT", "Data Editorial");
  obsidianDate.x = 0.5;
  obsidianDate.y = 0.33;
  obsidianDate.text = {
    content: "$tc(up, df(EEE · dd MMM))$",
    fontFamily: "Inter",
    fontPath: "",
    fontSize: 18,
    fontWeight: 500,
    letterSpacing: 0.28,
    align: "center",
    color: "#A1A1AA",
    stemStretch: 1.0,
    shadowBlur: 0,
  };

  const obsidianClock = createLayer("TEXT", "Relógio Principal");
  obsidianClock.x = 0.5;
  obsidianClock.y = 0.47;
  obsidianClock.parallaxX = 0.12;
  obsidianClock.parallaxY = 0.1;
  obsidianClock.text = {
    content: "$df(HH:mm)$",
    fontFamily: "Outfit",
    fontPath: "",
    fontSize: 188,
    fontWeight: 300,
    letterSpacing: -0.04,
    align: "center",
    color: "#FAFAFA",
    stemStretch: 1.0,
    shadowBlur: 16,
  };

  const obsidianStats = createLayer("TEXT", "Telemetria Discreta");
  obsidianStats.x = 0.5;
  obsidianStats.y = 0.63;
  obsidianStats.fx = { visible: "$gv(show_stats)$" };
  obsidianStats.text = {
    content: "CPU $sys(cpu)$%  ·  RAM $sys(ram)$%  ·  $mi(title)$",
    fontFamily: "JetBrains Mono",
    fontPath: "",
    fontSize: 14,
    fontWeight: 400,
    letterSpacing: 0.08,
    align: "center",
    color: "#71717A",
    stemStretch: 1.0,
    shadowBlur: 0,
  };

  // 2. Wallpaper: Minimal Frost (Neutral frosted glass card on subtle graphite canvas)
  const frostPlate = createLayer("GLASS", "Superfície Fosca");
  frostPlate.x = 0.5;
  frostPlate.y = 0.5;
  frostPlate.width = 580;
  frostPlate.height = 250;
  frostPlate.parallaxX = 0.1;
  frostPlate.parallaxY = 0.08;
  frostPlate.glass = {
    ...defaultGlass(true),
    blurPx: 28,
    frostOpacity: 0.06,
    borderOpacity: 0.14,
    borderRadius: 28,
    shadowBlur: 36,
    refractionScale: 4,
    chromaticAberration: 0,
  };
  frostPlate.fx = { opacity: "$gv(card_opacity)$" };

  const frostClock = createLayer("TEXT", "Hora");
  frostClock.x = 0.5;
  frostClock.y = 0.46;
  frostClock.parallaxX = 0.15;
  frostClock.parallaxY = 0.12;
  frostClock.text = {
    content: "$df(HH:mm)$",
    fontFamily: "Outfit",
    fontPath: "",
    fontSize: 124,
    fontWeight: 500,
    letterSpacing: -0.03,
    align: "center",
    color: "#FFFFFF",
    stemStretch: 1.0,
    shadowBlur: 8,
  };

  const frostSub = createLayer("TEXT", "Data & Mídia");
  frostSub.x = 0.5;
  frostSub.y = 0.57;
  frostSub.parallaxX = 0.15;
  frostSub.parallaxY = 0.12;
  frostSub.text = {
    content: "$tc(up, df(EEE dd MMM))$  ·  $mi(title)$",
    fontFamily: "Inter",
    fontPath: "",
    fontSize: 15,
    fontWeight: 500,
    letterSpacing: 0.12,
    align: "center",
    color: "#A1A1AA",
    stemStretch: 1.0,
    shadowBlur: 0,
  };

  // 3. Widget: Editorial Clock 4×2
  const wClockPlate = createLayer("GLASS", "Base Fosca");
  wClockPlate.x = 0.5;
  wClockPlate.y = 0.5;
  wClockPlate.width = 456;
  wClockPlate.height = 204;
  wClockPlate.glass = {
    ...defaultGlass(true),
    blurPx: 24,
    frostOpacity: 0.08,
    borderOpacity: 0.14,
    borderRadius: 24,
    shadowBlur: 24,
    chromaticAberration: 0,
  };

  const wClockTime = createLayer("TEXT", "Hora");
  wClockTime.x = 0.5;
  wClockTime.y = 0.43;
  wClockTime.text = {
    content: "$df(HH:mm)$",
    fontFamily: "Outfit",
    fontPath: "",
    fontSize: 84,
    fontWeight: 300,
    letterSpacing: -0.03,
    align: "center",
    color: "#FAFAFA",
    stemStretch: 1.0,
    shadowBlur: 4,
  };

  const wClockMeta = createLayer("TEXT", "Data & Sistema");
  wClockMeta.x = 0.5;
  wClockMeta.y = 0.77;
  wClockMeta.text = {
    content: "$tc(up, df(EEE dd MMM))$  ·  CPU $sys(cpu)$%  ·  RAM $sys(ram)$%",
    fontFamily: "JetBrains Mono",
    fontPath: "",
    fontSize: 12,
    fontWeight: 400,
    letterSpacing: 0.06,
    align: "center",
    color: "#A1A1AA",
    stemStretch: 1.0,
    shadowBlur: 0,
  };

  // 4. Widget: Interactive Minimal Media Player 4×2 (Live Cover + Progress + Play/Pause/Prev/Next)
  const wMediaPlate = createLayer("GLASS", "Base do Player");
  wMediaPlate.x = 0.5;
  wMediaPlate.y = 0.5;
  wMediaPlate.width = 456;
  wMediaPlate.height = 152;
  wMediaPlate.glass = {
    ...defaultGlass(true),
    blurPx: 26,
    frostOpacity: 0.09,
    borderOpacity: 0.14,
    borderRadius: 22,
    shadowBlur: 26,
    chromaticAberration: 0,
  };

  const wMediaCover = createLayer("IMAGE", "Capa do Álbum");
  wMediaCover.x = 0.16;
  wMediaCover.y = 0.5;
  wMediaCover.width = 104;
  wMediaCover.height = 104;
  wMediaCover.image = {
    src: "",
    objectFit: "cover",
    borderRadius: 14,
    sourceMode: "MEDIA_COVER",
  };

  const wMediaTitle = createLayer("TEXT", "Faixa Atual");
  wMediaTitle.x = 0.62;
  wMediaTitle.y = 0.28;
  wMediaTitle.text = {
    content: "$mi(title)$",
    fontFamily: "Inter",
    fontPath: "",
    fontSize: 17,
    fontWeight: 600,
    letterSpacing: -0.01,
    align: "center",
    color: "#FFFFFF",
    colorAlpha: 0.96,
    stemStretch: 1.0,
    shadowBlur: 0,
  };

  const wMediaArtist = createLayer("TEXT", "Artista");
  wMediaArtist.x = 0.62;
  wMediaArtist.y = 0.44;
  wMediaArtist.text = {
    content: "$mi(artist)$",
    fontFamily: "Inter",
    fontPath: "",
    fontSize: 12,
    fontWeight: 400,
    letterSpacing: 0.01,
    align: "center",
    color: "#FFFFFF",
    colorAlpha: 0.6,
    stemStretch: 1.0,
    shadowBlur: 0,
  };

  const wMediaBar = createLayer("PROGRESS", "Progresso da Faixa");
  wMediaBar.x = 0.62;
  wMediaBar.y = 0.61;
  wMediaBar.width = 266;
  wMediaBar.height = 4;
  wMediaBar.progress = {
    source: "MEDIA",
    fillColor: "#FFFFFF",
    fillAlpha: 0.9,
    trackColor: "#FFFFFF",
    trackAlpha: 0.16,
    borderRadius: 999,
  };

  const wMediaPrev = createLayer("SVG", "Anterior");
  wMediaPrev.x = 0.48;
  wMediaPrev.y = 0.78;
  wMediaPrev.width = 20;
  wMediaPrev.height = 20;
  wMediaPrev.clickAction = "MEDIA_PREV";
  wMediaPrev.svg = {
    iconName: "skip_prev",
    customSvg: "",
    color: "#FFFFFF",
    colorAlpha: 0.8,
    strokeWidth: 1.6,
  };

  const wMediaPlay = createLayer("SVG", "Play / Pause");
  wMediaPlay.x = 0.62;
  wMediaPlay.y = 0.78;
  wMediaPlay.width = 24;
  wMediaPlay.height = 24;
  wMediaPlay.clickAction = "MEDIA_PLAY_PAUSE";
  wMediaPlay.svg = {
    iconName: "play",
    customSvg: "",
    color: "#FFFFFF",
    colorAlpha: 1,
    strokeWidth: 1.6,
  };

  const wMediaNext = createLayer("SVG", "Próxima");
  wMediaNext.x = 0.76;
  wMediaNext.y = 0.78;
  wMediaNext.width = 20;
  wMediaNext.height = 20;
  wMediaNext.clickAction = "MEDIA_NEXT";
  wMediaNext.svg = {
    iconName: "skip_next",
    customSvg: "",
    color: "#FFFFFF",
    colorAlpha: 0.8,
    strokeWidth: 1.6,
  };

  return [
    {
      id: "preset-obsidian-editorial",
      title: "Obsidian Editorial",
      packageType: "WALLPAPER",
      dim: 0,
      backgroundColor: "radial-gradient(circle at 50% 18%, #1A1B21 0%, #09090C 62%, #050507 100%)",
      widget: { width: 480, height: 220, posX: 120, posY: 120, locked: false },
      globals: [
        { name: "show_stats", kind: "SWITCH", value: "1", label: "Mostrar Telemetria", exposed: true },
      ],
      layers: [obsidianDate, obsidianClock, obsidianStats],
    },
    {
      id: "preset-minimal-frost",
      title: "Minimal Frost",
      packageType: "WALLPAPER",
      dim: 0,
      backgroundColor: "linear-gradient(145deg, #16171C 0%, #0B0C10 55%, #060608 100%)",
      widget: { width: 480, height: 220, posX: 120, posY: 120, locked: false },
      globals: [
        { name: "card_opacity", kind: "NUMBER", value: "1", label: "Opacidade do Vidro", exposed: true },
      ],
      layers: [frostPlate, frostClock, frostSub],
    },
    createPure3DProject("Obsidian 3D Studio", "", "preset-obsidian-3d-studio"),
    createSolarAtelier3DProject("preset-solar-atelier-3d"),
    createPulseReactive3DProject("preset-pulse-reactive-3d"),
    {
      id: "preset-widget-editorial-clock",
      title: "Editorial Clock 4×2",
      packageType: "WIDGET",
      dim: 0,
      backgroundColor: "transparent",
      widget: { width: 480, height: 220, posX: 96, posY: 96, locked: false },
      globals: [],
      layers: [wClockPlate, wClockTime, wClockMeta],
    },
    {
      id: "preset-widget-minimal-player",
      title: "Minimal Media Player",
      packageType: "WIDGET",
      dim: 0,
      backgroundColor: "transparent",
      widget: { width: 480, height: 170, posX: 96, posY: 340, locked: false },
      globals: [],
      layers: [
        wMediaPlate,
        wMediaCover,
        wMediaTitle,
        wMediaArtist,
        wMediaBar,
        wMediaPrev,
        wMediaPlay,
        wMediaNext,
      ],
    },
  ];
}

export function computeLiveMediaProgress(
  media: LiveTelemetry["media"],
  nowMs: number
): { positionSecs: number; durationSecs: number; ratio: number } {
  const dur = Math.max(0, media.duration_secs ?? 0);
  const basePos = Math.max(0, media.position_secs ?? 0);
  const updated = media.updated_at_ms ?? 0;
  const elapsed = media.playing && updated > 0 ? Math.max(0, (nowMs - updated) / 1000) : 0;
  const pos = dur > 0 ? Math.min(dur, basePos + elapsed) : basePos + elapsed;
  const ratio = dur > 0.5 ? Math.max(0, Math.min(1, pos / dur)) : 0;
  return { positionSecs: pos, durationSecs: dur, ratio };
}

function formatDurationMMSS(secs: number): string {
  const total = Math.max(0, Math.floor(secs));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

export function evaluateFormula(
  template: string,
  telemetry: LiveTelemetry,
  globals: GlobalVar[],
  now: Date
): string {
  if (!template.includes("$")) return template;
  const gMap: Record<string, string> = {};
  for (const g of globals) gMap[g.name.toLowerCase()] = g.value;

  const h24 = now.getHours();
  const h12 = h24 % 12 === 0 ? 12 : h24 % 12;
  const HH = String(h24).padStart(2, "0");
  const hh = String(h12).padStart(2, "0");
  const mm = String(now.getMinutes()).padStart(2, "0");
  const ss = String(now.getSeconds()).padStart(2, "0");
  const days = ["DOM", "SEG", "TER", "QUA", "QUI", "SEX", "SÁB"];
  const months = ["JAN", "FEV", "MAR", "ABR", "MAI", "JUN", "JUL", "AGO", "SET", "OUT", "NOV", "DEZ"];

  const evalInner = (raw: string): string => {
    const s = raw.trim();
    const dfM = s.match(/^df\(([^)]+)\)$/i);
    if (dfM) {
      return dfM[1]
        .replace(/HH/g, HH)
        .replace(/hh/g, hh)
        .replace(/mm/g, mm)
        .replace(/ss/g, ss)
        .replace(/EEE/g, days[now.getDay()])
        .replace(/dd/g, String(now.getDate()).padStart(2, "0"))
        .replace(/MMM/g, months[now.getMonth()]);
    }
    const gvM = s.match(/^gv\(([^)]+)\)$/i);
    if (gvM) {
      const k = gvM[1].trim().replace(/^["']|["']$/g, "").toLowerCase();
      return gMap[k] ?? "";
    }
    const sysM = s.match(/^(?:sys|bi|si)\(([^)]+)\)$/i);
    if (sysM) {
      const k = sysM[1].trim().toLowerCase();
      if (k === "cpu") return String(Math.round(telemetry.cpu_percent));
      if (k === "ram" || k === "mem") return String(Math.round(telemetry.ram_percent));
      if (k === "ram_gb") return telemetry.ram_used_gb.toFixed(1);
      if (k === "level" || k === "bat") return String(telemetry.battery_percent);
      if (k === "host") return telemetry.hostname;
      return "0";
    }
    const miM = s.match(/^mi\(([^)]+)\)$/i);
    if (miM) {
      const k = miM[1].trim().toLowerCase();
      if (k === "title") return telemetry.media.title || "Sem mídia";
      if (k === "artist") return telemetry.media.artist || "";
      if (k === "album") return telemetry.media.album || "";
      if (k === "state") return telemetry.media.playing ? "PLAYING" : "PAUSED";
      if (k === "cover") return telemetry.media.cover_path || "";
      if (k === "pos" || k === "dur" || k === "pct") {
        const prog = computeLiveMediaProgress(telemetry.media, now.getTime());
        if (k === "pos") return formatDurationMMSS(prog.positionSecs);
        if (k === "dur") return formatDurationMMSS(prog.durationSecs);
        return String(Math.round(prog.ratio * 100));
      }
      return "";
    }
    const tcM = s.match(/^tc\(\s*(up|low)\s*,\s*(.+)\)$/i);
    if (tcM) {
      const v = evalInner(tcM[2]);
      return tcM[1].toLowerCase() === "up" ? v.toUpperCase() : v.toLowerCase();
    }
    if (/^if\(/i.test(s) && s.endsWith(")")) {
      const innerBody = s.slice(3, -1);
      // Split top-level arguments respecting nested parentheses and quoted strings
      const args: string[] = [];
      let depth = 0;
      let quote: string | null = null;
      let cur = "";
      for (let i = 0; i < innerBody.length; i++) {
        const ch = innerBody[i];
        if (quote) {
          cur += ch;
          if (ch === quote) quote = null;
          continue;
        }
        if (ch === "'" || ch === '"') {
          quote = ch;
          cur += ch;
        } else if (ch === "(") {
          depth++;
          cur += ch;
        } else if (ch === ")") {
          depth = Math.max(0, depth - 1);
          cur += ch;
        } else if (ch === "," && depth === 0 && args.length < 2) {
          args.push(cur.trim());
          cur = "";
        } else {
          cur += ch;
        }
      }
      args.push(cur.trim());

      if (args.length === 3) {
        const stripQuotes = (v: string) => v.trim().replace(/^["']|["']$/g, "");
        const cond = args[0];
        const trueBranch = () => stripQuotes(evalInner(args[1]));
        const falseBranch = () => stripQuotes(evalInner(args[2]));
        const opMatch = cond.match(/^(.+?)\s*(!=|>=|<=|=|>|<)\s*(.+)$/);
        if (opMatch) {
          const lhsRaw = stripQuotes(evalInner(opMatch[1]));
          const op = opMatch[2];
          const rhsRaw = stripQuotes(evalInner(opMatch[3]));
          const lhsNum = Number(lhsRaw);
          const rhsNum = Number(rhsRaw);
          const bothNum =
            lhsRaw !== "" && rhsRaw !== "" && Number.isFinite(lhsNum) && Number.isFinite(rhsNum);
          let ok = false;
          if (op === "=") {
            ok = bothNum ? lhsNum === rhsNum : lhsRaw.toLowerCase() === rhsRaw.toLowerCase();
          } else if (op === "!=") {
            ok = bothNum ? lhsNum !== rhsNum : lhsRaw.toLowerCase() !== rhsRaw.toLowerCase();
          } else if (op === ">") {
            ok = bothNum ? lhsNum > rhsNum : lhsRaw > rhsRaw;
          } else if (op === "<") {
            ok = bothNum ? lhsNum < rhsNum : lhsRaw < rhsRaw;
          } else if (op === ">=") {
            ok = bothNum ? lhsNum >= rhsNum : lhsRaw >= rhsRaw;
          } else if (op === "<=") {
            ok = bothNum ? lhsNum <= rhsNum : lhsRaw <= rhsRaw;
          }
          return ok ? trueBranch() : falseBranch();
        }
        const truthy = stripQuotes(evalInner(cond)).toLowerCase();
        return truthy && truthy !== "0" && truthy !== "false" ? trueBranch() : falseBranch();
      }
    }
    return s;
  };

  return template.replace(/\$([^$]+)\$/g, (_, inner: string) => evalInner(inner));
}
