model: opus
effort: medium
gate: rust
---
# E3-L2 — Swing/Move no motor nativo

Arquivos: `crates/prismview-engine/src/core/bindings/scene3d.rs` (`BindingSolver3d`), `core/scene3d/animation.rs` (`AdditiveLayer`, `refresh_pose`), `core/scene3d/config.rs`, `src-tauri/src/native_scene.rs`.

Tarefas:
1. Novos alvos `NODE_ROTATE_SWING` / `NODE_MOVE_OFFSET` no contrato (`ReactiveBinding3D`, camelCase, `axis/strength/smoothing/invert`, `objectId`), 1:1 com o TS de E3-L1 (mesma fórmula e mesma suavização).
2. Aplicar via `AdditiveLayer` refeita por quadro sobre a pose amostrada (`refresh_pose` → `apply`), sem sobrescrever base.
3. Resolução de alvo por nome só no load/`invalidate`, escopada por `objectId`.
4. `project_to_scene` repassa os campos novos.
5. Sem alocação por quadro, sem `unwrap/expect`.

Aceite: o gate rust sem erros novos. Registre no HANDOFF qualquer divergência numérica possível entre TS e Rust.
Proibido: UI.
