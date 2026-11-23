//! Bandeja, pausa manual e visibilidade do ícone do motor.

#[allow(unused_imports)]
use super::*;

pub(crate) fn apply_manual_pause_inner(app: &AppHandle, state: &SharedState, paused: bool) -> Result<(), String> {
    let (wg_targets, any_vis) = {
        let mut st = state.lock().map_err(|e| e.to_string())?;
        st.manual_paused = paused;
        st.force_vis_resync = true;
        vis_events::mark_dirty();
        save_session_from_state(&st);

        let wgs: Vec<(String, bool)> = st
            .widget_labels
            .iter()
            .filter(|(lbl, _)| st.ready_windows.contains(*lbl))
            .map(|(lbl, _)| (lbl.clone(), !paused && st.desktop_visible))
            .collect();
        (wgs, !paused && st.desktop_visible)
    };
    let reason = if paused { "manual_pause" } else { "resumed" };
    // Propaga pausa/retomada ao motor nativo em background (não bloqueia a UI).
    std::thread::spawn(move || native_engine::set_paused(paused));
    for (lbl, vis) in wg_targets {
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
pub(crate) async fn set_engine_paused(
    app: AppHandle,
    state: tauri::State<'_, SharedState>,
    paused: bool,
) -> Result<RuntimeStateDto, String> {
    let state_arc = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        apply_manual_pause_inner(&app, &state_arc, paused)?;
        let st = state_arc.lock().map_err(|e| e.to_string())?;
        Ok(build_runtime_state(&st))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Coordena o ícone de bandeja do Studio (`studio_tray`) com o ícone nativo do `prismview-engine`
/// para nunca existirem 2 ícones duplicados ao mesmo tempo:
/// - Quando a engine está rodando com bandeja ativa, ela assume o ícone único.
/// - O tray do Studio só aparece quando há widgets fixados sem o tray da engine visível.
pub(crate) fn sync_studio_tray_visibility(app: &AppHandle, state: &SharedState) {
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
pub(crate) fn get_engine_tray_enabled() -> Result<bool, String> {
    Ok(native_engine::get_tray_enabled())
}

#[tauri::command]
pub(crate) async fn set_engine_tray_visible(
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
