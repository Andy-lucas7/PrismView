E1-L5 OK (gate ts: 0 erros novos). Só `StudioInspector.tsx` mudou; o resto da UI de objetos (lista, +.glb via `pick_local_file`, undo) já existia de lotes anteriores.
- Novo `Vec3Field` (inputs numéricos X/Y/Z, commit em blur/Enter, `onRecordStep`+`onCommit`+`onFlushSave`) substitui os 7 sliders de posição/rotação/escala; escala agora por eixo (min 0.01).
- Duplicar/remover objeto no outliner usam `duplicateScene3DObject`/`removeScene3DObject` (religa filhos a raiz, apaga bindings do objeto).
- `patchObjects` não grava mais o campo legado `cfg.modelSrc`.
- Fundo: 3 botões TRANSPARENT/SOLID/ENVIRONMENT (antes faltava ENVIRONMENT) + cor para SOLID.
- "+ Escultura" nasce com `autoRotateSpeed` 0 (era 0.22).
- Não revogamos `blob:` ao excluir objeto (quebraria undo; só existe no fallback sem Tauri; limpeza geral em `blobUrlsRef`).
- Resíduo: `tools/autorun/_splice_l5.mjs` (script não usado, pode apagar).
- Pendente: `parentId` no runtime/outliner (UI sem hierarquia); motor Rust não lê `objectId`.
