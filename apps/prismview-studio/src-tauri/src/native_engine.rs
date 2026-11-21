//! Controle do motor nativo (`prismview-engine.exe --native`, wgpu + WorkerW).
//!
//! O Studio só inicia/para o processo e conversa por named pipe (JSON, 1 mensagem por
//! pedido, mesmo protocolo de `prism-core::schema::IpcRequest/IpcResponse`). Este crate não
//! depende de `prism-core`: os pedidos são montados como `serde_json::Value`.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::State;

const PIPE_PATH: &str = r"\\.\pipe\prismview_ipc";
const ENGINE_EXE: &str = "prismview-engine.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PIPE_CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const START_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct NativeEngine {
    child: Mutex<Option<Child>>,
}

type SharedEngine = Arc<NativeEngine>;

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

impl NativeEngine {
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
                .creation_flags(CREATE_NO_WINDOW)
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
