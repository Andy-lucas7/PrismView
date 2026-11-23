//! Estado por objeto da cena 3D (slots de modelo, instâncias, erros) e raio × AABB.

#[allow(unused_imports)]
use super::*;

// ───────────────────────── Estado por objeto ─────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct DynamicImageKey {
    pub(super) path: PathBuf,
    pub(super) len: u64,
    pub(super) modified_ns: u128,
}

impl DynamicImageKey {
    pub(super) fn from_path(path: PathBuf) -> Option<Self> {
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_DYNAMIC_IMAGE_BYTES {
            return None;
        }
        let modified_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        Some(Self {
            path,
            len: meta.len(),
            modified_ns,
        })
    }
}

pub(super) struct DecodedDynamicImage {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

pub(super) type DynamicTexResult = (DynamicImageKey, Result<DecodedDynamicImage, String>);

pub(super) enum DynamicTexSlot {
    Loading,
    Ready {
        _texture: wgpu::Texture,
        view: wgpu::TextureView,
        vram_bytes: u64,
    },
    Failed,
}

pub(super) struct SlotDynamicRule {
    pub(super) rule: Scene3dDynamicTexture,
    /// Chave resolvida em `set_config` para regras `CUSTOM_IMAGE`.
    pub(super) custom_key: Option<DynamicImageKey>,
}

pub(super) struct ObjectSlot {
    pub(super) id: String,
    pub(super) src: String,
    pub(super) kind: Scene3dObjectKind,
    pub(super) ticket: Option<Ticket>,
    pub(super) instance: Option<ModelInstance>,
    pub(super) error: Option<String>,
    /// Giro acumulado de `autoRotateSpeed` (rad).
    pub(super) spin: f32,
    /// Regras tipadas de `dynamicTextures` deste objeto.
    pub(super) dynamic_rules: Vec<SlotDynamicRule>,
    /// Overrides de material na GPU (indexados pelo índice de material do `GpuModel`).
    /// Vazio quando nenhuma textura dinâmica está ativa no objeto.
    pub(super) material_overrides: Vec<Option<GpuMaterial>>,
}

impl ObjectSlot {
    #[inline]
    pub(super) fn material<'a>(&'a self, instance: &'a ModelInstance, material_idx: u32) -> Option<&'a GpuMaterial> {
        let idx = material_idx as usize;
        self.material_overrides
            .get(idx)
            .and_then(Option::as_ref)
            .or_else(|| instance.model.materials.get(idx))
    }
}

pub(super) fn decode_dynamic_image(path: &Path, max_dim: u32) -> Result<DecodedDynamicImage, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("metadata: {e}"))?;
    if meta.len() == 0 {
        return Err("arquivo vazio".to_owned());
    }
    if meta.len() > MAX_DYNAMIC_IMAGE_BYTES {
        return Err("imagem excede o limite de 16 MiB".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("leitura: {e}"))?;
    let mut img = image::load_from_memory(&bytes).map_err(|e| format!("decodificação: {e}"))?;
    if img.width() == 0 || img.height() == 0 {
        return Err("dimensões inválidas".to_owned());
    }
    if img.width() > max_dim || img.height() > max_dim {
        img = img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Ok(DecodedDynamicImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

pub(super) fn rebuild_slot_overrides(
    device: &wgpu::Device,
    upload: &ModelUploadContext,
    slot: &mut ObjectSlot,
    media_cover_key: Option<&DynamicImageKey>,
    cache: &HashMap<DynamicImageKey, DynamicTexSlot>,
) -> bool {
    let had_overrides = !slot.material_overrides.is_empty();
    slot.material_overrides.clear();
    if slot.dynamic_rules.is_empty() {
        return had_overrides;
    }
    let Some(instance) = slot.instance.as_ref() else {
        return had_overrides;
    };
    let model = &instance.model;
    let mat_count = model.materials.len();
    if mat_count == 0 {
        return had_overrides;
    }

    let mut matched: Vec<Option<(&wgpu::TextureView, bool)>> = vec![None; mat_count];
    let mut any_match = false;
    // Wildcard ("", "*") primeiro; nomes específicos depois para sempre terem prioridade.
    for pass_named in [false, true] {
        for entry in &slot.dynamic_rules {
            if entry.rule.is_wildcard() == pass_named {
                continue;
            }
            let key = match entry.rule.source {
                Scene3dDynamicTextureSource::MediaCover => media_cover_key,
                Scene3dDynamicTextureSource::CustomImage => entry.custom_key.as_ref(),
            };
            let Some(DynamicTexSlot::Ready { view, .. }) = key.and_then(|k| cache.get(k)) else {
                continue;
            };
            for (mi, target) in matched.iter_mut().enumerate() {
                let mat_name = model.rig.material_names.get(mi).and_then(Option::as_deref);
                if entry.rule.matches_material(mat_name) {
                    *target = Some((view, entry.rule.emissive_map));
                    any_match = true;
                }
            }
        }
    }

    if !any_match {
        return had_overrides;
    }

    slot.material_overrides = matched
        .into_iter()
        .enumerate()
        .map(|(mi, m)| {
            let (view, emissive_map) = m?;
            let base = model.material_data.get(mi)?;
            Some(upload.build_material_override(device, base, &model.texture_views, view, emissive_map))
        })
        .collect();
    true
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DrawCmd {
    pub(super) key: u64,
    pub(super) depth: f32,
    pub(super) object: u32,
    pub(super) draw: u32,
    pub(super) slot: u32,
    pub(super) variant: u8,
}

pub(super) struct Targets {
    pub(super) size: (u32, u32),
    pub(super) _msaa: Option<wgpu::Texture>,
    pub(super) msaa_view: Option<wgpu::TextureView>,
    pub(super) _hdr: wgpu::Texture,
    pub(super) hdr_view: wgpu::TextureView,
    pub(super) _depth: wgpu::Texture,
    pub(super) depth_view: wgpu::TextureView,
    pub(super) output: wgpu::Texture,
    pub(super) output_view: wgpu::TextureView,
    pub(super) tonemap_bind: wgpu::BindGroup,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct EnvKey {
    pub(super) preset: Option<Scene3dEnvPreset>,
    pub(super) hdri: Option<PathBuf>,
}

pub(super) type HdriResult = (PathBuf, Result<EquirectImage, String>);

#[derive(Clone, Copy, Debug)]
pub struct Scene3dOptions {
    /// 1 ou 4 (MSAA do passe HDR).
    pub msaa: u32,
}

impl Default for Scene3dOptions {
    fn default() -> Self {
        Self { msaa: 4 }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Scene3dStats {
    pub drawn: u32,
    pub culled: u32,
    pub instances: u32,
    pub vram_bytes: u64,
}
