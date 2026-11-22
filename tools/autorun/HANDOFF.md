E2-L3 OK (gate ts: 0 erros novos).
- `StudioEditor`: estado único `scene3dEditMode` ("LAYOUT"|"ROOM"|"TEST", tipo `Scene3DEditMode` exportado de `Scene3DLayer.tsx`); `room3dMode`/`test3dMode` agora derivados. Seletor de 3 modos só aparece com SCENE_3D selecionada (ou fora de LAYOUT, p/ poder sair); rótulo "Testar Interações".
- ROOM: `reactiveNeeds` vazio (sensores WASAPI/teclado off) + `Scene3DLayer` ignora `signalBindings` quando há `editor`. TEST: câmera do wallpaper travada (sem editor) + cursor/clique do preview ao vivo (já existia).
- Atalhos: `useKeyboardShortcuts` ganhou `room?: RoomShortcuts` (G/R/S/F/Del/Ctrl+D/Esc, ignora repeat); com `room` definido os atalhos 2D não disparam, fora de ROOM os de sala não existem. Listener ad-hoc removido.
- Toolbar ROOM: + Duplicar, Excluir (1 `patchLayer` "discrete" = 1 undo), toggle Esqueleto (`showSkeleton3d`, só estado local; E3-L3 liga o helper). Grade já era `GridHelper` só no chunk `three-editor`, `showGrid3d` local.
- Próximo E2-L4: `editorApiRef.captureCameraState` / `saveWallpaperCamera` já existem em StudioEditor.
