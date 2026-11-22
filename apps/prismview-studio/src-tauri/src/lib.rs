mod desktop_host;
mod native_engine;
mod native_scene;
mod reactive_sensors;

use desktop_host::{GsmtcHandle, LiveMediaDto, MediaCommand, MonitorInfoDto};
use reactive_sensors::ReactiveSensorsHandle;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use uuid::Uuid;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
    OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickedFileDto {
    pub path: String,
    pub file_name: String,
    pub ext: String,
    pub is_video: bool,
    pub is_image: bool,
    pub is_svg: bool,
    pub is_font: bool,
    pub is_model_3d: bool,
    pub text_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveTelemetryDto {
    pub cpu_percent: f32,
    pub ram_percent: f32,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    pub hostname: String,
    pub battery_percent: u8,
    pub battery_charging: bool,
    pub media: LiveMediaDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStateDto {
    pub paused: bool,
    pub desktop_visible: bool,
    pub active_wallpaper_id: Option<String>,
    pub pinned_widget_ids: Vec<String>,
    pub locked_widget_ids: Vec<String>,
    #[serde(default)]
    pub monitors: Vec<MonitorInfoDto>,
    #[serde(default)]
    pub monitor_wallpapers: HashMap<String, String>,
    /// Aviso quando a migração automática de um wallpaper antigo (WebView) para o motor nativo falhou.
    #[serde(default)]
    pub legacy_migration_notice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisibilityEventPayload {
    pub visible: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorEventPayload {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPackageResult {
    pub archive_path: String,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowBootstrapDto {
    pub project: Option<Value>,
    pub visible: bool,
    pub telemetry: LiveTelemetryDto,
    #[serde(default)]
    pub monitor: Option<MonitorInfoDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedSession {
    #[serde(default)]
    active_wallpaper_id: Option<String>,
    #[serde(default)]
    active_wallpaper_snapshot: Option<Value>,
    #[serde(default)]
    monitor_wallpaper_ids: HashMap<String, String>,
    #[serde(default)]
    monitor_wallpaper_snapshots: HashMap<String, Value>,
    #[serde(default)]
    pinned_widget_ids: Vec<String>,
    #[serde(default)]
    paused: bool,
}

#[derive(Default)]
struct AppRuntimeState {
    active_wallpaper: Option<Value>,
    monitors: Vec<MonitorInfoDto>,
    monitor_wallpapers: HashMap<String, Value>,  // monitor_id -> project
    monitor_visibility: HashMap<String, bool>,   // monitor_id -> desktop uncovered
    wallpaper_labels: HashMap<String, String>,   // window_label -> monitor_id
    pinned_widgets: HashMap<String, Value>,
    widget_labels: HashMap<String, String>,      // window_label -> widget_id
    ready_windows: HashSet<String>,
    parallax_targets: HashSet<String>,
    audio_targets: HashSet<String>,
    keyboard_targets: HashSet<String>,
    studio_audio_active: bool,
    studio_keyboard_active: bool,
    dirty_widget_saves: HashMap<String, Instant>,
    media_cache: Arc<Mutex<LiveMediaDto>>,
    gsmtc: Option<Arc<GsmtcHandle>>,
    reactive_sensors: Option<Arc<ReactiveSensorsHandle>>,
    last_worker_w: isize,
    last_worker_is_true: bool,
    pending_recovery_at: Option<Instant>,
    force_vis_resync: bool,
    manual_paused: bool,
    desktop_visible: bool,
    allow_exit: bool,
    legacy_migration_notice: Option<String>,
}

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(
        hkey: isize,
        lpsubkey: *const u16,
        uloptions: u32,
        samdesired: u32,
        phkresult: *mut isize,
    ) -> i32;
    fn RegQueryValueExW(
        hkey: isize,
        lpvaluename: *const u16,
        lpreserved: *mut u32,
        lptype: *mut u32,
        lpdata: *mut u8,
        lpcbdata: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        hkey: isize,
        lpvaluename: *const u16,
        reserved: u32,
        dwtype: u32,
        lpdata: *const u8,
        cbdata: u32,
    ) -> i32;
    fn RegDeleteValueW(hkey: isize, lpvaluename: *const u16) -> i32;
    fn RegEnumValueW(
        hkey: isize,
        dwindex: u32,
        lpvaluename: *mut u16,
        lpcchvaluename: *mut u32,
        lpreserved: *mut u32,
        lptype: *mut u32,
        lpdata: *mut u8,
        lpcbdata: *mut u32,
    ) -> i32;
    fn RegCloseKey(hkey: isize) -> i32;
}

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
const KEY_READ: u32 = 0x0002_0019;
const KEY_SET_VALUE: u32 = 0x0000_0002;
const REG_SZ: u32 = 1;
const ERROR_SUCCESS: i32 = 0;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_NO_MORE_ITEMS: i32 = 259;

fn normalize_registry_font_family(raw_name: &str) -> Option<String> {
    let mut name = raw_name.trim().to_string();
    for suffix in [" (TrueType)", " (OpenType)", " (All res)"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.trim().to_string();
        }
    }
    if let Some((first, _)) = name.split_once(" & ") {
        name = first.trim().to_string();
    }
    let style_suffixes = [
        " Bold Italic",
        " SemiBold Italic",
        " Semibold Italic",
        " Light Italic",
        " Medium Italic",
        " Black Italic",
        " Thin Italic",
        " ExtraLight Italic",
        " ExtraBold Italic",
        " Bold",
        " Italic",
        " Oblique",
        " Regular",
        " Semibold",
        " SemiBold",
        " Semilight",
        " SemiLight",
        " ExtraLight",
        " ExtraBold",
        " Light",
        " Medium",
        " Black",
        " Thin",
        " Heavy",
        " Book",
        " Demibold",
        " DemiBold",
        " Condensed",
        " Narrow",
        " Normal",
    ];
    let mut changed = true;
    while changed {
        changed = false;
        for suf in &style_suffixes {
            if let Some(stripped) = name.strip_suffix(suf) {
                let trimmed = stripped.trim();
                if !trimmed.is_empty() {
                    name = trimmed.to_string();
                    changed = true;
                }
            }
        }
    }
    let lower = name.to_ascii_lowercase();
    if name.is_empty()
        || name.starts_with('@')
        || lower.contains("wingding")
        || lower.contains("webding")
        || lower.contains("marlett")
        || lower.contains("mdl2")
        || lower.contains("fluent icons")
        || lower == "symbol"
        || lower == "mt extra"
        || lower.contains("emoji")
    {
        return None;
    }
    Some(name)
}

fn collect_fonts_from_hive(hive: isize, out: &mut HashSet<String>) {
    let subkey = wide_null(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts");
    let mut hkey: isize = 0;
    let status = unsafe { RegOpenKeyExW(hive, subkey.as_ptr(), 0, KEY_READ, &mut hkey) };
    if status != ERROR_SUCCESS {
        return;
    }
    let mut index: u32 = 0;
    let mut name_buf = vec![0u16; 512];
    loop {
        let mut name_len = name_buf.len() as u32;
        let mut val_type: u32 = 0;
        let res = unsafe {
            RegEnumValueW(
                hkey,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                std::ptr::null_mut(),
                &mut val_type,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if res == ERROR_NO_MORE_ITEMS {
            break;
        }
        if res == ERROR_SUCCESS && val_type == REG_SZ {
            let raw = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            if let Some(clean) = normalize_registry_font_family(&raw) {
                out.insert(clean);
            }
        }
        index += 1;
        if index > 4096 {
            break;
        }
    }
    unsafe {
        RegCloseKey(hkey);
    }
}

#[tauri::command]
async fn list_system_fonts() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut set = HashSet::new();
        for base in ["Outfit", "Inter", "JetBrains Mono", "Segoe UI"] {
            set.insert(base.to_string());
        }
        collect_fonts_from_hive(HKEY_LOCAL_MACHINE, &mut set);
        collect_fonts_from_hive(HKEY_CURRENT_USER, &mut set);
        let mut list: Vec<String> = set.into_iter().collect();
        list.sort_by_key(|a| a.to_ascii_lowercase());
        Ok(list)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Valor legado: Studio `--minimized` no login. Hoje só existe se houver widgets fixados
/// (o wallpaper é do motor nativo, que sobe sozinho).
const STUDIO_RUN_VALUE: &str = "PrismView";

fn run_value_present(name: &str) -> bool {
    let subkey = wide_null(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let value_name = wide_null(name);
    let mut hkey: isize = 0;
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        )
    };
    if status != ERROR_SUCCESS {
        return false;
    }
    let mut val_type: u32 = 0;
    let mut cb_data: u32 = 0;
    let q_status = unsafe {
        RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut val_type,
            std::ptr::null_mut(),
            &mut cb_data,
        )
    };
    unsafe {
        RegCloseKey(hkey);
    }
    q_status == ERROR_SUCCESS && val_type == REG_SZ && cb_data > 2
}

/// Grava (`Some(cmd)`) ou remove (`None`) um valor em `HKCU\...\Run`.
fn write_run_value(name: &str, command: Option<&str>) -> Result<(), String> {
    let subkey = wide_null(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let value_name = wide_null(name);
    let mut hkey: isize = 0;
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("Falha ao abrir Registro do Windows (erro {status})"));
    }
    let res = match command {
        Some(cmd) => {
            let cmd = wide_null(cmd);
            let byte_len = (cmd.len() * std::mem::size_of::<u16>()) as u32;
            let st = unsafe {
                RegSetValueExW(
                    hkey,
                    value_name.as_ptr(),
                    0,
                    REG_SZ,
                    cmd.as_ptr() as *const u8,
                    byte_len,
                )
            };
            if st == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!("Falha ao gravar autostart no Registro (erro {st})"))
            }
        }
        None => {
            let st = unsafe { RegDeleteValueW(hkey, value_name.as_ptr()) };
            if st == ERROR_SUCCESS || st == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("Falha ao remover autostart no Registro (erro {st})"))
            }
        }
    };
    unsafe {
        RegCloseKey(hkey);
    }
    res
}

/// "Iniciar com o Windows" = motor nativo no login (relê `active_scene.json`).
fn is_autostart_enabled_inner() -> bool {
    run_value_present(native_engine::ENGINE_RUN_VALUE)
}

/// Liga/desliga o motor no login. O Studio só entra no login se houver widgets fixados (são
/// janelas dele); a escolha do usuário fica gravada para o "Aplicar" não religar sozinho.
fn set_autostart_enabled_inner(enabled: bool, has_widgets: bool) -> Result<bool, String> {
    if enabled {
        let cmd = native_engine::login_command()?;
        write_run_value(native_engine::ENGINE_RUN_VALUE, Some(&cmd))?;
    } else {
        write_run_value(native_engine::ENGINE_RUN_VALUE, None)?;
    }
    native_engine::set_autostart_opt_out(!enabled);
    sync_studio_login_entry(enabled && has_widgets);
    Ok(enabled)
}

/// Garante o motor no login após um "Aplicar" (padrão), salvo se o usuário desligou.
fn ensure_engine_login_autostart(has_widgets: bool) {
    if native_engine::autostart_opted_out() || is_autostart_enabled_inner() {
        return;
    }
    if let Err(err) = set_autostart_enabled_inner(true, has_widgets) {
        eprintln!("[prismview-studio] autostart do motor: {err}");
    }
}

/// Entrada legada do Studio no login: só quando há widgets (senão a janela principal abriria).
fn sync_studio_login_entry(want: bool) {
    if want == run_value_present(STUDIO_RUN_VALUE) {
        return;
    }
    let result = if want {
        std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|exe| {
                write_run_value(
                    STUDIO_RUN_VALUE,
                    Some(&format!("\"{}\" --minimized", exe.to_string_lossy())),
                )
            })
    } else {
        write_run_value(STUDIO_RUN_VALUE, None)
    };
    if let Err(err) = result {
        eprintln!("[prismview-studio] autostart do Studio: {err}");
    }
}

type SharedState = Arc<Mutex<AppRuntimeState>>;

fn percent_encode_query(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn sanitize_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_sep = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
            prev_sep = false;
        } else if !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "PrismView".to_string()
    } else {
        trimmed.to_string()
    }
}

static ROOT_DIR_CACHE: OnceLock<PathBuf> = OnceLock::new();

fn prismview_root_dir() -> PathBuf {
    ROOT_DIR_CACHE
        .get_or_init(|| {
            let base = std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let root = base.join("PrismView");
            let _ = fs::create_dir_all(root.join("projects"));
            let _ = fs::create_dir_all(root.join("packages"));
            let _ = fs::create_dir_all(root.join("imports"));
            let _ = fs::create_dir_all(root.join("fonts"));
            let _ = fs::create_dir_all(root.join("assets"));
            root
        })
        .clone()
}

fn open_or_focus_studio(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let app_clone = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = WebviewWindowBuilder::new(&app_clone, "main", WebviewUrl::App("index.html".into()))
            .title("PrismView")
            .inner_size(1440.0, 900.0)
            .min_inner_size(1120.0, 720.0)
            .center()
            .decorations(true)
            .resizable(true)
            .build();
    });
}

fn project_path(project_id: &str) -> PathBuf {
    prismview_root_dir()
        .join("projects")
        .join(format!("{}.json", sanitize_name(project_id)))
}

fn session_path() -> PathBuf {
    prismview_root_dir().join("session.json")
}

/// Escopo do asset protocol (tauri.conf.json cobre só `%APPDATA%\PrismView`). Arquivos que o
/// usuário escolheu fora dessa pasta entram aqui, um a um, ao escolher/carregar o projeto.
static ASSET_SCOPE: OnceLock<tauri::scope::fs::Scope> = OnceLock::new();

fn allow_asset_file(path: &Path) {
    if let Some(scope) = ASSET_SCOPE.get() {
        let _ = scope.allow_file(path);
    }
}

/// Libera no asset protocol os caminhos absolutos de arquivos existentes citados no projeto.
fn allow_project_media(value: &Value) {
    match value {
        Value::String(s) if s.len() < 1024 => {
            let p = Path::new(s);
            if p.is_absolute() && p.is_file() {
                allow_asset_file(p);
            }
        }
        Value::Array(items) => items.iter().for_each(allow_project_media),
        Value::Object(map) => map.values().for_each(allow_project_media),
        _ => {}
    }
}

fn load_project_from_disk(project_id: &str) -> Option<Value> {
    let p = project_path(project_id);
    let raw = fs::read_to_string(p).ok()?;
    let val = serde_json::from_str::<Value>(&raw).ok()?;
    allow_project_media(&val);
    Some(val)
}

fn wallpaper_window_label(monitor_id: &str) -> String {
    format!("wallpaper-{}", sanitize_name(monitor_id))
}

fn ensure_monitors_loaded(st: &mut AppRuntimeState) -> Vec<MonitorInfoDto> {
    if st.monitors.is_empty() {
        st.monitors = desktop_host::enumerate_monitors();
        for m in &st.monitors {
            st.monitor_visibility.entry(m.id.clone()).or_insert(true);
        }
    }
    st.monitors.clone()
}

fn save_session_from_state(st: &AppRuntimeState) {
    let active_wallpaper_id = st
        .active_wallpaper
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let active_wallpaper_snapshot = match (&active_wallpaper_id, &st.active_wallpaper) {
        (Some(id), Some(val)) if !project_path(id).is_file() => Some(val.clone()),
        _ => None,
    };
    let mut monitor_wallpaper_ids = HashMap::new();
    let mut monitor_wallpaper_snapshots = HashMap::new();
    for (mon_id, val) in &st.monitor_wallpapers {
        if let Some(pid) = val.get("id").and_then(|v| v.as_str()) {
            monitor_wallpaper_ids.insert(mon_id.clone(), pid.to_string());
            if !project_path(pid).is_file() {
                monitor_wallpaper_snapshots.insert(mon_id.clone(), val.clone());
            }
        }
    }
    let mut pinned_widget_ids: Vec<String> = st.pinned_widgets.keys().cloned().collect();
    pinned_widget_ids.sort();
    let session = PersistedSession {
        active_wallpaper_id,
        active_wallpaper_snapshot,
        monitor_wallpaper_ids,
        monitor_wallpaper_snapshots,
        pinned_widget_ids,
        paused: st.manual_paused,
    };
    if let Ok(json) = serde_json::to_string_pretty(&session) {
        let _ = fs::write(session_path(), json);
    }
    if is_autostart_enabled_inner() {
        sync_studio_login_entry(!st.pinned_widgets.is_empty());
    }
}

fn load_persisted_session() -> Option<PersistedSession> {
    let raw = fs::read_to_string(session_path()).ok()?;
    serde_json::from_str::<PersistedSession>(&raw).ok()
}

fn extract_import_uuid_from_path(raw: &str, imports_prefix_lower: &str) -> Option<String> {
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

fn collect_dynamic_tex_import_uuids(
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

fn collect_layer_import_uuids(
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

fn collect_project_import_uuids(
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

fn cleanup_unreferenced_import_uuids(candidate_uuids: &HashSet<String>) {
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
                let _ = fs::remove_dir_all(&dir);
            }
        }
    }
}

fn flush_dirty_widget_saves(state: &SharedState, force: bool) {
    let now = Instant::now();
    let ready: Vec<Value> = if let Ok(mut st) = state.lock() {
        let due_ids: Vec<String> = st
            .dirty_widget_saves
            .iter()
            .filter(|(_, ts)| force || now.duration_since(**ts) >= Duration::from_millis(400))
            .map(|(id, _)| id.clone())
            .collect();
        let mut out = Vec::with_capacity(due_ids.len());
        for id in due_ids {
            st.dirty_widget_saves.remove(&id);
            if let Some(proj) = st.pinned_widgets.get(&id) {
                out.push(proj.clone());
            }
        }
        out
    } else {
        Vec::new()
    };
    for proj in ready {
        let _ = save_project_inner(proj);
    }
}

fn build_runtime_state(st: &AppRuntimeState) -> RuntimeStateDto {
    let locked_widget_ids = st
        .pinned_widgets
        .iter()
        .filter(|(_, val)| {
            val.pointer("/widget/locked")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .map(|(k, _)| k.clone())
        .collect();
    let monitor_wallpapers: HashMap<String, String> = st
        .monitor_wallpapers
        .iter()
        .filter_map(|(mid, val)| {
            val.get("id")
                .and_then(|v| v.as_str())
                .map(|pid| (mid.clone(), pid.to_string()))
        })
        .collect();
    let active_wallpaper_id = st
        .active_wallpaper
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            st.monitors
                .first()
                .and_then(|m| monitor_wallpapers.get(&m.id).cloned())
        });
    RuntimeStateDto {
        paused: st.manual_paused || !st.desktop_visible,
        desktop_visible: st.desktop_visible,
        active_wallpaper_id,
        pinned_widget_ids: st.pinned_widgets.keys().cloned().collect(),
        locked_widget_ids,
        monitors: st.monitors.clone(),
        monitor_wallpapers,
        legacy_migration_notice: st.legacy_migration_notice.clone(),
    }
}

fn bundle_path_into_zip(
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
        "mp4" | "webm" | "mkv" | "mov" | "jpg" | "jpeg" | "png" | "webp" | "woff2"
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

fn bundle_dynamic_textures_array(
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

fn bundle_layer_files(
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

fn safe_relative_subpath(rel: &str) -> Option<PathBuf> {
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

fn resolve_imported_asset_path(rel: &str, extract_root: &Path, canonical_root: &Path) -> Option<String> {
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

fn remap_dynamic_textures_array(
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

fn remap_imported_layers(layers: &mut [Value], extract_root: &Path) {
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
async fn list_projects() -> Result<Vec<Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = prismview_root_dir().join("projects");
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Ok(raw) = fs::read_to_string(&p) {
                        if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                            allow_project_media(&val);
                            out.push(val);
                        }
                    }
                }
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn save_project_inner(project: Value) -> Result<Value, String> {
    let id = project
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Projeto sem id")?;
    let json = serde_json::to_string_pretty(&project).map_err(|e| e.to_string())?;
    fs::write(project_path(id), json).map_err(|e| e.to_string())?;
    Ok(project)
}

#[tauri::command]
async fn save_project(project: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || save_project_inner(project))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn delete_project(
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
            let matching_mons: Vec<String> = st
                .monitor_wallpapers
                .iter()
                .filter(|(_, p)| p.get("id").and_then(|v| v.as_str()) == Some(project_id.as_str()))
                .map(|(mid, _)| mid.clone())
                .collect();
            for mid in matching_mons {
                st.monitor_wallpapers.remove(&mid);
                let lbl = wallpaper_window_label(&mid);
                st.wallpaper_labels.remove(&lbl);
                st.ready_windows.remove(&lbl);
                close_list.push(lbl);
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

        cleanup_unreferenced_import_uuids(&candidate_uuids);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn layers_reactive_flags(layers: &[Value]) -> (bool, bool, bool) {
    let mut cursor = false;
    let mut audio = false;
    let mut keyboard = false;
    for layer in layers {
        if layer
            .get("hidden")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let px = layer
            .get("parallaxX")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let py = layer
            .get("parallaxY")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if px.abs() > 1e-4 || py.abs() > 1e-4 {
            cursor = true;
        }
        if let Some(cam) = layer.pointer("/scene3d/camera") {
            let ox = cam
                .get("orbitParallaxX")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let oy = cam
                .get("orbitParallaxY")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            if ox.abs() > 1e-4 || oy.abs() > 1e-4 {
                cursor = true;
            }
        }
        if let Some(bindings) = layer.pointer("/scene3d/bindings").and_then(|v| v.as_array()) {
            for b in bindings {
                let enabled = b
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if !enabled {
                    continue;
                }
                match b.get("source").and_then(|v| v.as_str()).unwrap_or("") {
                    "CURSOR_LOOK" | "HOVER_MESH" | "CLICK_MESH" => cursor = true,
                    "AUDIO_BASS" | "AUDIO_MID" | "AUDIO_TREBLE" => audio = true,
                    "KEY_PULSE" => keyboard = true,
                    _ => {}
                }
            }
        }
        if let Some(children) = layer.get("children").and_then(|v| v.as_array()) {
            let (cc, ca, ck) = layers_reactive_flags(children);
            cursor |= cc;
            audio |= ca;
            keyboard |= ck;
        }
    }
    (cursor, audio, keyboard)
}

fn project_reactive_flags(project: &Value) -> (bool, bool, bool) {
    project
        .get("layers")
        .and_then(|v| v.as_array())
        .map(|arr| layers_reactive_flags(arr))
        .unwrap_or((false, false, false))
}

fn refresh_parallax_targets(st: &mut AppRuntimeState) {
    st.parallax_targets.clear();
    st.audio_targets.clear();
    st.keyboard_targets.clear();
    for (mon_id, proj) in &st.monitor_wallpapers {
        let lbl = wallpaper_window_label(mon_id);
        let (has_cursor, has_audio, has_kbd) = project_reactive_flags(proj);
        if has_cursor {
            st.parallax_targets.insert(lbl.clone());
        }
        if has_audio {
            st.audio_targets.insert(lbl.clone());
        }
        if has_kbd {
            st.keyboard_targets.insert(lbl);
        }
    }
    for (wid, proj) in &st.pinned_widgets {
        let lbl = format!("widget-{}", sanitize_name(wid));
        let (has_cursor, has_audio, has_kbd) = project_reactive_flags(proj);
        if has_cursor {
            st.parallax_targets.insert(lbl.clone());
        }
        if has_audio {
            st.audio_targets.insert(lbl.clone());
        }
        if has_kbd {
            st.keyboard_targets.insert(lbl);
        }
    }
}

fn build_telemetry_snapshot(
    hostname: &str,
    media_cache: &Arc<Mutex<LiveMediaDto>>,
) -> LiveTelemetryDto {
    let (cpu, bat, charging) = desktop_host::sample_cpu_and_battery();
    let (ram_pct, used_gb, total_gb) = desktop_host::sample_memory_usage();
    let media = media_cache
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    LiveTelemetryDto {
        cpu_percent: cpu,
        ram_percent: ram_pct,
        ram_used_gb: used_gb,
        ram_total_gb: total_gb,
        hostname: hostname.to_string(),
        battery_percent: bat,
        battery_charging: charging,
        media,
    }
}

/// Fecha janelas WebView de wallpaper deixadas por versões antigas e zera o estado delas, sem
/// mexer no motor nativo.
fn close_legacy_wallpaper_windows(app: &AppHandle, state: &SharedState) -> Result<(), String> {
    let labels: Vec<String> = {
        let mut st = state.lock().map_err(|e| e.to_string())?;
        let mut all: Vec<String> = st.wallpaper_labels.keys().cloned().collect();
        for m in &st.monitors {
            all.push(wallpaper_window_label(&m.id));
        }
        all.push("wallpaper".to_string());
        st.monitor_wallpapers.clear();
        st.wallpaper_labels.clear();
        for lbl in &all {
            st.ready_windows.remove(lbl);
        }
        refresh_parallax_targets(&mut st);
        all
    };
    for label in labels {
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.close();
        }
    }
    Ok(())
}

/// Remove o wallpaper do desktop: motor nativo encerrado + cena ativa apagada + estado limpo.
fn clear_wallpaper_inner(
    app: &AppHandle,
    state: &SharedState,
    target_monitor_id: Option<String>,
) -> Result<(), String> {
    let _ = target_monitor_id;
    native_engine::clear_desktop()?;
    close_legacy_wallpaper_windows(app, state)?;
    if let Ok(mut st) = state.lock() {
        st.active_wallpaper = None;
        save_session_from_state(&st);
    }
    Ok(())
}

fn reconcile_monitors_and_worker_w(app: &AppHandle, state: &SharedState) {
    let fresh_monitors = desktop_host::enumerate_monitors();
    let fresh_ids: HashSet<String> = fresh_monitors.iter().map(|m| m.id.clone()).collect();

    let (stale_labels, pinned_widget_snapshots) = {
        let Ok(mut st) = state.lock() else {
            return;
        };
        st.monitors = fresh_monitors.clone();
        st.monitor_visibility
            .retain(|mid, _| fresh_ids.contains(mid));
        // Prune disconnected monitors from monitor_wallpapers too so RuntimeState and ExitRequested stay accurate
        st.monitor_wallpapers
            .retain(|mid, _| fresh_ids.contains(mid));
        for m in &fresh_monitors {
            st.monitor_visibility.entry(m.id.clone()).or_insert(true);
        }

        let mut stale = Vec::new();
        let existing_labels: Vec<(String, String)> = st
            .wallpaper_labels
            .iter()
            .map(|(l, m)| (l.clone(), m.clone()))
            .collect();
        for (lbl, mid) in existing_labels {
            if !fresh_ids.contains(&mid) {
                stale.push(lbl.clone());
                st.wallpaper_labels.remove(&lbl);
                st.ready_windows.remove(&lbl);
            }
        }

        let widgets: Vec<(String, bool)> = st
            .pinned_widgets
            .iter()
            .map(|(wid, val)| {
                let locked = val
                    .pointer("/widget/locked")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                (format!("widget-{}", sanitize_name(wid)), locked)
            })
            .collect();

        refresh_parallax_targets(&mut st);
        (stale, widgets)
    };

    for lbl in stale_labels {
        if let Some(win) = app.get_webview_window(&lbl) {
            let _ = win.close();
        }
    }

    for (w_label, locked) in pinned_widget_snapshots {
        if let Some(win) = app.get_webview_window(&w_label) {
            if let Ok(hwnd) = win.hwnd() {
                let hwnd_raw = hwnd.0 as isize;
                let _ = app.run_on_main_thread(move || {
                    desktop_host::configure_desktop_widget_window(hwnd_raw, locked);
                });
            }
        }
    }

    if app.get_webview_window("main").is_some() {
        let _ = app.emit_to("main", "prism://monitors-changed", &fresh_monitors);
    }
}

#[tauri::command]
fn list_monitors(state: tauri::State<'_, SharedState>) -> Result<Vec<MonitorInfoDto>, String> {
    let fresh = desktop_host::enumerate_monitors();
    if let Ok(mut st) = state.lock() {
        st.monitors = fresh.clone();
        for m in &fresh {
            st.monitor_visibility.entry(m.id.clone()).or_insert(true);
        }
    }
    Ok(fresh)
}

/// "Aplicar no Desktop": sempre pelo motor nativo (`prismview-engine --native`), em todos os
/// monitores (`monitor_id` é ignorado: o motor desenha a mesma cena em cada saída). Falhas
/// (conversor, exe ausente, pipe) voltam como erro para a UI; não existe mais wallpaper WebView.
#[tauri::command]
async fn apply_desktop_wallpaper(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    engine: tauri::State<'_, native_engine::SharedEngine>,
    project: Value,
    monitor_id: Option<String>,
) -> Result<RuntimeStateDto, String> {
    let state_arc = Arc::clone(state.inner());
    let engine_arc = Arc::clone(engine.inner());
    let _ = monitor_id;
    tauri::async_runtime::spawn_blocking(move || {
        engine_arc.apply_project(&project)?;
        // Limpa restos de sessões antigas (janelas WebView de wallpaper) sem tocar no motor.
        close_legacy_wallpaper_windows(&app, &state_arc)?;
        let has_widgets = {
            let mut st = state_arc.lock().map_err(|e| e.to_string())?;
            st.active_wallpaper = Some(project);
            save_session_from_state(&st);
            !st.pinned_widgets.is_empty()
        };
        ensure_engine_login_autostart(has_widgets);
        sync_studio_tray_visibility(&app, &state_arc);
        let st = state_arc.lock().map_err(|e| e.to_string())?;
        Ok(build_runtime_state(&st))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Remove o wallpaper: encerra o motor nativo e apaga a cena ativa (o Windows volta ao fundo
/// dele). O motor desenha todos os monitores juntos, então `monitor_id` limpa tudo.
#[tauri::command]
async fn clear_desktop_wallpaper(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    monitor_id: Option<String>,
) -> Result<RuntimeStateDto, String> {
    let state_arc = Arc::clone(state.inner());
    let _ = monitor_id;
    tauri::async_runtime::spawn_blocking(move || {
        clear_wallpaper_inner(&app, &state_arc, None)?;
        sync_studio_tray_visibility(&app, &state_arc);
        let st = state_arc.lock().map_err(|e| e.to_string())?;
        Ok(build_runtime_state(&st))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn pin_widget_inner(
    app: &AppHandle,
    state: &SharedState,
    project: Value,
    persist_session: bool,
) -> Result<(), String> {
    let widget_id = project
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Widget sem id")?
        .to_string();
    let w = project
        .pointer("/widget/width")
        .and_then(|v| v.as_f64())
        .unwrap_or(460.0)
        .clamp(160.0, 1400.0);
    let h = project
        .pointer("/widget/height")
        .and_then(|v| v.as_f64())
        .unwrap_or(240.0)
        .clamp(100.0, 1000.0);
    let x = project
        .pointer("/widget/posX")
        .and_then(|v| v.as_f64())
        .unwrap_or(120.0);
    let y = project
        .pointer("/widget/posY")
        .and_then(|v| v.as_f64())
        .unwrap_or(120.0);
    let locked = project
        .pointer("/widget/locked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // posX / posY are stored in physical virtual-desktop coordinates so mixed-DPI monitors never drift
    let (vx, vy, vw, vh) = desktop_host::virtual_screen_bounds();
    let phys_x = (x.round() as i32).clamp(vx, (vx + vw - 60).max(vx));
    let phys_y = (y.round() as i32).clamp(vy, (vy + vh - 60).max(vy));

    let label = format!("widget-{}", sanitize_name(&widget_id));
    let configure_on_main = |win: &tauri::WebviewWindow, is_locked: bool| {
        if let Ok(hwnd) = win.hwnd() {
            let hwnd_raw = hwnd.0 as isize;
            let _ = app.run_on_main_thread(move || {
                desktop_host::configure_desktop_widget_window(hwnd_raw, is_locked);
            });
        }
    };

    if let Some(win) = app.get_webview_window(&label) {
        let is_ready = {
            let mut st = state.lock().map_err(|e| e.to_string())?;
            st.pinned_widgets.insert(widget_id.clone(), project.clone());
            st.widget_labels.insert(label.clone(), widget_id.clone());
            refresh_parallax_targets(&mut st);
            if persist_session {
                save_session_from_state(&st);
            }
            st.ready_windows.contains(&label)
        };
        let _ = win.set_size(tauri::LogicalSize::new(w, h));
        let need_move = win
            .outer_position()
            .map(|p| (p.x - phys_x).abs() > 1 || (p.y - phys_y).abs() > 1)
            .unwrap_or(true);
        if need_move {
            let _ = win.set_position(tauri::PhysicalPosition::new(phys_x, phys_y));
        }
        configure_on_main(&win, locked);
        if is_ready {
            let _ = app.emit_to(&label, "prism://widget-scene", &project);
        }
        let _ = win.show();
    } else {
        let primary_scale = state
            .lock()
            .ok()
            .and_then(|st| {
                st.monitors
                    .iter()
                    .find(|m| m.is_primary)
                    .or_else(|| st.monitors.first())
                    .map(|m| m.scale_factor.max(0.5) as f64)
            })
            .unwrap_or(1.0);
        let init_logical_x = phys_x as f64 / primary_scale;
        let init_logical_y = phys_y as f64 / primary_scale;
        let url = format!(
            "desktop.html?mode=widget&id={}",
            percent_encode_query(&widget_id)
        );

        // Pre-insert state BEFORE build() so an early widget_window_ready IPC never misses the project,
        // rolling back completely if WebviewWindowBuilder::build() fails.
        let (prev_proj, prev_lbl) = {
            let mut st = state.lock().map_err(|e| e.to_string())?;
            st.ready_windows.remove(&label);
            let prev_proj = st.pinned_widgets.insert(widget_id.clone(), project.clone());
            let prev_lbl = st.widget_labels.insert(label.clone(), widget_id.clone());
            refresh_parallax_targets(&mut st);
            (prev_proj, prev_lbl)
        };

        let win = match WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
            .title(format!("PrismView Widget - {widget_id}"))
            .inner_size(w, h)
            .position(init_logical_x, init_logical_y)
            .decorations(false)
            .transparent(true)
            .resizable(false)
            .skip_taskbar(true)
            .shadow(false)
            .visible(true)
            .build()
        {
            Ok(w) => w,
            Err(e) => {
                if let Ok(mut st) = state.lock() {
                    st.ready_windows.remove(&label);
                    match prev_proj {
                        Some(old) => {
                            st.pinned_widgets.insert(widget_id.clone(), old);
                        }
                        None => {
                            st.pinned_widgets.remove(&widget_id);
                        }
                    }
                    match prev_lbl {
                        Some(old) => {
                            st.widget_labels.insert(label.clone(), old);
                        }
                        None => {
                            st.widget_labels.remove(&label);
                        }
                    }
                    refresh_parallax_targets(&mut st);
                }
                return Err(e.to_string());
            }
        };

        let _ = win.set_position(tauri::PhysicalPosition::new(phys_x, phys_y));
        configure_on_main(&win, locked);

        if persist_session {
            if let Ok(st) = state.lock() {
                save_session_from_state(&st);
            }
        }
    }

    Ok(())
}

#[tauri::command]
async fn pin_standalone_widget(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    project: Value,
) -> Result<RuntimeStateDto, String> {
    let state_arc = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        pin_widget_inner(&app, &state_arc, project, true)?;
        sync_studio_tray_visibility(&app, &state_arc);
        let st = state_arc.lock().map_err(|e| e.to_string())?;
        Ok(build_runtime_state(&st))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn unpin_standalone_widget(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    widget_id: String,
) -> Result<RuntimeStateDto, String> {
    let state_arc = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        let label = format!("widget-{}", sanitize_name(&widget_id));
        let pending_to_flush = {
            let mut st = state_arc.lock().map_err(|e| e.to_string())?;
            let flushed = if st.dirty_widget_saves.remove(&widget_id).is_some() {
                st.pinned_widgets.get(&widget_id).cloned()
            } else {
                None
            };
            st.pinned_widgets.remove(&widget_id);
            st.widget_labels.remove(&label);
            st.ready_windows.remove(&label);
            refresh_parallax_targets(&mut st);
            save_session_from_state(&st);
            flushed
        };
        if let Some(proj) = pending_to_flush {
            let _ = save_project_inner(proj);
        }
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.close();
        }
        sync_studio_tray_visibility(&app, &state_arc);
        let st = state_arc.lock().map_err(|e| e.to_string())?;
        Ok(build_runtime_state(&st))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn set_widget_locked_inner(
    app: &AppHandle,
    state: &SharedState,
    widget_id: &str,
    locked: bool,
) -> Result<(), String> {
    let updated_proj = {
        let mut st = state.lock().map_err(|e| e.to_string())?;
        let mut cloned = None;
        if let Some(val) = st.pinned_widgets.get_mut(widget_id) {
            if let Some(w_obj) = val.get_mut("widget").and_then(|v| v.as_object_mut()) {
                w_obj.insert("locked".to_string(), Value::Bool(locked));
            }
            cloned = Some(val.clone());
        }
        cloned
    };
    if let Some(p) = updated_proj {
        let _ = save_project_inner(p.clone());
        let label = format!("widget-{}", sanitize_name(widget_id));
        if let Some(win) = app.get_webview_window(&label) {
            if let Ok(hwnd) = win.hwnd() {
                let hwnd_raw = hwnd.0 as isize;
                let _ = app.run_on_main_thread(move || {
                    desktop_host::configure_desktop_widget_window(hwnd_raw, locked);
                });
            }
            // Use emit_to(&label, ...) so we never broadcast/overwrite other open widgets!
            let _ = app.emit_to(&label, "prism://widget-scene", &p);
        }
    }
    Ok(())
}

#[tauri::command]
async fn set_widget_locked(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    widget_id: String,
    locked: bool,
) -> Result<RuntimeStateDto, String> {
    let state_arc = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        set_widget_locked_inner(&app, &state_arc, &widget_id, locked)?;
        let st = state_arc.lock().map_err(|e| e.to_string())?;
        Ok(build_runtime_state(&st))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn unlock_all_widgets_inner(app: &AppHandle, state: &SharedState) {
    let ids: Vec<String> = state
        .lock()
        .map(|st| st.pinned_widgets.keys().cloned().collect())
        .unwrap_or_default();
    for id in ids {
        let _ = set_widget_locked_inner(app, state, &id, false);
    }
}

fn apply_manual_pause_inner(app: &AppHandle, state: &SharedState, paused: bool) -> Result<(), String> {
    let (wp_targets, wg_targets, any_vis) = {
        let mut st = state.lock().map_err(|e| e.to_string())?;
        st.manual_paused = paused;
        st.force_vis_resync = true;
        save_session_from_state(&st);

        let wps: Vec<(String, bool)> = st
            .wallpaper_labels
            .iter()
            .filter(|(lbl, _)| st.ready_windows.contains(*lbl))
            .map(|(lbl, mid)| {
                let mon_vis = st.monitor_visibility.get(mid).copied().unwrap_or(true);
                (lbl.clone(), !paused && mon_vis)
            })
            .collect();
        let wgs: Vec<(String, bool)> = st
            .widget_labels
            .iter()
            .filter(|(lbl, _)| st.ready_windows.contains(*lbl))
            .map(|(lbl, _)| (lbl.clone(), !paused && st.desktop_visible))
            .collect();
        (wps, wgs, !paused && st.desktop_visible)
    };
    let reason = if paused { "manual_pause" } else { "resumed" };
    // Propaga pausa/retomada ao motor nativo em background (não bloqueia a UI).
    std::thread::spawn(move || native_engine::set_paused(paused));
    for (lbl, vis) in wp_targets.into_iter().chain(wg_targets) {
        let _ = app.emit_to(
            &lbl,
            "prism://visibility-changed",
            VisibilityEventPayload {
                visible: vis,
                reason: reason.to_string(),
            },
        );
    }
    if app.get_webview_window("main").is_some() {
        let _ = app.emit_to(
            "main",
            "prism://visibility-changed",
            VisibilityEventPayload {
                visible: any_vis,
                reason: reason.to_string(),
            },
        );
    }
    Ok(())
}

#[tauri::command]
fn widget_window_ready(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    widget_id: String,
) -> Result<WindowBootstrapDto, String> {
    let label = format!("widget-{}", sanitize_name(&widget_id));
    let phys_pos = app
        .get_webview_window(&label)
        .and_then(|w| w.outer_position().ok())
        .map(|p| (p.x, p.y));
    let (project, visible, media_cache, monitor) = {
        let mut st = state.lock().map_err(|e| e.to_string())?;
        let monitors = ensure_monitors_loaded(&mut st);
        st.ready_windows.insert(label);
        let proj = st.pinned_widgets.get(&widget_id).cloned();
        let (wx, wy) = phys_pos.unwrap_or_else(|| {
            let x = proj
                .as_ref()
                .and_then(|p| p.pointer("/widget/posX"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as i32;
            let y = proj
                .as_ref()
                .and_then(|p| p.pointer("/widget/posY"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as i32;
            (x, y)
        });
        let mon_id = desktop_host::monitor_id_for_point(&monitors, wx, wy);
        let mon = mon_id
            .as_deref()
            .and_then(|mid| monitors.iter().find(|m| m.id == mid).cloned());
        let mon_vis = mon_id
            .as_deref()
            .and_then(|mid| st.monitor_visibility.get(mid).copied())
            .unwrap_or(st.desktop_visible);
        (
            proj,
            mon_vis && !st.manual_paused,
            Arc::clone(&st.media_cache),
            mon,
        )
    };
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".to_string());
    let telemetry = build_telemetry_snapshot(&hostname, &media_cache);
    Ok(WindowBootstrapDto {
        project,
        visible,
        telemetry,
        monitor,
    })
}

#[tauri::command]
fn set_engine_paused(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    paused: bool,
) -> Result<RuntimeStateDto, String> {
    apply_manual_pause_inner(&app, state.inner(), paused)?;
    get_runtime_state(state)
}

/// Coordena o ícone de bandeja do Studio (`studio_tray`) com o ícone nativo do `prismview-engine`
/// para nunca existirem 2 ícones duplicados ao mesmo tempo:
/// - Quando a engine está rodando com bandeja ativa, ela assume o ícone único.
/// - O tray do Studio só aparece quando há widgets fixados sem o tray da engine visível.
fn sync_studio_tray_visibility(app: &AppHandle, state: &SharedState) {
    let Some(tray) = app.tray_by_id("studio_tray") else {
        return;
    };
    let has_widgets = state
        .lock()
        .map(|st| !st.pinned_widgets.is_empty())
        .unwrap_or(false);
    let engine_owns_tray = native_engine::get_tray_enabled() && native_engine::is_running_fast();
    let show_studio_tray = has_widgets && !engine_owns_tray;
    let _ = tray.set_visible(show_studio_tray);
}

#[tauri::command]
fn get_engine_tray_enabled() -> Result<bool, String> {
    Ok(native_engine::get_tray_enabled())
}

#[tauri::command]
async fn set_engine_tray_visible(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    visible: bool,
) -> Result<bool, String> {
    let state_arc = Arc::clone(state.inner());
    let result = tauri::async_runtime::spawn_blocking(move || native_engine::set_tray_visible(visible))
        .await
        .map_err(|e| format!("tray_visible_task: {e}"))??;
    sync_studio_tray_visibility(&app, &state_arc);
    Ok(result)
}

#[tauri::command]
fn get_autostart_enabled() -> Result<bool, String> {
    Ok(is_autostart_enabled_inner())
}

#[tauri::command]
fn set_autostart_enabled(
    state: tauri::State<'_, SharedState>,
    enabled: bool,
) -> Result<bool, String> {
    let has_widgets = state
        .lock()
        .map(|st| !st.pinned_widgets.is_empty())
        .unwrap_or(false);
    set_autostart_enabled_inner(enabled, has_widgets)
}

#[tauri::command]
fn get_runtime_state(state: tauri::State<'_, SharedState>) -> Result<RuntimeStateDto, String> {
    let st = state.lock().map_err(|e| e.to_string())?;
    Ok(build_runtime_state(&st))
}

#[tauri::command]
fn dismiss_legacy_migration_notice(state: tauri::State<'_, SharedState>) -> Result<(), String> {
    let mut st = state.lock().map_err(|e| e.to_string())?;
    st.legacy_migration_notice = None;
    Ok(())
}

#[tauri::command]
async fn pick_save_archive_path(
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
                    let _ = fs::create_dir_all(parent);
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

    let file = File::create(&dest).map_err(|e| e.to_string())?;
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

    let payload = serde_json::to_string_pretty(&project).map_err(|e| e.to_string())?;
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
    zip.finish().map_err(|e| e.to_string())?;

    Ok(ExportPackageResult {
        archive_path: dest.to_string_lossy().to_string(),
        file_name,
    })
}

#[tauri::command]
async fn export_prism_archive(
    project: Value,
    dest_path: Option<String>,
) -> Result<ExportPackageResult, String> {
    tauri::async_runtime::spawn_blocking(move || export_prism_archive_sync(project, dest_path))
        .await
        .map_err(|e| e.to_string())?
}

const MAX_ZIP_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ZIP_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_PROJECT_JSON_BYTES: u64 = 8 * 1024 * 1024;

#[tauri::command]
async fn import_prism_archive(archive_path: String) -> Result<Value, String> {
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
            let _ = fs::remove_dir_all(&extract_dir);
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
                let _ = fs::remove_dir_all(&extract_dir);
                Err(err)
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn pick_local_file(kind: Option<String>) -> Result<Option<PickedFileDto>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Initialize STA COM apartment on this worker thread and pass null hwndOwner
        // to avoid cross-thread AttachThreadInput deadlocks with the main WebView2 UI thread.
        let _com = desktop_host::ComApartmentGuard::init_sta();
        let filter_str = match kind.as_deref() {
            Some("video") => "Vídeos (*.mp4;*.webm;*.mkv;*.mov)\0*.mp4;*.webm;*.mkv;*.mov\0\0",
            Some("image") => "Imagens (*.png;*.jpg;*.jpeg;*.webp)\0*.png;*.jpg;*.jpeg;*.webp\0\0",
            Some("svg") => "Vetores SVG (*.svg)\0*.svg\0\0",
            Some("font") => "Fontes (*.ttf;*.otf;*.woff2)\0*.ttf;*.otf;*.woff2\0\0",
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

#[tauri::command]
fn get_live_telemetry(state: tauri::State<'_, SharedState>) -> Result<LiveTelemetryDto, String> {
    let media_cache = {
        let st = state.lock().map_err(|e| e.to_string())?;
        Arc::clone(&st.media_cache)
    };
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".to_string());
    Ok(build_telemetry_snapshot(&hostname, &media_cache))
}

fn spawn_background_monitors(app: AppHandle, state: SharedState, stop: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("prism-bg-monitor".to_string())
        .spawn(move || {
            let mut last_global_vis = true;
            let mut last_window_vis: HashMap<String, bool> = HashMap::new();
            let boot_past = Instant::now()
                .checked_sub(Duration::from_secs(5))
                .unwrap_or_else(Instant::now);
            let mut last_telemetry = boot_past;
            let mut last_vis_check = boot_past;
            let mut last_cursor_raw: Option<(i32, i32)> = None;
            let mut prev_lbutton_down = false;
            let mut last_reactive_quiet = true;
            let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".to_string());

            while !stop.load(Ordering::Relaxed) {
                let now = Instant::now();
                flush_dirty_widget_saves(&state, false);

                // Execute debounced WorkerW / multi-monitor recovery (WM_DISPLAYCHANGE / TaskbarCreated)
                let (due_recovery, need_resync) = if let Ok(mut st) = state.lock() {
                    let worker_died = st.last_worker_w != 0
                        && !st.monitor_wallpapers.is_empty()
                        && !desktop_host::is_window_alive(st.last_worker_w);
                    if worker_died && st.pending_recovery_at.is_none() {
                        st.pending_recovery_at = Some(now);
                    }
                    let due = if let Some(ts) = st.pending_recovery_at {
                        if now.duration_since(ts) >= Duration::from_millis(600) {
                            st.pending_recovery_at = None;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    let resync = st.force_vis_resync;
                    st.force_vis_resync = false;
                    (due, resync)
                } else {
                    (false, false)
                };
                if due_recovery {
                    reconcile_monitors_and_worker_w(&app, &state);
                    last_window_vis.clear();
                } else if need_resync {
                    last_window_vis.clear();
                }

                let main_win = app.get_webview_window("main");
                let main_visible = main_win
                    .as_ref()
                    .and_then(|w| w.is_visible().ok())
                    .unwrap_or(false);

                if need_resync || now.duration_since(last_vis_check) >= Duration::from_millis(500) {
                    last_vis_check = now;
                    let studio_hwnd = main_win
                        .as_ref()
                        .and_then(|w| w.hwnd().ok())
                        .map(|h| h.0 as isize);

                    let monitors = if let Ok(mut st) = state.lock() {
                        ensure_monitors_loaded(&mut st)
                    } else {
                        Vec::new()
                    };

                    let per_mon_vis = desktop_host::compute_monitors_visibility(&monitors, studio_hwnd);
                    let any_desktop_vis = per_mon_vis.values().any(|&v| v);

                    let (manual_paused, wp_windows, widget_labels_ready) =
                        if let Ok(mut st) = state.lock() {
                            st.monitor_visibility = per_mon_vis.clone();
                            st.desktop_visible = any_desktop_vis;
                            let wps: Vec<(String, String)> = st
                                .wallpaper_labels
                                .iter()
                                .filter(|(lbl, _)| st.ready_windows.contains(*lbl))
                                .map(|(lbl, mid)| (lbl.clone(), mid.clone()))
                                .collect();
                            let wgs: Vec<(String, i32, i32)> = st
                                .widget_labels
                                .iter()
                                .filter(|(lbl, _)| st.ready_windows.contains(*lbl))
                                .map(|(lbl, wid)| {
                                    let (fx, fy) = st
                                        .pinned_widgets
                                        .get(wid)
                                        .map(|p| {
                                            (
                                                p.pointer("/widget/posX")
                                                    .and_then(|v| v.as_f64())
                                                    .unwrap_or(0.0)
                                                    as i32,
                                                p.pointer("/widget/posY")
                                                    .and_then(|v| v.as_f64())
                                                    .unwrap_or(0.0)
                                                    as i32,
                                            )
                                        })
                                        .unwrap_or((0, 0));
                                    (lbl.clone(), fx, fy)
                                })
                                .collect();
                            (st.manual_paused, wps, wgs)
                        } else {
                            (false, Vec::new(), Vec::new())
                        };

                    // Prune stale entries from last_window_vis so closed windows never leak memory
                    let mut live_labels: HashSet<String> = HashSet::new();
                    for (lbl, _) in &wp_windows {
                        live_labels.insert(lbl.clone());
                    }
                    for (lbl, _, _) in &widget_labels_ready {
                        live_labels.insert(lbl.clone());
                    }
                    last_window_vis.retain(|k, _| live_labels.contains(k));

                    // Emit independent per-monitor visibility changes ONLY to affected windows
                    for (label, mon_id) in wp_windows {
                        let eff = !manual_paused
                            && per_mon_vis.get(&mon_id).copied().unwrap_or(true);
                        if last_window_vis.get(&label).copied() != Some(eff) {
                            last_window_vis.insert(label.clone(), eff);
                            let _ = app.emit_to(
                                &label,
                                "prism://visibility-changed",
                                VisibilityEventPayload {
                                    visible: eff,
                                    reason: if eff {
                                        "monitor_visible"
                                    } else if manual_paused {
                                        "manual_pause"
                                    } else {
                                        "monitor_occluded"
                                    }
                                    .to_string(),
                                },
                            );
                        }
                    }

                    for (label, fallback_x, fallback_y) in widget_labels_ready {
                        // Use real physical outer_position so HiDPI monitors match physical bounds accurately
                        let (wx, wy) = app
                            .get_webview_window(&label)
                            .and_then(|w| w.outer_position().ok())
                            .map(|p| (p.x, p.y))
                            .unwrap_or((fallback_x, fallback_y));
                        let mon_id_opt = desktop_host::monitor_id_for_point(&monitors, wx, wy);
                        let eff = !manual_paused
                            && mon_id_opt
                                .as_deref()
                                .and_then(|mid| per_mon_vis.get(mid).copied())
                                .unwrap_or(any_desktop_vis);
                        if last_window_vis.get(&label).copied() != Some(eff) {
                            last_window_vis.insert(label.clone(), eff);
                            let _ = app.emit_to(
                                &label,
                                "prism://visibility-changed",
                                VisibilityEventPayload {
                                    visible: eff,
                                    reason: if eff {
                                        "monitor_visible"
                                    } else if manual_paused {
                                        "manual_pause"
                                    } else {
                                        "monitor_occluded"
                                    }
                                    .to_string(),
                                },
                            );
                        }
                    }

                    let global_eff = any_desktop_vis && !manual_paused;
                    if global_eff != last_global_vis {
                        last_global_vis = global_eff;
                        if main_visible {
                            let _ = app.emit_to(
                                "main",
                                "prism://visibility-changed",
                                VisibilityEventPayload {
                                    visible: global_eff,
                                    reason: if global_eff {
                                        "desktop_visible"
                                    } else {
                                        "occluded_or_paused"
                                    }
                                    .to_string(),
                                },
                            );
                        }
                    }
                }

                // Collect targeted recipients per visible monitor without cloning scene JSON
                // IMPORTANT: Drop state.lock() BEFORE calling outer_position() (which dispatches to the main UI thread)
                // to prevent AB-BA deadlocks with main-thread handlers acquiring state.lock().
                let (
                    mut telemetry_targets,
                    mut cursor_targets,
                    mut reactive_targets,
                    mut need_audio,
                    mut need_keyboard,
                    ready_widgets_snap,
                    monitors_snap,
                    mon_vis_snap,
                    desktop_vis_snap,
                    media_cache,
                    reactive_sensors_opt,
                ) = if let Ok(st) = state.lock() {
                    let mut t_targets = Vec::new();
                    let mut r_targets = Vec::new();
                    let mut want_audio = false;
                    let mut want_kbd = false;
                    if main_visible {
                        t_targets.push("main".to_string());
                        if st.studio_audio_active || st.studio_keyboard_active {
                            r_targets.push("main".to_string());
                            want_audio |= st.studio_audio_active;
                            want_kbd |= st.studio_keyboard_active;
                        }
                    }
                    let mut c_targets: Vec<(String, MonitorInfoDto)> = Vec::new();
                    let mut w_snap: Vec<(String, i32, i32, bool, bool, bool)> = Vec::new();

                    if !st.manual_paused {
                        for (lbl, mid) in &st.wallpaper_labels {
                            let mon_vis = st
                                .monitor_visibility
                                .get(mid)
                                .copied()
                                .unwrap_or(true);
                            if mon_vis && st.ready_windows.contains(lbl) {
                                t_targets.push(lbl.clone());
                                if st.parallax_targets.contains(lbl) {
                                    if let Some(mon) = st.monitors.iter().find(|m| &m.id == mid) {
                                        c_targets.push((lbl.clone(), mon.clone()));
                                    }
                                }
                                let wa = st.audio_targets.contains(lbl);
                                let wk = st.keyboard_targets.contains(lbl);
                                if wa || wk {
                                    r_targets.push(lbl.clone());
                                    want_audio |= wa;
                                    want_kbd |= wk;
                                }
                            }
                        }
                        for (lbl, wid) in &st.widget_labels {
                            if !st.ready_windows.contains(lbl) {
                                continue;
                            }
                            let (fx, fy) = st
                                .pinned_widgets
                                .get(wid)
                                .map(|p| {
                                    (
                                        p.pointer("/widget/posX")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0)
                                            as i32,
                                        p.pointer("/widget/posY")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(0.0)
                                            as i32,
                                    )
                                })
                                .unwrap_or((0, 0));
                            let has_parallax = st.parallax_targets.contains(lbl);
                            let has_audio = st.audio_targets.contains(lbl);
                            let has_kbd = st.keyboard_targets.contains(lbl);
                            w_snap.push((lbl.clone(), fx, fy, has_parallax, has_audio, has_kbd));
                        }
                    }

                    (
                        t_targets,
                        c_targets,
                        r_targets,
                        want_audio,
                        want_kbd,
                        w_snap,
                        st.monitors.clone(),
                        st.monitor_visibility.clone(),
                        st.desktop_visible,
                        Arc::clone(&st.media_cache),
                        st.reactive_sensors.clone(),
                    )
                } else {
                    (
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        false,
                        false,
                        Vec::new(),
                        Vec::new(),
                        HashMap::new(),
                        true,
                        Arc::new(Mutex::new(LiveMediaDto::default())),
                        None,
                    )
                };

                for (lbl, fx, fy, has_parallax, has_audio, has_kbd) in ready_widgets_snap {
                    let (wx, wy) = app
                        .get_webview_window(&lbl)
                        .and_then(|w| w.outer_position().ok())
                        .map(|p| (p.x, p.y))
                        .unwrap_or((fx, fy));
                    let mon_opt = desktop_host::monitor_id_for_point(&monitors_snap, wx, wy)
                        .and_then(|mid| monitors_snap.iter().find(|m| m.id == mid).cloned());
                    let mon_vis = mon_opt
                        .as_ref()
                        .and_then(|m| mon_vis_snap.get(&m.id).copied())
                        .unwrap_or(desktop_vis_snap);
                    if mon_vis {
                        telemetry_targets.push(lbl.clone());
                        if has_parallax {
                            if let Some(mon) = mon_opt {
                                cursor_targets.push((lbl.clone(), mon));
                            }
                        }
                        if has_audio || has_kbd {
                            reactive_targets.push(lbl);
                            need_audio |= has_audio;
                            need_keyboard |= has_kbd;
                        }
                    }
                }

                if !telemetry_targets.is_empty()
                    && now.duration_since(last_telemetry) >= Duration::from_millis(1500)
                {
                    last_telemetry = now;
                    let dto = build_telemetry_snapshot(&hostname, &media_cache);
                    for label in &telemetry_targets {
                        let _ = app.emit_to(label, "prism://telemetry", &dto);
                    }
                }

                // On-demand WASAPI Audio FFT & Anonymous Keyboard Rhythm emission (0% idle when unused/quiet)
                if let Some(ref sensors) = reactive_sensors_opt {
                    sensors.set_active(need_audio, need_keyboard);
                    if !reactive_targets.is_empty() && (need_audio || need_keyboard) {
                        let sig = sensors.snapshot(need_audio, need_keyboard);
                        let is_quiet = sig.is_quiet();
                        if !is_quiet || !last_reactive_quiet {
                            last_reactive_quiet = is_quiet;
                            for label in &reactive_targets {
                                let _ = app.emit_to(label, "prism://reactive", &sig);
                            }
                        }
                    } else {
                        last_reactive_quiet = true;
                    }
                }

                // Cursor polling strictly on demand for visible monitors with parallax or 3D bindings
                let fast_tick = !cursor_targets.is_empty()
                    || (!reactive_targets.is_empty() && (need_audio || need_keyboard));
                if !cursor_targets.is_empty() {
                    if let Some((cx, cy)) = desktop_host::poll_cursor_raw() {
                        let moved = match last_cursor_raw {
                            Some((px, py)) => (cx - px).abs() >= 3 || (cy - py).abs() >= 3,
                            None => true,
                        };
                        if moved {
                            last_cursor_raw = Some((cx, cy));
                            for (label, mon) in &cursor_targets {
                                let norm =
                                    desktop_host::normalize_cursor_for_monitor(cx, cy, mon);
                                let _ = app.emit_to(
                                    label,
                                    "prism://cursor",
                                    CursorEventPayload {
                                        x: norm[0],
                                        y: norm[1],
                                    },
                                );
                            }
                        }

                        // Desktop left-click edge detection for 3D raycast click triggers
                        let ldown = desktop_host::is_left_button_down();
                        if ldown && !prev_lbutton_down && desktop_host::is_point_on_desktop(cx, cy)
                        {
                            for (label, mon) in &cursor_targets {
                                let inside = cx >= mon.x
                                    && cx < mon.x + mon.width
                                    && cy >= mon.y
                                    && cy < mon.y + mon.height;
                                if inside {
                                    let norm =
                                        desktop_host::normalize_cursor_for_monitor(cx, cy, mon);
                                    let _ = app.emit_to(
                                        label,
                                        "prism://cursor-click",
                                        CursorEventPayload {
                                            x: norm[0],
                                            y: norm[1],
                                        },
                                    );
                                }
                            }
                        }
                        prev_lbutton_down = ldown;
                    }
                } else {
                    prev_lbutton_down = false;
                }

                if fast_tick {
                    thread::sleep(Duration::from_millis(24));
                } else {
                    thread::sleep(Duration::from_millis(250));
                }
            }
        })
        .ok();
}

#[tauri::command]
fn media_control(state: tauri::State<'_, SharedState>, action: String) -> Result<(), String> {
    let cmd = match action.trim().to_ascii_lowercase().as_str() {
        "play_pause" | "toggle" => MediaCommand::PlayPause,
        "next" => MediaCommand::Next,
        "prev" | "previous" => MediaCommand::Prev,
        _ => return Err("Ação de mídia desconhecida".to_string()),
    };
    let handle = {
        let st = state.lock().map_err(|e| e.to_string())?;
        st.gsmtc.clone()
    };
    if let Some(h) = handle {
        h.send_command(cmd);
    }
    Ok(())
}

#[tauri::command]
fn set_studio_reactive_sensors(
    state: tauri::State<'_, SharedState>,
    audio: bool,
    keyboard: bool,
) -> Result<(), String> {
    let mut st = state.lock().map_err(|e| e.to_string())?;
    st.studio_audio_active = audio;
    st.studio_keyboard_active = keyboard;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    desktop_host::enable_per_monitor_dpi();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let cover_dir = prismview_root_dir().join("assets");
    let gsmtc = desktop_host::spawn_gsmtc_worker(Arc::clone(&stop_flag), cover_dir);
    let reactive_sensors = reactive_sensors::spawn_reactive_sensors(Arc::clone(&stop_flag));
    let media_cache = Arc::clone(&gsmtc.media_cache);
    let shared_state: SharedState = Arc::new(Mutex::new(AppRuntimeState {
        desktop_visible: true,
        media_cache,
        gsmtc: Some(gsmtc),
        reactive_sensors: Some(reactive_sensors),
        ..AppRuntimeState::default()
    }));

    let state_for_setup = Arc::clone(&shared_state);
    let stop_for_setup = Arc::clone(&stop_flag);
    let state_for_events = Arc::clone(&shared_state);
    let state_for_run = Arc::clone(&shared_state);
    let stop_for_run = Arc::clone(&stop_flag);

    let app = tauri::Builder::default()
        .manage(shared_state)
        .manage(native_engine::new_state())
        .setup(move |app| {
            let _ = ASSET_SCOPE.set(app.asset_protocol_scope());
            let open_item =
                MenuItem::with_id(app, "open_studio", "Abrir PrismView", true, None::<&str>)?;
            let toggle_pause =
                MenuItem::with_id(app, "toggle_pause", "Pausar / Retomar", true, None::<&str>)?;
            let toggle_autostart = MenuItem::with_id(
                app,
                "toggle_autostart",
                if is_autostart_enabled_inner() {
                    "✓ Iniciar com o Windows"
                } else {
                    "Iniciar com o Windows"
                },
                true,
                None::<&str>,
            )?;
            let unlock_widgets =
                MenuItem::with_id(app, "unlock_widgets", "Destravar Widgets", true, None::<&str>)?;
            let clear_wp =
                MenuItem::with_id(app, "clear_wp", "Remover Wallpaper", true, None::<&str>)?;
            let quit_item =
                MenuItem::with_id(app, "quit_all", "Sair", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &open_item,
                    &toggle_pause,
                    &toggle_autostart,
                    &unlock_widgets,
                    &clear_wp,
                    &quit_item,
                ],
            )?;

            let st_menu = Arc::clone(&state_for_setup);
            let autostart_item = toggle_autostart.clone();
            let mut tray_builder = TrayIconBuilder::with_id("studio_tray")
                .menu(&menu)
                .tooltip("PrismView")
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "open_studio" => open_or_focus_studio(app),
                    "toggle_pause" => {
                        let next_paused = st_menu
                            .lock()
                            .map(|st| !st.manual_paused)
                            .unwrap_or(false);
                        let _ = apply_manual_pause_inner(app, &st_menu, next_paused);
                    }
                    "toggle_autostart" => {
                        let next = !is_autostart_enabled_inner();
                        let has_widgets = st_menu
                            .lock()
                            .map(|st| !st.pinned_widgets.is_empty())
                            .unwrap_or(false);
                        if let Ok(enabled) = set_autostart_enabled_inner(next, has_widgets) {
                            let _ = autostart_item.set_text(if enabled {
                                "✓ Iniciar com o Windows"
                            } else {
                                "Iniciar com o Windows"
                            });
                        }
                    }
                    "unlock_widgets" => unlock_all_widgets_inner(app, &st_menu),
                    "clear_wp" => {
                        let _ = clear_wallpaper_inner(app, &st_menu, None);
                    }
                    "quit_all" => {
                        flush_dirty_widget_saves(&st_menu, true);
                        if let Ok(mut st) = st_menu.lock() {
                            st.allow_exit = true;
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        open_or_focus_studio(tray.app_handle());
                    }
                });

            if let Some(icon) = app.default_window_icon().cloned() {
                tray_builder = tray_builder.icon(icon);
            }
            let studio_tray = tray_builder.build(app)?;
            // Inicia oculto até sincronizar com widgets/engine (evita flash de 2 ícones).
            let _ = studio_tray.set_visible(false);

            // Enumerar monitores físicos iniciais e restaurar última sessão (compatível com schema legado e multi-monitor)
            let initial_monitors = desktop_host::enumerate_monitors();
            if let Ok(mut st) = state_for_setup.lock() {
                for m in &initial_monitors {
                    st.monitor_visibility.insert(m.id.clone(), true);
                }
                st.monitors = initial_monitors.clone();
            }

            let mut restored_any = false;
            // Sessão antiga (wallpaper WebView, sem cena nativa): migra aplicando no motor.
            let mut legacy_wallpaper: Option<Value> = None;
            if let Some(session) = load_persisted_session() {
                if let Ok(mut st) = state_for_setup.lock() {
                    st.manual_paused = session.paused;
                }
                // Wallpaper: só o id/projeto em memória (UI sabe qual está ativo e re-aplica ao
                // salvar). Quem desenha é o motor nativo, que já relê `active_scene.json`.
                let wallpaper_proj = session
                    .active_wallpaper_id
                    .as_deref()
                    .and_then(load_project_from_disk)
                    .or(session.active_wallpaper_snapshot)
                    .or_else(|| {
                        session
                            .monitor_wallpaper_ids
                            .values()
                            .next()
                            .and_then(|id| load_project_from_disk(id))
                    });
                if let Some(wp) = wallpaper_proj {
                    if !native_engine::has_active_scene() {
                        legacy_wallpaper = Some(wp.clone());
                    }
                    if let Ok(mut st) = state_for_setup.lock() {
                        st.active_wallpaper = Some(wp);
                    }
                }
                for wid in session.pinned_widget_ids {
                    if let Some(w_proj) = load_project_from_disk(&wid) {
                        if pin_widget_inner(app.handle(), &state_for_setup, w_proj, false).is_ok() {
                            restored_any = true;
                        }
                    }
                }
                if session.paused {
                    let _ = app.emit(
                        "prism://visibility-changed",
                        VisibilityEventPayload {
                            visible: false,
                            reason: "manual_pause".to_string(),
                        },
                    );
                }
            }

            {
                let engine = Arc::clone(app.state::<native_engine::SharedEngine>().inner());
                let state_for_migration = Arc::clone(&state_for_setup);
                let handle_for_migration = app.handle().clone();
                std::thread::spawn(move || {
                    let migrating = legacy_wallpaper.is_some();
                    let result = match legacy_wallpaper {
                        Some(project) => engine.apply_project(&project),
                        None => engine.ensure_running(),
                    };
                    sync_studio_tray_visibility(&handle_for_migration, &state_for_migration);
                    let Err(err) = result else { return };
                    eprintln!("[prismview-studio] motor nativo: {err}");
                    if !migrating {
                        return;
                    }
                    // Migração falhou: o Hub não pode fingir que o wallpaper legado está aplicado.
                    let notice = "Seu wallpaper antigo usa o formato legado (WebView) e não pôde ser migrado para o motor nativo. Abra-o no Estúdio e use \"Aplicar\" novamente para reaplicá-lo."
                        .to_string();
                    if let Ok(mut st) = state_for_migration.lock() {
                        st.active_wallpaper = None;
                        st.monitor_wallpapers.clear();
                        st.legacy_migration_notice = Some(notice.clone());
                        save_session_from_state(&st);
                    }
                    let _ = handle_for_migration.emit("prism://legacy-migration-failed", notice);
                });
            }
            // Migração: entrada antiga do Studio no login → motor nativo no login.
            if run_value_present(STUDIO_RUN_VALUE)
                && !is_autostart_enabled_inner()
                && !native_engine::autostart_opted_out()
            {
                let has_widgets = state_for_setup
                    .lock()
                    .map(|st| !st.pinned_widgets.is_empty())
                    .unwrap_or(false);
                let _ = set_autostart_enabled_inner(true, has_widgets);
            }

            let start_minimized = std::env::args().any(|a| a == "--minimized");
            if start_minimized && restored_any {
                if let Some(main_win) = app.get_webview_window("main") {
                    let _ = main_win.hide();
                }
            }

            // Auto-recovery Win32 listener (WM_DISPLAYCHANGE + TaskbarCreated)
            let state_for_recovery = Arc::clone(&state_for_setup);
            desktop_host::spawn_desktop_recovery_listener(
                Arc::clone(&stop_for_setup),
                move |_reason| {
                    if let Ok(mut st) = state_for_recovery.lock() {
                        st.pending_recovery_at = Some(Instant::now());
                    }
                },
            );

            spawn_background_monitors(app.handle().clone(), state_for_setup, stop_for_setup);
            Ok(())
        })
        .on_window_event(move |window, event| {
            let label = window.label().to_string();
            if let WindowEvent::Destroyed = event {
                if let Ok(mut st) = state_for_events.lock() {
                    st.ready_windows.remove(&label);
                    if let Some(wid) = st.widget_labels.remove(&label) {
                        st.dirty_widget_saves.remove(&wid);
                        st.pinned_widgets.remove(&wid);
                        refresh_parallax_targets(&mut st);
                    } else if st.wallpaper_labels.remove(&label).is_some() {
                        // Keep st.monitor_wallpapers intact so explorer.exe / WorkerW crash recovery
                        // (TaskbarCreated) and tray residency can respawn the wallpaper automatically.
                        refresh_parallax_targets(&mut st);
                    }
                }
                return;
            }
            if label.starts_with("widget-") {
                if let WindowEvent::Moved(pos) = event {
                    // Store physical virtual-desktop coordinates directly (matching PhysicalPosition & monitor_id_for_point)
                    // so mixed-DPI multi-monitor setups never drift on restart.
                    if let Ok(mut st) = state_for_events.lock() {
                        if let Some(wid) = st.widget_labels.get(&label).cloned() {
                            if let Some(proj) = st.pinned_widgets.get_mut(&wid) {
                                if let Some(w_obj) =
                                    proj.get_mut("widget").and_then(|v| v.as_object_mut())
                                {
                                    w_obj.insert("posX".to_string(), Value::from(pos.x));
                                    w_obj.insert("posY".to_string(), Value::from(pos.y));
                                }
                                st.dirty_widget_saves.insert(wid, Instant::now());
                            }
                        }
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_projects,
            save_project,
            delete_project,
            list_monitors,
            list_system_fonts,
            apply_desktop_wallpaper,
            clear_desktop_wallpaper,
            pin_standalone_widget,
            unpin_standalone_widget,
            set_widget_locked,
            widget_window_ready,
            set_engine_paused,
            get_engine_tray_enabled,
            set_engine_tray_visible,
            get_autostart_enabled,
            set_autostart_enabled,
            get_runtime_state,
            dismiss_legacy_migration_notice,
            export_prism_archive,
            pick_save_archive_path,
            import_prism_archive,
            pick_local_file,
            get_live_telemetry,
            media_control,
            set_studio_reactive_sensors,
            native_engine::native_engine_start,
            native_engine::native_engine_stop,
            native_engine::native_engine_clear,
            native_engine::native_engine_status,
            native_engine::native_engine_apply,
            native_engine::native_engine_set_power,
        ])
        .build(tauri::generate_context!())
        .expect("Erro ao construir PrismView");

    app.run(move |_handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            flush_dirty_widget_saves(&state_for_run, true);
            let (should_stay_resident, gsmtc_opt) = state_for_run
                .lock()
                .map(|st| {
                    let stay = !st.allow_exit && !st.pinned_widgets.is_empty();
                    (stay, st.gsmtc.clone())
                })
                .unwrap_or((false, None));
            if should_stay_resident {
                api.prevent_exit();
            } else {
                stop_for_run.store(true, Ordering::Relaxed);
                if let Some(g) = gsmtc_opt {
                    g.wake();
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_subpath_blocks_traversal_and_outside_roots() {
        assert!(safe_relative_subpath("assets/0_video.mp4").is_some());
        assert!(safe_relative_subpath("fonts/1_Outfit.ttf").is_some());
        assert!(safe_relative_subpath("assets/sub/img.png").is_some());
        assert!(safe_relative_subpath("assets/../secret.txt").is_none());
        assert!(safe_relative_subpath("assets/..\\..\\Windows\\win.ini").is_none());
        assert!(safe_relative_subpath("C:\\Windows\\System32\\cmd.exe").is_none());
        assert!(safe_relative_subpath("/etc/passwd").is_none());
        assert!(safe_relative_subpath("project.json").is_none());
    }

    #[test]
    fn percent_encode_query_escapes_special_chars() {
        assert_eq!(percent_encode_query("DISPLAY1"), "DISPLAY1");
        assert_eq!(percent_encode_query("w#1&a=b?c%20"), "w%231%26a%3Db%3Fc%2520");
    }

    #[test]
    fn memory_and_cpu_sampling_are_bounded() {
        let (ram_pct, used_gb, total_gb) = desktop_host::sample_memory_usage();
        assert!(total_gb > 0.0);
        assert!(used_gb >= 0.0 && used_gb <= total_gb + 0.5);
        assert!((0.0..=100.0).contains(&ram_pct));

        let (cpu1, bat1, _) = desktop_host::sample_cpu_and_battery();
        let (cpu2, bat2, _) = desktop_host::sample_cpu_and_battery();
        assert!((0.0..=100.0).contains(&cpu1));
        assert!((0.0..=100.0).contains(&cpu2));
        assert!(bat1 <= 100 && bat2 <= 100);
    }

    #[test]
    fn collect_import_uuids_handles_nested_groups_and_paths() {
        let prefix = r"c:\users\test\appdata\roaming\prismview\imports\";
        let proj = serde_json::json!({
            "importDir": "uuid-root-1",
            "layers": [
                { "video": { "src": r"C:\Users\Test\AppData\Roaming\PrismView\imports\uuid-vid-2\assets\bg.mp4" } },
                { "scene3d": {
                    "modelSrc": r"C:\Users\Test\AppData\Roaming\PrismView\imports\uuid-glb-4\assets\hero.glb",
                    "objects": [
                        {
                            "id": "obj-2",
                            "modelSrc": r"C:\Users\Test\AppData\Roaming\PrismView\imports\uuid-obj-5\assets\desk.glb",
                            "dynamicTextures": [
                                { "customSrc": r"C:\Users\Test\AppData\Roaming\PrismView\imports\uuid-tex-6\assets\screen.png" }
                            ]
                        }
                    ],
                    "camera": { "orbitParallaxX": 12.0, "orbitParallaxY": 0.0 }
                } },
                { "children": [
                    { "text": { "fontPath": "C:/Users/Test/AppData/Roaming/PrismView/imports/uuid-font-3/fonts/f.ttf" } },
                    { "image": { "src": r"C:\Users\Test\Pictures\local.png" } }
                ]}
            ]
        });
        let mut out = HashSet::new();
        collect_project_import_uuids(&proj, prefix, &mut out);
        assert!(out.contains("uuid-root-1"));
        assert!(out.contains("uuid-vid-2"));
        assert!(out.contains("uuid-font-3"));
        assert!(out.contains("uuid-glb-4"));
        assert!(out.contains("uuid-obj-5"));
        assert!(out.contains("uuid-tex-6"));
        assert_eq!(out.len(), 6);
        let (has_cursor, has_audio, has_kbd) = project_reactive_flags(&proj);
        assert!(has_cursor);
        assert!(!has_audio);
        assert!(!has_kbd);
    }

    #[test]
    fn export_bundles_every_scene3d_object_glb_and_remap_restores_both() {
        let base = std::env::temp_dir().join(format!("prismview-test-{}", Uuid::new_v4()));
        let src_dir = base.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let a = src_dir.join("hero.glb");
        // mesmo nome de arquivo em pastas diferentes não pode colidir
        let sub = src_dir.join("other");
        fs::create_dir_all(&sub).unwrap();
        let b2 = sub.join("hero.glb");
        fs::write(&a, b"AAAA").unwrap();
        fs::write(&b2, b"BBBBBB").unwrap();
        let project = serde_json::json!({
            "id": "p1",
            "title": "T",
            "layers": [{ "scene3d": { "objects": [
                { "id": "o1", "modelSrc": a.to_string_lossy() },
                { "id": "o2", "modelSrc": b2.to_string_lossy() },
                { "id": "o3", "modelSrc": a.to_string_lossy() }
            ] } }]
        });
        let dest = base.join("out.prism");
        export_prism_archive_sync(project, Some(dest.to_string_lossy().to_string())).unwrap();

        let mut zip = ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let mut raw = String::new();
        zip.by_name("project.json")
            .unwrap()
            .read_to_string(&mut raw)
            .unwrap();
        let exported: Value = serde_json::from_str(&raw).unwrap();
        let objs = exported.pointer("/layers/0/scene3d/objects").unwrap().as_array().unwrap();
        let r1 = objs[0]["modelSrc"].as_str().unwrap().to_string();
        let r2 = objs[1]["modelSrc"].as_str().unwrap().to_string();
        let r3 = objs[2]["modelSrc"].as_str().unwrap().to_string();
        assert!(r1.starts_with("assets/") && r2.starts_with("assets/"));
        assert_ne!(r1, r2);
        assert_eq!(r1, r3);

        let extract = base.join("extract");
        for rel in [&r1, &r2] {
            let safe = safe_relative_subpath(rel).unwrap();
            let out = extract.join(safe);
            fs::create_dir_all(out.parent().unwrap()).unwrap();
            let mut data = Vec::new();
            zip.by_name(rel).unwrap().read_to_end(&mut data).unwrap();
            fs::write(out, data).unwrap();
        }
        let mut layers = exported["layers"].as_array().unwrap().clone();
        remap_imported_layers(&mut layers, &extract);
        let objs = layers[0].pointer("/scene3d/objects").unwrap().as_array().unwrap();
        let p1 = PathBuf::from(objs[0]["modelSrc"].as_str().unwrap());
        let p2 = PathBuf::from(objs[1]["modelSrc"].as_str().unwrap());
        assert_eq!(fs::read(&p1).unwrap(), b"AAAA");
        assert_eq!(fs::read(&p2).unwrap(), b"BBBBBB");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn reactive_fft_separates_bass_and_treble() {
        let sr = 48_000u32;
        let mut bass_wave = [0.0f32; 256];
        let mut treble_wave = [0.0f32; 256];
        for i in 0..256 {
            let t = i as f32 / sr as f32;
            bass_wave[i] = (2.0 * std::f32::consts::PI * 187.5 * t).sin() * 0.8;
            treble_wave[i] = (2.0 * std::f32::consts::PI * 6000.0 * t).sin() * 0.8;
        }
        let (b1, _m1, t1, l1) = reactive_sensors::compute_fft_bands(&bass_wave, sr);
        let (b2, _m2, t2, l2) = reactive_sensors::compute_fft_bands(&treble_wave, sr);
        assert!(b1 > t1 && b1 > 0.25);
        assert!(t2 > b2 && t2 > 0.25);
        assert!(l1 > 0.2 && l2 > 0.2);
    }
}
