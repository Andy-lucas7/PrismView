E1-L1 OK (gate ts: 0 erros novos).
- Schema v3 já estava em `types/prism.ts` (checkpoint anterior): `Scene3DObject`, `Scene3DBackgroundMode`, `backgroundColor`, `objects[]`, `camera.target`, `axis/invertAxis/objectId` no binding, `NODE_ROTATE_SWING`/`NODE_MOVE_OFFSET`. Lote auditou e só preencheu o CONTRATO FINAL no CONTEXT.md.
- Migração `normalizeScene3DConfig` idempotente (fast path devolve mesma ref; legado ≤v2 → `objects[0]` `obj-primary`, 0.22→0, TRANSPARENT; campos legados removidos via `stripLegacyScene3DKeys`). Save (`persistProject*`), load, import e bridge passam por `normalizeProjectScene3D`.
- Decisões: kind continua `GLB|SHOWCASE` (DISPLAY_SLAB = SHOWCASE/KINETIC_SLAB; sem FLOOR_GRID); `playback.clipName` "" = nenhum (não null); câmera = órbita esférica + `target` + `orbitParallaxX/Y` (sem `position` explícito).
- Import .glb (StudioEditor) já nasce com `autoRotateSpeed: 0`. Templates Solar/Pulse usam spin explícito 0.28/0.12 (intencional, não é o 0.22 legado).
- Pendente p/ lotes seguintes: `StudioInspector.tsx:1320` adiciona SHOWCASE com `autoRotateSpeed: 0.22` (fora do escopo, Inspector proibido aqui); `Scene3DLayer.tsx` ainda lê campos `@deprecated` até E1-L3.
- Rust (`native_scene.rs`, `core/scene3d/config.rs`) ainda não espelha `objects[]`/`backgroundMode` — conferir no lote de espelhamento.
