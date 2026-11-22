E2-L1 OK (gate ts: 0 erros novos).
- Novo `src/engine/scene3d/editorCamera.ts`: `createEditorCamera(host)` → `EditorCameraController` {update, needsFrames, isNavigating, getState, setState, setView, focusBox, dispose}. RMB=olhar, Alt+RMB=orbitar em torno do ponto focal, RMB+WASD/QE=voar (Shift ×3), MMB/Alt+LMB=pan, scroll=dolly (não atravessa o foco). Vetores/flags pré-alocados (zero alocação por quadro); `contextmenu`/teclas (captura no window) só enquanto montado; dispose remove tudo.
- Tipos `Scene3DCameraCapture`/`Scene3DEditorView` movidos para editorCamera.ts (re-exportados por `scene3dEditorTools`). Só importado por `scene3dEditorTools` (import dinâmico ⇒ chunk do editor, nunca desktop/widget).
- `scene3dEditorTools.ts`: sessão delega câmera ao controlador; ficou só com clique-seleção LMB + gizmo + grade. Sessão ganhou `setCameraState`, `focusOn(id)` (Box3 pré-alocada); `needsFrames` = gizmo arrastando | câmera voando/animando | hold do commit.
- `Scene3DLayer.tsx`: `Scene3DEditorApi` += `getCameraState`, `setCameraState(state, animate?)`, `focusOn(objectId)`; `captureCameraState` mantido como alias deprecated (StudioEditor usa). Editor agora respeita `fpsCap` do governador (piso 15 fps) e `dprCap` (FROZEN → NORMAL só no editor).
- `setCameraState` ignora `fov` (o loop reaplica `cfg.camera.fov` todo quadro). Convenção orbital = `computeConfigCameraView`.
- Fora do ROOM nada mudou (montagem continua `editMode3d && interactive && !isThumbnail`, `room3d` só passado em ROOM).
- Próximos: E2-L2 seleção/gizmo, E2-L3 toolbar, E2-L4 persistir câmera (usar `getCameraState`).
