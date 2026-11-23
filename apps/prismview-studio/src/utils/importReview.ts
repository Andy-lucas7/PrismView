import type { SceneLayer } from "../types/prism";

/** Camada de um pacote importado cuja ação "Abrir app" foi desativada para revisão. */
export interface OpenAppReviewItem {
  layerId: string;
  layerName: string;
  target: string;
}

/** Lista (pré-ordem) as camadas com `clickTargetNeedsReview`. */
export function collectOpenAppReview(layers: SceneLayer[], out: OpenAppReviewItem[] = []): OpenAppReviewItem[] {
  for (const layer of layers) {
    if (layer.clickTargetNeedsReview) {
      out.push({ layerId: layer.id, layerName: layer.name, target: layer.clickTargetPending ?? "" });
    }
    if (layer.children?.length) collectOpenAppReview(layer.children, out);
  }
  return out;
}

/**
 * Resolve a revisão: camadas em `allowed` recebem o alvo original; as demais ficam sem alvo.
 * Em ambos os casos as marcas de revisão saem. Camadas sem marca são devolvidas intactas.
 */
export function resolveOpenAppReview(layers: SceneLayer[], allowed: ReadonlySet<string>): SceneLayer[] {
  return layers.map((layer) => {
    const children = layer.children?.length ? resolveOpenAppReview(layer.children, allowed) : layer.children;
    if (!layer.clickTargetNeedsReview) {
      return children === layer.children ? layer : { ...layer, children };
    }
    const { clickTargetPending, clickTargetNeedsReview: _review, ...rest } = layer;
    return {
      ...rest,
      children,
      clickTarget: allowed.has(layer.id) ? clickTargetPending ?? "" : "",
    };
  });
}
