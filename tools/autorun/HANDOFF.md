E1-L2 OK (gate ts: 0 erros novos).
- `WebGLRenderer` já tinha alpha+premultipliedAlpha+`setClearColor(0,0)` (checkpoint); mantido. `ENV_SPECS.bgHex` só vai pro PMREM e pro modo ENVIRONMENT, nunca no TRANSPARENT.
- `environment.ts`: `applySceneBackground(scene,cfg,dayNightBg?)` agora cobre TRANSPARENT(null)/SOLID/ENVIRONMENT (bgHex do preset, ou cor dia-noite se `autoDayNight`). Novos: `sampleDayNightBackground(hour)` (Color scratch), `dayNightSegment` (compartilhado com `applyDayNightLighting`).
- `Scene3DLayer.tsx`: helpers `dayNightHourFor(cfg)` + `syncSceneBackground(scene,cfg)`; chamados só no effect de env/modo/cor, no restore de contexto (que também refaz `setClearColor(0,0)`) e via `applyDayNightLighting` (cfg mudou / 29 s). Nada por quadro.
- `opticalGlassWebGL.ts`: backdrop `kind:"canvas"` sobe com `UNPACK_PREMULTIPLY_ALPHA_WEBGL=1` (reset logo após) → sem halo nas bordas/blur; shaders só leem `.rgb`.
- Pendente p/ E1-L3: `Scene3DLayer` ainda lê campos `@deprecated`; Rust não espelha `backgroundMode` (motor já entrega RGBA pré-mult.).
- Testar em máquina: 3D TRANSPARENT sobre fundo/vidro sem halo; ENVIRONMENT+autoDayNight; perda/restore de contexto.
