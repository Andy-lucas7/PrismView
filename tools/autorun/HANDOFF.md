E1-L4 OK (gate ts: 0 erros novos). Só `Scene3DLayer.tsx` mudou; `prism.ts` já tinha `objectId?` (E1-L1).
- Novo `ResolvedBindingTarget {rt,node,mats}` + `bindingOwnerId(objs, objectId)` (objectId existente, senão `objects[0]`). Não cai em outro objeto: dono ainda carregando ⇒ binding ignorado.
- `ensureBindingCache(eng,cfg)` (render + clique): listas por cfg/thumb e alvos resolvidos (`cachedSignalTargets`/`cachedCursorTargets`, paralelos às listas) só quando muda `eng.targetsRev`. `resolveTargetRuntime` e os `nodesByName.get` por quadro foram removidos.
- `eng.targetsRev++` em `clearRuntimeModel` (se tinha índice) e nas 3 atribuições de `modelIndex` (procedural, glb, erro→procedural).
- `applyScalarTarget(b, target, ...)` usa node/mats resolvidos; PLAY_CLIP já usava o mixer/override do próprio `rt`.
- Hover/clique: exigem `hoveredObjectId`/`hit.objectId === target.rt.id`, inclusive sem `objectId` (antes um `sourceNode` vazio disparava em qualquer objeto). `performRaycastAt` já devolve `{object, objectId}`.
- Texturas dinâmicas: loads em voo deduplicados por `cacheKey` (`pendingLoads`), aplicados a cada `rt` ainda vivo; antes 2 objetos com a mesma URL vazavam 1 textura. Restaurar `baseMap` antes de trocar/descartar já existia.
- Testar em máquina: 2 GLB, bindings HOVER_MESH/CLICK_MESH com `sourceNode` vazio em cada um ⇒ só o objeto sob o cursor reage; mesma capa em 2 objetos ⇒ 1 textura no HUD.
- Pendente (outros lotes): `parentId` no runtime; motor nativo (Rust) não lê `objectId`.
