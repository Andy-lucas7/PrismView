import type { PrismProject, SceneLayer } from "../types/prism";

/**
 * Recursos aceitos no Estúdio que o motor nativo (desktop) ainda não desenha. Espelha os limites
 * do conversor (`native_scene.rs`) e do motor; mudar um lado exige revisar o outro.
 */
export interface NativeCompatIssue {
  layerId: string;
  message: string;
}

function walk(layers: SceneLayer[], visit: (layer: SceneLayer) => void): void {
  for (const layer of layers) {
    visit(layer);
    if (layer.children?.length) walk(layer.children, visit);
  }
}

export function nativeCompatIssues(project: PrismProject): NativeCompatIssue[] {
  const issues: NativeCompatIssue[] = [];
  let scene3dCount = 0;
  walk(project.layers, (layer) => {
    if (layer.kind === "SCENE_3D") {
      scene3dCount += 1;
      if (scene3dCount === 2) {
        issues.push({
          layerId: layer.id,
          message: `Só a primeira cena 3D aparece no desktop ("${layer.name}" fica de fora).`,
        });
      }
    }
    const font = layer.text?.fontPath ?? "";
    if (/\.woff2?$/i.test(font)) {
      issues.push({
        layerId: layer.id,
        message: `Fonte .woff/.woff2 não é suportada no desktop ("${layer.name}"): use .ttf ou .otf.`,
      });
    }
  });
  return issues;
}
