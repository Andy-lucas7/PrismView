// Projetos prontos (templates 2D/3D/vídeo) do Hub.
import { evaluateFxTemplate } from "../engine/fx/fxEngine";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { PrismProject, Scene3DDynamicTexture, defaultGlass } from "./prismBase";
import { createScene3DObject, defaultScene3D } from "./prismScene3D";
import { createLayer } from "./prismFactory";

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
      target: [0, 0, 0],
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
      target: [0, 0, 0],
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
