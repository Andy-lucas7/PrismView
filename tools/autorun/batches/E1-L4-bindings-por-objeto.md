model: opus
effort: medium
gate: ts
---
# E1-L4 — Bindings, clips e texturas dinâmicas por objeto

Arquivos: `src/components/Scene3DLayer.tsx` (grep: `ReactiveBinding3D`, `NODE_LOOK_AT`, `NODE_SCALE_PULSE`, `MORPH_WEIGHT`, `EMISSIVE_GLOW`, `PLAY_CLIP`, `dynamicTexCache`, `dynamicTextures`), `src/types/prism.ts` (tipo do binding).

Tarefas:
1. Binding ganha `objectId?: string`. Vazio ou inexistente = primeiro objeto (compat com projetos migrados). Resolução de alvo por `objectId` + `targetName` no índice do objeto certo, só no load/invalidate (não por quadro).
2. `PLAY_CLIP`/playback: mixer independente por objeto.
3. `dynamicTextures` por objeto: aplicar no material do objeto correto; `baseMap` devolvido antes de dispor (AUDITORIA 1.4); `dynamicTexCache` segue compartilhado.
4. Raycast de hover/clique devolve `objectId` + nó; bindings de mouse atuam só no objeto-alvo.
5. Manter drift-free; reutilizar Maps/pools já existentes, nada novo por quadro.

Aceite: o gate ts passa; dois objetos com bindings distintos não interferem entre si.
Proibido: UI, novos tipos de ação (E3), Rust.
