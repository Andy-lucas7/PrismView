// Fábricas de camadas, assets nativos (SVG embutido) e avaliação de fórmulas.
import { evaluateFxTemplate } from "../engine/fx/fxEngine";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { BUILTIN_SVG_ICONS, GlobalVar, LayerKind, LiveTelemetry, PrismProject, SceneLayer, defaultGlass } from "./prismBase";
import { defaultScene3D } from "./prismScene3D";

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

/** Markup do ícone embutido com a espessura de traço da camada (mesma regra do compositor). */
export function builtinSvgMarkup(iconName: string, strokeWidth: number): string {
  const sw = Math.max(0.6, Math.min(4.5, strokeWidth || 1.6));
  return (BUILTIN_SVG_ICONS[iconName] || BUILTIN_SVG_ICONS.prism).replace(
    /stroke-width="[^"]*"/g,
    `stroke-width="${sw}"`
  );
}

/**
 * Cópia do projeto para o motor nativo: ícones embutidos (só existem no TS) viram markup em
 * `svg.resolvedSvg` (+ `resolvedSvgAlt` = "pause" no botão Play/Pause, que alterna pelo estado
 * da mídia). O conversor Rust (`native_scene.rs`) rasteriza esse markup no motor.
 */
export function withNativeAssets(project: PrismProject): PrismProject {
  const mapLayer = (layer: SceneLayer): SceneLayer => {
    let next = layer;
    if (layer.kind === "SVG" && layer.svg && !layer.svg.customSvg.trim()) {
      const isToggle =
        layer.clickAction === "MEDIA_PLAY_PAUSE" &&
        (layer.svg.iconName === "play" || layer.svg.iconName === "pause");
      next = {
        ...layer,
        svg: {
          ...layer.svg,
          resolvedSvg: builtinSvgMarkup(isToggle ? "play" : layer.svg.iconName, layer.svg.strokeWidth),
          resolvedSvgAlt: isToggle ? builtinSvgMarkup("pause", layer.svg.strokeWidth) : undefined,
        },
      };
    }
    if (layer.children?.length) {
      next = { ...next, children: layer.children.map(mapLayer) };
    }
    return next;
  };
  return { ...project, layers: project.layers.map(mapLayer) };
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

/**
 * Avalia `$...$` via parser de passada única (`engine/fx/fxEngine.ts`, espelho de
 * `crates/prism-core/src/fx.rs`).
 */
export function evaluateFormula(
  template: string,
  telemetry: LiveTelemetry,
  globals: GlobalVar[],
  now: Date
): string {
  if (!template.includes("$")) return template;
  const gMap: Record<string, string> = {};
  for (const g of globals) gMap[g.name.toLowerCase()] = g.value;
  return evaluateFxTemplate(template, { now, telemetry, globals: gMap });
}
