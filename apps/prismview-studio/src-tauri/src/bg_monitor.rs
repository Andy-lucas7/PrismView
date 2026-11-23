//! Thread de fundo: visibilidade por eventos, telemetria, cursor/sensores e recuperação do WorkerW.

#[allow(unused_imports)]
use super::*;

/// Thread `prism-bg-monitor` (dorme em `park` quando não há trabalho; ver `wake_background`).
pub(crate) static BG_THREAD: OnceLock<thread::Thread> = OnceLock::new();

/// Acorda a thread de fundo (evento de visibilidade, widget novo/movido, saída, recuperação).
pub(crate) fn wake_background() {
    if let Some(t) = BG_THREAD.get() {
        t.unpark();
    }
}

pub(crate) fn build_telemetry_snapshot(
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

pub(crate) fn reconcile_monitors_and_worker_w(app: &AppHandle, state: &SharedState) {
    let fresh_monitors = desktop_host::enumerate_monitors();
    let fresh_ids: HashSet<String> = fresh_monitors.iter().map(|m| m.id.clone()).collect();

    let (stale_labels, pinned_widget_snapshots) = {
        let Ok(mut st) = state.lock() else {
            return;
        };
        st.monitors = fresh_monitors.clone();
        st.monitor_visibility
            .retain(|mid, _| fresh_ids.contains(mid));
        for m in &fresh_monitors {
            st.monitor_visibility.entry(m.id.clone()).or_insert(true);
        }

        let stale: Vec<String> = Vec::new();

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

/// Telemetria para janelas visíveis (Hub/widgets).
pub(crate) const TELEMETRY_INTERVAL: Duration = Duration::from_millis(1500);

/// Intervalo mínimo entre recálculos de cobertura (arrastos geram muitos eventos).
pub(crate) const VIS_MIN_INTERVAL: Duration = Duration::from_millis(100);

/// Rede de segurança para eventos perdidos (só com widgets/Hub visíveis).
pub(crate) const VIS_SAFETY_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) fn spawn_background_monitors(app: AppHandle, state: SharedState, stop: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("prism-bg-monitor".to_string())
        .spawn(move || {
            if BG_THREAD.set(thread::current()).is_err() {
                eprintln!("[prismview-studio] prism-bg-monitor já registrado");
            }
            vis_events::set_waker(Box::new(wake_background));
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
                        && !st.pinned_widgets.is_empty()
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

                // Cobertura do desktop só sob evento (`vis_events`), no máx. 1×/100 ms durante
                // arrastos; rede de segurança a cada 5 s.
                let vis_due = now.duration_since(last_vis_check) >= VIS_MIN_INTERVAL
                    && (vis_events::take_dirty()
                        || now.duration_since(last_vis_check) >= VIS_SAFETY_INTERVAL);
                if need_resync || vis_due {
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

                    let (manual_paused, widget_labels_ready) =
                        if let Ok(mut st) = state.lock() {
                            st.monitor_visibility = per_mon_vis.clone();
                            st.desktop_visible = any_desktop_vis;
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
                            (st.manual_paused, wgs)
                        } else {
                            (false, Vec::new())
                        };

                    // Prune stale entries from last_window_vis so closed windows never leak memory
                    let mut live_labels: HashSet<String> = HashSet::new();
                    for (lbl, _, _) in &widget_labels_ready {
                        live_labels.insert(lbl.clone());
                    }
                    last_window_vis.retain(|k, _| live_labels.contains(k));

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
                    let c_targets: Vec<(String, MonitorInfoDto)> = Vec::new();
                    let mut w_snap: Vec<(String, i32, i32, bool, bool, bool)> = Vec::new();

                    if !st.manual_paused {
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
                    && now.duration_since(last_telemetry) >= TELEMETRY_INTERVAL
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

                // Dorme até o próximo trabalho: cursor/sensores (24 ms), telemetria visível (1,5 s),
                // saves de widget pendentes; sem nada disso, `park` sem prazo (0 ciclos) até um
                // evento de visibilidade/widget/saída chamar `wake_background`.
                let (has_dirty_saves, has_widgets, recovering) = state
                    .lock()
                    .map(|st| {
                        (
                            !st.dirty_widget_saves.is_empty(),
                            !st.pinned_widgets.is_empty(),
                            st.pending_recovery_at.is_some(),
                        )
                    })
                    .unwrap_or((false, false, false));
                let wait = if fast_tick {
                    Some(Duration::from_millis(24))
                } else if has_dirty_saves || recovering {
                    Some(Duration::from_millis(250))
                } else if !telemetry_targets.is_empty() {
                    Some(TELEMETRY_INTERVAL.saturating_sub(now.duration_since(last_telemetry)).max(Duration::from_millis(50)))
                } else if has_widgets {
                    Some(VIS_SAFETY_INTERVAL)
                } else {
                    None
                };
                match wait {
                    Some(d) => thread::park_timeout(d),
                    None => thread::park(),
                }
            }
        })
        .ok();
}
