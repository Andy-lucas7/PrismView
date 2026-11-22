model: sonnet
effort: high
gate: ts
---
# E3-L5 — Editor das 8 ações (alvo, eixo, força, spring, inversão)

Arquivos: `src/components/StudioInspector.tsx` (seção de bindings 3D existente), `src/types/prism.ts`.

Tarefas:
1. Editor de binding 3D cobre as 8 ações: NODE_LOOK_AT, NODE_ROTATE_SWING, NODE_MOVE_OFFSET, NODE_ROTATE_SPIN, NODE_SCALE_PULSE, MORPH_WEIGHT, EMISSIVE_GLOW, PLAY_CLIP. Rótulos em PT-BR com exemplo curto.
2. Campos: objeto (select de `objects[]`), alvo (select/busca a partir da árvore do E3-L3, só tipos válidos por ação: morph só para MORPH_WEIGHT, material para EMISSIVE_GLOW, clip para PLAY_CLIP), fonte do sinal, eixo X/Y/Z (só onde se aplica), força, suavidade (spring), inversão.
3. Validação: binding com `objectId`/`targetName` inexistente aparece marcado como inválido, sem quebrar o render.
4. Patches imutáveis + undo/redo; reutilizar componentes de input existentes.

Aceite: o gate ts passa; criar/editar/remover bindings dos 8 tipos reflete no preview.
Proibido: runtime/motor.
