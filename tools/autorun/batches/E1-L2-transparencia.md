model: sonnet
effort: high
gate: ts
---
# E1-L2 — Transparência real no preview do Estúdio

Arquivos: `src/components/Scene3DLayer.tsx` (grep: `new WebGLRenderer`, `ENV_SPECS`, `scene.background`, `applySceneBackground`, `setClearColor`, `preserveDrawingBuffer`), `src/engine/scene3d/environment.ts`.

Tarefas:
1. `WebGLRenderer({ alpha: true, premultipliedAlpha: true })` + `setClearColor(0x000000, 0)`. Mantenha o restante das opções.
2. `applySceneBackground` obedece `config.backgroundMode`: TRANSPARENT → `scene.background = null`; SOLID → `backgroundColor`; ENVIRONMENT → cor/ambiente do dia-noite (comportamento antigo). `scene.environment` (IBL) continua sempre ligado.
3. Remova a cor de fundo cinza forçada do `ENV_SPECS` do caminho TRANSPARENT; preserve o uso no ENVIRONMENT.
4. Chame `applySceneBackground` só quando `backgroundMode`/cor/hora mudam, não por quadro.
5. Verifique o fluxo do vidro óptico sobre 3D (`onFrameReady`, backdrop `kind:"canvas", live`): alpha pré-multiplicado não pode gerar halo escuro. Ajuste na amostragem do backdrop se necessário (`utils/opticalGlassWebGL.ts`, patch pontual).
6. Contexto WebGL restaurado deve manter o modo de fundo.

Aceite: o gate ts passa; com TRANSPARENT o canvas não pinta fundo.
Proibido: multi-objeto, UI, Rust.
