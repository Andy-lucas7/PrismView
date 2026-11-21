//! Loader síncrono de `.glb`/`.gltf` → `ModelAsset` (dados de CPU). Roda **fora** do thread de
//! render (ver `loader.rs`).
//!
//! Suporte:
//! - container GLB e `.gltf` + buffers externos/`data:` URI (sem `..`/caminhos absolutos);
//! - `EXT_meshopt_compression` (buffers de fallback sem dados recebem os bytes decodificados,
//!   então os accessors leem tudo de forma uniforme) e `KHR_mesh_quantization` (inteiros
//!   normalizados/não normalizados em qualquer atributo);
//! - texturas PNG/JPEG/WebP (decodifica 1×, limita a dimensão) e KTX2/UASTC (`KHR_texture_basisu`,
//!   inclusive arquivos sem o `source` de fallback, que o crate `gltf` rejeitaria);
//! - hierarquia de nós (`parent`/`children`, ciclos quebrados), skins (joints + inverse bind),
//!   materiais PBR metálico/rugoso (`KHR_materials_emissive_strength`);
//! - morph targets (deltas de posição/normal, `mesh.weights`/`node.weights`) e animações
//!   (T/R/S/pesos, STEP/LINEAR/CUBICSPLINE).
//!
//! Fora do escopo desta fase: Draco, accessors esparsos, sampler (wrap/filter), câmeras e luzes. Primitivas não suportadas viram `warnings`, nunca erro fatal.
//!
//! Regras: nenhum `unwrap/expect`; todo índice vindo do arquivo é validado (o loader desliga a
//! validação do crate `gltf`, então `loader.rs` ainda envolve a chamada em `catch_unwind`).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Component, Path};
use std::sync::Arc;

use base64::Engine as _;
use glam::{Mat4, Quat, Vec3};
use gltf::accessor::DataType;
use gltf::material::AlphaMode as GltfAlpha;
use gltf::mesh::Mode;
use gltf::{buffer, image as gimage, Document, Semantic};
use serde_json::Value;

use super::ktx2::{decode_ktx2, is_ktx2, Ktx2Options};
use super::meshopt::MeshoptView;
use super::model::{
    Aabb, AlphaMode, AnimPath, AnimationChannel, AnimationClip, AssetError, Interpolation,
    MaterialData, MeshData, ModelAsset, MorphTargetData, NodeData, PrimitiveData, SkinData,
    TexelFormat, TextureAsset, TextureLevel, TextureSlot,
};
use super::vault::{AssetHash, AssetVault};

const MAX_FILE_BYTES: u64 = 1 << 30;

#[derive(Clone, Copy, Debug)]
pub struct LoaderConfig {
    /// O device aceita BC7 (`TEXTURE_COMPRESSION_BC`): KTX2/UASTC é transcodificado para BC7.
    pub allow_bc7: bool,
    /// Maior lado de textura mantido em RAM/VRAM (PNG/JPEG são reduzidos; mips KTX2 maiores caem).
    pub max_texture_dim: u32,
    /// Threads de decodificação (1 mantém o pico de RAM baixo).
    pub worker_threads: usize,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            allow_bc7: false,
            max_texture_dim: 2048,
            worker_threads: 1,
        }
    }
}

// ───────────────────────── Entrada ─────────────────────────

/// Converte os bytes de um `.glb`/`.gltf` em `ModelAsset`. `base_dir` resolve URIs relativas.
pub fn parse_model(
    bytes: Vec<u8>,
    hash: AssetHash,
    base_dir: Option<&Path>,
    vault: &AssetVault,
    cfg: &LoaderConfig,
) -> Result<ModelAsset, AssetError> {
    let (json_bytes, blob) = split_container(&bytes)?;
    drop(bytes);

    let mut value: Value = serde_json::from_slice(&json_bytes)
        .map_err(|e| AssetError::Format(format!("JSON do glTF: {e}")))?;
    drop(json_bytes);
    patch_basisu_sources(&mut value);
    let root: gltf::json::Root = serde_json::from_value(value)
        .map_err(|e| AssetError::Format(format!("glTF: {e}")))?;
    let document = Document::from_json_without_validation(root);

    let mut buffers = load_buffers(&document, blob, base_dir)?;
    decode_meshopt_views(&document, &mut buffers)?;

    let mut warnings: Vec<String> = Vec::new();
    let mut textures = TextureTable::default();

    let materials: Vec<MaterialData> = document
        .materials()
        .map(|m| convert_material(&m, &document, &buffers, base_dir, vault, cfg, &mut textures, &mut warnings))
        .collect();

    let meshes: Vec<MeshData> = document
        .meshes()
        .map(|mesh| convert_mesh(&mesh, &buffers, materials.len(), &mut warnings))
        .collect();

    let mut nodes = convert_nodes(&document);
    let roots = resolve_hierarchy(&document, &mut nodes);
    let skins: Vec<SkinData> = document
        .skins()
        .map(|skin| convert_skin(&skin, &buffers, &mut warnings))
        .collect();
    let animations = convert_animations(&document, &buffers, &meshes, &nodes, &mut warnings);
    drop(buffers);

    let mut asset = ModelAsset {
        hash,
        name: document.scenes().next().and_then(|s| s.name().map(str::to_owned)),
        nodes,
        roots,
        meshes,
        materials,
        skins,
        animations,
        textures: textures.list,
        bounds: Aabb::EMPTY,
        warnings,
    };
    let world = asset.world_matrices();
    let mut bounds = Aabb::EMPTY;
    for (index, node) in asset.nodes.iter().enumerate() {
        let Some(mesh) = node.mesh.and_then(|m| asset.meshes.get(m as usize)) else {
            continue;
        };
        for prim in &mesh.primitives {
            if let (Some(b), Some(m)) = (prim.bounds.as_ref(), world.get(index)) {
                bounds.union(&b.transformed(m));
            }
        }
    }
    asset.bounds = bounds;
    Ok(asset)
}

fn split_container(bytes: &[u8]) -> Result<(Vec<u8>, Option<Vec<u8>>), AssetError> {
    if bytes.starts_with(b"glTF") {
        let glb = gltf::Glb::from_slice(bytes)
            .map_err(|e| AssetError::Format(format!("GLB: {e}")))?;
        Ok((glb.json.into_owned(), glb.bin.map(Cow::into_owned)))
    } else {
        Ok((bytes.to_vec(), None))
    }
}

/// `KHR_texture_basisu` permite omitir `texture.source` quando a extensão é obrigatória; o crate
/// `gltf` exige o campo. Copia o `source` da extensão para o campo padrão.
fn patch_basisu_sources(root: &mut Value) {
    let Some(textures) = root.get_mut("textures").and_then(Value::as_array_mut) else {
        return;
    };
    for texture in textures {
        if texture.get("source").is_some() {
            continue;
        }
        let basisu = texture
            .pointer("/extensions/KHR_texture_basisu/source")
            .cloned();
        if let (Some(source), Some(object)) = (basisu, texture.as_object_mut()) {
            object.insert("source".to_owned(), source);
        }
    }
}

// ───────────────────────── Buffers ─────────────────────────

fn read_uri(uri: &str, base: Option<&Path>) -> Result<Vec<u8>, AssetError> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (header, payload) = rest
            .split_once(',')
            .ok_or_else(|| AssetError::Format("data URI inválida".into()))?;
        if !header.ends_with(";base64") {
            return Err(AssetError::Unsupported("data URI sem base64".into()));
        }
        return base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|e| AssetError::Format(format!("base64: {e}")));
    }
    let base = base.ok_or_else(|| AssetError::Io(format!("URI relativa sem pasta base: {uri}")))?;
    if uri.contains("://") || uri.contains('\\') {
        return Err(AssetError::Unsupported(format!("URI não suportada: {uri}")));
    }
    let relative = Path::new(uri);
    let safe = relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !safe {
        return Err(AssetError::Format(format!("URI fora da pasta do modelo: {uri}")));
    }
    let path = base.join(relative);
    let meta = std::fs::metadata(&path).map_err(|e| AssetError::Io(format!("{}: {e}", path.display())))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(AssetError::Io(format!("{}: arquivo grande demais", path.display())));
    }
    std::fs::read(&path).map_err(|e| AssetError::Io(format!("{}: {e}", path.display())))
}

fn load_buffers(
    document: &Document,
    mut blob: Option<Vec<u8>>,
    base: Option<&Path>,
) -> Result<Vec<Vec<u8>>, AssetError> {
    let mut out = Vec::with_capacity(document.buffers().len());
    for buf in document.buffers() {
        let is_fallback = buf
            .extension_value("EXT_meshopt_compression")
            .and_then(|v| v.get("fallback"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let data = match buf.source() {
            // Fallback sem dados: só reserva o espaço onde os views decodificados serão escritos.
            buffer::Source::Bin if is_fallback => vec![0u8; buf.length()],
            buffer::Source::Bin => blob
                .take()
                .ok_or_else(|| AssetError::Format("buffer sem dados (chunk BIN ausente)".into()))?,
            buffer::Source::Uri(uri) => {
                if is_fallback {
                    vec![0u8; buf.length()]
                } else {
                    read_uri(uri, base)?
                }
            }
        };
        out.push(data);
    }
    Ok(out)
}

fn decode_meshopt_views(document: &Document, buffers: &mut [Vec<u8>]) -> Result<(), AssetError> {
    for view in document.views() {
        let Some(ext) = view.extension_value("EXT_meshopt_compression") else {
            continue;
        };
        let params = MeshoptView::from_json(ext)?;
        let decoded = {
            let source = buffers
                .get(params.buffer)
                .ok_or_else(|| AssetError::Format("meshopt: buffer inexistente".into()))?;
            params.decode(source)?
        };
        let target = buffers
            .get_mut(view.buffer().index())
            .ok_or_else(|| AssetError::Format("meshopt: buffer de destino inexistente".into()))?;
        let start = view.offset();
        let end = start
            .checked_add(decoded.len())
            .filter(|&e| e <= target.len())
            .ok_or_else(|| AssetError::Format("meshopt: view descomprimido não cabe no buffer".into()))?;
        target[start..end].copy_from_slice(&decoded);
    }
    Ok(())
}

// ───────────────────────── Accessors ─────────────────────────

enum Comp {
    Int(i64),
    Float(f32),
}

fn read_comp(buf: &[u8], off: usize, dt: DataType) -> Option<Comp> {
    Some(match dt {
        DataType::I8 => Comp::Int(i64::from(*buf.get(off)? as i8)),
        DataType::U8 => Comp::Int(i64::from(*buf.get(off)?)),
        DataType::I16 => Comp::Int(i64::from(i16::from_le_bytes(buf.get(off..off + 2)?.try_into().ok()?))),
        DataType::U16 => Comp::Int(i64::from(u16::from_le_bytes(buf.get(off..off + 2)?.try_into().ok()?))),
        DataType::U32 => Comp::Int(i64::from(u32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))),
        DataType::F32 => Comp::Float(f32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?)),
    })
}

fn comp_to_f32(c: Comp, dt: DataType, normalized: bool) -> f32 {
    match c {
        Comp::Float(v) => v,
        Comp::Int(v) => {
            let v = v as f32;
            if !normalized {
                return v;
            }
            match dt {
                DataType::I8 => (v / 127.0).max(-1.0),
                DataType::U8 => v / 255.0,
                DataType::I16 => (v / 32767.0).max(-1.0),
                DataType::U16 => v / 65535.0,
                DataType::U32 => v / 4_294_967_295.0,
                DataType::F32 => v,
            }
        }
    }
}

struct Layout<'a> {
    buf: &'a [u8],
    start: usize,
    stride: usize,
    comp_size: usize,
    comps: usize,
    count: usize,
    dt: DataType,
    normalized: bool,
}

fn layout<'a>(acc: &gltf::Accessor, buffers: &'a [Vec<u8>]) -> Result<Option<Layout<'a>>, AssetError> {
    let dt = acc.data_type();
    let comps = acc.dimensions().multiplicity();
    let comp_size = dt.size();
    let elem_size = comp_size * comps;
    let count = acc.count();
    let Some(view) = acc.view() else {
        return Ok(None); // sem bufferView: valores zerados (esparso ignorado)
    };
    let buf = buffers
        .get(view.buffer().index())
        .ok_or_else(|| AssetError::Format("accessor: buffer inexistente".into()))?
        .as_slice();
    let stride = view.stride().unwrap_or(elem_size).max(elem_size);
    let start = view
        .offset()
        .checked_add(acc.offset())
        .ok_or_else(|| AssetError::Format("accessor: offset inválido".into()))?;
    if count > 0 {
        let last = (count - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(start))
            .and_then(|n| n.checked_add(elem_size))
            .ok_or_else(|| AssetError::Format("accessor: tamanho inválido".into()))?;
        if last > buf.len() {
            return Err(AssetError::Format("accessor fora do buffer".into()));
        }
    }
    Ok(Some(Layout {
        buf,
        start,
        stride,
        comp_size,
        comps,
        count,
        dt,
        normalized: acc.normalized(),
    }))
}

/// Lê um accessor como `f32` (inteiros normalizados convertidos). Devolve `(valores, componentes)`.
fn read_f32(acc: &gltf::Accessor, buffers: &[Vec<u8>]) -> Result<(Vec<f32>, usize), AssetError> {
    let comps = acc.dimensions().multiplicity();
    let Some(l) = layout(acc, buffers)? else {
        return Ok((vec![0.0; acc.count() * comps], comps));
    };
    let mut out = Vec::with_capacity(l.count * l.comps);
    for i in 0..l.count {
        let base = l.start + i * l.stride;
        for c in 0..l.comps {
            let comp = read_comp(l.buf, base + c * l.comp_size, l.dt)
                .ok_or_else(|| AssetError::Format("accessor truncado".into()))?;
            out.push(comp_to_f32(comp, l.dt, l.normalized));
        }
    }
    Ok((out, l.comps))
}

/// Lê um accessor inteiro (índices, joints). Floats são arredondados.
fn read_u32(acc: &gltf::Accessor, buffers: &[Vec<u8>]) -> Result<(Vec<u32>, usize), AssetError> {
    let comps = acc.dimensions().multiplicity();
    let Some(l) = layout(acc, buffers)? else {
        return Ok((vec![0; acc.count() * comps], comps));
    };
    let mut out = Vec::with_capacity(l.count * l.comps);
    for i in 0..l.count {
        let base = l.start + i * l.stride;
        for c in 0..l.comps {
            let comp = read_comp(l.buf, base + c * l.comp_size, l.dt)
                .ok_or_else(|| AssetError::Format("accessor truncado".into()))?;
            out.push(match comp {
                Comp::Int(v) => u32::try_from(v.max(0)).unwrap_or(u32::MAX),
                Comp::Float(v) => v.max(0.0).round() as u32,
            });
        }
    }
    Ok((out, l.comps))
}

fn chunk3(v: &[f32]) -> Vec<[f32; 3]> {
    v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
}

fn chunk2(v: &[f32]) -> Vec<[f32; 2]> {
    v.chunks_exact(2).map(|c| [c[0], c[1]]).collect()
}

fn chunk4(v: &[f32]) -> Vec<[f32; 4]> {
    v.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect()
}

// ───────────────────────── Malhas ─────────────────────────

fn convert_mesh(
    mesh: &gltf::Mesh,
    buffers: &[Vec<u8>],
    material_count: usize,
    warnings: &mut Vec<String>,
) -> MeshData {
    let mut out = MeshData {
        name: mesh.name().map(str::to_owned),
        primitives: Vec::with_capacity(mesh.primitives().len()),
        weights: mesh.weights().map(<[f32]>::to_vec).unwrap_or_default(),
    };
    let label = mesh.name().unwrap_or("<sem nome>").to_owned();
    for prim in mesh.primitives() {
        if prim.mode() != Mode::Triangles {
            warnings.push(format!("malha `{label}`: primitiva {:?} ignorada (só triângulos)", prim.mode()));
            continue;
        }
        if prim.extension_value("KHR_draco_mesh_compression").is_some() {
            warnings.push(format!("malha `{label}`: KHR_draco_mesh_compression não suportado"));
            continue;
        }
        match convert_primitive(&prim, buffers, material_count, &label, warnings) {
            Ok(p) => out.primitives.push(p),
            Err(e) => warnings.push(format!("malha `{label}`: {e}")),
        }
    }
    out
}

fn convert_primitive(
    prim: &gltf::Primitive,
    buffers: &[Vec<u8>],
    material_count: usize,
    label: &str,
    warnings: &mut Vec<String>,
) -> Result<PrimitiveData, AssetError> {
    let mut out = PrimitiveData::default();
    let mut positions_seen = false;

    for (semantic, acc) in prim.attributes() {
        match semantic {
            Semantic::Positions => {
                let (v, comps) = read_f32(&acc, buffers)?;
                if comps != 3 {
                    return Err(AssetError::Format("POSITION não é VEC3".into()));
                }
                out.positions = chunk3(&v);
                positions_seen = true;
            }
            Semantic::Normals => {
                let (v, comps) = read_f32(&acc, buffers)?;
                if comps == 3 {
                    out.normals = chunk3(&v);
                }
            }
            Semantic::Tangents => {
                let (v, comps) = read_f32(&acc, buffers)?;
                if comps == 4 {
                    out.tangents = chunk4(&v);
                }
            }
            Semantic::TexCoords(0) => {
                let (v, comps) = read_f32(&acc, buffers)?;
                if comps == 2 {
                    out.uv0 = chunk2(&v);
                }
            }
            Semantic::TexCoords(1) => {
                let (v, comps) = read_f32(&acc, buffers)?;
                if comps == 2 {
                    out.uv1 = chunk2(&v);
                }
            }
            Semantic::Colors(0) => {
                let (v, comps) = read_f32(&acc, buffers)?;
                out.colors = match comps {
                    4 => chunk4(&v),
                    3 => v.chunks_exact(3).map(|c| [c[0], c[1], c[2], 1.0]).collect(),
                    _ => Vec::new(),
                };
            }
            Semantic::Joints(0) => {
                let (v, comps) = read_u32(&acc, buffers)?;
                if comps == 4 {
                    let j16 = |j: u32| u16::try_from(j).unwrap_or(u16::MAX);
                    out.joints = v
                        .chunks_exact(4)
                        .map(|c| [j16(c[0]), j16(c[1]), j16(c[2]), j16(c[3])])
                        .collect();
                }
            }
            Semantic::Weights(0) => {
                let (v, comps) = read_f32(&acc, buffers)?;
                if comps == 4 {
                    out.weights = chunk4(&v);
                }
            }
            _ => {}
        }
    }
    if !positions_seen || out.positions.is_empty() {
        return Err(AssetError::Format("primitiva sem POSITION".into()));
    }
    let vertex_count = out.positions.len();

    // Atributos com contagem diferente de POSITION são descartados (arquivo incoerente).
    if out.normals.len() != vertex_count {
        out.normals.clear();
    }
    if out.tangents.len() != vertex_count {
        out.tangents.clear();
    }
    if out.uv0.len() != vertex_count {
        out.uv0.clear();
    }
    if out.uv1.len() != vertex_count {
        out.uv1.clear();
    }
    if out.colors.len() != vertex_count {
        out.colors.clear();
    }
    if out.joints.len() != vertex_count || out.weights.len() != vertex_count {
        out.joints.clear();
        out.weights.clear();
    }

    out.indices = match prim.indices() {
        Some(acc) => read_u32(&acc, buffers)?.0,
        None => (0..u32::try_from(vertex_count).unwrap_or(u32::MAX)).collect(),
    };
    let tri_len = out.indices.len() - out.indices.len() % 3;
    out.indices.truncate(tri_len);
    let max_index = u32::try_from(vertex_count).unwrap_or(u32::MAX);
    if out.indices.iter().any(|&i| i >= max_index) {
        return Err(AssetError::Format("índice fora do intervalo de vértices".into()));
    }

    out.material = prim
        .material()
        .index()
        .filter(|&m| m < material_count)
        .and_then(|m| u32::try_from(m).ok());

    if out.normals.is_empty() {
        out.normals = smooth_normals(&out.positions, &out.indices);
    }

    out.morph_targets = read_morph_targets(prim, buffers, vertex_count, label, warnings);

    let mut bounds = Aabb::EMPTY;
    for p in &out.positions {
        bounds.extend(Vec3::from(*p));
    }
    out.bounds = Some(bounds);
    Ok(out)
}

/// Lê os deltas dos morph targets. Alvo com contagem incoerente vira alvo nulo (mantém o índice
/// alinhado com `mesh.weights`).
fn read_morph_targets(
    prim: &gltf::Primitive,
    buffers: &[Vec<u8>],
    vertex_count: usize,
    label: &str,
    warnings: &mut Vec<String>,
) -> Vec<MorphTargetData> {
    let mut targets = Vec::new();
    for (index, target) in prim.morph_targets().enumerate() {
        let mut data = MorphTargetData::default();
        if let Some(acc) = target.positions() {
            match read_f32(&acc, buffers) {
                Ok((v, 3)) if v.len() == vertex_count * 3 => data.positions = chunk3(&v),
                Ok(_) => warnings.push(format!("malha `{label}`: morph #{index} POSITION incoerente")),
                Err(e) => warnings.push(format!("malha `{label}`: morph #{index}: {e}")),
            }
        }
        if let Some(acc) = target.normals() {
            match read_f32(&acc, buffers) {
                Ok((v, 3)) if v.len() == vertex_count * 3 => data.normals = chunk3(&v),
                Ok(_) => warnings.push(format!("malha `{label}`: morph #{index} NORMAL incoerente")),
                Err(e) => warnings.push(format!("malha `{label}`: morph #{index}: {e}")),
            }
        }
        if data.positions.is_empty() {
            data.positions = vec![[0.0; 3]; vertex_count];
        }
        targets.push(data);
    }
    targets
}

/// Normais suaves ponderadas pela área quando o arquivo não traz `NORMAL`.
fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks_exact(3) {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let (pa, pb, pc) = (Vec3::from(positions[a]), Vec3::from(positions[b]), Vec3::from(positions[c]));
        let n = (pb - pa).cross(pc - pa); // |n| = 2 × área
        acc[a] += n;
        acc[b] += n;
        acc[c] += n;
    }
    acc.into_iter()
        .map(|n| {
            let n = n.try_normalize().unwrap_or(Vec3::Y);
            n.to_array()
        })
        .collect()
}

// ───────────────────────── Nós e skins ─────────────────────────

fn convert_nodes(document: &Document) -> Vec<NodeData> {
    document
        .nodes()
        .map(|node| {
            let (t, r, s) = node.transform().decomposed();
            NodeData {
                name: node.name().map(str::to_owned),
                parent: None,
                children: node
                    .children()
                    .filter_map(|c| u32::try_from(c.index()).ok())
                    .collect(),
                translation: Vec3::from(t),
                rotation: Quat::from_array(r).normalize(),
                scale: Vec3::from(s),
                mesh: node.mesh().and_then(|m| u32::try_from(m.index()).ok()),
                skin: node.skin().and_then(|s| u32::try_from(s.index()).ok()),
                weights: node.weights().map(<[f32]>::to_vec).unwrap_or_default(),
            }
        })
        .collect()
}

/// Preenche `parent` a partir de `children`, quebra ciclos e devolve as raízes da cena padrão.
fn resolve_hierarchy(document: &Document, nodes: &mut [NodeData]) -> Vec<u32> {
    let count = nodes.len();
    for i in 0..count {
        let children = nodes[i].children.clone();
        for child in children {
            if let Some(c) = nodes.get_mut(child as usize) {
                if c.parent.is_none() && child as usize != i {
                    c.parent = u32::try_from(i).ok();
                }
            }
        }
    }
    // Ciclo: subir mais de `count` passos significa que o nó nunca chega a uma raiz.
    for i in 0..count {
        let mut cursor = nodes[i].parent;
        let mut steps = 0usize;
        while let Some(p) = cursor {
            steps += 1;
            if steps > count {
                nodes[i].parent = None;
                break;
            }
            cursor = nodes.get(p as usize).and_then(|n| n.parent);
        }
    }
    // Filhos coerentes com os pais finais.
    for i in 0..count {
        let me = u32::try_from(i).ok();
        let kept: Vec<u32> = nodes[i]
            .children
            .iter()
            .copied()
            .filter(|&c| nodes.get(c as usize).is_some_and(|n| n.parent == me))
            .collect();
        nodes[i].children = kept;
    }

    let scene = document.default_scene().or_else(|| document.scenes().next());
    let mut roots: Vec<u32> = match scene {
        Some(scene) => scene
            .nodes()
            .filter_map(|n| u32::try_from(n.index()).ok())
            .filter(|&n| nodes.get(n as usize).is_some_and(|node| node.parent.is_none()))
            .collect(),
        None => Vec::new(),
    };
    if roots.is_empty() {
        roots = (0..count)
            .filter(|&i| nodes[i].parent.is_none())
            .filter_map(|i| u32::try_from(i).ok())
            .collect();
    }
    roots
}

fn convert_skin(skin: &gltf::Skin, buffers: &[Vec<u8>], warnings: &mut Vec<String>) -> SkinData {
    let joints: Vec<u32> = skin
        .joints()
        .filter_map(|j| u32::try_from(j.index()).ok())
        .collect();
    let mut inverse_bind: Vec<Mat4> = Vec::new();
    if let Some(acc) = skin.inverse_bind_matrices() {
        match read_f32(&acc, buffers) {
            Ok((v, 16)) => {
                inverse_bind = v
                    .chunks_exact(16)
                    .filter_map(|m| <[f32; 16]>::try_from(m).ok())
                    .map(|m| Mat4::from_cols_array(&m))
                    .collect();
            }
            Ok(_) => warnings.push("skin: inverseBindMatrices não é MAT4".to_owned()),
            Err(e) => warnings.push(format!("skin: {e}")),
        }
    }
    if inverse_bind.len() != joints.len() {
        inverse_bind.resize(joints.len(), Mat4::IDENTITY);
    }
    SkinData {
        name: skin.name().map(str::to_owned),
        joints,
        inverse_bind,
        skeleton_root: skin.skeleton().and_then(|n| u32::try_from(n.index()).ok()),
    }
}

// ───────────────────────── Animações ─────────────────────────

fn convert_animations(
    document: &Document,
    buffers: &[Vec<u8>],
    meshes: &[MeshData],
    nodes: &[NodeData],
    warnings: &mut Vec<String>,
) -> Vec<AnimationClip> {
    use gltf::animation::{Interpolation as GInterp, Property};

    let mut clips = Vec::with_capacity(document.animations().len());
    for (clip_index, anim) in document.animations().enumerate() {
        let label = anim.name().map_or_else(|| format!("#{clip_index}"), str::to_owned);
        let mut clip = AnimationClip {
            name: anim.name().map(str::to_owned),
            channels: Vec::with_capacity(anim.channels().count()),
            duration: 0.0,
        };
        for channel in anim.channels() {
            let target = channel.target();
            let node = target.node().index();
            let Some(node_data) = nodes.get(node) else {
                warnings.push(format!("animação `{label}`: nó inexistente"));
                continue;
            };
            let path = match target.property() {
                Property::Translation => AnimPath::Translation,
                Property::Rotation => AnimPath::Rotation,
                Property::Scale => AnimPath::Scale,
                Property::MorphTargetWeights => AnimPath::Weights,
            };
            let sampler = channel.sampler();
            let interpolation = match sampler.interpolation() {
                GInterp::Step => Interpolation::Step,
                GInterp::Linear => Interpolation::Linear,
                GInterp::CubicSpline => Interpolation::CubicSpline,
            };
            let inputs = match read_f32(&sampler.input(), buffers) {
                Ok((v, 1)) => v,
                Ok(_) => {
                    warnings.push(format!("animação `{label}`: input não é SCALAR"));
                    continue;
                }
                Err(e) => {
                    warnings.push(format!("animação `{label}`: {e}"));
                    continue;
                }
            };
            let outputs = match read_f32(&sampler.output(), buffers) {
                Ok((v, _)) => v,
                Err(e) => {
                    warnings.push(format!("animação `{label}`: {e}"));
                    continue;
                }
            };
            let keys = inputs.len();
            let valid_times = keys > 0
                && inputs.iter().all(|t| t.is_finite())
                && inputs.windows(2).all(|w| w[1] >= w[0]);
            if !valid_times || outputs.iter().any(|v| !v.is_finite()) {
                warnings.push(format!("animação `{label}`: chaves inválidas"));
                continue;
            }
            let per_key = if interpolation == Interpolation::CubicSpline { 3 } else { 1 };
            let components = match path {
                AnimPath::Translation | AnimPath::Scale => 3,
                AnimPath::Rotation => 4,
                AnimPath::Weights => {
                    let mesh_targets = node_data
                        .mesh
                        .and_then(|m| meshes.get(m as usize))
                        .map_or(0, MeshData::morph_target_count);
                    if mesh_targets == 0 {
                        continue; // pesos num nó sem morph targets: nada a animar
                    }
                    mesh_targets
                }
            };
            if outputs.len() != keys * components * per_key {
                warnings.push(format!("animação `{label}`: output com tamanho incoerente"));
                continue;
            }
            clip.duration = clip.duration.max(inputs[keys - 1]);
            clip.channels.push(AnimationChannel {
                node: u32::try_from(node).unwrap_or(u32::MAX),
                path,
                interpolation,
                inputs,
                outputs,
                components,
            });
        }
        if !clip.channels.is_empty() {
            clips.push(clip);
        }
    }
    clips
}

// ───────────────────────── Materiais e texturas ─────────────────────────

/// Texturas únicas deste modelo: `(textura glTF, sRGB) → índice em `ModelAsset::textures``.
#[derive(Default)]
struct TextureTable {
    index: HashMap<(usize, bool), Option<u32>>,
    list: Vec<Arc<TextureAsset>>,
}

#[allow(clippy::too_many_arguments)]
fn texture_slot(
    texture: &gltf::Texture,
    uv_set: u32,
    srgb: bool,
    document: &Document,
    buffers: &[Vec<u8>],
    base: Option<&Path>,
    vault: &AssetVault,
    cfg: &LoaderConfig,
    table: &mut TextureTable,
    warnings: &mut Vec<String>,
) -> Option<TextureSlot> {
    let key = (texture.index(), srgb);
    if let Some(cached) = table.index.get(&key) {
        return cached.map(|texture| TextureSlot { texture, uv_set });
    }
    // Prefere a imagem KTX2 (KHR_texture_basisu); cai para a imagem padrão se falhar.
    let mut candidates: Vec<usize> = Vec::with_capacity(2);
    if let Some(n) = texture
        .extension_value("KHR_texture_basisu")
        .and_then(|v| v.get("source"))
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
    {
        candidates.push(n);
    }
    let default_source = texture.source().index();
    if !candidates.contains(&default_source) {
        candidates.push(default_source);
    }

    let mut resolved: Option<u32> = None;
    for image_index in candidates {
        let Some(image) = document.images().nth(image_index) else {
            continue;
        };
        match load_image(&image, buffers, base, srgb, vault, cfg) {
            Ok(asset) => {
                resolved = u32::try_from(table.list.len()).ok();
                table.list.push(asset);
                break;
            }
            Err(e) => warnings.push(format!("textura #{}: {e}", texture.index())),
        }
    }
    table.index.insert(key, resolved);
    resolved.map(|texture| TextureSlot { texture, uv_set })
}

fn load_image(
    image: &gltf::Image,
    buffers: &[Vec<u8>],
    base: Option<&Path>,
    srgb: bool,
    vault: &AssetVault,
    cfg: &LoaderConfig,
) -> Result<Arc<TextureAsset>, AssetError> {
    let bytes: Cow<'_, [u8]> = match image.source() {
        gimage::Source::View { view, .. } => {
            let buf = buffers
                .get(view.buffer().index())
                .ok_or_else(|| AssetError::Format("imagem: buffer inexistente".into()))?;
            let end = view
                .offset()
                .checked_add(view.length())
                .filter(|&e| e <= buf.len())
                .ok_or_else(|| AssetError::Format("imagem fora do buffer".into()))?;
            Cow::Borrowed(&buf[view.offset()..end])
        }
        gimage::Source::Uri { uri, .. } => Cow::Owned(read_uri(uri, base)?),
    };

    // Chave: conteúdo + espaço de cor + limites (o resultado depende deles).
    let key = AssetHash::of_bytes(&bytes).combine(
        u64::from(srgb) | (u64::from(cfg.allow_bc7) << 1) | (u64::from(cfg.max_texture_dim) << 8),
    );
    if let Some(hit) = vault.textures.get(key) {
        return Ok(hit);
    }

    let asset = if is_ktx2(&bytes) {
        decode_ktx2(
            &bytes,
            Ktx2Options {
                allow_bc7: cfg.allow_bc7,
                max_dim: cfg.max_texture_dim,
            },
        )?
    } else {
        decode_raster(&bytes, srgb, cfg.max_texture_dim)?
    };
    let size = asset.byte_size();
    Ok(vault.textures.insert(key, asset, size))
}

fn decode_raster(bytes: &[u8], srgb: bool, max_dim: u32) -> Result<TextureAsset, AssetError> {
    let mut rgba = image::load_from_memory(bytes)
        .map_err(|e| AssetError::Decode(format!("imagem: {e}")))?
        .into_rgba8();
    let (w, h) = rgba.dimensions();
    if max_dim > 0 && w.max(h) > max_dim {
        let k = max_dim as f32 / w.max(h) as f32;
        let nw = ((w as f32 * k) as u32).max(1);
        let nh = ((h as f32 * k) as u32).max(1);
        rgba = image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Triangle);
    }
    let (w, h) = rgba.dimensions();
    let data = rgba.into_raw();
    Ok(TextureAsset {
        width: w,
        height: h,
        format: TexelFormat::Rgba8,
        srgb,
        levels: vec![TextureLevel {
            offset: 0,
            len: data.len(),
            width: w,
            height: h,
        }],
        generate_mips: true,
        data,
    })
}

#[allow(clippy::too_many_arguments)]
fn convert_material(
    mat: &gltf::Material,
    document: &Document,
    buffers: &[Vec<u8>],
    base: Option<&Path>,
    vault: &AssetVault,
    cfg: &LoaderConfig,
    table: &mut TextureTable,
    warnings: &mut Vec<String>,
) -> MaterialData {
    let pbr = mat.pbr_metallic_roughness();
    let mut slot = |texture: gltf::Texture<'_>, uv: u32, srgb: bool| {
        texture_slot(&texture, uv, srgb, document, buffers, base, vault, cfg, table, warnings)
    };

    let base_color_tex = pbr
        .base_color_texture()
        .and_then(|i| slot(i.texture(), i.tex_coord(), true));
    let metallic_roughness_tex = pbr
        .metallic_roughness_texture()
        .and_then(|i| slot(i.texture(), i.tex_coord(), false));
    let (normal_tex, normal_scale) = match mat.normal_texture() {
        Some(n) => (slot(n.texture(), n.tex_coord(), false), n.scale()),
        None => (None, 1.0),
    };
    let (occlusion_tex, occlusion_strength) = match mat.occlusion_texture() {
        Some(o) => (slot(o.texture(), o.tex_coord(), false), o.strength()),
        None => (None, 1.0),
    };
    let emissive_tex = mat
        .emissive_texture()
        .and_then(|i| slot(i.texture(), i.tex_coord(), true));

    let strength = mat
        .extension_value("KHR_materials_emissive_strength")
        .and_then(|v| v.get("emissiveStrength"))
        .and_then(Value::as_f64)
        .map_or(1.0, |v| v as f32);
    let emissive = mat.emissive_factor().map(|c| c * strength);

    MaterialData {
        name: mat.name().map(str::to_owned),
        base_color: pbr.base_color_factor(),
        base_color_tex,
        metallic: pbr.metallic_factor(),
        roughness: pbr.roughness_factor(),
        metallic_roughness_tex,
        normal_tex,
        normal_scale,
        occlusion_tex,
        occlusion_strength,
        emissive,
        emissive_tex,
        alpha_mode: match mat.alpha_mode() {
            GltfAlpha::Opaque => AlphaMode::Opaque,
            GltfAlpha::Mask => AlphaMode::Mask,
            GltfAlpha::Blend => AlphaMode::Blend,
        },
        alpha_cutoff: mat.alpha_cutoff().unwrap_or(0.5),
        double_sided: mat.double_sided(),
    }
}
