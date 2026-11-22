E2-L4 OK (gate both: 0 erros novos; Rust NAO VALIDADO em máquina).
- A câmera já era órbita persistida (CONTRATO FINAL: target+distance+azimuth+elevation+fov+orbitParallaxX/Y+damping = position derivada). Lotes anteriores já tinham: `saveWallpaperCamera` (1 patch "discrete" = 1 undo), `computeConfigCameraView` (preview fora de ROOM parte do enquadramento salvo + paralaxe amortecida, sem alocar), sliders FOV/Amplitude H/V/Suavidade no Inspetor, motor `camera_view` + órbita por cursor (mesma fórmula do TS).
- Mudou: botão ROOM renomeado "Definir Câmera do Wallpaper" (StudioEditor).
- `native_scene.rs`: `normalize_camera` (espelha normalizeScene3DConfig: projeto sem `camera`/campos ausentes → enquadramento padrão do Estúdio, 22°/14° se 1º objeto SHOWCASE, senão 0°/8°; `target` sempre preenchido) + `number_in`.
- `config.rs` `Scene3dCamera::default` agora = padrão do Estúdio (fov 42, dist 4.8, el 8, parallax 12/7) em vez de 35/4/10/0.
- Máquina: confirmar que reabrir projeto e wallpaper nativo coincidem com o enquadramento do preview.
- Próximo (E3): ações de ossos; nada de câmera pendente.
