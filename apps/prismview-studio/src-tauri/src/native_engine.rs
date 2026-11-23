//! Controle do motor nativo (`prismview-engine.exe --native`, wgpu + WorkerW).
//!
//! O Studio só inicia/para o processo e conversa por named pipe (JSON, 1 mensagem por
//! pedido, mesmo protocolo de `prism-core::schema::IpcRequest/IpcResponse`). Este crate não
//! depende de `prism-core`: os pedidos são montados como `serde_json::Value`.

use std::collections::HashMap;
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
/// Widgets nativos persistidos pelo motor (mesmo formato de `prism-core::ActiveWidgetsFile`).
const ACTIVE_WIDGETS_FILE: &str = "active_widgets.json";
/// Cenas dos widgets nativos (`native/widgets/<id>.json`).
const WIDGET_SCENES_DIR: &str = "widgets";
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
/// Espera máxima por uma instância livre quando o pipe responde BUSY (motor vivo, ocupado).
const PIPE_BUSY_WAIT: Duration = Duration::from_millis(2000);
/// Prefixo de erro: motor vivo porém ocupado. Nunca justifica subir outro processo.
const ENGINE_BUSY: &str = "engine_busy";
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
    /// Última posição (física) que o Studio mandou por widget: decide se o arrasto feito no
    /// desktop (estado do motor) ou a edição no Studio vence no próximo `UPSERT_WIDGET`.
    widget_sent_pos: Mutex<HashMap<String, (i32, i32)>>,
}

/// Posição/tamanho (px físicos) e flags de um widget nativo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWidgetPlacement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub always_on_top: bool,
    pub locked: bool,
    pub click_through: bool,
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

#[link(name = "kernel32")]
extern "system" {
    fn WaitNamedPipeW(name: *const u16, timeout_ms: u32) -> i32;
}

/// Bloqueia até haver instância livre do pipe (ou `timeout`). `false` = continua ocupado/ausente.
fn wait_named_pipe(pipe_path: &str, timeout: Duration) -> bool {
    let wide: Vec<u16> = pipe_path.encode_utf16().chain(std::iter::once(0)).collect();
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX).max(1);
    // SAFETY: `wide` termina em NUL e vive até o fim da chamada.
    unsafe { WaitNamedPipeW(wide.as_ptr(), ms) != 0 }
}

/// `true` se o erro indica motor vivo porém ocupado (não subir outro processo).
fn is_engine_busy(err: &str) -> bool {
    err.starts_with(ENGINE_BUSY)
}

/// Tentativa instantânea em `pipe_path`: falha imediatamente (< 1ms) se o pipe não existe.
/// `ERROR_PIPE_BUSY` = motor vivo: espera instância livre com `WaitNamedPipeW` (até 2 s) e, se
/// continuar ocupado, devolve `engine_busy` (nunca "fora do ar").
fn pipe_try_once_at(pipe_path: &str, request: &Value) -> Result<Value, String> {
    let payload = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + PIPE_BUSY_WAIT;
    let pipe = loop {
        match OpenOptions::new().read(true).write(true).open(pipe_path) {
            Ok(file) => break file,
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() || !wait_named_pipe(pipe_path, remaining) {
                    return Err(format!("{ENGINE_BUSY}: {err}"));
                }
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
        if let Err(e) = crate::atomic_fs::write_atomic(&marker, b"1", false) {
            eprintln!("[native_engine] autostart_opt_out: {e}");
        }
    } else {
        crate::atomic_fs::log_io("remove_file", fs::remove_file(marker));
    }
}

/// Há cena aplicada que o motor relê sozinho (login / reinício).
pub fn has_active_scene() -> bool {
    state_dir().map(|d| d.join(ACTIVE_SCENE_FILE).is_file()).unwrap_or(false)
}

/// Há widgets nativos persistidos (o motor os recria no login, mesmo sem wallpaper).
pub fn has_active_widgets() -> bool {
    state_dir()
        .ok()
        .and_then(|d| fs::read_to_string(d.join(ACTIVE_WIDGETS_FILE)).ok())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("widgets").and_then(Value::as_array).map(|a| !a.is_empty()))
        .unwrap_or(false)
}

/// Mesmo critério do motor (`NativeWidgetSpec::is_valid_id`): vira nome de arquivo/janela.
pub fn is_valid_widget_id(id: &str) -> bool {
    !id.trim().is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && id != "."
        && id != ".."
}

/// Widget pode rodar nativo? Vídeo em widget ainda não tem decoder no motor por widget:
/// esses continuam no WebView (fallback explícito, sem quebrar projetos existentes).
pub fn widget_native_eligible(project: &Value) -> Result<(), String> {
    fn has_video(layers: &[Value]) -> bool {
        layers.iter().any(|l| {
            l.get("kind").and_then(Value::as_str) == Some("VIDEO")
                || l.get("children").and_then(Value::as_array).is_some_and(|c| has_video(c))
        })
    }
    let layers = project.get("layers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    if has_video(layers) {
        return Err("native_widget_unsupported: VIDEO".into());
    }
    Ok(())
}

/// Tira `id` do `active_widgets.json` (motor fora do ar) e apaga a cena dele.
fn remove_persisted_widget(id: &str) -> Result<(), String> {
    let dir = state_dir()?;
    let path = dir.join(ACTIVE_WIDGETS_FILE);
    if let Ok(raw) = fs::read_to_string(&path) {
        if let Ok(mut v) = serde_json::from_str::<Value>(&raw) {
            if let Some(list) = v.get_mut("widgets").and_then(Value::as_array_mut) {
                list.retain(|w| w.get("id").and_then(Value::as_str) != Some(id));
                let empty = list.is_empty();
                if empty {
                    crate::atomic_fs::log_io("remove_file", fs::remove_file(&path));
                } else {
                    let bytes = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
                    crate::atomic_fs::write_atomic(&path, &bytes, false)
                        .map_err(|e| format!("active_widgets_write: {e}"))?;
                }
            }
        }
    }
    crate::atomic_fs::log_io("remove_file", fs::remove_file(dir.join(WIDGET_SCENES_DIR).join(format!("{id}.json"))));
    Ok(())
}

/// Remove o wallpaper nativo (`CLEAR_WALLPAPER`: widgets nativos seguem no ar; sem eles o motor
/// encerra e o Windows volta a pintar o próprio fundo) e apaga a cena ativa para o login seguinte
/// não reaplicar. Não precisa do handle do processo filho.
pub fn clear_desktop() -> Result<(), String> {
    if pipe_try_once(&json!({ "cmd": "CLEAR_WALLPAPER" })).is_err() {
        // Motor antigo (sem o comando) ou fora do ar.
        let _ = pipe_try_once(&json!({ "cmd": "SHUTDOWN" }));
    }
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
    crate::atomic_fs::write_atomic(&path, if visible { b"1" } else { b"0" }, false)
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
    /// O pacote `.prism` em `packages/` é atualizado depois em background sem travar a UI, só
    /// quando `archive` (Aplicar explícito; o autosave não re-zipa). Devolve avisos para a UI.
    pub fn apply_project(self: &Arc<Self>, project: &Value, archive: bool) -> Result<Vec<String>, String> {
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
        let warnings = crate::native_scene::materialize_assets(&mut scene, &data_root, &state.join("assets"));
        for warning in &warnings {
            eprintln!("[native_engine] {warning}");
        }
        // 2) Cena ativa: escrita atômica (tmp + rename) para o motor nunca ler pela metade.
        let final_path = state.join(ACTIVE_SCENE_FILE);
        let json = serde_json::to_vec(&scene).map_err(|e| e.to_string())?;
        crate::atomic_fs::write_atomic(&final_path, &json, false)
            .map_err(|e| format!("native_scene_write: {e}"))?;
        // 3) Motor no ar (check instantâneo sem sleep de 1,5s) + recarga imediata.
        self.ensure_started()?;
        pipe_request(&json!({
            "cmd": "LOAD_SCENE_FILE",
            "path": final_path.to_string_lossy(),
        }))?;
        drop(_guard);
        // 4) Atualiza o `.prism` otimizado em `packages/` fora do caminho crítico.
        if archive {
            schedule_background_archive(self, project.clone());
        }
        Ok(warnings)
    }

    /// Entrega 3 — widget nativo (janela DirectComposition no motor, sem WebView2). Converte e
    /// materializa a cena (mesmo pipeline do wallpaper, cache próprio por widget), grava
    /// `native/widgets/<id>.json` e manda `UPSERT_WIDGET`. Devolve a posição efetiva (física):
    /// se o usuário arrastou o widget no desktop, a posição do motor vence a do projeto, a menos
    /// que o Studio tenha mudado `posX/posY` desde o último envio.
    pub fn upsert_widget(&self, project: &Value, placement: &NativeWidgetPlacement) -> Result<(i32, i32), String> {
        let id = project
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_valid_widget_id(id))
            .ok_or("widget_id_invalido")?
            .to_string();
        widget_native_eligible(project)?;
        let _guard = self
            .apply_lock
            .lock()
            .map_err(|_| "native_engine_apply_poisoned".to_string())?;
        let mut scene = crate::native_scene::project_to_scene(project)?;
        let state = state_dir()?;
        let data_root = state.parent().map(Path::to_path_buf).unwrap_or_else(|| state.clone());
        let cache = state.join("assets").join("widgets").join(&id);
        for warning in crate::native_scene::materialize_assets(&mut scene, &data_root, &cache) {
            eprintln!("[native_engine] widget {id}: {warning}");
        }
        let dir = state.join(WIDGET_SCENES_DIR);
        fs::create_dir_all(&dir).map_err(|e| format!("native_widget_dir: {e}"))?;
        let final_path = dir.join(format!("{id}.json"));
        let json = serde_json::to_vec(&scene).map_err(|e| e.to_string())?;
        crate::atomic_fs::write_atomic(&final_path, &json, false)
            .map_err(|e| format!("native_widget_write: {e}"))?;

        self.ensure_started()?;
        let (mut x, mut y) = (placement.x, placement.y);
        let studio_moved = self
            .widget_sent_pos
            .lock()
            .ok()
            .and_then(|m| m.get(&id).copied())
            .map_or(false, |prev| prev != (x, y));
        if !studio_moved {
            if let Some(current) = self.list_widgets().into_iter().find(|w| w.get("id").and_then(Value::as_str) == Some(id.as_str())) {
                x = current.get("x").and_then(Value::as_i64).map_or(x, |v| v as i32);
                y = current.get("y").and_then(Value::as_i64).map_or(y, |v| v as i32);
            }
        }
        pipe_request(&json!({
            "cmd": "UPSERT_WIDGET",
            "id": id,
            "scene_path": final_path.to_string_lossy(),
            "x": x,
            "y": y,
            "width": placement.width.max(16),
            "height": placement.height.max(16),
            "always_on_top": placement.always_on_top,
            "locked": placement.locked,
            "click_through": placement.click_through,
        }))?;
        if let Ok(mut m) = self.widget_sent_pos.lock() {
            m.insert(id, (x, y));
        }
        Ok((x, y))
    }

    /// Remove o widget nativo (o motor apaga a cena dele e sai se não sobrar nada).
    pub fn remove_widget(&self, id: &str) -> Result<(), String> {
        if let Ok(mut m) = self.widget_sent_pos.lock() {
            m.remove(id);
        }
        // Motor fora do ar: o estado persistido ainda pode citar o widget → limpa à mão.
        if pipe_try_once(&json!({ "cmd": "REMOVE_WIDGET", "id": id })).is_err() {
            remove_persisted_widget(id)?;
        }
        if let Ok(state) = state_dir() {
            crate::atomic_fs::log_io("remove_dir_all", fs::remove_dir_all(state.join("assets").join("widgets").join(id)));
        }
        Ok(())
    }

    /// Travar/destravar arrasto, sempre-no-topo, click-through (campos `None` não mudam).
    pub fn set_widget_flags(
        &self,
        id: &str,
        locked: Option<bool>,
        always_on_top: Option<bool>,
        click_through: Option<bool>,
    ) -> Result<(), String> {
        pipe_try_once(&json!({
            "cmd": "SET_WIDGET_FLAGS",
            "id": id,
            "locked": locked,
            "always_on_top": always_on_top,
            "click_through": click_through,
        }))
        .map(|_| ())
    }

    /// Widgets nativos no ar (posições atuais após arrasto). Vazio se o motor não responde.
    pub fn list_widgets(&self) -> Vec<Value> {
        pipe_try_once(&json!({ "cmd": "LIST_WIDGETS" }))
            .ok()
            .and_then(|r| r.get("widgets").and_then(Value::as_array).cloned())
            .unwrap_or_default()
    }

    /// Grava a preferência no arquivo que o motor lê ao iniciar e, se ele estiver no ar, aplica na
    /// hora pelo pipe (sem esperar 1,5s se o motor estiver parado).
    fn set_power(&self, preference: &str) -> Result<Value, String> {
        let preference = preference.trim().to_ascii_uppercase();
        if !POWER_PREFERENCES.contains(&preference.as_str()) {
            return Err(format!("invalid_power_preference: {preference}"));
        }
        let path = state_dir()?.join(POWER_PREFERENCE_FILE);
        crate::atomic_fs::write_atomic(&path, preference.as_bytes(), false)
            .map_err(|e| format!("power_preference_write: {e}"))?;
        if pipe_try_once(&json!({ "cmd": "SET_POWER_PREFERENCE", "preference": preference })).is_ok() {
            Ok(self.status())
        } else {
            Ok(json!({ "running": false }))
        }
    }

    /// Garante o motor no ar com a cena ativa (abertura do Studio após o motor ter caído).
    pub fn ensure_running(&self) -> Result<(), String> {
        if !has_active_scene() && !has_active_widgets() {
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
            Err(e) if is_engine_busy(&e) => json!({ "running": true, "busy": true }),
            Err(_) => json!({ "running": false }),
        }
    }

    /// Garante que existe um processo do motor respondendo no pipe (sem roundtrip extra de status).
    fn ensure_started(&self) -> Result<(), String> {
        // Responder OU estar ocupado (BUSY) = motor vivo: nunca subir um segundo processo.
        let alive = |r: Result<Value, String>| match r {
            Ok(_) => true,
            Err(e) => is_engine_busy(&e),
        };
        if alive(pipe_try_once(&json!({ "cmd": "PING" }))) {
            return Ok(()); // já há um engine respondendo no pipe (< 1ms)
        }

        let mut child_slot = self.lock_child()?;
        // Re-checa sob o lock para evitar spawn duplo com a thread de startup.
        if alive(pipe_try_once(&json!({ "cmd": "PING" }))) {
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
            if alive(pipe_try_once(&json!({ "cmd": "PING" }))) {
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
        e.apply_project(&project, true)?;
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
    fn widget_ids_and_eligibility() {
        assert!(is_valid_widget_id("w-1.a_b"));
        for bad in ["", "..", "a/b", "a\\b", "x y"] {
            assert!(!is_valid_widget_id(bad), "{bad}");
        }
        let ok = json!({ "layers": [{ "kind": "TEXT" }, { "kind": "GROUP", "children": [{ "kind": "SCENE_3D" }] }] });
        assert!(widget_native_eligible(&ok).is_ok());
        let video = json!({ "layers": [{ "kind": "GROUP", "children": [{ "kind": "VIDEO" }] }] });
        assert!(widget_native_eligible(&video).is_err());
    }

    #[test]
    fn upsert_widget_request_matches_engine_contract() {
        // Mesmas chaves que `prism-core::IpcRequest::UpsertWidget` (snake_case, tag `cmd`).
        let req = json!({
            "cmd": "UPSERT_WIDGET", "id": "w", "scene_path": "C:/x.json", "x": -10, "y": 5,
            "width": 300, "height": 200, "always_on_top": false, "locked": true, "click_through": true,
        });
        for key in ["id", "scene_path", "x", "y", "width", "height", "always_on_top", "locked", "click_through"] {
            assert!(req.get(key).is_some(), "{key}");
        }
    }

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
