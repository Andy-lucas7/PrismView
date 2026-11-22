model: opus
effort: medium
gate: rust
---
# E1-L7 — Espelhar Etapa 1 no motor nativo

Arquivos: `src-tauri/src/native_scene.rs` (`project_to_scene`, SCENE_3D), `crates/prismview-engine/src/core/scene3d/config.rs` e `graph.rs`, `renderer.rs`/`instance.rs` (como o 3D é montado), `core/scene2d.rs` (`draw_scene3d`, `scene3d_config`). Leia CLAUDE.md (seção motor) antes.

Tarefas:
1. `project_to_scene` emite `objects[]` (config camelCase, caminhos `asset.localhost` decodificados) e `backgroundMode`/`backgroundColor`. Mantenha compat com cena antiga de 1 modelo.
2. `config.rs` ganha os campos novos com `#[serde(default)]`.
3. Renderer: instanciar N objetos (cada um com `ModelRig`/instância, transform YXZ + spin por objeto, normalização 2,4 por objeto); diff por `id` no `refresh_scene3d` (não recarregar o que não mudou); `visible` respeitado.
4. `backgroundMode`: TRANSPARENT → saída RGBA com alpha 0 fora dos modelos (já pré-multiplicado); SOLID/ENVIRONMENT → fundo correspondente no passe PBR. IBL segue ligado.
5. Sem alocação por quadro; sem `unwrap/expect` no loop de render.
6. Se algo do contrato não puder ser feito sem compilar/testar, deixe o código completo mas registre no HANDOFF o que precisa de validação em máquina.

Aceite: o gate rust não acusa erros novos.
Proibido: bindings novos (E3), câmera persistida (E2-L4).
