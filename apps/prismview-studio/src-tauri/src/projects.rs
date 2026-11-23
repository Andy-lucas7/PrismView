//! Projetos: caminhos, leitura com recuperação, CRUD e escopo do asset protocol.

#[allow(unused_imports)]
use super::*;

pub(crate) fn project_path(project_id: &str) -> PathBuf {
    prismview_root_dir()
        .join("projects")
        .join(format!("{}.json", sanitize_name(project_id)))
}

/// Escopo do asset protocol (tauri.conf.json cobre só `%APPDATA%\PrismView`). Arquivos que o
/// usuário escolheu fora dessa pasta entram aqui, um a um, ao escolher/carregar o projeto.
pub(crate) static ASSET_SCOPE: OnceLock<tauri::scope::fs::Scope> = OnceLock::new();

pub(crate) fn allow_asset_file(path: &Path) {
    if let Some(scope) = ASSET_SCOPE.get() {
        let _ = scope.allow_file(path);
        // .gltf text files reference sibling .bin buffers and textures in the same directory.
        if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("gltf"))
        {
            if let Some(dir) = path.parent() {
                let _ = scope.allow_directory(dir, false);
            }
        }
    }
}

/// Extensões de mídia/fonte/modelo que podem entrar no escopo do asset protocol.
pub(crate) const ASSET_SCOPE_EXTENSIONS: &[&str] = &[
    "mp4", "webm", "mkv", "mov", "png", "jpg", "jpeg", "webp", "gif", "svg", "ttf", "otf",
    "woff", "woff2", "glb", "gltf", "hdr", "ktx2",
];

pub(crate) fn allow_media_path(raw: &str) {
    if raw.len() >= 1024 {
        return;
    }
    let p = Path::new(raw);
    let ext_ok = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ASSET_SCOPE_EXTENSIONS.iter().any(|ok| e.eq_ignore_ascii_case(ok)));
    if ext_ok && p.is_absolute() && p.is_file() {
        allow_asset_file(p);
    }
}

pub(crate) fn allow_dynamic_textures_media(list: Option<&Value>) {
    for item in list.and_then(|v| v.as_array()).into_iter().flatten() {
        if let Some(src) = item.get("customSrc").and_then(|v| v.as_str()) {
            allow_media_path(src);
        }
    }
}

pub(crate) fn allow_layers_media(layers: &[Value]) {
    for layer in layers {
        for ptr in [
            "/video/src",
            "/image/src",
            "/text/fontPath",
            "/scene3d/modelSrc",
            "/scene3d/envMapSrc",
        ] {
            if let Some(src) = layer.pointer(ptr).and_then(|v| v.as_str()) {
                allow_media_path(src);
            }
        }
        if let Some(svg) = layer.get("svg").and_then(|v| v.as_object()) {
            for (key, val) in svg {
                if key.ends_with("Src") {
                    if let Some(src) = val.as_str() {
                        allow_media_path(src);
                    }
                }
            }
        }
        allow_dynamic_textures_media(layer.pointer("/scene3d/dynamicTextures"));
        for obj in layer
            .pointer("/scene3d/objects")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(src) = obj.get("modelSrc").and_then(|v| v.as_str()) {
                allow_media_path(src);
            }
            allow_dynamic_textures_media(obj.get("dynamicTextures"));
        }
        if let Some(children) = layer.get("children").and_then(|v| v.as_array()) {
            allow_layers_media(children);
        }
    }
}

/// Libera no asset protocol só os campos de mídia conhecidos do projeto (vídeo, imagem, fonte,
/// modelos 3D, texturas, SVG) com extensões de mídia. Outras strings nunca entram no escopo.
pub(crate) fn allow_project_media(project: &Value) {
    if let Some(layers) = project.get("layers").and_then(|v| v.as_array()) {
        allow_layers_media(layers);
    }
}

pub(crate) fn projects_dir() -> PathBuf {
    prismview_root_dir().join("projects")
}

/// Destino de quarentena de um projeto ilegível: `projects/_corrompidos/<nome>-<ts>.json`.
pub(crate) fn project_quarantine_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("projeto");
    projects_dir()
        .join("_corrompidos")
        .join(format!("{stem}-{}.json", atomic_fs::unix_secs()))
}

#[derive(Serialize)]
pub(crate) struct BrokenProjectDto {
    pub(crate) file: String,
    pub(crate) error: String,
    /// `true` quando o `.bak` foi restaurado no lugar (o projeto aparece na lista).
    pub(crate) restored: bool,
}

/// Lê um projeto do disco com recuperação (quarentena + `.bak`). `Err` = não aproveitável.
pub(crate) fn read_project_file(path: &Path) -> Result<(Value, Option<String>), String> {
    match atomic_fs::load_json_recovering(path, &project_quarantine_path(path)) {
        atomic_fs::JsonLoad::Ok(v) => Ok((v, None)),
        atomic_fs::JsonLoad::Restored { value, error } => Ok((value, Some(error))),
        atomic_fs::JsonLoad::Missing => Err("arquivo não encontrado".to_string()),
        atomic_fs::JsonLoad::Broken { error } => Err(error),
        atomic_fs::JsonLoad::Unreadable { error, .. } => Err(error),
    }
}

pub(crate) fn load_project_from_disk(project_id: &str) -> Option<Value> {
    let (val, _) = read_project_file(&project_path(project_id)).ok()?;
    allow_project_media(&val);
    Some(val)
}

#[tauri::command]
pub(crate) async fn list_projects() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = projects_dir();
        let mut projects = Vec::new();
        let mut broken = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(serde_json::json!({ "projects": projects, "broken": broken }))
            }
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let file = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match read_project_file(&p) {
                Ok((val, restored_from)) => {
                    if let Some(error) = restored_from {
                        broken.push(BrokenProjectDto { file, error, restored: true });
                    }
                    allow_project_media(&val);
                    projects.push(val);
                }
                Err(error) => broken.push(BrokenProjectDto { file, error, restored: false }),
            }
        }
        Ok(serde_json::json!({ "projects": projects, "broken": broken }))
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(crate) fn save_project_inner(project: Value) -> Result<Value, String> {
    let id = project
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Projeto sem id")?;
    let json = serde_json::to_string_pretty(&project).map_err(|e| e.to_string())?;
    atomic_fs::write_atomic(&project_path(id), json.as_bytes(), true)?;
    Ok(project)
}

#[tauri::command]
pub(crate) async fn save_project(project: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || save_project_inner(project))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn delete_project(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    project_id: String,
) -> Result<(), String> {
    let state_arc = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        // Remove from runtime state first and close any active windows referencing this project
        let labels_to_close: Vec<String> = {
            let mut st = state_arc.lock().map_err(|e| e.to_string())?;
            let mut close_list = Vec::new();
            st.dirty_widget_saves.remove(&project_id);
            if st.pinned_widgets.remove(&project_id).is_some() {
                let wlbl = format!("widget-{}", sanitize_name(&project_id));
                st.widget_labels.remove(&wlbl);
                st.ready_windows.remove(&wlbl);
                close_list.push(wlbl);
            }
            if st
                .active_wallpaper
                .as_ref()
                .and_then(|p| p.get("id"))
                .and_then(|v| v.as_str())
                == Some(project_id.as_str())
            {
                st.active_wallpaper = None;
            }
            if !close_list.is_empty() {
                refresh_parallax_targets(&mut st);
                save_session_from_state(&st);
            }
            close_list
        };
        for lbl in labels_to_close {
            if let Some(win) = app.get_webview_window(&lbl) {
                let _ = win.close();
            }
        }

        let imports_root = prismview_root_dir().join("imports");
        let mut imports_prefix_lower = imports_root
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        if !imports_prefix_lower.ends_with('\\') {
            imports_prefix_lower.push('\\');
        }

        let p = project_path(&project_id);
        let mut candidate_uuids = HashSet::new();
        if let Ok(raw) = fs::read_to_string(&p) {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                collect_project_import_uuids(&val, &imports_prefix_lower, &mut candidate_uuids);
            }
        }

        if p.is_file() {
            fs::remove_file(&p).map_err(|e| e.to_string())?;
        }
        atomic_fs::discard_temp(&atomic_fs::backup_path(&p));

        cleanup_unreferenced_import_uuids(&candidate_uuids);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
