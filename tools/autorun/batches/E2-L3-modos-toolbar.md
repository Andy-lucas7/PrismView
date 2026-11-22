model: sonnet
effort: high
gate: ts
---
# E2-L3 — Modos (Layout 2D / Sala 3D / Testar), toolbar e atalhos

Arquivos: `src/components/StudioEditor.tsx`, `src/hooks/useKeyboardShortcuts.ts`, `src/components/Scene3DLayer.tsx`, `src/utils/scene3dEditorTools.ts`.

Tarefas:
1. Com camada SCENE_3D selecionada, seletor de 3 modos no topo: `Layout 2D`, `Sala 3D`, `Testar Interações`. Estado em `scene3dEditMode` ("LAYOUT" | "ROOM" | "TEST"). TEST trava a câmera do wallpaper e liga mouse/clique/áudio/teclado ao vivo; ROOM desliga sensores reativos.
2. Toolbar flutuante no modo ROOM: Mover (G), Girar (R), Escalar (S), Focar (F), + Adicionar .glb, Duplicar (Ctrl+D), Excluir (Del), mostrar/ocultar Grade, mostrar/ocultar Esqueleto (placeholder de estado apenas; o helper vem no E3-L3).
3. Atalhos registrados em `useKeyboardShortcuts` ativos só no modo ROOM, sem conflitar com campos de texto nem com os atalhos 2D.
4. `GridHelper` do chão só no Estúdio (chunk `three-editor`), toggle persistido apenas em estado local do editor.
5. Duplicar/Excluir geram patch único (undo/redo corretos).

Aceite: o gate ts passa; atalhos não disparam fora do modo ROOM.
Proibido: câmera persistida (E2-L4), SkeletonHelper real (E3-L3).
