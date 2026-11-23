//! Widgets fixados (WebView): fixar/desafixar, travar, prontidão e alvos reativos.

#[allow(unused_imports)]
use super::*;

pub(crate) fn flush_dirty_widget_saves(state: &SharedState, force: bool) {
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

pub(crate) fn layers_reactive_flags(layers: &[Value]) -> (bool, bool, bool) {
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

pub(crate) fn project_reactive_flags(project: &Value) -> (bool, bool, bool) {
    project
        .get("layers")
        .and_then(|v| v.as_array())
        .map(|arr| layers_reactive_flags(arr))
        .unwrap_or((false, false, false))
}

pub(crate) fn refresh_parallax_targets(st: &mut AppRuntimeState) {
    st.parallax_targets.clear();
    st.audio_targets.clear();
    st.keyboard_targets.clear();
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

pub(crate) fn pin_widget_inner(
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
pub(crate) async fn pin_standalone_widget(
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
pub(crate) async fn unpin_standalone_widget(
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

pub(crate) fn set_widget_locked_inner(
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
pub(crate) async fn set_widget_locked(
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

pub(crate) fn unlock_all_widgets_inner(app: &AppHandle, state: &SharedState) {
    let ids: Vec<String> = state
        .lock()
        .map(|st| st.pinned_widgets.keys().cloned().collect())
        .unwrap_or_default();
    for id in ids {
        let _ = set_widget_locked_inner(app, state, &id, false);
    }
}

#[tauri::command]
pub(crate) fn widget_window_ready(
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
        vis_events::mark_dirty();
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
