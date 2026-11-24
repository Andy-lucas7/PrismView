mod intro_gate;
mod atomic_fs;
mod bundled_fonts;
mod desktop_host;
mod log_file;
mod native_engine;
mod native_scene;
mod reactive_sensors;
mod vis_events;
mod projects;
mod package;
mod session;
mod widgets;
mod tray;
mod bg_monitor;

use bg_monitor::*;
use tray::*;
use widgets::*;
use session::*;
use package::*;
use projects::*;
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
    /// Aviso quando a migração automática de um wallpaper antigo (WebView) para o motor nativo falhou.
    #[serde(default)]
    pub legacy_migration_notice: Option<String>,
    /// Avisos do último "Aplicar" (arquivo ausente etc.); vazio fora dele.
    #[serde(default)]
    pub warnings: Vec<String>,
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

#[derive(Default)]
struct AppRuntimeState {
    active_wallpaper: Option<Value>,
    monitors: Vec<MonitorInfoDto>,
    monitor_visibility: HashMap<String, bool>,   // monitor_id -> desktop uncovered
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
    pending_recovery_at: Option<Instant>,
    force_vis_resync: bool,
    manual_paused: bool,
    desktop_visible: bool,
    allow_exit: bool,
    /// Logoff/desligamento ou saída do app em curso: janelas de widget destruídas agora NÃO
    /// significam "desafixar" (senão a sessão seria salva sem widgets).
    shutting_down: bool,
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
const REG_EXPAND_SZ: u32 = 2;
const ERROR_SUCCESS: i32 = 0;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_NO_MORE_ITEMS: i32 = 259;

fn is_supported_outline_font(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("ttf" | "otf" | "ttc" | "otc")
    )
}

fn system_fonts_dir() -> PathBuf {
    std::env::var_os("SYSTEMROOT")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(|root| PathBuf::from(root).join("Fonts"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows\Fonts"))
}

fn user_fonts_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|local| PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"))
}

fn registry_font_exists(raw_value: &str, is_hkcu: bool, sys_dir: &Path, user_dir: Option<&Path>) -> bool {
    let trimmed = raw_value.trim().trim_matches('"');
    if trimmed.is_empty() {
        return false;
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return is_supported_outline_font(candidate) && candidate.is_file();
    }
    if is_hkcu {
        if let Some(dir) = user_dir {
            let p = dir.join(trimmed);
            if is_supported_outline_font(&p) && p.is_file() {
                return true;
            }
        }
    }
    let sys = sys_dir.join(trimmed);
    if is_supported_outline_font(&sys) && sys.is_file() {
        return true;
    }
    if !is_hkcu {
        if let Some(dir) = user_dir {
            let p = dir.join(trimmed);
            if is_supported_outline_font(&p) && p.is_file() {
                return true;
            }
        }
    }
    false
}

fn normalize_registry_font_family(raw_name: &str) -> Option<String> {
    let mut name = raw_name.trim().to_string();
    for suffix in [" (TrueType)", " (OpenType)", " (All res)", " (VGA res)", " (Type 1)"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.trim().to_string();
        }
    }
    if let Some((first, _)) = name.split_once(" & ") {
        name = first.trim().to_string();
    }
    let style_suffixes = [
        "Bold Italic",
        "SemiBold Italic",
        "Semibold Italic",
        "Light Italic",
        "Medium Italic",
        "Black Italic",
        "Thin Italic",
        "ExtraLight Italic",
        "ExtraBold Italic",
        "UltraLight Italic",
        "UltraBold Italic",
        "Bold",
        "Italic",
        "Oblique",
        "Regular",
        "Semibold",
        "SemiBold",
        "Semilight",
        "SemiLight",
        "ExtraLight",
        "ExtraBold",
        "UltraLight",
        "UltraBold",
        "Light",
        "Medium",
        "Black",
        "Thin",
        "Heavy",
        "Book",
        "Demibold",
        "DemiBold",
        "Condensed",
        "Narrow",
        "Normal",
    ];
    let mut changed = true;
    while changed {
        changed = false;
        for suf in &style_suffixes {
            let matched = [' ', '-', '_'].into_iter().find_map(|sep| {
                let with_sep = format!("{sep}{suf}");
                name.strip_suffix(&with_sep)
            });
            if let Some(stripped) = matched {
                let trimmed = stripped.trim();
                if !trimmed.is_empty() {
                    name = trimmed.to_string();
                    changed = true;
                    break;
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

fn collect_fonts_from_hive(hive: isize, is_hkcu: bool, sys_dir: &Path, user_dir: Option<&Path>, out: &mut HashSet<String>) {
    let subkey = wide_null(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts");
    let mut hkey: isize = 0;
    let status = unsafe { RegOpenKeyExW(hive, subkey.as_ptr(), 0, KEY_READ, &mut hkey) };
    if status != ERROR_SUCCESS {
        return;
    }
    let mut index: u32 = 0;
    let mut name_buf = vec![0u16; 512];
    let mut data_buf = vec![0u16; 512];
    loop {
        let mut name_len = name_buf.len() as u32;
        let mut val_type: u32 = 0;
        let mut data_bytes = (data_buf.len() * std::mem::size_of::<u16>()) as u32;
        let res = unsafe {
            RegEnumValueW(
                hkey,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                std::ptr::null_mut(),
                &mut val_type,
                data_buf.as_mut_ptr() as *mut u8,
                &mut data_bytes,
            )
        };
        if res == ERROR_NO_MORE_ITEMS {
            break;
        }
        if res == ERROR_SUCCESS && (val_type == REG_SZ || val_type == REG_EXPAND_SZ) {
            let raw = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            let u16_len = (data_bytes as usize / 2).min(data_buf.len());
            let trimmed_len = data_buf[..u16_len]
                .iter()
                .rposition(|&c| c != 0)
                .map_or(0, |pos| pos + 1);
            let raw_value = String::from_utf16_lossy(&data_buf[..trimmed_len]);
            if registry_font_exists(&raw_value, is_hkcu, sys_dir, user_dir) {
                if let Some(clean) = normalize_registry_font_family(&raw) {
                    out.insert(clean);
                }
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
        let sys_dir = system_fonts_dir();
        let user_dir = user_fonts_dir();
        collect_fonts_from_hive(HKEY_LOCAL_MACHINE, false, &sys_dir, user_dir.as_deref(), &mut set);
        collect_fonts_from_hive(HKEY_CURRENT_USER, true, &sys_dir, user_dir.as_deref(), &mut set);
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
            atomic_fs::log_io("create_dir_all", fs::create_dir_all(root.join("projects")));
            atomic_fs::log_io("create_dir_all", fs::create_dir_all(root.join("packages")));
            atomic_fs::log_io("create_dir_all", fs::create_dir_all(root.join("imports")));
            atomic_fs::log_io("create_dir_all", fs::create_dir_all(root.join("fonts")));
            atomic_fs::log_io("create_dir_all", fs::create_dir_all(root.join("assets")));
            root
        })
        .clone()
}

fn open_or_focus_studio(app: &AppHandle) {
    wake_background();
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

fn ensure_monitors_loaded(st: &mut AppRuntimeState) -> Vec<MonitorInfoDto> {
    if st.monitors.is_empty() {
        st.monitors = desktop_host::enumerate_monitors();
        for m in &st.monitors {
            st.monitor_visibility.entry(m.id.clone()).or_insert(true);
        }
    }
    st.monitors.clone()
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
    let active_wallpaper_id = st
        .active_wallpaper
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    RuntimeStateDto {
        paused: st.manual_paused,
        desktop_visible: st.desktop_visible,
        active_wallpaper_id,
        pinned_widget_ids: st.pinned_widgets.keys().cloned().collect(),
        locked_widget_ids,
        monitors: st.monitors.clone(),
        legacy_migration_notice: st.legacy_migration_notice.clone(),
        warnings: Vec::new(),
    }
}

/// Remove o wallpaper do desktop: motor nativo encerrado + cena ativa apagada + estado limpo.
fn clear_wallpaper_inner(state: &SharedState) -> Result<(), String> {
    native_engine::clear_desktop()?;
    if let Ok(mut st) = state.lock() {
        st.active_wallpaper = None;
        save_session_from_state(&st);
    }
    Ok(())
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
    autosave: Option<bool>,
) -> Result<RuntimeStateDto, String> {
    let state_arc = Arc::clone(state.inner());
    let engine_arc = Arc::clone(engine.inner());
    let _ = monitor_id;
    tauri::async_runtime::spawn_blocking(move || {
        // Autosave do wallpaper ativo: só cena + LOAD_SCENE_FILE; o `.prism` em `packages/`
        // é refeito apenas no "Aplicar" explícito (re-zipar vídeos grandes a cada pausa custa CPU/SSD).
        let warnings = engine_arc.apply_project(&project, !autosave.unwrap_or(false))?;
        let has_widgets = {
            let mut st = state_arc.lock().map_err(|e| e.to_string())?;
            st.active_wallpaper = Some(project);
            save_session_from_state(&st);
            !st.pinned_widgets.is_empty()
        };
        ensure_engine_login_autostart(has_widgets);
        sync_studio_tray_visibility(&app, &state_arc);
        let st = state_arc.lock().map_err(|e| e.to_string())?;
        let mut dto = build_runtime_state(&st);
        dto.warnings = warnings;
        Ok(dto)
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
        clear_wallpaper_inner(&state_arc)?;
        sync_studio_tray_visibility(&app, &state_arc);
        let st = state_arc.lock().map_err(|e| e.to_string())?;
        Ok(build_runtime_state(&st))
    })
    .await
    .map_err(|e| e.to_string())?
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
fn get_live_telemetry(state: tauri::State<'_, SharedState>) -> Result<LiveTelemetryDto, String> {
    let media_cache = {
        let st = state.lock().map_err(|e| e.to_string())?;
        Arc::clone(&st.media_cache)
    };
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".to_string());
    Ok(build_telemetry_snapshot(&hostname, &media_cache))
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
    // Sem console: `eprintln!`/pânicos vão para `%APPDATA%\PrismView\studio.log`.
    if let Err(err) = log_file::redirect_stderr_to_file(&prismview_root_dir(), "studio.log") {
        eprintln!("[prismview-studio] log: {err}");
    }
    eprintln!("[prismview-studio] início pid={} t={}", std::process::id(), atomic_fs::unix_secs());
    // Fontes padrão no lugar antes de o motor subir (ele registra `fonts\` ao criar o renderer).
    bundled_fonts::install(&prismview_root_dir().join("fonts"));
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
        // Instância única: o 2º clique no atalho só foca o Studio existente (registrar primeiro).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            open_or_focus_studio(app);
        }))
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
                        if let Err(err) = clear_wallpaper_inner(&st_menu) {
                            eprintln!("[prismview-studio] limpar wallpaper: {err}");
                        }
                    }
                    "quit_all" => {
                        flush_dirty_widget_saves(&st_menu, true);
                        if let Ok(mut st) = st_menu.lock() {
                            st.allow_exit = true;
                            st.shutting_down = true;
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
                        Some(project) => engine.apply_project(&project, true).map(|_| ()),
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
                move |reason| {
                    if let Ok(mut st) = state_for_recovery.lock() {
                        if reason == "end_session" {
                            st.shutting_down = true;
                        } else if reason == "end_session_cancelled" {
                            st.shutting_down = st.allow_exit;
                        } else {
                            st.pending_recovery_at = Some(Instant::now());
                            wake_background();
                        }
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
                    if st.shutting_down {
                        // Logoff/saída: a janela morre mas o widget continua fixado na sessão.
                        st.widget_labels.remove(&label);
                    } else if let Some(wid) = st.widget_labels.remove(&label) {
                        st.dirty_widget_saves.remove(&wid);
                        st.pinned_widgets.remove(&wid);
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
                                wake_background();
                            }
                        }
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            intro_gate::get_session_marker,
            projects::list_projects,
            projects::save_project,
            projects::delete_project,
            list_monitors,
            list_system_fonts,
            apply_desktop_wallpaper,
            clear_desktop_wallpaper,
            widgets::pin_standalone_widget,
            widgets::unpin_standalone_widget,
            widgets::set_widget_locked,
            widgets::widget_window_ready,
            tray::set_engine_paused,
            tray::get_engine_tray_enabled,
            tray::set_engine_tray_visible,
            session::get_autostart_enabled,
            session::set_autostart_enabled,
            get_runtime_state,
            dismiss_legacy_migration_notice,
            package::export_prism_archive,
            package::pick_save_archive_path,
            package::import_prism_archive,
            package::pick_local_file,
            get_live_telemetry,
            media_control,
            set_studio_reactive_sensors,
            native_engine::native_engine_start,
            native_engine::native_engine_stop,
            native_engine::native_engine_clear,
            native_engine::native_engine_status,
            native_engine::native_engine_apply,
            native_engine::native_engine_set_power,
            native_engine::get_sync_windows_wallpaper,
            native_engine::set_sync_windows_wallpaper,
        ])
        .build(tauri::generate_context!())
        .expect("Erro ao construir PrismView");

    app.run(move |_handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            flush_dirty_widget_saves(&state_for_run, true);
            let (should_stay_resident, gsmtc_opt) = state_for_run
                .lock()
                .map(|mut st| {
                    let stay = !st.allow_exit && !st.pinned_widgets.is_empty();
                    if !stay {
                        st.shutting_down = true;
                    }
                    (stay, st.gsmtc.clone())
                })
                .unwrap_or((false, None));
            if should_stay_resident {
                api.prevent_exit();
            } else {
                stop_for_run.store(true, Ordering::Relaxed);
                wake_background();
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
    fn imported_open_app_actions_are_disabled_for_review() {
        let mut layers = vec![serde_json::json!({
            "id": "g",
            "clickAction": "OPEN_URL",
            "children": [{ "id": "a", "clickAction": "OPEN_APP", "clickTarget": "\\\\evil\\s\\a.exe" }]
        })];
        assert_eq!(quarantine_open_app_actions(&mut layers), 1);
        let child = &layers[0]["children"][0];
        assert_eq!(child["clickTarget"], "");
        assert_eq!(child["clickTargetPending"], "\\\\evil\\s\\a.exe");
        assert_eq!(child["clickTargetNeedsReview"], true);
        assert!(layers[0].get("clickTargetNeedsReview").is_none());
    }

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

    #[test]
    fn font_filter_rejects_fon_and_normalizes_hyphen_styles() {
        assert!(!is_supported_outline_font(Path::new("SMALLE.FON")));
        assert!(!is_supported_outline_font(Path::new("modern.fon")));
        assert!(is_supported_outline_font(Path::new("Inter-400.ttf")));
        assert!(is_supported_outline_font(Path::new("samsungsharpsans.otf")));
        assert!(is_supported_outline_font(Path::new("cambria.ttc")));

        assert_eq!(
            normalize_registry_font_family("Cairo-Regular (TrueType)").as_deref(),
            Some("Cairo")
        );
        assert_eq!(
            normalize_registry_font_family("Unispace-Bold (TrueType)").as_deref(),
            Some("Unispace")
        );
        assert_eq!(
            normalize_registry_font_family("MingLiU-ExtB & PMingLiU-ExtB (TrueType)").as_deref(),
            Some("MingLiU-ExtB")
        );
        assert_eq!(normalize_registry_font_family("Wingdings (TrueType)"), None);
    }
}
