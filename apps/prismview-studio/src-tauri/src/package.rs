//! Pacote .prism: exportar (zip atômico), importar (conflito de id, OPEN_APP em revisão) e seletores de arquivo.

#[allow(unused_imports)]
use super::*;

pub(crate) fn extract_import_uuid_from_path(raw: &str, imports_prefix_lower: &str) -> Option<String> {
    let norm = raw.replace('/', "\\");
    let lower = norm.to_ascii_lowercase();
    let rest = lower.strip_prefix(imports_prefix_lower)?;
    let offset = norm.len() - rest.len();
    let seg = norm[offset..].split('\\').next()?.trim();
    if seg.is_empty() || seg.contains("..") {
        None
    } else {
        Some(seg.to_string())
    }
}

pub(crate) fn collect_dynamic_tex_import_uuids(
    dtex_val: Option<&Value>,
    imports_prefix_lower: &str,
    out: &mut HashSet<String>,
) {
    if let Some(dtex) = dtex_val.and_then(|v| v.as_array()) {
        for item in dtex {
            if let Some(src) = item.get("customSrc").and_then(|v| v.as_str()) {
                if let Some(uuid) = extract_import_uuid_from_path(src, imports_prefix_lower) {
                    out.insert(uuid);
                }
            }
        }
    }
}

pub(crate) fn collect_layer_import_uuids(
    layers: &[Value],
    imports_prefix_lower: &str,
    out: &mut HashSet<String>,
) {
    for layer in layers {
        for (obj_key, field_key) in [
            ("video", "src"),
            ("image", "src"),
            ("text", "fontPath"),
            ("scene3d", "modelSrc"),
        ] {
            if let Some(src) = layer
                .get(obj_key)
                .and_then(|o| o.get(field_key))
                .and_then(|v| v.as_str())
            {
                if let Some(uuid) = extract_import_uuid_from_path(src, imports_prefix_lower) {
                    out.insert(uuid);
                }
            }
        }
        collect_dynamic_tex_import_uuids(
            layer.pointer("/scene3d/dynamicTextures"),
            imports_prefix_lower,
            out,
        );
        collect_dynamic_tex_import_uuids(layer.pointer("/shader/textures"), imports_prefix_lower, out);
        if let Some(objs) = layer.pointer("/scene3d/objects").and_then(|v| v.as_array()) {
            for obj in objs {
                if let Some(src) = obj.get("modelSrc").and_then(|v| v.as_str()) {
                    if let Some(uuid) = extract_import_uuid_from_path(src, imports_prefix_lower) {
                        out.insert(uuid);
                    }
                }
                collect_dynamic_tex_import_uuids(
                    obj.get("dynamicTextures"),
                    imports_prefix_lower,
                    out,
                );
            }
        }
        if let Some(children) = layer.get("children").and_then(|v| v.as_array()) {
            collect_layer_import_uuids(children, imports_prefix_lower, out);
        }
    }
}

pub(crate) fn collect_project_import_uuids(
    project: &Value,
    imports_prefix_lower: &str,
    out: &mut HashSet<String>,
) {
    if let Some(dir) = project.get("importDir").and_then(|v| v.as_str()) {
        let clean = dir.trim();
        if !clean.is_empty() && !clean.contains("..") && !clean.contains('\\') && !clean.contains('/')
        {
            out.insert(clean.to_string());
        }
    }
    if let Some(layers) = project.get("layers").and_then(|v| v.as_array()) {
        collect_layer_import_uuids(layers, imports_prefix_lower, out);
    }
}

pub(crate) fn cleanup_unreferenced_import_uuids(candidate_uuids: &HashSet<String>) {
    if candidate_uuids.is_empty() {
        return;
    }
    let imports_root = prismview_root_dir().join("imports");
    let mut imports_prefix_lower = imports_root
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    if !imports_prefix_lower.ends_with('\\') {
        imports_prefix_lower.push('\\');
    }
    let mut still_referenced = HashSet::new();
    let projects_dir = prismview_root_dir().join("projects");
    if let Ok(entries) = fs::read_dir(&projects_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(raw) = fs::read_to_string(&path) {
                    if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                        collect_project_import_uuids(
                            &val,
                            &imports_prefix_lower,
                            &mut still_referenced,
                        );
                    }
                }
            }
        }
    }
    for uuid in candidate_uuids {
        if !still_referenced.contains(uuid) {
            let dir = imports_root.join(uuid);
            if dir.is_dir() {
                atomic_fs::log_io("remove_dir_all", fs::remove_dir_all(&dir));
            }
        }
    }
}

pub(crate) fn bundle_path_into_zip(
    src_val: &str,
    folder: &str,
    fallback_name: &str,
    zip: &mut ZipWriter<File>,
    opts: SimpleFileOptions,
    bundled: &mut HashMap<String, String>,
) -> Result<Option<String>, String> {
    if src_val.is_empty()
        || src_val.starts_with("http://")
        || src_val.starts_with("https://")
        || src_val.starts_with("data:")
    {
        return Ok(None);
    }
    let src_path = Path::new(src_val);
    if !src_path.is_file() {
        return Ok(None);
    }
    if let Some(existing) = bundled.get(src_val) {
        return Ok(Some(existing.clone()));
    }
    let fname = src_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(fallback_name);
    let unique = format!("{folder}/{}_{}", bundled.len(), fname);
    let ext = src_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let file_opts = if matches!(
        ext.as_str(),
        "mp4" | "webm" | "mkv" | "mov" | "jpg" | "jpeg" | "png" | "webp" | "woff2" | "glb" | "ktx2"
    ) {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    } else {
        opts
    };
    if let Ok(mut f) = File::open(src_path) {
        zip.start_file(&unique, file_opts)
            .map_err(|e| format!("Falha ao iniciar entrada ZIP ({unique}): {e}"))?;
        std::io::copy(&mut f, zip)
            .map_err(|e| format!("Falha ao empacotar mídia ({unique}): {e}"))?;
        bundled.insert(src_val.to_string(), unique.clone());
        Ok(Some(unique))
    } else {
        Ok(None)
    }
}

pub(crate) fn bundle_dynamic_textures_array(
    dtex_val: Option<&mut Value>,
    zip: &mut ZipWriter<File>,
    opts: SimpleFileOptions,
    bundled: &mut HashMap<String, String>,
) -> Result<(), String> {
    if let Some(dtex) = dtex_val.and_then(|v| v.as_array_mut()) {
        for item in dtex.iter_mut() {
            let src_val = item
                .get("customSrc")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(rel_entry) =
                bundle_path_into_zip(&src_val, "assets", "tex.png", zip, opts, bundled)?
            {
                if let Some(obj) = item.as_object_mut() {
                    obj.insert("customSrc".to_string(), Value::String(rel_entry));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn bundle_layer_files(
    layers: &mut [Value],
    zip: &mut ZipWriter<File>,
    opts: SimpleFileOptions,
    bundled: &mut HashMap<String, String>,
) -> Result<(), String> {
    for layer in layers.iter_mut() {
        let pairs: &[(&str, &str, &str)] = &[
            ("video", "src", "assets"),
            ("image", "src", "assets"),
            ("text", "fontPath", "fonts"),
            ("scene3d", "modelSrc", "assets"),
        ];
        for (obj_key, field_key, folder) in pairs {
            let src_val = layer
                .get(*obj_key)
                .and_then(|o| o.get(*field_key))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(rel_entry) =
                bundle_path_into_zip(&src_val, folder, "file.bin", zip, opts, bundled)?
            {
                if let Some(slot) = layer
                    .get_mut(*obj_key)
                    .and_then(|o| o.as_object_mut())
                {
                    slot.insert((*field_key).to_string(), Value::String(rel_entry));
                }
            }
        }

        bundle_dynamic_textures_array(
            layer.pointer_mut("/scene3d/dynamicTextures"),
            zip,
            opts,
            bundled,
        )?;
        // Modo Espectro: imagens das declarações `//@texture` vão junto no `.prism`.
        bundle_dynamic_textures_array(layer.pointer_mut("/shader/textures"), zip, opts, bundled)?;

        if let Some(objs) = layer
            .pointer_mut("/scene3d/objects")
            .and_then(|v| v.as_array_mut())
        {
            for obj in objs.iter_mut() {
                let src_val = obj
                    .get("modelSrc")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(rel_entry) =
                    bundle_path_into_zip(&src_val, "assets", "model.glb", zip, opts, bundled)?
                {
                    if let Some(obj_map) = obj.as_object_mut() {
                        obj_map.insert("modelSrc".to_string(), Value::String(rel_entry));
                    }
                }
                bundle_dynamic_textures_array(
                    obj.get_mut("dynamicTextures"),
                    zip,
                    opts,
                    bundled,
                )?;
            }
        }

        if let Some(children) = layer.get_mut("children").and_then(|v| v.as_array_mut()) {
            bundle_layer_files(children, zip, opts, bundled)?;
        }
    }
    Ok(())
}

pub(crate) fn safe_relative_subpath(rel: &str) -> Option<PathBuf> {
    let normalized = rel.replace('\\', "/");
    if !(normalized.starts_with("assets/") || normalized.starts_with("fonts/")) {
        return None;
    }
    let mut clean = PathBuf::new();
    for comp in Path::new(&normalized).components() {
        match comp {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if clean.as_os_str().is_empty() {
        None
    } else {
        Some(clean)
    }
}

pub(crate) fn resolve_imported_asset_path(rel: &str, extract_root: &Path, canonical_root: &Path) -> Option<String> {
    let safe_rel = safe_relative_subpath(rel)?;
    let full = extract_root.join(&safe_rel);
    if !full.is_file() {
        return None;
    }
    let canon_file = fs::canonicalize(&full).ok()?;
    if !canon_file.starts_with(canonical_root) {
        return None;
    }
    Some(full.to_string_lossy().to_string())
}

pub(crate) fn remap_dynamic_textures_array(
    dtex_val: Option<&mut Value>,
    extract_root: &Path,
    canonical_root: &Path,
) {
    if let Some(dtex) = dtex_val.and_then(|v| v.as_array_mut()) {
        for item in dtex.iter_mut() {
            let rel = item
                .get("customSrc")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(mapped) = resolve_imported_asset_path(&rel, extract_root, canonical_root) {
                if let Some(obj) = item.as_object_mut() {
                    obj.insert("customSrc".to_string(), Value::String(mapped));
                }
            }
        }
    }
}

pub(crate) fn remap_imported_layers(layers: &mut [Value], extract_root: &Path) {
    let canonical_root = fs::canonicalize(extract_root).unwrap_or_else(|_| extract_root.to_path_buf());
    for layer in layers.iter_mut() {
        for (obj_key, field_key) in [
            ("video", "src"),
            ("image", "src"),
            ("text", "fontPath"),
            ("scene3d", "modelSrc"),
        ] {
            let rel = layer
                .get(obj_key)
                .and_then(|o| o.get(field_key))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(mapped) = resolve_imported_asset_path(&rel, extract_root, &canonical_root) {
                if let Some(slot) = layer.get_mut(obj_key).and_then(|o| o.as_object_mut()) {
                    slot.insert(field_key.to_string(), Value::String(mapped));
                }
            }
        }
        remap_dynamic_textures_array(
            layer.pointer_mut("/scene3d/dynamicTextures"),
            extract_root,
            &canonical_root,
        );
        remap_dynamic_textures_array(layer.pointer_mut("/shader/textures"), extract_root, &canonical_root);
        if let Some(objs) = layer
            .pointer_mut("/scene3d/objects")
            .and_then(|v| v.as_array_mut())
        {
            for obj in objs.iter_mut() {
                let rel = obj
                    .get("modelSrc")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(mapped) =
                    resolve_imported_asset_path(&rel, extract_root, &canonical_root)
                {
                    if let Some(obj_map) = obj.as_object_mut() {
                        obj_map.insert("modelSrc".to_string(), Value::String(mapped));
                    }
                }
                remap_dynamic_textures_array(
                    obj.get_mut("dynamicTextures"),
                    extract_root,
                    &canonical_root,
                );
            }
        }
        if let Some(children) = layer.get_mut("children").and_then(|v| v.as_array_mut()) {
            remap_imported_layers(children, extract_root);
        }
    }
}

#[tauri::command]
pub(crate) async fn pick_save_archive_path(
    default_file_name: String,
    is_widget: bool,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _com = desktop_host::ComApartmentGuard::init_sta();
        let filter_str = if is_widget {
            "Pacote Widget PrismView (*.widget.prism)\0*.widget.prism\0Todos os arquivos (*.*)\0*.*\0\0"
        } else {
            "Pacote Wallpaper PrismView (*.prism)\0*.prism\0Todos os arquivos (*.*)\0*.*\0\0"
        };
        let filter_w: Vec<u16> = filter_str.encode_utf16().collect();
        let def_ext_w: Vec<u16> = "prism\0".encode_utf16().collect();
        let mut file_buf = vec![0u16; 4096];
        let clean_default = default_file_name
            .trim()
            .replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");
        let default_w: Vec<u16> = clean_default.encode_utf16().collect();
        let copy_len = default_w.len().min(file_buf.len().saturating_sub(1));
        file_buf[..copy_len].copy_from_slice(&default_w[..copy_len]);

        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: HWND::default(),
            lpstrFilter: PCWSTR(filter_w.as_ptr()),
            lpstrFile: PWSTR(file_buf.as_mut_ptr()),
            nMaxFile: file_buf.len() as u32,
            lpstrDefExt: PCWSTR(def_ext_w.as_ptr()),
            Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };

        let ok = unsafe { GetSaveFileNameW(&mut ofn).as_bool() };
        if !ok {
            return Ok(None);
        }
        let end = file_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(file_buf.len());
        let mut path_str = String::from_utf16_lossy(&file_buf[..end]).trim().to_string();
        if path_str.is_empty() {
            return Ok(None);
        }
        let lower = path_str.to_ascii_lowercase();
        if is_widget {
            if !lower.ends_with(".widget.prism") {
                if lower.ends_with(".prism") {
                    let stem = &path_str[..path_str.len() - 6];
                    path_str = format!("{stem}.widget.prism");
                } else {
                    path_str.push_str(".widget.prism");
                }
            }
        } else if !lower.ends_with(".prism") {
            path_str.push_str(".prism");
        }
        Ok(Some(path_str))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Exporta o `.prism` otimizado (mídias empacotadas). Síncrona: usada pelo comando e pelo
/// fluxo "Aplicar no Desktop" do motor nativo (`native_engine`).
pub(crate) fn export_prism_archive_sync(
    mut project: Value,
    dest_path: Option<String>,
) -> Result<ExportPackageResult, String> {
    let is_widget = project
        .get("packageType")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("WIDGET"))
        .unwrap_or(false);
    let title = project
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("PrismView_Project")
        .to_string();
    let ext = if is_widget { "widget.prism" } else { "prism" };
    let default_file_name = format!("{}.{ext}", sanitize_name(&title));
    let dest = match dest_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(custom) => {
            let p = PathBuf::from(custom);
            if let Some(parent) = p.parent() {
                if !parent.as_os_str().is_empty() {
                    atomic_fs::log_io("create_dir_all", fs::create_dir_all(parent));
                }
            }
            p
        }
        None => prismview_root_dir().join("packages").join(&default_file_name),
    };
    let file_name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or(default_file_name);

    // Grava num temporário ao lado e só troca no fim: o `.prism` anterior fica íntegro se o
    // Studio fechar no meio do zip.
    let tmp = atomic_fs::temp_path_for(&dest);
    match write_prism_zip(&tmp, &mut project, is_widget, &title) {
        Ok(()) => atomic_fs::commit_temp(&tmp, &dest)?,
        Err(e) => {
            atomic_fs::discard_temp(&tmp);
            return Err(e);
        }
    }

    Ok(ExportPackageResult {
        archive_path: dest.to_string_lossy().to_string(),
        file_name,
    })
}

pub(crate) fn write_prism_zip(
    path: &Path,
    project: &mut Value,
    is_widget: bool,
    title: &str,
) -> Result<(), String> {
    let file = File::create(path).map_err(|e| e.to_string())?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut bundled = HashMap::new();
    if let Some(layers) = project.get_mut("layers").and_then(|v| v.as_array_mut()) {
        bundle_layer_files(layers, &mut zip, opts, &mut bundled)?;
    }

    let manifest = serde_json::json!({
        "version": 3,
        "packageType": if is_widget { "WIDGET" } else { "WALLPAPER" },
        "isWidget": is_widget,
        "title": title,
        "bundledFiles": bundled.len(),
    });
    zip.start_file("manifest.json", opts)
        .map_err(|e| e.to_string())?;
    zip.write_all(
        serde_json::to_string_pretty(&manifest)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())?;

    let payload = serde_json::to_string_pretty(&*project).map_err(|e| e.to_string())?;
    zip.start_file("project.json", opts)
        .map_err(|e| e.to_string())?;
    zip.write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    if is_widget {
        zip.start_file("widget.json", opts)
            .map_err(|e| e.to_string())?;
        zip.write_all(payload.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let file = zip.finish().map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn export_prism_archive(
    project: Value,
    dest_path: Option<String>,
) -> Result<ExportPackageResult, String> {
    tauri::async_runtime::spawn_blocking(move || export_prism_archive_sync(project, dest_path))
        .await
        .map_err(|e| e.to_string())?
}

pub(crate) const MAX_ZIP_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) const MAX_ZIP_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

pub(crate) const MAX_PROJECT_JSON_BYTES: u64 = 8 * 1024 * 1024;

/// Desativa `clickAction == "OPEN_APP"` vindas de um pacote: o alvo vai para
/// `clickTargetPending` e a camada fica marcada com `clickTargetNeedsReview` até o usuário
/// permitir no diálogo pós-import. Devolve quantas camadas foram marcadas.
pub(crate) fn quarantine_open_app_actions(layers: &mut [Value]) -> usize {
    let mut flagged = 0;
    for layer in layers.iter_mut() {
        if let Some(obj) = layer.as_object_mut() {
            if obj.get("clickAction").and_then(|v| v.as_str()) == Some("OPEN_APP") {
                let target = obj
                    .get("clickTarget")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                obj.insert("clickTarget".to_string(), Value::String(String::new()));
                obj.insert("clickTargetPending".to_string(), Value::String(target));
                obj.insert("clickTargetNeedsReview".to_string(), Value::Bool(true));
                flagged += 1;
            }
            if let Some(children) = obj.get_mut("children").and_then(|v| v.as_array_mut()) {
                flagged += quarantine_open_app_actions(children);
            }
        }
    }
    flagged
}

#[tauri::command]
pub(crate) async fn import_prism_archive(
    archive_path: String,
    replace: Option<bool>,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = Path::new(&archive_path);
        let file = File::open(p).map_err(|e| e.to_string())?;
        let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;

        // Validate & parse project.json BEFORE extracting any media files
        let mut project_json: Option<String> = None;
        for i in 0..archive.len() {
            let Ok(entry) = archive.by_index(i) else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            let rel = entry.name().replace('\\', "/");
            if matches!(
                rel.as_str(),
                "project.json" | "widget.json" | "scene.json" | "theme.json"
            ) && (project_json.is_none() || rel == "project.json")
            {
                let mut raw = String::new();
                let mut limited = entry.take(MAX_PROJECT_JSON_BYTES);
                if limited.read_to_string(&mut raw).is_ok() {
                    project_json = Some(raw);
                }
            }
        }

        let raw = project_json.ok_or("Pacote .prism inválido: project.json ausente")?;
        let mut val: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let proj_id = val
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or("Pacote .prism inválido: id do projeto ausente")?
            .to_string();

        // Mesmo id já existe: nunca sobrescrever sem o usuário escolher. `replace` ausente →
        // o front recebe o conflito e pergunta; `false` → importa como cópia com id novo.
        let proj_id = if project_path(&proj_id).is_file() {
            match replace {
                None => {
                    let existing_title = load_project_from_disk(&proj_id)
                        .and_then(|v| v.get("title").and_then(|t| t.as_str()).map(str::to_string))
                        .unwrap_or_default();
                    return Ok(serde_json::json!({
                        "conflict": true,
                        "existingId": proj_id,
                        "existingTitle": existing_title,
                    }));
                }
                Some(true) => proj_id,
                Some(false) => {
                    let new_id = Uuid::new_v4().to_string();
                    if let Some(obj) = val.as_object_mut() {
                        let title = obj
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("Projeto")
                            .to_string();
                        obj.insert("id".to_string(), Value::String(new_id.clone()));
                        obj.insert(
                            "title".to_string(),
                            Value::String(format!("{title} (importado)")),
                        );
                    }
                    new_id
                }
            }
        } else {
            proj_id
        };

        // Ações "abrir programa" de pacotes de terceiros entram desativadas até o usuário revisar.
        if let Some(layers) = val.get_mut("layers").and_then(|v| v.as_array_mut()) {
            quarantine_open_app_actions(layers);
        }

        let import_uuid = Uuid::new_v4().to_string();
        let extract_dir = prismview_root_dir().join("imports").join(&import_uuid);
        let extract_res = (|| -> Result<(), String> {
            fs::create_dir_all(extract_dir.join("assets")).map_err(|e| e.to_string())?;
            fs::create_dir_all(extract_dir.join("fonts")).map_err(|e| e.to_string())?;

            let mut total_written: u64 = 0;
            for i in 0..archive.len() {
                let Ok(entry) = archive.by_index(i) else {
                    continue;
                };
                if entry.is_dir() {
                    continue;
                }
                let Some(safe_rel) = safe_relative_subpath(entry.name()) else {
                    continue;
                };
                let dest = extract_dir.join(&safe_rel);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut out_file = File::create(&dest).map_err(|e| e.to_string())?;
                let remaining = MAX_ZIP_TOTAL_BYTES.saturating_sub(total_written);
                let cap = remaining.min(MAX_ZIP_ENTRY_BYTES);
                let mut limited = entry.take(cap.saturating_add(1));
                let copied =
                    std::io::copy(&mut limited, &mut out_file).map_err(|e| e.to_string())?;
                if copied > cap {
                    return Err(
                        "Pacote .prism excede o limite máximo permitido de extração".to_string(),
                    );
                }
                total_written = total_written.saturating_add(copied);
            }
            Ok(())
        })();

        if let Err(err) = extract_res {
            atomic_fs::log_io("remove_dir_all", fs::remove_dir_all(&extract_dir));
            return Err(err);
        }

        if let Some(obj) = val.as_object_mut() {
            obj.insert("importDir".to_string(), Value::String(import_uuid));
            if archive_path.to_ascii_lowercase().contains(".widget.") {
                obj.insert(
                    "packageType".to_string(),
                    Value::String("WIDGET".to_string()),
                );
            }
        }
        if let Some(layers) = val.get_mut("layers").and_then(|v| v.as_array_mut()) {
            remap_imported_layers(layers, &extract_dir);
        }

        // Collect old import UUIDs if overwriting an existing project ID
        let imports_root = prismview_root_dir().join("imports");
        let mut imports_prefix_lower = imports_root
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        if !imports_prefix_lower.ends_with('\\') {
            imports_prefix_lower.push('\\');
        }
        let mut old_uuids = HashSet::new();
        if let Some(existing) = load_project_from_disk(&proj_id) {
            collect_project_import_uuids(&existing, &imports_prefix_lower, &mut old_uuids);
        }

        match save_project_inner(val) {
            Ok(saved) => {
                // Only remove old import folders if NO other project in projects/ references them
                cleanup_unreferenced_import_uuids(&old_uuids);
                // Libera no asset protocol todos os arquivos extraídos (.glb de cada objeto 3D etc.).
                allow_project_media(&saved);
                Ok(saved)
            }
            Err(err) => {
                atomic_fs::log_io("remove_dir_all", fs::remove_dir_all(&extract_dir));
                Err(err)
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn pick_local_file(kind: Option<String>) -> Result<Option<PickedFileDto>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Initialize STA COM apartment on this worker thread and pass null hwndOwner
        // to avoid cross-thread AttachThreadInput deadlocks with the main WebView2 UI thread.
        let _com = desktop_host::ComApartmentGuard::init_sta();
        let filter_str = match kind.as_deref() {
            Some("video") => "Vídeos (*.mp4;*.webm;*.mkv;*.mov)\0*.mp4;*.webm;*.mkv;*.mov\0\0",
            Some("image") => "Imagens (*.png;*.jpg;*.jpeg;*.webp)\0*.png;*.jpg;*.jpeg;*.webp\0\0",
            Some("svg") => "Vetores SVG (*.svg)\0*.svg\0\0",
            // Sem woff/woff2: o motor nativo (desktop) não decodifica WOFF2.
            Some("font") => "Fontes (*.ttf;*.otf)\0*.ttf;*.otf\0\0",
            Some("model3d") => "Modelos 3D Blender / glTF (*.glb;*.gltf)\0*.glb;*.gltf\0\0",
            Some("prism") => "Pacotes PrismView (*.prism;*.widget.prism)\0*.prism\0\0",
            _ => "Mídia e Modelos 3D (*.glb;*.gltf;*.mp4;*.webm;*.png;*.jpg;*.webp;*.svg)\0*.glb;*.gltf;*.mp4;*.webm;*.mkv;*.mov;*.png;*.jpg;*.jpeg;*.webp;*.svg\0\0",
        };
        let filter_w: Vec<u16> = filter_str.encode_utf16().collect();
        let mut file_buf = vec![0u16; 4096];
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: HWND::default(),
            lpstrFilter: PCWSTR(filter_w.as_ptr()),
            lpstrFile: PWSTR(file_buf.as_mut_ptr()),
            nMaxFile: file_buf.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };

        let ok = unsafe { GetOpenFileNameW(&mut ofn).as_bool() };
        if !ok {
            return Ok(None);
        }
        let end = file_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(file_buf.len());
        let path_str = String::from_utf16_lossy(&file_buf[..end]);
        let p = PathBuf::from(&path_str);
        if !p.is_file() {
            return Ok(None);
        }
        allow_asset_file(&p);
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let is_video = matches!(ext.as_str(), "mp4" | "webm" | "mkv" | "mov");
        let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif");
        let is_svg = ext == "svg";
        let is_font = matches!(ext.as_str(), "ttf" | "otf" | "woff" | "woff2");
        let is_model_3d = matches!(ext.as_str(), "glb" | "gltf");
        let text_content = if is_svg {
            fs::read_to_string(&p).unwrap_or_default()
        } else {
            String::new()
        };
        let file_name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Arquivo")
            .to_string();

        Ok(Some(PickedFileDto {
            path: path_str,
            file_name,
            ext,
            is_video,
            is_image,
            is_svg,
            is_font,
            is_model_3d,
            text_content,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}
