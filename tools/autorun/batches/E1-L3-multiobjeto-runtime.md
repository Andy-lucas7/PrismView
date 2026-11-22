model: opus
effort: medium
gate: ts
---
# E1-L3 — Runtime multi-objeto em Scene3DLayer (carga, transform, dispose)

Arquivos: `src/components/Scene3DLayer.tsx` (grep: `runtimeRef`/`clearRuntimeModel`/`loadingKey`/`modelSrc`/`autoRotateSpeed`), `src/engine/scene3d/loaders.ts`, `src/engine/scene3d/modelIndex.ts`.

Tarefas:
1. Troque o runtime de modelo único por `Map<objectId, ObjectRuntime>` (grupo raiz, mixer, índice de nós/morphs/materiais, clip atual, chave de carga).
2. Efeito de sincronização por diff de `config.objects`: adicionar → carregar; remover → `disposeObject3D` + `mixer.uncacheRoot` + release dos loaders; mudou transform/visible/playback → só atualizar (sem recarregar).
3. Carga em voo por `objectId+modelSrc` (promessa guardada; não cancelar nem baixar de novo se a chave for igual — AUDITORIA 1.6).
4. Transform por objeto: position/rotation (graus, ordem YXZ) /scale aplicados no grupo raiz; `autoRotateSpeed` por objeto; normalização de tamanho do modelo mantida por objeto.
5. `kind` DISPLAY_SLAB/FLOOR_GRID/SHOWCASE: reutilize o que já existe em `engine/scene3d/sculptures` quando houver; senão carregue apenas GLB e deixe os outros kinds invisíveis sem erro (registrar no HANDOFF).
6. Sem alocação por quadro; `dispose()` correto; texturas do `dynamicTexCache` não são dispostas.

Aceite: o gate ts passa; 2 `.glb` na mesma cena aparecem e removê-los volta contagem de geometrias/texturas ao base (HUD `onStats`).
Proibido: bindings/dynamicTextures por objeto (E1-L4), UI, Rust.
