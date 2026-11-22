//! Controle do motor nativo (`prismview-engine.exe --native`, wgpu + WorkerW).
//!
//! O Studio só inicia/para o processo e conversa por named pipe (JSON, 1 mensagem por
//! pedido, mesmo protocolo de `prism-core::schema::IpcRequest/IpcResponse`). Este crate não
//! depende de `prism-core`: os pedidos são montados como `serde_json::Value`.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::State;

const PIPE_PATH: &str = r"\\.\pipe\prismview_ipc";
const ENGINE_EXE: &str = "prismview-engine.exe";
/// O motor sobrevive ao Studio: processo destacado, sem console e em grupo próprio.
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const ACTIVE_SCENE_FILE: &str = "active_scene.json";
/// Preferência de energia lida pelo motor ao iniciar (mesmo arquivo que ele grava no IPC).
const POWER_PREFERENCE_FILE: &str = "power_preference.txt";
/// Preferência de exibição da engine na bandeja do sistema (`"1"` / `"0"`, padrão: `"1"`).
const TRAY_ENABLED_FILE: &str = "tray_enabled.txt";
/// Marcador: o usuário desligou o início com o Windows (não reativar sozinho no "Aplicar").
const AUTOSTART_OPT_OUT_FILE: &str = "autostart_disabled";
/// Valor em `HKCU\...\Run` que sobe o motor no login.
pub const ENGINE_RUN_VALUE: &str = "PrismViewEngine";
const POWER_PREFERENCES: [&str; 5] = ["AUTO", "NORMAL", "ECO", "ULTRA", "FROZEN"];
/// Win32 `ERROR_PIPE_BUSY`: o servidor está vivo e reciclando a instância do pipe.
const ERROR_PIPE_BUSY: i32 = 231;
const PIPE_CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const PIPE_BUSY_TIMEOUT: Duration = Duration::from_millis(150);
const PIPE_RETRY_INTERVAL: Duration = Duration::from_millis(12);
const START_POLL_INTERVAL: Duration = Duration::from_millis(15);
const START_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct NativeEngine {
    child: Mutex<Option<Child>>,
    /// Serializa escrita de `active_scene.json` + `LOAD_SCENE_FILE` (evita corrida entre autosave e clique).
    apply_lock: Mutex<()>,
    /// Export `.prism` em background coalescido (último projeto vence, sem travar "Aplicar").
    pending_export: Mutex<Option<Value>>,
    export_worker_active: AtomicBool,
}

pub type SharedEngine = Arc<NativeEngine>;

fn exchange_pipe(mut pipe: fs::File, payload: &[u8]) -> Result<Value, String> {
    pipe.write_all(payload).map_err(|e| format!("pipe_write: {e}"))?;
    let mut buf = vec![0u8; 64 * 1024];
    let read = pipe.read(&mut buf).map_err(|e| format!("pipe_read: {e}"))?;
    if read == 0 {
        return Err("pipe_closed".into());
    }
    let response: Value =
        serde_json::from_slice(&buf[..read]).map_err(|e| format!("invalid_response: {e}"))?;
    if response.get("status").and_then(Value::as_str) == Some("ERROR") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("engine_error");
        return Err(message.to_string());
    }
    Ok(response)
}

/// Tentativa instantânea em `pipe_path`: falha imediatamente (< 1ms) se o pipe não existe;
/// só aguarda brevemente se receber `ERROR_PIPE_BUSY` (servidor vivo reciclando o pipe).
fn pipe_try_once_at(pipe_path: &str, request: &Value) -> Result<Value, String> {
    let payload = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + PIPE_BUSY_TIMEOUT;
    let pipe = loop {
        match OpenOptions::new().read(true).write(true).open(pipe_path) {
            Ok(file) => break file,
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(format!("engine_unreachable: {err}")),
        }
    };
    exchange_pipe(pipe, &payload)
}

fn pipe_try_once(request: &Value) -> Result<Value, String> {
    pipe_try_once_at(PIPE_PATH, request)
}

/// Envia um pedido com retentativa curta (usar quando o motor já deveria estar respondendo).
fn pipe_request(request: &Value) -> Result<Value, String> {
    let payload = serde_json::to_vec(request).map_err(|e| e.to_string())?;

    // Entre uma conexão e outra o servidor recria a instância do pipe: tenta com polling curto.
    let deadline = Instant::now() + PIPE_CONNECT_TIMEOUT;
    let pipe = loop {
        match OpenOptions::new().read(true).write(true).open(PIPE_PATH) {
            Ok(file) => break file,
            Err(err) if Instant::now() >= deadline => {
                return Err(format!("engine_unreachable: {err}"));
            }
            Err(_) => sleep(PIPE_RETRY_INTERVAL),
        }
    };

    exchange_pipe(pipe, &payload)
}

fn engine_exe() -> Result<PathBuf, String> {
    if let Some(custom) = std::env::var_os("PRISMVIEW_ENGINE_EXE") {
        let path = PathBuf::from(custom);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!("PRISMVIEW_ENGINE_EXE inválido: {}", path.display()))
        };
    }
    // Dev e release compartilham a pasta `target/<perfil>/` com o Studio.
    let sibling = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .map(|dir| dir.join(ENGINE_EXE));
    match sibling {
        Some(path) if path.is_file() => Ok(path),
        _ => Err(format!(
            "{ENGINE_EXE} não encontrado ao lado do Studio (rode `cargo build -p prismview-engine`)"
        )),
    }
}

/// `%APPDATA%\PrismView\native`: estado lido pelo próprio motor (cena ativa, preferência de energia).
fn state_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("APPDATA").ok_or("APPDATA ausente")?;
    let dir = PathBuf::from(base).join("PrismView").join("native");
    fs::create_dir_all(&dir).map_err(|e| format!("native_state_dir: {e}"))?;
    Ok(dir)
}

/// Linha de comando do login: `--login` faz o motor sair na hora se não houver cena ativa
/// (não cobre o wallpaper do Windows com preto).
pub fn login_command() -> Result<String, String> {
    Ok(format!("\"{}\" --native --login", engine_exe()?.display()))
}

pub fn autostart_opted_out() -> bool {
    state_dir().map(|d| d.join(AUTOSTART_OPT_OUT_FILE).is_file()).unwrap_or(false)
}

pub fn set_autostart_opt_out(opt_out: bool) {
    let Ok(dir) = state_dir() else {
        return;
    };
    let marker = dir.join(AUTOSTART_OPT_OUT_FILE);
    if opt_out {
        let _ = fs::write(marker, b"1");
    } else {
        let _ = fs::remove_file(marker);
    }
}

/// Há cena aplicada que o motor relê sozinho (login / reinício).
pub fn has_active_scene() -> bool {
    state_dir().map(|d| d.join(ACTIVE_SCENE_FILE).is_file()).unwrap_or(false)
}

/// Remove o wallpaper nativo: encerra o motor (o Windows volta a pintar o próprio fundo) e apaga
/// a cena ativa para o login seguinte não reaplicar. Não precisa do handle do processo filho.
pub fn clear_desktop() -> Result<(), String> {
    let _ = pipe_try_once(&json!({ "cmd": "SHUTDOWN" }));
    let dir = state_dir()?;
    match fs::remove_file(dir.join(ACTIVE_SCENE_FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("native_scene_remove: {e}")),
    }
}

/// Pausa ou retoma o motor nativo se estiver rodando (tentativa instantânea, sem bloquear).
pub fn set_paused(paused: bool) {
    let _ = pipe_try_once(&json!({ "cmd": "SET_PAUSED", "paused": paused }));
}

/// Checagem instantânea (< 1ms) se o motor nativo está respondendo no pipe.
pub fn is_running_fast() -> bool {
    pipe_try_once(&json!({ "cmd": "PING" })).is_ok()
}

/// Lê a preferência persistida de ícone da engine na bandeja (padrão: `true`).
pub fn get_tray_enabled() -> bool {
    let Ok(dir) = state_dir() else {
        return true;
    };
    match fs::read_to_string(dir.join(TRAY_ENABLED_FILE)) {
        Ok(raw) => {
            let t = raw.trim();
            !(t == "0" || t.eq_ignore_ascii_case("false") || t.eq_ignore_ascii_case("off"))
        }
        Err(_) => true,
    }
}

/// Persiste a visibilidade da bandeja da engine e sincroniza em tempo real via IPC se rodando.
pub fn set_tray_visible(visible: bool) -> Result<bool, String> {
    let path = state_dir()?.join(TRAY_ENABLED_FILE);
    fs::write(&path, if visible { b"1" } else { b"0" })
        .map_err(|e| format!("tray_enabled_write: {e}"))?;
    let _ = pipe_try_once(&json!({ "cmd": "SET_TRAY_VISIBLE", "visible": visible }));
    Ok(visible)
}

/// Agenda a atualização do `.prism` em `packages/` numa única thread em background (coalescida:
/// aplicações rápidas em sequência só compactam o estado mais recente, sem bloquear o "Aplicar").
fn schedule_background_archive(engine: &SharedEngine, project: Value) {
    if let Ok(mut slot) = engine.pending_export.lock() {
        *slot = Some(project);
    }
    if engine
        .export_worker_active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let engine_bg = Arc::clone(engine);
    std::thread::spawn(move || loop {
        let next = engine_bg
            .pending_export
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        match next {
            Some(proj) => {
                if let Err(err) = crate::export_prism_archive_sync(proj, None) {
                    eprintln!("[native_engine] export background .prism: {err}");
                }
            }
            None => {
                engine_bg.export_worker_active.store(false, Ordering::Release);
                // Re-checa após baixar a flag para não perder item enfileirado na janela.
                let has_more = engine_bg
                    .pending_export
                    .lock()
                    .map(|slot| slot.is_some())
                    .unwrap_or(false);
                if has_more
                    && engine_bg
                        .export_worker_active
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    continue;
                }
                break;
            }
        }
    });
}

impl NativeEngine {
    /// Fluxo "Aplicar no Desktop": converte o projeto, materializa assets (hardlink/cache), grava a
    /// cena ativa atomicamente, garante o motor no ar e manda `LOAD_SCENE_FILE` na hora (< 300ms).
    /// O pacote `.prism` em `packages/` é atualizado depois em background sem travar a UI.
    pub fn apply_project(self: &Arc<Self>, project: &Value) -> Result<(), String> {
        let _guard = self
            .apply_lock
            .lock()
            .map_err(|_| "native_engine_apply_poisoned".to_string())?;
        // 1) Converte antes de qualquer efeito colateral (todo tipo de camada é suportado).
        let mut scene = crate::native_scene::project_to_scene(project)?;
        // 1b) Mídia/fontes/modelos → caminhos absolutos reais, espelhados em `native/assets/`
        //     (a cena ativa não depende de onde o Estúdio guardou os arquivos).
        let state = state_dir()?;
        let data_root = state.parent().map(Path::to_path_buf).unwrap_or_else(|| state.clone());
        for warning in crate::native_scene::materialize_assets(&mut scene, &data_root, &state.join("assets")) {
            eprintln!("[native_engine] {warning}");
        }
        // 2) Cena ativa: escrita atômica (tmp + rename) para o motor nunca ler pela metade.
        let final_path = state.join(ACTIVE_SCENE_FILE);
        let tmp_path = state.join(format!("{ACTIVE_SCENE_FILE}.tmp"));
        let json = serde_json::to_vec(&scene).map_err(|e| e.to_string())?;
        fs::write(&tmp_path, json).map_err(|e| format!("native_scene_write: {e}"))?;
        fs::rename(&tmp_path, &final_path).map_err(|e| format!("native_scene_rename: {e}"))?;
        // 3) Motor no ar (check instantâneo sem sleep de 1,5s) + recarga imediata.
        self.ensure_started()?;
        pipe_request(&json!({
            "cmd": "LOAD_SCENE_FILE",
            "path": final_path.to_string_lossy(),
        }))?;
        drop(_guard);
        // 4) Atualiza o `.prism` otimizado em `packages/` fora do caminho crítico.
        schedule_background_archive(self, project.clone());
        Ok(())
    }

    /// Grava a preferência no arquivo que o motor lê ao iniciar e, se ele estiver no ar, aplica na
    /// hora pelo pipe (sem esperar 1,5s se o motor estiver parado).
    fn set_power(&self, preference: &str) -> Result<Value, String> {
        let preference = preference.trim().to_ascii_uppercase();
        if !POWER_PREFERENCES.contains(&preference.as_str()) {
            return Err(format!("invalid_power_preference: {preference}"));
        }
        let path = state_dir()?.join(POWER_PREFERENCE_FILE);
        fs::write(&path, preference.as_bytes()).map_err(|e| format!("power_preference_write: {e}"))?;
        if pipe_try_once(&json!({ "cmd": "SET_POWER_PREFERENCE", "preference": preference })).is_ok() {
            Ok(self.status())
        } else {
            Ok(json!({ "running": false }))
        }
    }

    /// Garante o motor no ar com a cena ativa (abertura do Studio após o motor ter caído).
    pub fn ensure_running(&self) -> Result<(), String> {
        if !has_active_scene() {
            return Ok(());
        }
        self.ensure_started()
    }

    fn lock_child(&self) -> Result<std::sync::MutexGuard<'_, Option<Child>>, String> {
        self.child.lock().map_err(|_| "native_engine_state_poisoned".to_string())
    }

    fn status(&self) -> Value {
        match pipe_try_once(&json!({ "cmd": "GET_STATUS" })) {
            Ok(response) => {
                let mut engine = response.get("engine").cloned().unwrap_or_else(|| json!({}));
                if let Some(map) = engine.as_object_mut() {
                    map.insert("running".into(), json!(true));
                }
                engine
            }
            Err(_) => json!({ "running": false }),
        }
    }

    /// Garante que existe um processo do motor respondendo no pipe (sem roundtrip extra de status).
    fn ensure_started(&self) -> Result<(), String> {
        if pipe_try_once(&json!({ "cmd": "PING" })).is_ok() {
            return Ok(()); // já há um engine respondendo no pipe (< 1ms)
        }

        let mut child_slot = self.lock_child()?;
        // Re-checa sob o lock para evitar spawn duplo com a thread de startup.
        if pipe_try_once(&json!({ "cmd": "PING" })).is_ok() {
            return Ok(());
        }
        if let Some(child) = child_slot.as_mut() {
            if matches!(child.try_wait(), Ok(Some(_))) {
                *child_slot = None; // processo anterior já morreu
            }
        }
        if child_slot.is_none() {
            let child = Command::new(engine_exe()?)
                .arg("--native")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
                .spawn()
                .map_err(|e| format!("engine_spawn_failed: {e}"))?;
            *child_slot = Some(child);
        }

        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if pipe_try_once(&json!({ "cmd": "PING" })).is_ok() {
                break;
            }
            if let Some(child) = child_slot.as_mut() {
                if let Ok(Some(exit)) = child.try_wait() {
                    *child_slot = None;
                    return Err(format!("engine_exited_early: {exit}"));
                }
            }
            if Instant::now() >= deadline {
                if let Some(mut child) = child_slot.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err("engine_start_timeout".into());
            }
            sleep(START_POLL_INTERVAL);
        }
        Ok(())
    }

    fn start(&self) -> Result<Value, String> {
        self.ensure_started()?;
        Ok(self.status())
    }

    fn stop(&self) -> Result<Value, String> {
        let was_running = pipe_try_once(&json!({ "cmd": "SHUTDOWN" })).is_ok();
        let mut child_slot = self.lock_child()?;
        if let Some(mut child) = child_slot.take() {
            let deadline = Instant::now() + STOP_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(30)),
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        Ok(json!({ "running": false, "was_running": was_running }))
    }

    fn clear(&self, color: Option<[f32; 4]>) -> Result<Value, String> {
        pipe_request(&json!({ "cmd": "CLEAR_DESKTOP", "color": color }))?;
        Ok(self.status())
    }
}

/// Estado a registrar com `.manage(native_engine::new_state())`.
pub fn new_state() -> SharedEngine {
    Arc::new(NativeEngine::default())
}

async fn blocking<T, F>(engine: &State<'_, SharedEngine>, job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&SharedEngine) -> Result<T, String> + Send + 'static,
{
    let engine = Arc::clone(engine.inner());
    tauri::async_runtime::spawn_blocking(move || job(&engine))
        .await
        .map_err(|e| format!("native_engine_task: {e}"))?
}

/// Inicia o engine nativo (se já responde no pipe, só devolve o status).
#[tauri::command]
pub async fn native_engine_start(engine: State<'_, SharedEngine>) -> Result<Value, String> {
    blocking(&engine, |e| e.start()).await
}

/// Encerra o engine nativo e devolve o wallpaper do Windows.
#[tauri::command]
pub async fn native_engine_stop(engine: State<'_, SharedEngine>) -> Result<Value, String> {
    blocking(&engine, |e| e.stop()).await
}

/// Pinta o desktop (WorkerW) com `color` RGBA 0..1; sem `color` = preto.
#[tauri::command]
pub async fn native_engine_clear(
    engine: State<'_, SharedEngine>,
    color: Option<[f32; 4]>,
) -> Result<Value, String> {
    blocking(&engine, move |e| e.clear(color)).await
}

/// `{ running: false }` se o engine não responde; senão o `EngineStatus` + `running: true`.
#[tauri::command]
pub async fn native_engine_status(engine: State<'_, SharedEngine>) -> Result<Value, String> {
    blocking(&engine, |e| Ok(e.status())).await
}

/// "Aplicar no Desktop" pelo motor nativo (o `apply_desktop_wallpaper` já faz isso por padrão).
#[tauri::command]
pub async fn native_engine_apply(
    engine: State<'_, SharedEngine>,
    project: Value,
) -> Result<Value, String> {
    blocking(&engine, move |e| {
        e.apply_project(&project)?;
        Ok(e.status())
    })
    .await
}

/// Preferência de energia do motor: `AUTO|NORMAL|ECO|ULTRA|FROZEN` (persistida pelo motor).
#[tauri::command]
pub async fn native_engine_set_power(
    engine: State<'_, SharedEngine>,
    preference: String,
) -> Result<Value, String> {
    blocking(&engine, move |e| e.set_power(&preference)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_try_once_fails_immediately_when_pipe_missing() {
        let start = Instant::now();
        let res = pipe_try_once_at(r"\\.\pipe\prismview_ipc_nonexistent_test", &json!({ "cmd": "PING" }));
        assert!(res.is_err());
        assert!(
            start.elapsed() < Duration::from_millis(250),
            "pipe_try_once deveria falhar instantaneamente, levou {:?}",
            start.elapsed()
        );
    }
}
