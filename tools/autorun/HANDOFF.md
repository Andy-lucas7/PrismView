E2-L2 OK (gate ts: 0 erros novos).
- Seleção por clique (Box3 broad-phase `performRaycastAt`), gizmo, commit único (`commitRoomTransform` → 1 undo, sem passo vazio) e sync Inspetor↔viewport já existiam (E1-L5/Fase 2); mantidos.
- `editorCamera.ts`: host ganhou `onNavigate(active)`; controlador ganhou `setEnabled(bool)` (aborta drag, bloqueia pointer/wheel/teclas WASD). Blur/disable usam `releaseNavigation`.
- `scene3dEditorTools.ts`: gizmo arrastando ⇒ `cam.setEnabled(false)`; câmera navegando ⇒ `controls.enabled=false` (Alt+LMB não inicia arrasto do gizmo).
- Highlight: 1 `BoxHelper` único retargetado (`setFromObject` só ao trocar pivô, `update()` por quadro sem alocar); dispose de geometria+material em `dispose()` (ao sair do ROOM a sessão inteira é descartada).
- Clique no vazio ⇒ `onSelectObject(null)`; Inspetor cai em `objs[0]` visualmente (selected3dObjectId=null).
- Próximos: E2-L3 toolbar/atalhos (G/R/S/F já em StudioEditor), E2-L4 persistir câmera (`editorApiRef.getCameraState`).
