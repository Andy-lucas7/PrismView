//! Sessão persistida (session.json) e entradas de login (HKCU Run).

#[allow(unused_imports)]
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PersistedSession {
    #[serde(default)]
    pub(crate) active_wallpaper_id: Option<String>,
    #[serde(default)]
    pub(crate) active_wallpaper_snapshot: Option<Value>,
    #[serde(default)]
    pub(crate) monitor_wallpaper_ids: HashMap<String, String>,
    #[serde(default)]
    pub(crate) monitor_wallpaper_snapshots: HashMap<String, Value>,
    #[serde(default)]
    pub(crate) pinned_widget_ids: Vec<String>,
    #[serde(default)]
    pub(crate) paused: bool,
}

/// Valor legado: Studio `--minimized` no login. Hoje só existe se houver widgets fixados
/// (o wallpaper é do motor nativo, que sobe sozinho).
pub(crate) const STUDIO_RUN_VALUE: &str = "PrismView";

pub(crate) fn run_value_present(name: &str) -> bool {
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
pub(crate) fn write_run_value(name: &str, command: Option<&str>) -> Result<(), String> {
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
pub(crate) fn is_autostart_enabled_inner() -> bool {
    run_value_present(native_engine::ENGINE_RUN_VALUE)
}

/// Liga/desliga o motor no login. O Studio só entra no login se houver widgets fixados (são
/// janelas dele); a escolha do usuário fica gravada para o "Aplicar" não religar sozinho.
pub(crate) fn set_autostart_enabled_inner(enabled: bool, has_widgets: bool) -> Result<bool, String> {
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
pub(crate) fn ensure_engine_login_autostart(has_widgets: bool) {
    if native_engine::autostart_opted_out() || is_autostart_enabled_inner() {
        return;
    }
    if let Err(err) = set_autostart_enabled_inner(true, has_widgets) {
        eprintln!("[prismview-studio] autostart do motor: {err}");
    }
}

/// Entrada legada do Studio no login: só quando há widgets (senão a janela principal abriria).
pub(crate) fn sync_studio_login_entry(want: bool) {
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

pub(crate) fn session_path() -> PathBuf {
    prismview_root_dir().join("session.json")
}

pub(crate) fn save_session_from_state(st: &AppRuntimeState) {
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
    let mut pinned_widget_ids: Vec<String> = st.pinned_widgets.keys().cloned().collect();
    pinned_widget_ids.sort();
    let session = PersistedSession {
        active_wallpaper_id,
        active_wallpaper_snapshot,
        // Legado (wallpaper WebView por monitor): só lido na migração, nunca mais gravado.
        monitor_wallpaper_ids: HashMap::new(),
        monitor_wallpaper_snapshots: HashMap::new(),
        pinned_widget_ids,
        paused: st.manual_paused,
    };
    match serde_json::to_string_pretty(&session) {
        Ok(json) => {
            if let Err(e) = atomic_fs::write_atomic(&session_path(), json.as_bytes(), true) {
                eprintln!("[prismview-studio] salvar sessão: {e}");
            }
        }
        Err(e) => eprintln!("[prismview-studio] serializar sessão: {e}"),
    }
    if is_autostart_enabled_inner() {
        sync_studio_login_entry(!st.pinned_widgets.is_empty());
    }
}

/// Lê `session.json`. Conteúdo inválido vira `session.corrupt-<ts>.json` e o `.bak` é usado.
pub(crate) fn load_persisted_session() -> Option<PersistedSession> {
    let path = session_path();
    let quarantine = prismview_root_dir().join(format!("session.corrupt-{}.json", atomic_fs::unix_secs()));
    let value = match atomic_fs::load_json_recovering(&path, &quarantine) {
        atomic_fs::JsonLoad::Ok(v) => v,
        atomic_fs::JsonLoad::Missing => return None,
        atomic_fs::JsonLoad::Restored { value, error } => {
            eprintln!("[prismview-studio] session.json inválido ({error}); restaurado do .bak");
            value
        }
        atomic_fs::JsonLoad::Broken { error } => {
            eprintln!("[prismview-studio] session.json inválido ({error}) e sem .bak");
            return None;
        }
        atomic_fs::JsonLoad::Unreadable { error, backup } => {
            eprintln!("[prismview-studio] ler session.json: {error}");
            backup?
        }
    };
    match serde_json::from_value::<PersistedSession>(value) {
        Ok(session) => Some(session),
        Err(e) => {
            eprintln!("[prismview-studio] session.json fora do formato: {e}");
            None
        }
    }
}

#[tauri::command]
pub(crate) fn get_autostart_enabled() -> Result<bool, String> {
    Ok(is_autostart_enabled_inner())
}

#[tauri::command]
pub(crate) fn set_autostart_enabled(
    state: tauri::State<'_, SharedState>,
    enabled: bool,
) -> Result<bool, String> {
    let has_widgets = state
        .lock()
        .map(|st| !st.pinned_widgets.is_empty())
        .unwrap_or(false);
    set_autostart_enabled_inner(enabled, has_widgets)
}
