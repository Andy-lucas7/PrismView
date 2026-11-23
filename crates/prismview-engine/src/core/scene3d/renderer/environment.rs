//! Ambiente (PMREM/HDRI), capa de mídia e texturas dinâmicas.

#[allow(unused_imports)]
use super::*;

impl Scene3dRenderer {
    pub(super) fn update_environment(&mut self, gpu: &GpuContext) {
        let hdri = self
            .config
            .env_map_src
            .as_deref()
            .and_then(|raw| self.resolve_path(raw));
        let key = EnvKey {
            preset: Some(self.config.env_preset),
            hdri,
        };
        if key == self.env_key {
            return;
        }
        let preset_changed = key.preset != self.env_key.preset;
        let had_hdri = self.env_key.hdri.is_some();
        self.env_key = key.clone();
        self.hdri_rx = None;

        // Procedural já (também é o fallback enquanto o HDRI carrega ou se ele falhar).
        if preset_changed || had_hdri {
            let env = ProceduralEnv::from_preset(self.config.env_preset);
            self.install_environment(gpu, EnvSource::Procedural(&env));
        }
        if let Some(path) = key.hdri {
            let (tx, rx) = mpsc::channel::<HdriResult>();
            let waker = self.waker.clone();
            let spawned = std::thread::Builder::new()
                .name("prismview-hdri".into())
                .spawn(move || {
                    let result = load_hdri(&path);
                    let _ = tx.send((path, result));
                    if let Some(wake) = waker {
                        wake();
                    }
                });
            if spawned.is_ok() {
                self.hdri_rx = Some(rx);
            }
        }
    }

    pub(super) fn install_environment(&mut self, gpu: &GpuContext, source: EnvSource<'_>) {
        self.env = self.shared.baker.bake(gpu.device(), gpu.queue(), source);
        self.frame_bind = create_frame_bind(&self.shared, gpu.device(), &self.frame_buffer, &self.env);
        self.dirty = true;
    }

    /// Atualiza a capa da mídia ativa (`MEDIA_COVER`). Só decodifica/sobe na GPU se o arquivo
    /// ou sua assinatura `(len, mtime)` mudou; devolve `true` se a cena precisa redesenhar já.
    pub fn set_media_cover(&mut self, gpu: &GpuContext, cover_path: &str) -> bool {
        let trimmed = cover_path.trim();
        let next_key = if trimmed.is_empty() {
            None
        } else {
            self.resolve_path(trimmed).and_then(DynamicImageKey::from_path)
        };
        if next_key == self.media_cover_key && trimmed == self.media_cover_raw {
            return false;
        }
        self.media_cover_raw.clear();
        self.media_cover_raw.push_str(trimmed);
        self.media_cover_key = next_key;
        if !self.config.uses_media_cover() {
            return false;
        }
        let changed = self.sync_dynamic_textures(gpu);
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// Garante que todas as texturas dinâmicas referenciadas estão carregando ou prontas, descarta
    /// capas/imagens antigas do cache (mantendo a VRAM constante) e reconstrói overrides.
    pub(super) fn sync_dynamic_textures(&mut self, gpu: &GpuContext) -> bool {
        let mut wanted: HashSet<DynamicImageKey> = HashSet::new();
        for slot in &self.slots {
            for entry in &slot.dynamic_rules {
                let key = match entry.rule.source {
                    Scene3dDynamicTextureSource::MediaCover => self.media_cover_key.clone(),
                    Scene3dDynamicTextureSource::CustomImage => entry.custom_key.clone(),
                };
                if let Some(k) = key {
                    wanted.insert(k);
                }
            }
        }
        self.dynamic_textures.retain(|k, _| wanted.contains(k));

        for key in wanted {
            if self.dynamic_textures.contains_key(&key) {
                continue;
            }
            self.dynamic_textures.insert(key.clone(), DynamicTexSlot::Loading);
            let thread_key = key.clone();
            let tx = self.dynamic_tex_tx.clone();
            let waker = self.waker.clone();
            let max_dim = self.max_dynamic_tex_dim;
            let spawned = std::thread::Builder::new()
                .name("prismview-dyntex".into())
                .spawn(move || {
                    let result = decode_dynamic_image(&thread_key.path, max_dim);
                    let _ = tx.send((thread_key, result));
                    if let Some(wake) = waker {
                        wake();
                    }
                });
            if spawned.is_err() {
                self.dynamic_textures.remove(&key);
            }
        }

        let mut any_changed = false;
        let device = gpu.device();
        for slot in &mut self.slots {
            any_changed |= rebuild_slot_overrides(
                device,
                &self.shared.upload,
                slot,
                self.media_cover_key.as_ref(),
                &self.dynamic_textures,
            );
        }
        any_changed
    }

    pub(super) fn poll_dynamic_textures(&mut self, gpu: &GpuContext) {
        let mut any_ready = false;
        loop {
            match self.dynamic_tex_rx.try_recv() {
                Ok((key, result)) => {
                    if !self.dynamic_textures.contains_key(&key) {
                        continue; // capa/cena antiga já descartada
                    }
                    match result {
                        Ok(decoded) => {
                            if let Some((texture, view, vram_bytes)) = self.shared.upload.upload_rgba_srgb(
                                gpu.device(),
                                gpu.queue(),
                                decoded.width,
                                decoded.height,
                                &decoded.rgba,
                            ) {
                                self.dynamic_textures.insert(
                                    key,
                                    DynamicTexSlot::Ready {
                                        _texture: texture,
                                        view,
                                        vram_bytes,
                                    },
                                );
                                any_ready = true;
                            } else {
                                self.dynamic_textures.insert(key, DynamicTexSlot::Failed);
                            }
                        }
                        Err(err) => {
                            eprintln!("[prismview-engine] textura dinâmica {}: {err}", key.path.display());
                            self.dynamic_textures.insert(key, DynamicTexSlot::Failed);
                        }
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if any_ready {
            let device = gpu.device();
            for slot in &mut self.slots {
                rebuild_slot_overrides(
                    device,
                    &self.shared.upload,
                    slot,
                    self.media_cover_key.as_ref(),
                    &self.dynamic_textures,
                );
            }
            self.dirty = true;
        }
    }

    pub(super) fn poll_hdri(&mut self, gpu: &GpuContext) {
        let Some(rx) = &self.hdri_rx else {
            return;
        };
        match rx.try_recv() {
            Ok((path, result)) => {
                self.hdri_rx = None;
                if self.env_key.hdri.as_deref() != Some(path.as_path()) {
                    return; // pedido antigo
                }
                match result {
                    Ok(image) => self.install_environment(gpu, EnvSource::Equirect(&image, 1.0)),
                    Err(e) => eprintln!("[prismview-engine] HDRI {}: {e}", path.display()),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.hdri_rx = None,
        }
    }

    pub(super) fn write_tonemap(&self, queue: &wgpu::Queue) {
        let exposure = if self.config.exposure.is_finite() && self.config.exposure > 0.0 {
            self.config.exposure
        } else {
            1.05
        };
        // Fundo opaco composto por baixo (sem tone mapping, como `scene.background = Color`);
        // TRANSPARENT = alpha 0 fora dos modelos. A IBL não depende do modo.
        let bg = match self.config.background_mode {
            Scene3dBackgroundMode::Transparent => None,
            Scene3dBackgroundMode::Solid => Some(
                self.config
                    .background_color
                    .as_deref()
                    .filter(|c| !c.trim().is_empty())
                    .and_then(parse_hex_srgb)
                    .unwrap_or_else(|| parse_hex_srgb("#07080b").unwrap_or([0.0, 0.0, 0.0, 1.0])),
            ),
            Scene3dBackgroundMode::Environment => {
                let sky = self.sky_background;
                Some([linear_to_srgb(sky.x), linear_to_srgb(sky.y), linear_to_srgb(sky.z), 1.0])
            }
        };
        let bg_rgb = bg.unwrap_or([0.0; 4]);
        let uniform = TonemapUniform {
            info: [exposure, if bg.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0],
            background: [bg_rgb[0], bg_rgb[1], bg_rgb[2], 1.0],
        };
        queue.write_buffer(&self.tonemap_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    /// Hora do ciclo dia/noite: `dayNightHourOverride` do Estúdio ou o relógio local.
    pub(super) fn day_night_hour(&self) -> f32 {
        self.config.hour_override().unwrap_or_else(local_hour)
    }
}
