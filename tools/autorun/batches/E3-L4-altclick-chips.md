model: sonnet
effort: high
gate: ts
---
# E3-L4 — Alt+Clique em peça + chips de auto-sugestão

Arquivos: `src/components/Scene3DLayer.tsx` (raycast do modo ROOM), `src/components/StudioInspector.tsx`, `src/engine/scene3d/modelIndex.ts`.

Tarefas:
1. Alt+Clique no viewport (modo ROOM) seleciona a peça/osso sob o cursor (raycast até a malha, subir até o nó nomeado mais útil: osso se skinned, senão grupo/malha), destaca e mostra o atalho "+ Interagir com esta parte" (cria um binding com `objectId` + `targetName`).
2. Auto-detecção por nome (case-insensitive, sem alocar por quadro; só ao carregar o modelo): Head/Neck/Eye/Jaw/Wing/Fan/Wheel/Speaker/Screen → chips de 1 clique: Head/Neck/Eye → NODE_LOOK_AT no Mouse; Jaw → MORPH/ROTATE_SWING no Médio; Wing → SWING no Grave; Fan/Wheel → NODE_ROTATE_SPIN na CPU/Mídia; Speaker → NODE_SCALE_PULSE ou MOVE_OFFSET no Grave; Screen → textura dinâmica de capa.
3. Chip cria o binding com defaults sensatos (eixo, força, suavização) e seleciona-o no editor de ações.
4. Alt+Clique não pode conflitar com o pan por Alt+arrastar da câmera: só dispara clique sem movimento (limiar de px).

Aceite: o gate ts passa; um `.glb` com `Head`/`Speaker` mostra os chips corretos.
Proibido: UI completa de ações (E3-L5).
