E3-L3 OK (gate ts: 0 erros novos).
- Novo `engine/scene3d/modelTree.ts` (só importado pelo chunk do editor): `getModelTree(idx)` (WeakMap por `ModelSceneIndex`, nós `{id,name,type bone|mesh|group,depth,parentId,children,morphs,materials}`; `id` = índice pré-ordem em `flat`), `getTreeNodeObject(idx,id)`.
- `scene3dEditorTools.ts`: host ganhou `getModelIndex`; sessão ganhou `setSkeleton` (SkeletonHelper do objeto selecionado, religado se o modelo recarrega, `dispose` ao desligar/sair), `getObjectTree`, `highlightNode` (1 `Box3Helper` reutilizado, osso sem geometria = cubo 0.08).
- `Scene3DLayer`: `Scene3DEditorApi.getObjectTree/highlightNode`; props `showSkeleton`, `onEditorSync` (dispara ao criar o editor e a cada (re)load de modelo). `Room3DEditBinding` (UnifiedCompositor) ganhou `showSkeleton`/`onEditorSync`.
- `StudioEditor`: `outlinerRev` + `bumpOutlinerRev`; botão Esqueleto agora funcional. `StudioInspector`: props `room3dMode/editorApiRef/outlinerRev`; seção "Ossos & Peças" (só objeto GLB) usa novo `components/Scene3DOutliner.tsx` (busca, colapsar, janela virtual >300 linhas, clique = destaque; limpa destaque ao trocar objeto/desmontar).
- Árvore só existe em ROOM (API do editor); fora dele o inspetor mostra aviso. Esqueleto = só objeto selecionado.
- E3-L4: Alt+Clique pode reusar `highlightNode`/`getObjectTree` (id de nó → nome em `tree.flat[id].name`); não há ainda mapeamento clique→nodeId (usar `matchesMeshTarget`/raycast em `engine.pick`).
- Não testado em runtime (precisa .glb esquelético na máquina).
