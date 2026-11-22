E1-L3 OK (gate ts: 0 erros novos).
- Checkpoint já tinha runtime `Map<objectId, LoadedObjectRuntime>` (`eng.objectsMap`) + efeito de diff por `objectsSourceKey`, playback por `playbackSyncKey`, transform/spin YXZ por quadro no loop (pivot por objeto, normalização 2,4 por objeto). Lote endureceu isso:
- `LoadedObjectRuntime.loadKey` tipado (substitui cast `loadingKey`); carga em voo com mesma chave não reinicia (1.6); callbacks validam `objectsMap.get(id) === capturedRt` + `loadToken` (descarta e dá dispose no gltf atrasado).
- `retainSharedLoaders()` por runtime criado; `releaseSharedLoaders()` na remoção do objeto e no dispose do engine (layer mantém sua própria ref).
- Remoção marca `syncChanged` (inspect/`setModelLoadRev`/wake só quando algo mudou); dispose do engine agora faz `uncacheRoot`, zera token/loadKey.
- `modelIndex.ts::disposeObject3D` também chama `skeleton.dispose()` (bone texture de SkinnedMesh).
- Kinds: contrato só tem GLB/SHOWCASE; SHOWCASE (e GLB sem src/erro) usa `buildShowcaseSculpture` (sculptures). DISPLAY_SLAB = SHOWCASE KINETIC_SLAB; FLOOR_GRID não existe.
- `parentId` ainda NÃO aplicado no runtime (pivots todos filhos de `stagePivot`); bindings/dynamicTextures por objeto = E1-L4.
- Testar em máquina: 2 .glb na cena, remover ambos → HUD `onStats` volta à contagem base de geometrias/texturas.
