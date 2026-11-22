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
/// Marcador: o usuário desligou o início com o Windows (não reativar sozinho no "Aplicar").
const AUTOSTART_OPT_OUT_FILE: &str = "autostart_disabled";
/// Valor em `HKCU\...\Run` que sobe o motor no login.
pub const ENGINE_RUN_VALUE: &str = "PrismViewEngine";
const POWER_PREFERENCES: [&str; 5] = ["AUTO", "NORMAL", "ECO", "ULTRA", "FROZEN"];
const PIPE_CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const START_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct NativeEngine {
    child: Mutex<Option<Child>>,
}

pub type SharedEngine = Arc<NativeEngine>;

/// Envia um pedido e devolve a resposta JSON. `Err` se o pipe não existe ou a resposta é
/// `{"status":"ERROR"}`.
fn pipe_request(request: &Value) -> Result<Value, String> {
    let payload = serde_json::to_vec(request).map_err(|e| e.to_string())?;

    // Entre uma conexão e outra o servidor recria a instância do pipe: tenta por um instante.
    let deadline = Instant::now() + PIPE_CONNECT_TIMEOUT;
    let mut pipe = loop {
        match OpenOptions::new().read(true).write(true).open(PIPE_PATH) {
            Ok(file) => break file,
            Err(err) if Instant::now() >= deadline => {
                return Err(format!("engine_unreachable: {err}"));
            }
            Err(_) => sleep(Duration::from_millis(40)),
        }
    };

    pipe.write_all(&payload).map_err(|e| format!("pipe_write: {e}"))?;
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
    let _ = pipe_request(&json!({ "cmd": "SHUTDOWN" }));
    let dir = state_dir()?;
    match fs::remove_file(dir.join(ACTIVE_SCENE_FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("native_scene_remove: {e}")),
    }
}

impl NativeEngine {
    /// Fluxo "Aplicar no Desktop": converte o projeto, exporta o `.prism` otimizado, grava a cena
    /// ativa em disco (o motor a relê sozinho se reiniciar), sobe o motor se preciso e manda
    /// `LOAD_SCENE_FILE` para recarregar na hora. Depois disso o Studio pode ser fechado.
    pub fn apply_project(&self, project: &Value) -> Result<(), String> {
        // 1) Converte antes de qualquer efeito colateral (todo tipo de camada é suportado).
        let mut scene = crate::native_scene::project_to_scene(project)?;
        // 1b) Mídia/fontes/modelos → caminhos absolutos reais, espelhados em `native/assets/`
        //     (a cena ativa não depende de onde o Estúdio guardou os arquivos).
        let state = state_dir()?;
        let data_root = state.parent().map(Path::to_path_buf).unwrap_or_else(|| state.clone());
        for warning in crate::native_scene::materialize_assets(&mut scene, &data_root, &state.join("assets")) {
            eprintln!("[native_engine] {warning}");
        }
        // 2) `.prism` otimizado em `packages/` (mídias empacotadas, reimportável pelo Studio).
        crate::export_prism_archive_sync(project.clone(), None)?;
        // 3) Cena ativa: escrita atômica (tmp + rename) para o motor nunca ler pela metade.
        let dir = state_dir()?;
        let final_path = dir.join(ACTIVE_SCENE_FILE);
        let tmp_path = dir.join(format!("{ACTIVE_SCENE_FILE}.tmp"));
        let json = serde_json::to_vec(&scene).map_err(|e| e.to_string())?;
        fs::write(&tmp_path, json).map_err(|e| format!("native_scene_write: {e}"))?;
        fs::rename(&tmp_path, &final_path).map_err(|e| format!("native_scene_rename: {e}"))?;
        // 4) Motor no ar (reaproveita o que já responde no pipe) + recarga instantânea.
        self.start()?;
        pipe_request(&json!({
            "cmd": "LOAD_SCENE_FILE",
            "path": final_path.to_string_lossy(),
        }))?;
        Ok(())
    }

    /// Grava a preferência no arquivo que o motor lê ao iniciar e, se ele estiver no ar, aplica na
    /// hora pelo pipe. Motor parado não é erro: a escolha vale no próximo início.
    fn set_power(&self, preference: &str) -> Result<Value, String> {
        let preference = preference.trim().to_ascii_uppercase();
        if !POWER_PREFERENCES.contains(&preference.as_str()) {
            return Err(format!("invalid_power_preference: {preference}"));
        }
        let path = state_dir()?.join(POWER_PREFERENCE_FILE);
        fs::write(&path, preference.as_bytes()).map_err(|e| format!("power_preference_write: {e}"))?;
        if pipe_request(&json!({ "cmd": "PING" })).is_ok() {
            pipe_request(&json!({ "cmd": "SET_POWER_PREFERENCE", "preference": preference }))?;
        }
        Ok(self.status())
    }

    /// Garante o motor no ar com a cena ativa (abertura do Studio após o motor ter caído).
    pub fn ensure_running(&self) -> Result<(), String> {
        if !has_active_scene() {
            return Ok(());
        }
        self.start().map(|_| ())
    }

    fn lock_child(&self) -> Result<std::sync::MutexGuard<'_, Option<Child>>, String> {
        self.child.lock().map_err(|_| "native_engine_state_poisoned".to_string())
    }

    fn status(&self) -> Value {
        match pipe_request(&json!({ "cmd": "GET_STATUS" })) {
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

    fn start(&self) -> Result<Value, String> {
        if pipe_request(&json!({ "cmd": "PING" })).is_ok() {
            return Ok(self.status()); // já há um engine respondendo no pipe
        }

        let mut child_slot = self.lock_child()?;
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
            if pipe_request(&json!({ "cmd": "PING" })).is_ok() {
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
            sleep(Duration::from_millis(100));
        }
        drop(child_slot);
        Ok(self.status())
    }

    fn stop(&self) -> Result<Value, String> {
        let was_running = pipe_request(&json!({ "cmd": "SHUTDOWN" })).is_ok();
        let mut child_slot = self.lock_child()?;
        if let Some(mut child) = child_slot.take() {
            let deadline = Instant::now() + STOP_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(50)),
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
    F: FnOnce(&NativeEngine) -> Result<T, String> + Send + 'static,
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
