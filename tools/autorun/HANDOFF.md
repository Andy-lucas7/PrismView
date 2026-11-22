E3-L2 OK (gate rust: 0 erros novos; 11 pré-existentes ignorados).
- Motor já tinha SWING/MOVE_OFFSET (`bindings/scene3d.rs` `Target3d::RotateSwing/MoveOffset`, `apply_scalar`, `apply_click_pulse`, `add_axis_rotation/offset`; `AdditiveLayer` begin→acumula→`commit_additive`→`refresh_pose` restaura/reamostra→`apply`). Fórmulas/defaults (25°, 0.15, alpha 1-exp(-clamp(s,0.02,0.45)·60·dt), snap 0.002, pulso exp(-max(2,s·45)·dt)) já 1:1 com TS.
- Fix `BindingSolver3d::resolve`: escopo por `objectId` = `bindingOwnerId` do TS (id existente → esse objeto; vazio/desconhecido → `objects[0]`; dono sem modelo = sem alvo até `invalidate`). Antes caía no 1º objeto carregado (aplicava no objeto errado).
- `native_scene.rs`: bindings 3D passam crus dentro de `scene3d` (sobrevivem à migração v2→v3); teste `project_to_scene` agora cobre SWING (`objectId/axis/invertAxis/strength`). Doc de `config.rs` atualizado.
- Divergências numéricas possíveis TS×Rust (testar em máquina):
  1. Unidade do MOVE: TS `nodeUnitScale` = maxAbs(escala mundo holder)/maxAbs(escala mundo pai), em cache (pose do bind); Rust `display_to_node_units` = 1/(|x_axis normalize|·|x_axis mundo do pai|) com a pose animada atual → difere com escala não uniforme ou pai com escala animada.
  2. Limiar de ângulo/offset nulo: TS ignora |v|<1e-6, Rust só ==0 (efeito desprezível).
  3. `strength`/`smoothing` 0: ambos tratam como padrão (`||` no TS, `or_default`/`spring_alpha` no Rust).
- Nada compilado em runtime (`cargo test` não rodado); validar em máquina com modelo esquelético.
