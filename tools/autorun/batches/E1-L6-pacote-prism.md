model: sonnet
effort: high
gate: both
---
# E1-L6 — Empacotamento .prism com todos os .glb da sala

Arquivos: `apps/prismview-studio/src-tauri/src/lib.rs` (grep: `export_prism_archive`, `export_prism_archive_sync`, `import_prism_archive`, `modelSrc`, `allow_project_media`, `load_project_from_disk`).

Tarefas:
1. Exportação: percorrer `scene3d.objects[*].modelSrc` (e `dynamicTextures` com arquivo, se houver), copiar cada arquivo para o zip com nome estável (hash/uuid, sem colisão), reescrever o caminho no JSON do projeto exportado.
2. Importação: extrair todos para `%APPDATA%\PrismView\imports|assets`, reescrever `objects[*].modelSrc` para o caminho extraído, liberar escopo (`ASSET_SCOPE.allow_file`).
3. Compat: pacote antigo com `modelSrc` legado continua importando (a migração do TS cuida de `objects[0]`).
4. `list_projects`/`allow_project_media` passam a liberar todos os `.glb` dos objetos.
5. Sem unwrap/expect em caminho de erro novo; erros viram `Result` para a UI.

Aceite: o gate (both) passa; export→import de projeto com 2 `.glb` preserva ambos.
Proibido: tocar no crate do motor.
