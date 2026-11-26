// Multiseleção pura (Etapa 5 + colar estilo da Etapa 8): zero dependência de React/DOM.
import type {
  BlendModeCss,
  ClickAction,
  GlassEffect,
  LayerKind,
  SceneLayer,
} from "../types/prismBase";

export interface MergedField<T> {
  value: T;
  mixed: boolean;
}

export const LAYER_KIND_LABELS: Readonly<Record<LayerKind, string>> = {
  VIDEO: "Vídeo",
  IMAGE: "Imagem",
  SVG: "SVG / Ícone",
  TEXT: "Texto",
  GLASS: "Vidro",
  GROUP: "Grupo",
  PROGRESS: "Barra",
  SCENE_3D: "Modelo 3D",
  SHADER: "Efeito Espectro",
};

export function mergeField<T>(
  layers: readonly SceneLayer[],
  getter: (layer: SceneLayer) => T,
  fallback: T,
  equals: (a: T, b: T) => boolean = Object.is
): MergedField<T> {
  if (layers.length === 0) return { value: fallback, mixed: false };
  const first = getter(layers[0]);
  for (let i = 1; i < layers.length; i++) {
    if (!equals(first, getter(layers[i]))) {
      return { value: first, mixed: true };
    }
  }
  return { value: first, mixed: false };
}

const eqNum = (tol = 1e-4) => (a: number, b: number) => Math.abs(a - b) <= tol;
const eqColor = (a: string, b: string) => (a || "").trim().toLowerCase() === (b || "").trim().toLowerCase();

export interface MergedGlassProps {
  enabled: MergedField<boolean>;
  blurPx: MergedField<number>;
  refractionScale: MergedField<number>;
  chromaticAberration: MergedField<number>;
  frostOpacity: MergedField<number>;
  tintColor: MergedField<string>;
  borderRadius: MergedField<number>;
  borderOpacity: MergedField<number>;
  shadowBlur: MergedField<number>;
  saturation: MergedField<number>;
  brightness: MergedField<number>;
  contrast: MergedField<number>;
  noiseOpacity: MergedField<number>;
  bevelWidth: MergedField<number>;
  specularIntensity: MergedField<number>;
  textMode: MergedField<"GLYPH" | "PILL">;
}

export interface MergedCommonProps {
  name: MergedField<string>;
  x: MergedField<number>;
  y: MergedField<number>;
  width: MergedField<number>;
  height: MergedField<number>;
  rotation: MergedField<number>;
  flipX: MergedField<boolean>;
  flipY: MergedField<boolean>;
  opacity: MergedField<number>;
  blendMode: MergedField<BlendModeCss>;
  hidden: MergedField<boolean>;
  locked: MergedField<boolean>;
  parallaxX: MergedField<number>;
  parallaxY: MergedField<number>;
  clickAction: MergedField<ClickAction>;
  glass: MergedGlassProps;
}

export interface MergedTextProps {
  content: MergedField<string>;
  fontFamily: MergedField<string>;
  fontPath: MergedField<string>;
  fontSize: MergedField<number>;
  fontWeight: MergedField<number>;
  letterSpacing: MergedField<number>;
  align: MergedField<"left" | "center" | "right">;
  color: MergedField<string>;
  colorAlpha: MergedField<number>;
  stemStretch: MergedField<number>;
  shadowBlur: MergedField<number>;
  maxWidth: MergedField<number>;
}

export interface MergedVideoProps {
  objectFit: MergedField<"cover" | "contain">;
  speed: MergedField<number>;
  clipRadius: MergedField<number>;
  muted: MergedField<boolean>;
  volume: MergedField<number>;
}

export interface MergedImageProps {
  sourceMode: MergedField<"FILE" | "MEDIA_COVER">;
  objectFit: MergedField<"cover" | "contain">;
  borderRadius: MergedField<number>;
}

export interface MergedSvgProps {
  iconName: MergedField<string>;
  strokeWidth: MergedField<number>;
  color: MergedField<string>;
  colorAlpha: MergedField<number>;
}

export interface MergedProgressProps {
  source: MergedField<"MEDIA" | "CPU" | "RAM" | "BATTERY">;
  fillColor: MergedField<string>;
  fillAlpha: MergedField<number>;
  trackColor: MergedField<string>;
  trackAlpha: MergedField<number>;
  borderRadius: MergedField<number>;
}

export interface MergedSelectionProps {
  count: number;
  /** Preenchido quando todas as camadas selecionadas possuem o mesmo `kind`; `null` se mistas ou vazio. */
  sameKind: LayerKind | null;
  /** Lista de tipos únicos presentes na seleção, na ordem de aparição. */
  kinds: LayerKind[];
  /** Todos os itens selecionados suportam placa/glifo de vidro completo (`GLASS`, `TEXT`, `GROUP`, `IMAGE`). */
  allSupportFullGlass: boolean;
  common: MergedCommonProps;
  text?: MergedTextProps;
  video?: MergedVideoProps;
  image?: MergedImageProps;
  svg?: MergedSvgProps;
  progress?: MergedProgressProps;
}

const FULL_GLASS_KINDS = new Set<LayerKind>(["GLASS", "TEXT", "GROUP", "IMAGE"]);

/**
 * Combina as propriedades de N camadas selecionadas, marcando `mixed: true` em cada campo cujo
 * valor diverge entre os itens. Quando todos compartilham o mesmo `kind`, inclui também o bloco
 * específico daquele tipo (`text`, `video`, `image`, `svg`, `progress`).
 */
export function mergeProps(layers: readonly SceneLayer[]): MergedSelectionProps {
  const kinds: LayerKind[] = [];
  for (const l of layers) {
    if (!kinds.includes(l.kind)) kinds.push(l.kind);
  }
  const sameKind = layers.length > 0 && kinds.length === 1 ? kinds[0] : null;
  const allSupportFullGlass =
    layers.length > 0 && layers.every((l) => FULL_GLASS_KINDS.has(l.kind));

  const common: MergedCommonProps = {
    name: mergeField(layers, (l) => l.name, ""),
    x: mergeField(layers, (l) => l.x, 0.5, eqNum(1e-4)),
    y: mergeField(layers, (l) => l.y, 0.5, eqNum(1e-4)),
    width: mergeField(layers, (l) => l.width, 320, eqNum(0.5)),
    height: mergeField(layers, (l) => l.height, 140, eqNum(0.5)),
    rotation: mergeField(layers, (l) => l.rotation, 0, eqNum(0.05)),
    flipX: mergeField(layers, (l) => Boolean(l.flipX), false),
    flipY: mergeField(layers, (l) => Boolean(l.flipY), false),
    opacity: mergeField(layers, (l) => l.opacity, 1, eqNum(0.005)),
    blendMode: mergeField(layers, (l) => l.blendMode, "normal"),
    hidden: mergeField(layers, (l) => l.hidden, false),
    locked: mergeField(layers, (l) => l.locked, false),
    parallaxX: mergeField(layers, (l) => l.parallaxX, 0, eqNum(0.005)),
    parallaxY: mergeField(layers, (l) => l.parallaxY, 0, eqNum(0.005)),
    clickAction: mergeField(layers, (l) => l.clickAction ?? "NONE", "NONE"),
    glass: {
      enabled: mergeField(layers, (l) => Boolean(l.glass?.enabled), false),
      blurPx: mergeField(layers, (l) => l.glass?.blurPx ?? 22, 22, eqNum(0.5)),
      refractionScale: mergeField(layers, (l) => l.glass?.refractionScale ?? 16, 16, eqNum(0.5)),
      chromaticAberration: mergeField(layers, (l) => l.glass?.chromaticAberration ?? 0, 0, eqNum(0.05)),
      frostOpacity: mergeField(layers, (l) => l.glass?.frostOpacity ?? 0.08, 0.08, eqNum(0.005)),
      tintColor: mergeField(layers, (l) => l.glass?.tintColor ?? "#FFFFFF", "#FFFFFF", eqColor),
      borderRadius: mergeField(layers, (l) => l.glass?.borderRadius ?? 24, 24, eqNum(0.5)),
      borderOpacity: mergeField(layers, (l) => l.glass?.borderOpacity ?? 0.16, 0.16, eqNum(0.005)),
      shadowBlur: mergeField(layers, (l) => l.glass?.shadowBlur ?? 28, 28, eqNum(0.5)),
      saturation: mergeField(layers, (l) => l.glass?.saturation ?? 1.2, 1.2, eqNum(0.01)),
      brightness: mergeField(layers, (l) => l.glass?.brightness ?? 1.06, 1.06, eqNum(0.01)),
      contrast: mergeField(layers, (l) => l.glass?.contrast ?? 1.04, 1.04, eqNum(0.01)),
      noiseOpacity: mergeField(layers, (l) => l.glass?.noiseOpacity ?? 0.04, 0.04, eqNum(0.005)),
      bevelWidth: mergeField(layers, (l) => l.glass?.bevelWidth ?? 22, 22, eqNum(0.5)),
      specularIntensity: mergeField(layers, (l) => l.glass?.specularIntensity ?? 0.45, 0.45, eqNum(0.01)),
      textMode: mergeField(layers, (l) => l.glass?.textMode ?? "GLYPH", "GLYPH"),
    },
  };

  const out: MergedSelectionProps = {
    count: layers.length,
    sameKind,
    kinds,
    allSupportFullGlass,
    common,
  };

  if (sameKind === "TEXT") {
    out.text = {
      content: mergeField(layers, (l) => l.text?.content ?? "", ""),
      fontFamily: mergeField(layers, (l) => l.text?.fontFamily ?? "Outfit", "Outfit"),
      fontPath: mergeField(layers, (l) => l.text?.fontPath ?? "", ""),
      fontSize: mergeField(layers, (l) => l.text?.fontSize ?? 96, 96, eqNum(0.5)),
      fontWeight: mergeField(layers, (l) => l.text?.fontWeight ?? 600, 600),
      letterSpacing: mergeField(layers, (l) => l.text?.letterSpacing ?? 0, 0, eqNum(0.002)),
      align: mergeField(layers, (l) => l.text?.align ?? "center", "center"),
      color: mergeField(layers, (l) => l.text?.color ?? "#FFFFFF", "#FFFFFF", eqColor),
      colorAlpha: mergeField(layers, (l) => l.text?.colorAlpha ?? 1, 1, eqNum(0.005)),
      stemStretch: mergeField(layers, (l) => l.text?.stemStretch ?? 1, 1, eqNum(0.01)),
      shadowBlur: mergeField(layers, (l) => l.text?.shadowBlur ?? 0, 0, eqNum(0.5)),
      maxWidth: mergeField(layers, (l) => l.text?.maxWidth ?? 0, 0, eqNum(1)),
    };
  } else if (sameKind === "VIDEO") {
    out.video = {
      objectFit: mergeField(layers, (l) => l.video?.objectFit ?? "cover", "cover"),
      speed: mergeField(layers, (l) => l.video?.speed ?? 1, 1, eqNum(0.01)),
      clipRadius: mergeField(layers, (l) => l.video?.clipRadius ?? 0, 0, eqNum(0.5)),
      muted: mergeField(layers, (l) => l.video?.muted ?? true, true),
      volume: mergeField(layers, (l) => l.video?.volume ?? 0.5, 0.5, eqNum(0.01)),
    };
  } else if (sameKind === "IMAGE") {
    out.image = {
      sourceMode: mergeField(layers, (l) => l.image?.sourceMode ?? "FILE", "FILE"),
      objectFit: mergeField(layers, (l) => l.image?.objectFit ?? "contain", "contain"),
      borderRadius: mergeField(layers, (l) => l.image?.borderRadius ?? 0, 0, eqNum(0.5)),
    };
  } else if (sameKind === "SVG") {
    out.svg = {
      iconName: mergeField(layers, (l) => (l.svg?.customSvg ? "__CUSTOM__" : l.svg?.iconName ?? "prism"), "prism"),
      strokeWidth: mergeField(layers, (l) => l.svg?.strokeWidth ?? 1.8, 1.8, eqNum(0.05)),
      color: mergeField(layers, (l) => l.svg?.color ?? "#FFFFFF", "#FFFFFF", eqColor),
      colorAlpha: mergeField(layers, (l) => l.svg?.colorAlpha ?? 1, 1, eqNum(0.005)),
    };
  } else if (sameKind === "PROGRESS") {
    out.progress = {
      source: mergeField(layers, (l) => l.progress?.source ?? "MEDIA", "MEDIA"),
      fillColor: mergeField(layers, (l) => l.progress?.fillColor ?? "#FFFFFF", "#FFFFFF", eqColor),
      fillAlpha: mergeField(layers, (l) => l.progress?.fillAlpha ?? 1, 1, eqNum(0.005)),
      trackColor: mergeField(layers, (l) => l.progress?.trackColor ?? "#FFFFFF", "#FFFFFF", eqColor),
      trackAlpha: mergeField(layers, (l) => l.progress?.trackAlpha ?? 0.2, 0.2, eqNum(0.005)),
      borderRadius: mergeField(layers, (l) => l.progress?.borderRadius ?? 999, 999, eqNum(0.5)),
    };
  }

  return out;
}

/**
 * Coleta as camadas da árvore correspondentes aos `ids`, preservando a ordem da lista `ids`.
 */
export function resolveSelectedLayers(
  tree: readonly SceneLayer[],
  ids: readonly string[]
): SceneLayer[] {
  if (ids.length === 0) return [];
  const byId = new Map<string, SceneLayer>();
  const walk = (list: readonly SceneLayer[]) => {
    for (const l of list) {
      byId.set(l.id, l);
      if (l.children?.length) walk(l.children);
    }
  };
  walk(tree);
  const out: SceneLayer[] = [];
  for (const id of ids) {
    const hit = byId.get(id);
    if (hit) out.push(hit);
  }
  return out;
}

/**
 * Aplica `patcher` de forma imutável a todas as camadas da árvore cujo `id` pertence a `ids`.
 * Ramos não tocados mantêm exatamente a mesma referência (structural sharing).
 * Se `skipLocked` for `true`, camadas com `locked: true` não são alteradas.
 */
export function applyPatchToMany(
  tree: SceneLayer[],
  ids: ReadonlySet<string>,
  patcher: (layer: SceneLayer) => SceneLayer,
  options?: { skipLocked?: boolean }
): SceneLayer[] {
  if (ids.size === 0) return tree;
  const skipLocked = Boolean(options?.skipLocked);
  let changed = false;

  const next = tree.map((layer) => {
    let cur = layer;
    if (ids.has(layer.id) && (!skipLocked || !layer.locked)) {
      const patched = patcher(layer);
      if (patched !== layer) {
        cur = patched;
        changed = true;
      }
    }
    if (cur.children?.length) {
      const nextChildren = applyPatchToMany(cur.children, ids, patcher, options);
      if (nextChildren !== cur.children) {
        cur = { ...cur, children: nextChildren };
        changed = true;
      }
    }
    return cur;
  });

  return changed ? next : tree;
}

/** Rótulo curto para a pílula de multiseleção e cabeçalho do inspetor (ex.: `"3 itens · Texto"` ou `"4 itens · Tipos mistos"`). */
export function formatMultiSelectionSummary(merged: MergedSelectionProps): string {
  const noun = merged.count === 1 ? "item" : "itens";
  const kindLabel = merged.sameKind ? LAYER_KIND_LABELS[merged.sameKind] : "Tipos mistos";
  return `${merged.count} ${noun} · ${kindLabel}`;
}

// ───────────────────── Copiar / Colar Estilo (Etapa 8) ─────────────────────

export interface CopiedLayerStyle {
  sourceKind: LayerKind;
  opacity: number;
  blendMode: BlendModeCss;
  glass: GlassEffect;
  fxVisual: Record<string, string>;
  textStyle?: {
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
  borderRadius?: number;
  svgStyle?: {
    color: string;
    colorAlpha?: number;
    strokeWidth: number;
  };
  progressStyle?: {
    fillColor: string;
    fillAlpha: number;
    trackColor: string;
    trackAlpha: number;
    borderRadius: number;
  };
}

/** Extrai o estilo visual de uma camada sem copiar conteúdo, arquivo, id ou posição. */
export function extractLayerStyle(layer: SceneLayer): CopiedLayerStyle {
  const fxVisual: Record<string, string> = {};
  for (const key of ["opacity", "color", "visible", "rotation"]) {
    if (layer.fx?.[key]) fxVisual[key] = layer.fx[key];
  }
  const style: CopiedLayerStyle = {
    sourceKind: layer.kind,
    opacity: layer.opacity,
    blendMode: layer.blendMode,
    glass: { ...layer.glass },
    fxVisual,
  };
  if (layer.text) {
    style.textStyle = {
      fontFamily: layer.text.fontFamily,
      fontPath: layer.text.fontPath,
      fontSize: layer.text.fontSize,
      fontWeight: layer.text.fontWeight,
      letterSpacing: layer.text.letterSpacing,
      align: layer.text.align,
      color: layer.text.color,
      colorAlpha: layer.text.colorAlpha,
      stemStretch: layer.text.stemStretch,
      shadowBlur: layer.text.shadowBlur,
    };
  }
  if (layer.image) style.borderRadius = layer.image.borderRadius;
  else if (layer.video) style.borderRadius = layer.video.clipRadius;
  else style.borderRadius = layer.glass.borderRadius;

  if (layer.svg) {
    style.svgStyle = {
      color: layer.svg.color,
      colorAlpha: layer.svg.colorAlpha,
      strokeWidth: layer.svg.strokeWidth,
    };
  }
  if (layer.progress) {
    style.progressStyle = {
      fillColor: layer.progress.fillColor,
      fillAlpha: layer.progress.fillAlpha,
      trackColor: layer.progress.trackColor,
      trackAlpha: layer.progress.trackAlpha,
      borderRadius: layer.progress.borderRadius,
    };
  }
  return style;
}

/** Cola o estilo copiado em `target`, preservando conteúdo (`text.content`, `src`), tamanho e coordenadas. */
export function applyLayerStyle(target: SceneLayer, style: CopiedLayerStyle): SceneLayer {
  const supportsFullGlass = FULL_GLASS_KINDS.has(target.kind);
  const nextGlass: GlassEffect = supportsFullGlass
    ? {
        ...style.glass,
        // Preserva textMode coerente quando colando entre placa e texto
        textMode: target.kind === "TEXT" ? style.glass.textMode ?? "GLYPH" : target.glass.textMode,
      }
    : target.kind === "SVG"
    ? {
        ...target.glass,
        enabled: style.glass.enabled && style.glass.chromaticAberration > 0.05,
        chromaticAberration: style.glass.chromaticAberration,
      }
    : target.glass;

  const nextFx = { ...target.fx };
  if (style.fxVisual.opacity) nextFx.opacity = style.fxVisual.opacity;
  if (style.fxVisual.color && (target.kind === "TEXT" || target.kind === "SVG")) {
    nextFx.color = style.fxVisual.color;
  }

  const next: SceneLayer = {
    ...target,
    opacity: style.opacity,
    blendMode: style.blendMode,
    glass: nextGlass,
    fx: nextFx,
  };

  if (target.kind === "TEXT" && target.text) {
    if (style.textStyle) {
      next.text = {
        ...target.text,
        ...style.textStyle,
      };
    } else if (style.svgStyle) {
      next.text = {
        ...target.text,
        color: style.svgStyle.color,
        colorAlpha: style.svgStyle.colorAlpha ?? target.text.colorAlpha,
      };
    }
  }
  if (target.kind === "IMAGE" && target.image && style.borderRadius !== undefined) {
    next.image = { ...target.image, borderRadius: style.borderRadius };
  }
  if (target.kind === "VIDEO" && target.video && style.borderRadius !== undefined) {
    next.video = { ...target.video, clipRadius: style.borderRadius };
  }
  if (target.kind === "SVG" && target.svg) {
    if (style.svgStyle) {
      next.svg = { ...target.svg, ...style.svgStyle };
    } else if (style.textStyle) {
      next.svg = {
        ...target.svg,
        color: style.textStyle.color,
        colorAlpha: style.textStyle.colorAlpha ?? target.svg.colorAlpha,
      };
    }
  }
  if (target.kind === "PROGRESS" && target.progress && style.progressStyle) {
    next.progress = { ...target.progress, ...style.progressStyle };
  }

  return next;
}

// ───────────────────── Transformação de Caixa em Texto / Fórmulas ƒx ─────────────────────

export type TextCaseMode = "upper" | "lower" | "capitalize";

function capitalizeLiteral(s: string): string {
  return s
    .toLocaleLowerCase("pt-BR")
    .replace(/(^|[\s\-_/([{"'“‘])(\p{L})/gu, (_, sep: string, letter: string) => {
      return `${sep}${letter.toLocaleUpperCase("pt-BR")}`;
    });
}

function transformLiteralSegment(seg: string, mode: TextCaseMode): string {
  if (!seg) return seg;
  if (mode === "upper") return seg.toLocaleUpperCase("pt-BR");
  if (mode === "lower") return seg.toLocaleLowerCase("pt-BR");
  return capitalizeLiteral(seg);
}

/**
 * Converte caixa alta / baixa / capitalizada preservando tokens de fórmulas `$...$` (como `df(HH:mm)`),
 * envolvendo expressões `ƒx` com `tc(up|low|cap, ...)` sem duplicar chamadas `tc` existentes.
 */
export function transformTextCase(content: string, mode: TextCaseMode): string {
  if (!content) return content;
  const fxFn = mode === "upper" ? "up" : mode === "lower" ? "low" : "cap";
  // Divide em trechos fora e dentro de `$...$` pareados
  const parts = content.split(/(\$[^$]+\$)/g);
  return parts
    .map((part) => {
      if (part.length >= 2 && part.startsWith("$") && part.endsWith("$")) {
        const inner = part.slice(1, -1).trim();
        if (!inner) return part;
        const existingTc = inner.match(/^tc\(\s*(up|upper|low|lower|cap|capitalize)\s*,\s*([\s\S]+)\)$/i);
        if (existingTc) {
          return `$tc(${fxFn}, ${existingTc[2].trim()})$`;
        }
        return `$tc(${fxFn}, ${inner})$`;
      }
      return transformLiteralSegment(part, mode);
    })
    .join("");
}

/** Coleta recursivamente todas as camadas-folha (não-GROUP) dentro de um grupo, na ordem visual. */
export function collectGroupLeafLayers(group: SceneLayer): SceneLayer[] {
  if (group.kind !== "GROUP" || !group.children || group.children.length === 0) return [];
  const out: SceneLayer[] = [];
  const walk = (list: readonly SceneLayer[]) => {
    for (const child of list) {
      if (child.kind === "GROUP") {
        if (child.children && child.children.length > 0) walk(child.children);
      } else {
        out.push(child);
      }
    }
  };
  walk(group.children);
  return out;
}

/** Conta quantas camadas de cada tipo existem em uma lista, preservando a ordem da primeira ocorrência. */
export function summarizeGroupKinds(
  leaves: readonly SceneLayer[]
): Array<{ kind: LayerKind; count: number }> {
  const counts = new Map<LayerKind, number>();
  for (const l of leaves) {
    counts.set(l.kind, (counts.get(l.kind) ?? 0) + 1);
  }
  return Array.from(counts.entries()).map(([kind, count]) => ({ kind, count }));
}

/**
 * Aplica `fn` aos descendentes de `group` cujos IDs estão em `targetIds`,
 * preservando referências intactas de subárvores não alteradas.
 */
export function patchGroupDescendants(
  group: SceneLayer,
  targetIds: ReadonlySet<string>,
  fn: (layer: SceneLayer) => SceneLayer
): SceneLayer {
  if (!group.children || group.children.length === 0 || targetIds.size === 0) return group;
  const mapList = (list: SceneLayer[]): SceneLayer[] => {
    let changed = false;
    const next = list.map((item) => {
      let cur = targetIds.has(item.id) ? fn(item) : item;
      if (cur.children && cur.children.length > 0) {
        const nextChildren = mapList(cur.children);
        if (nextChildren !== cur.children) {
          cur = { ...cur, children: nextChildren };
        }
      }
      if (cur !== item) changed = true;
      return cur;
    });
    return changed ? next : list;
  };
  const nextChildren = mapList(group.children);
  return nextChildren === group.children ? group : { ...group, children: nextChildren };
}
