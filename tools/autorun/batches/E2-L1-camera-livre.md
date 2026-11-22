model: opus
effort: medium
gate: ts
---
# E2-L1 — Câmera livre (orbit / fly / pan / zoom), só no Estúdio

Arquivos: `src/components/Scene3DLayer.tsx` (grep: `Scene3DEditorApi`, `interactive`, `scene3dEditMode`, `camera`), `src/utils/scene3dEditorTools.ts`, `vite.config.ts` (chunk `three-editor`).

Tarefas:
1. Novo módulo `src/engine/scene3d/editorCamera.ts` (chunk `three-editor`, import dinâmico, nunca no desktop/widget): controlador com RMB+arrastar = olhar/orbitar; RMB+W A S D + Q/E = voar (Shift acelera); botão do meio ou Alt+arrastar = pan; scroll = zoom/dolly.
2. Montado só quando `interactive && scene3dEditMode === "ROOM"`. Listeners removidos no cleanup; `contextmenu` suprimido só nesse modo.
3. Loop reusa vetores pré-alocados; respeita `powerCaps`; render sob demanda (invalida só quando a câmera mexe).
4. `Scene3DEditorApi` expõe `getCameraState()` / `setCameraState()` / `focusOn(objectId)`.
5. Fora do modo ROOM a câmera segue a do wallpaper (config + parallax), sem mudar o comportamento atual.

Aceite: o gate ts passa; o chunk do desktop não importa o módulo.
Proibido: seleção/gizmo (E2-L2), toolbar (E2-L3).
