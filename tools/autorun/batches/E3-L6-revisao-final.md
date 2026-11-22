model: opus
effort: medium
gate: both
---
# E3-L6 — Revisão final (só correções, sem features novas)

Escopo: `git diff main...HEAD --stat` primeiro; leia só os arquivos alterados, em janelas.

Checklist (corrija cada falha com patch pontual):
1. Alocação por quadro nos loops novos (câmera livre, bindings swing/move, pick/hover).
2. `dispose()` ausente: gizmo, grade, SkeletonHelper, BoxHelper, runtimes de objetos removidos, `mixer.uncacheRoot`; textura de `dynamicTexCache` nunca disposta.
3. Listeners/handlers do modo ROOM removidos no cleanup; nada de `three-editor` importado em `desktop.tsx` nem nos caminhos do widget.
4. Migração de projeto antigo e pacote `.prism` antigo continuam abrindo.
5. Paridade TS ↔ Rust nas fórmulas de swing/move e no contrato de `objects[]`/`camera`/`backgroundMode`.
6. `powerCaps` respeitado nos loops novos.
7. Atualize o `CLAUDE.md` (Frontend/Motor) com 5–8 linhas sobre `objects[]`, modo ROOM, `backgroundMode` e novos alvos.

Aceite: o gate (both) sem erros novos. HANDOFF final lista pendências que exigem teste em máquina (Windows, GPU).
