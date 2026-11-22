//! Dados de CPU dos assets 3D carregados (`ModelAsset`, `TextureAsset`). São imutáveis e
//! compartilhados via `Arc` pelo `AssetVault`; o upload para a GPU (buffers/texturas) é feito pelo
//! renderer 3D, que decide o formato final.

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};

use super::vault::AssetHash;

// ───────────────────────── Erros ─────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetError {
    Io(String),
    /// glTF/JSON/KTX2 malformado ou incoerente.
    Format(String),
    /// Recurso válido, porém não suportado (ex.: Draco, ETC1S/BasisLZ).
    Unsupported(String),
    /// Falha de decodificação (meshopt, transcoder, imagem).
    Decode(String),
    /// O parser entrou em pânico (arquivo corrompido); o processo seguiu vivo.
    Panic(String),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(m) => write!(f, "E/S: {m}"),
            Self::Format(m) => write!(f, "formato: {m}"),
            Self::Unsupported(m) => write!(f, "não suportado: {m}"),
            Self::Decode(m) => write!(f, "decodificação: {m}"),
            Self::Panic(m) => write!(f, "parser abortou: {m}"),
        }
    }
}

impl std::error::Error for AssetError {}

// ───────────────────────── Texturas ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexelFormat {
    /// RGBA 8 bits, alpha reto (não pré-multiplicado).
    Rgba8,
    /// BC7 (blocos 4×4 de 16 B), transcodificado de UASTC ou lido do KTX2.
    Bc7,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureLevel {
    pub offset: usize,
    pub len: usize,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub struct TextureAsset {
    pub width: u32,
    pub height: u32,
    pub format: TexelFormat,
    /// Espaço de cor decidido pelo uso (baseColor/emissive = sRGB) ou pelo KTX2.
    pub srgb: bool,
    /// Níveis de mip (nível 0 primeiro) dentro de `data`. Um único nível + `generate_mips` =
    /// o renderer gera a cadeia na GPU.
    pub levels: Vec<TextureLevel>,
    pub generate_mips: bool,
    pub data: Vec<u8>,
}

impl TextureAsset {
    pub fn wgpu_format(&self) -> wgpu::TextureFormat {
        match (self.format, self.srgb) {
            (TexelFormat::Rgba8, false) => wgpu::TextureFormat::Rgba8Unorm,
            (TexelFormat::Rgba8, true) => wgpu::TextureFormat::Rgba8UnormSrgb,
            (TexelFormat::Bc7, false) => wgpu::TextureFormat::Bc7RgbaUnorm,
            (TexelFormat::Bc7, true) => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
        }
    }

    pub fn byte_size(&self) -> usize {
        self.data.len()
    }

    pub fn level_bytes(&self, level: &TextureLevel) -> Option<&[u8]> {
        self.data.get(level.offset..level.offset.checked_add(level.len)?)
    }
}

// ───────────────────────── Geometria ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }

    pub fn extend(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn union(&mut self, other: &Self) {
        if !other.is_empty() {
            self.extend(other.min);
            self.extend(other.max);
        }
    }

    /// AABB dos 8 cantos transformados por `m`.
    pub fn transformed(&self, m: &Mat4) -> Self {
        if self.is_empty() {
            return *self;
        }
        let mut out = Self::EMPTY;
        for i in 0..8u32 {
            let corner = Vec3::new(
                if i & 1 == 0 { self.min.x } else { self.max.x },
                if i & 2 == 0 { self.min.y } else { self.max.y },
                if i & 4 == 0 { self.min.z } else { self.max.z },
            );
            out.extend(m.transform_point3(corner));
        }
        out
    }
}

/// Primitiva triangulada. Atributos ausentes ficam com vetores vazios (menos `positions` e
/// `indices`, sempre presentes; normais ausentes são geradas no carregamento).
#[derive(Debug, Default)]
pub struct PrimitiveData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 4]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Índice em `ModelAsset::materials`; `None` = material padrão do glTF.
    pub material: Option<u32>,
    pub bounds: Option<Aabb>,
    /// Morph targets (deltas). Todos têm `positions.len() == vertex_count`; `normals` vazio =
    /// o alvo não altera normais.
    pub morph_targets: Vec<MorphTargetData>,
}

impl PrimitiveData {
    pub fn byte_size(&self) -> usize {
        self.positions.len() * 12
            + self.normals.len() * 12
            + self.tangents.len() * 16
            + (self.uv0.len() + self.uv1.len()) * 8
            + self.colors.len() * 16
            + self.joints.len() * 8
            + self.weights.len() * 16
            + self.indices.len() * 4
            + self
                .morph_targets
                .iter()
                .map(|t| (t.positions.len() + t.normals.len()) * 12)
                .sum::<usize>()
    }
}

/// Deltas de um morph target (glTF `targets[i]`). Tangentes não são lidas: o shader de deformação
/// re-ortogonaliza a tangente contra a normal deformada.
#[derive(Debug, Default)]
pub struct MorphTargetData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}

#[derive(Debug, Default)]
pub struct MeshData {
    pub name: Option<String>,
    pub primitives: Vec<PrimitiveData>,
    /// Pesos padrão dos morph targets (`mesh.weights`); pode ser menor que o nº de alvos (resto 0).
    pub weights: Vec<f32>,
    /// Nomes dos morph targets (`mesh.extras.targetNames`); vazio se o arquivo não os define.
    pub target_names: Vec<String>,
}

impl MeshData {
    /// Maior número de morph targets entre as primitivas (os pesos são por malha no glTF).
    pub fn morph_target_count(&self) -> usize {
        self.primitives
            .iter()
            .map(|p| p.morph_targets.len())
            .max()
            .unwrap_or(0)
    }
}

// ───────────────────────── Animação ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
    /// Hermite: cada chave tem `[in_tangent, valor, out_tangent]` em `outputs`.
    CubicSpline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimPath {
    Translation,
    /// Quaternion `[x, y, z, w]`.
    Rotation,
    Scale,
    /// Pesos de morph: `components` = nº de alvos da malha do nó.
    Weights,
}

#[derive(Clone, Debug)]
pub struct AnimationChannel {
    pub node: u32,
    pub path: AnimPath,
    pub interpolation: Interpolation,
    /// Tempos das chaves (segundos, crescentes, já validados).
    pub inputs: Vec<f32>,
    /// Valores achatados: `inputs.len() × components` (× 3 em `CubicSpline`).
    pub outputs: Vec<f32>,
    /// Componentes por valor: 3 (T/S), 4 (R) ou nº de morph targets (W).
    pub components: usize,
}

#[derive(Clone, Debug)]
pub struct AnimationClip {
    pub name: Option<String>,
    pub channels: Vec<AnimationChannel>,
    /// Maior tempo de chave entre os canais (s).
    pub duration: f32,
}

// ───────────────────────── Materiais ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureSlot {
    /// Índice em `ModelAsset::textures`.
    pub texture: u32,
    pub uv_set: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AlphaMode {
    #[default]
    Opaque,
    Mask,
    Blend,
}

#[derive(Clone, Debug)]
pub struct MaterialData {
    pub name: Option<String>,
    pub base_color: [f32; 4],
    pub base_color_tex: Option<TextureSlot>,
    pub metallic: f32,
    pub roughness: f32,
    /// glTF: G = rugosidade, B = metalicidade.
    pub metallic_roughness_tex: Option<TextureSlot>,
    pub normal_tex: Option<TextureSlot>,
    pub normal_scale: f32,
    pub occlusion_tex: Option<TextureSlot>,
    pub occlusion_strength: f32,
    pub emissive: [f32; 3],
    pub emissive_tex: Option<TextureSlot>,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
}

impl Default for MaterialData {
    fn default() -> Self {
        Self {
            name: None,
            base_color: [1.0; 4],
            base_color_tex: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_tex: None,
            normal_tex: None,
            normal_scale: 1.0,
            occlusion_tex: None,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            emissive_tex: None,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
        }
    }
}

// ───────────────────────── Hierarquia ─────────────────────────

#[derive(Clone, Debug)]
pub struct NodeData {
    pub name: Option<String>,
    /// Índice do pai em `ModelAsset::nodes` (`parentId` do plano). `None` = raiz.
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    pub mesh: Option<u32>,
    pub skin: Option<u32>,
    /// `node.weights` (sobrepõe `mesh.weights`); vazio = usa os da malha.
    pub weights: Vec<f32>,
}

impl NodeData {
    pub fn local_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SkinData {
    pub name: Option<String>,
    /// Índices de nós que formam o esqueleto, na ordem de `JOINTS_0`.
    pub joints: Vec<u32>,
    pub inverse_bind: Vec<Mat4>,
    pub skeleton_root: Option<u32>,
}

#[derive(Debug)]
pub struct ModelAsset {
    pub hash: AssetHash,
    pub name: Option<String>,
    pub nodes: Vec<NodeData>,
    /// Raízes da cena padrão.
    pub roots: Vec<u32>,
    pub meshes: Vec<MeshData>,
    pub materials: Vec<MaterialData>,
    pub skins: Vec<SkinData>,
    pub animations: Vec<AnimationClip>,
    /// Texturas referenciadas pelos materiais (compartilhadas com o vault por `Arc`).
    pub textures: Vec<Arc<TextureAsset>>,
    /// AABB do modelo inteiro no espaço do modelo (nós aplicados).
    pub bounds: Aabb,
    /// Avisos não fatais (primitiva Draco ignorada, textura ETC1S, etc.).
    pub warnings: Vec<String>,
}

impl ModelAsset {
    /// Matrizes de mundo (espaço do modelo) de todos os nós, pai antes do filho. Percurso
    /// iterativo com guarda contra ciclos de arquivos corrompidos; nós inalcançáveis ficam com
    /// a matriz local.
    pub fn world_matrices(&self) -> Vec<Mat4> {
        let mut world: Vec<Mat4> = self.nodes.iter().map(NodeData::local_matrix).collect();
        let mut visited = vec![false; self.nodes.len()];
        let mut stack: Vec<u32> = Vec::with_capacity(self.nodes.len());
        for &root in self.roots.iter().rev() {
            stack.push(root);
        }
        while let Some(index) = stack.pop() {
            let i = index as usize;
            if i >= self.nodes.len() || visited[i] {
                continue;
            }
            visited[i] = true;
            let parent_world = self.nodes[i]
                .parent
                .and_then(|p| world.get(p as usize).copied())
                .unwrap_or(Mat4::IDENTITY);
            world[i] = parent_world * self.nodes[i].local_matrix();
            for &child in self.nodes[i].children.iter().rev() {
                stack.push(child);
            }
        }
        world
    }

    /// Estimativa de memória de CPU (sem as texturas, contadas no vault de texturas).
    pub fn byte_size(&self) -> usize {
        let meshes: usize = self
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .map(PrimitiveData::byte_size)
            .sum();
        let anims: usize = self
            .animations
            .iter()
            .flat_map(|a| a.channels.iter())
            .map(|c| (c.inputs.len() + c.outputs.len()) * 4)
            .sum();
        meshes + anims + self.nodes.len() * 96 + self.materials.len() * 160
    }
}
