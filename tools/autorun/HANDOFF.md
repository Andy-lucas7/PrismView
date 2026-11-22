E1-L6 OK (gate both: 0 erros novos). Só `src-tauri/src/lib.rs`.
- Já existiam (lotes anteriores): export `bundle_layer_files` percorre `scene3d.objects[*].modelSrc` + `dynamicTextures[*].customSrc` (+ legado `scene3d.modelSrc`), nome `assets/{n}_{arquivo}` único + dedup por caminho; import `remap_imported_layers` reescreve tudo; `collect_layer_import_uuids` cobre objetos.
- Novo: `import_prism_archive` chama `allow_project_media(&saved)` após salvar (libera todos os .glb extraídos no `ASSET_SCOPE`). `list_projects`/`load_project_from_disk` já varrem o JSON inteiro (liberam todos os objetos).
- Novo teste `export_bundles_every_scene3d_object_glb_and_remap_restores_both` (2 .glb de mesmo nome em pastas distintas + dedup; não roda no gate, precisa `cargo test`).
- Pacote antigo só com `scene3d.modelSrc` continua válido (migração TS cria `objects[0]`).
- Arquivo fonte ausente no export é ignorado silenciosamente (caminho absoluto fica no JSON).
- Resíduo: `tools/autorun/_splice_l5.mjs` pode ser apagado.
- Pendente (outros lotes): `parentId` runtime/outliner; motor Rust não lê `objectId`.
