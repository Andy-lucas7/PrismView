model: sonnet
effort: high
gate: ts
---
# E2-L2 — Seleção por clique + TransformControls

Arquivos: `src/components/Scene3DLayer.tsx`, `src/utils/scene3dEditorTools.ts` (gizmos/snap já existem: reutilize), `src/engine/scene3d/editorCamera.ts` (E2-L1).

Tarefas:
1. Clique esquerdo no viewport (modo ROOM) faz raycast e seleciona o `objectId` (raycast em Box3 do objeto antes das malhas — AUDITORIA 3.7). Clique no vazio desseleciona.
2. `TransformControls` anexado ao grupo raiz do objeto selecionado, modos mover/girar/escalar via API. Desativar a câmera livre enquanto arrasta o gizmo.
3. Ao soltar o gizmo, escrever position/rotation(graus)/scale de volta em `objects[]` (um único patch por arrasto → um passo de undo). Snap já existente respeitado.
4. Seleção sincronizada com a lista do Inspetor (E1-L5) nos dois sentidos.
5. Highlight leve do selecionado (BoxHelper reaproveitado, sem alocar por quadro). Cleanup/`dispose` completo ao sair do modo ROOM.

Aceite: o gate ts passa; arrastar um objeto atualiza os campos numéricos e desfaz com Ctrl+Z.
Proibido: toolbar/atalhos (E2-L3), câmera persistida (E2-L4).
