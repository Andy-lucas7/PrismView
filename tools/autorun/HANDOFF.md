E3-L6 revisão final concluída (gates ts/rust NÃO executados por solicitação do usuário).
Correções aplicadas na auditoria geral:
- Zero-alloc per-frame: `applyConfigCamera` em `environment.ts` (sem alocar objeto/arrays no orbit easing) e reuso de `rayHitResult` em `Scene3DLayer.tsx::performRaycastAt`.
- Isolamento de `scene.background`: `applySceneBackground` não compartilha mais `Color` global entre múltiplas instâncias 3D.
- Paridade TS↔Rust: `holder.updateMatrixWorld(true)` após normalização de `.glb`/procedural (corrige `nodeUnitScale` de `NODE_MOVE_OFFSET` no 1º quadro) + sincronização de hierarquia `parentId` em `Scene3DLayer.tsx` + escala mínima `0.05` alinhada no `StudioInspector`.
- Correções de UI/HUD: `emitStats` relata `powerCapsRef.current.state` real em modo ROOM; `activeObjInfo` não cai mais nos dados do objeto 0 para objetos secundários carregando; `saveWallpaperCamera` usa `getCameraState()`.
- `CLAUDE.md` atualizado com schema v3, modo ROOM, `backgroundMode`, câmera e novos alvos.
Pendências para teste em máquina (Windows + GPU):
- Rodar `node tools/autorun/gate.mjs both` (e `cargo test -p prismview-engine` / `cargo test --manifest-path apps/prismview-studio/src-tauri/Cargo.toml`) para validar TS + Rust.
- Testar em GPU: `.glb` esquelético + morphs (Alt+Clique em osso, Esqueleto, SWING/MOVE/MORPH, chips), multi-objeto + `.prism` export/import, fundo TRANSPARENT/SOLID/ENVIRONMENT sobre vidro óptico, e wallpaper nativo no motor wgpu.
