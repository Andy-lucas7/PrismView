model: opus
effort: medium
gate: ts
---
# E1-L1 — Schema multi-objetos + migração (só tipos/dados, sem tocar render)

Arquivos: `apps/prismview-studio/src/types/prism.ts` (grep: `Scene3DConfig`, `normalizeProjectScene3D`, `autoRotateSpeed`, `modelSrc`, `create*Project`, templates 3D).

Tarefas:
1. Inspecione o `Scene3DConfig` atual e o contrato rascunho do CONTEXT.md. Reutilize campos existentes; adicione `Scene3DBackgroundMode`, `backgroundColor`, `Scene3DObject`, `objects`, `camera {position,target,fov,parallax}`.
2. Bump do schema (v2 → v3). `normalizeProjectScene3D` migra projetos antigos de forma idempotente: `modelSrc`, `autoRotateSpeed`, `playback`, `dynamicTextures`, escala/posição legados → `objects[0]`. `autoRotateSpeed` legado igual a 0.22 (antigo default) vira 0; outros valores são preservados. `backgroundMode` = TRANSPARENT para todos.
3. Defaults: novo `.glb` importado nasce com `autoRotateSpeed: 0`. Atualize `create*Project` e templates 3D para `objects[]`.
4. Mantenha os campos legados (`modelSrc` etc.) como `@deprecated` opcionais, só para o `Scene3DLayer.tsx` continuar compilando até E1-L3. Não os escreva mais ao salvar.
5. Preencha a seção "CONTRATO FINAL" do CONTEXT.md com os tipos reais (nomes exatos).

Aceite: o gate ts passa; migração idempotente (rodar 2× dá o mesmo resultado).
Proibido: mexer em `Scene3DLayer.tsx`, Inspector, Rust.
