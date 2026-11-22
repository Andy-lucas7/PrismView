model: sonnet
effort: high
gate: ts
---
# E3-L3 — Outliner de ossos/partes + SkeletonHelper

Arquivos: `src/engine/scene3d/modelIndex.ts` (índice de nós/morphs/materiais), `src/components/StudioInspector.tsx`, `src/components/Scene3DLayer.tsx` (`Scene3DEditorApi`).

Tarefas:
1. `Scene3DEditorApi.getObjectTree(objectId)` devolve a árvore (nome, tipo bone/mesh/group, filhos, morph targets, materiais) a partir do índice já existente; cache por modelo, sem recomputar a cada render.
2. Inspetor, para o objeto selecionado: lista em árvore com busca rápida, colapsável (virtualizar se > 300 nós). Clicar num nó destaca a peça no viewport.
3. Toggle "Esqueleto" (da toolbar E2-L3) monta `SkeletonHelper` só no modo ROOM do Estúdio e o remove com `dispose` ao sair.
4. Nada disso existe no bundle do desktop/widget.

Aceite: o gate ts passa; árvore reflete ossos/malhas do `.glb`; sem vazamento ao trocar de objeto.
Proibido: Alt+Clique/chips (E3-L4), edição de ações (E3-L5).
