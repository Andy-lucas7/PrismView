//! Imagens: decodificação em thread com limites, redução à saída e upload com mips.

#[allow(unused_imports)]
use super::*;

// ───────────────────────── Imagens ─────────────────────────

#[derive(Clone, Copy)]
pub(super) struct ImageEntry {
    pub(super) id: TextureId,
    pub(super) w: u32,
    pub(super) h: u32,
}

/// Teto de memória da decodificação de uma imagem (bytes).
pub(super) const IMAGE_MAX_ALLOC_BYTES: u64 = 512 * 1024 * 1024;
/// Lado máximo aceito na decodificação (px); maiores são recusados.
pub(super) const IMAGE_MAX_SIDE: u32 = 16384;

/// Fator (≤ 1) para a imagem ainda cobrir a saída inteira (`viewport`) sem ultrapassar `max`.
/// Maior que isso só gastaria VRAM: na tela ela nunca aparece maior que a saída.
pub(super) fn image_target_scale(size: (u32, u32), viewport: (u32, u32), max: u32) -> f32 {
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let cover = (viewport.0.max(1) as f32 / w).max(viewport.1.max(1) as f32 / h);
    let device = max.max(1) as f32 / w.max(h);
    cover.min(device).min(1.0)
}

/// Acorda o loop quando uma imagem termina de decodificar (registrado 1× pelo host).
pub type ImageWaker = Arc<dyn Fn() + Send + Sync>;

pub(super) static IMAGE_WAKER: OnceLock<ImageWaker> = OnceLock::new();

/// Registra quem acorda o loop após decodificações em segundo plano (chamar uma vez no início).
pub fn set_image_waker(waker: ImageWaker) {
    if IMAGE_WAKER.set(waker).is_err() {
        eprintln!("[scene2d] image waker já registrado");
    }
}

/// Pixels RGBA8 pré-multiplicados prontos para upload.
pub(super) struct DecodedImage {
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) rgba: Vec<u8>,
}

/// Imagens: decodificadas em threads (nunca no loop de render) e enviadas à GPU no rebuild
/// seguinte. Enquanto decodifica, a camada não é desenhada.
#[derive(Default)]
pub(super) struct ImageCache {
    pub(super) loaded: HashMap<String, ImageEntry>,
    pub(super) failed: HashSet<String>,
    pub(super) pending: HashMap<String, mpsc::Receiver<Option<DecodedImage>>>,
    /// Alguma decodificação terminou desde o último `take_completed`.
    pub(super) completed: Arc<AtomicBool>,
}

impl ImageCache {
    pub(super) fn get(
        &mut self,
        path: &Path,
        viewport: (u32, u32),
        compositor: &mut Compositor2D,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<ImageEntry> {
        let key = path.to_string_lossy();
        if let Some(entry) = self.loaded.get(&*key) {
            return Some(*entry);
        }
        if self.failed.contains(&*key) {
            return None;
        }
        let key = key.into_owned();
        let Some(rx) = self.pending.get(&key) else {
            self.spawn_decode(path, key, compositor.max_texture_dim(), viewport);
            return None;
        };
        let decoded = match rx.try_recv() {
            Ok(decoded) => decoded,
            Err(mpsc::TryRecvError::Empty) => return None,
            // Thread caiu (pânico no decoder): trata como falha.
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        self.pending.remove(&key);
        let entry = decoded.and_then(|img| {
            compositor
                .create_texture_rgba_mipped(device, queue, img.w, img.h, &img.rgba)
                .map(|id| ImageEntry { id, w: img.w, h: img.h })
        });
        match entry {
            Some(entry) => {
                self.loaded.insert(key, entry);
                Some(entry)
            }
            None => {
                eprintln!("[scene2d] imagem não carregou: {}", path.display());
                self.failed.insert(key);
                None
            }
        }
    }

    pub(super) fn spawn_decode(&mut self, path: &Path, key: String, max_dim: u32, viewport: (u32, u32)) {
        let (tx, rx) = mpsc::channel();
        let completed = Arc::clone(&self.completed);
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("prismview-image-decode".into())
            .spawn(move || {
                let decoded = Self::decode(&path, max_dim, viewport);
                let delivered = tx.send(decoded).is_ok();
                completed.store(true, Ordering::Release);
                if delivered {
                    if let Some(wake) = IMAGE_WAKER.get() {
                        wake();
                    }
                }
            });
        match spawned {
            Ok(_) => {
                self.pending.insert(key, rx);
            }
            Err(err) => {
                eprintln!("[scene2d] thread de imagem: {err}");
                self.failed.insert(key);
            }
        }
    }

    /// Decodifica (CPU, fora do loop), limita ao máximo do device e pré-multiplica.
    /// Decodifica com limites (sem "bomba" de descompressão), reduz ao necessário para cobrir a
    /// saída (`viewport`, px) e ao máximo do device, e pré-multiplica. Fora do loop de render.
    pub(super) fn decode(path: &Path, max: u32, viewport: (u32, u32)) -> Option<DecodedImage> {
        let decoded = (|| -> image::ImageResult<image::DynamicImage> {
            let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(IMAGE_MAX_ALLOC_BYTES);
            limits.max_image_width = Some(IMAGE_MAX_SIDE);
            limits.max_image_height = Some(IMAGE_MAX_SIDE);
            reader.limits(limits);
            reader.decode()
        })();
        let mut img = match decoded {
            Ok(img) => img.into_rgba8(),
            Err(err) => {
                eprintln!("[scene2d] imagem {}: {err}", path.display());
                return None;
            }
        };
        let (w, h) = img.dimensions();
        let k = image_target_scale((w, h), viewport, max);
        if k < 1.0 {
            let nw = ((w as f32 * k).ceil() as u32).max(1);
            let nh = ((h as f32 * k).ceil() as u32).max(1);
            img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::CatmullRom);
        }
        let (w, h) = img.dimensions();
        for px in img.chunks_exact_mut(4) {
            let a = u32::from(px[3]);
            if a != 255 {
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * a + 127) / 255) as u8;
                }
            }
        }
        Some(DecodedImage { w, h, rgba: img.into_raw() })
    }

    /// `true` uma vez por lote de decodificações concluídas (o rebuild as envia à GPU).
    pub(super) fn take_completed(&self) -> bool {
        !self.pending.is_empty() && self.completed.swap(false, Ordering::AcqRel)
    }

    pub(super) fn release_all(&mut self, compositor: &mut Compositor2D) {
        for (_, entry) in self.loaded.drain() {
            compositor.release_texture(entry.id);
        }
        self.failed.clear();
        // Threads em curso terminam sozinhas (o envio falha com o receptor descartado).
        self.pending.clear();
    }
}

#[derive(Clone)]
pub(super) struct TextBinding {
    pub(super) id: TextureId,
    pub(super) gen: u64,
    pub(super) key: Arc<str>,
}
