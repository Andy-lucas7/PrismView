model: sonnet
effort: high
gate: ts
---
# E1-L5 — UI: lista de objetos da sala + fundo

Arquivos: `src/components/StudioInspector.tsx` (grep: `SCENE_3D`, `modelSrc`, `autoRotateSpeed`), `src/components/StudioEditor.tsx` (grep: `createObjectURL`, `pick_local_file`, `PickedFileDto`), `src/types/prism.ts`.

Tarefas:
1. Seção "Objetos da Sala" no Inspetor da camada SCENE_3D: lista com nome, visível (olho), excluir, duplicar, selecionar; botão "+ Adicionar .glb".
2. Adicionar `.glb/.gltf` via `pick_local_file` (caminho real, `ASSET_SCOPE.allow_file`). Nunca `blob:` salvo no projeto; se precisar de preview temporário, revogar URL antiga ao trocar (AUDITORIA 1.2/2.6).
3. Edição numérica de position/rotation/scale, `autoRotateSpeed` (default 0) e playback (clip, speed, loop) do objeto selecionado.
4. Controles de `backgroundMode` (TRANSPARENT/SOLID/ENVIRONMENT) + seletor de cor para SOLID.
5. Tudo via patch imutável + `useHistory` (undo/redo). Reutilize componentes de input já existentes.

Aceite: o gate ts passa; adicionar/remover/duplicar objetos funciona e persiste ao salvar/reabrir.
Proibido: gizmos/câmera (E2), Rust.
