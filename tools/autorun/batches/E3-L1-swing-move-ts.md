model: opus
effort: medium
gate: ts
---
# E3-L1 — NODE_ROTATE_SWING e NODE_MOVE_OFFSET (runtime TS, sem drift)

Arquivos: `src/types/prism.ts` (binding 3D), `src/components/Scene3DLayer.tsx` (grep: `NODE_SCALE_PULSE`, `NODE_ROTATE_SPIN`, `NODE_LOOK_AT`, `AnimationMixer`, `mixer.update`).

Tarefas:
1. Adicionar os dois `targetType` ao tipo e aos defaults/normalização. Campos do binding: `axis` X|Y|Z, `strength`, `smoothing` (spring), `invert`.
2. SWING: rotação proporcional ao sinal no eixo escolhido (graus = sinal × strength × (invert ? -1 : 1)), suavizada por spring. MOVE_OFFSET: deslocamento local no eixo (unidades do modelo normalizado), mesma suavização.
3. Drift-free: aplicar como delta aditivo DEPOIS do `mixer.update`, guardando a pose animada do quadro e refazendo por quadro; nunca acumular sobre a base. Verificar o mesmo padrão já usado por SPIN/LOOK_AT e reaproveitar.
4. Alvo resolvido por `objectId` + `targetName` (osso ou malha) no load/invalidate. Estado de spring em estruturas pré-alocadas.
5. Respeitar `powerCaps`; sem alocação por quadro.

Aceite: o gate ts passa; osso animado no Blender continua tocando enquanto o delta é aplicado, sem acúmulo ao longo do tempo.
Proibido: UI (E3-L5), motor Rust (E3-L2).
