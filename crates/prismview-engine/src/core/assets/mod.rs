//! Assets 3D (Fase 3): cache por hash com refcount `Arc` (`AssetVault`) e loader assíncrono de
//! `.glb`/`.gltf` (meshopt, KTX2/UASTC, hierarquia de nós). Dados de CPU apenas; o upload para a
//! GPU é responsabilidade do renderer 3D. Nenhum código de SO aqui.
//!
//! ```ignore
//! let vault = AssetVault::new();
//! let mut loader = AssetLoader::new(vault, LoaderConfig::default(), move || { proxy.wake(); });
//! let ticket = loader.request("C:/.../modelo.glb");
//! // no loop de eventos, ao ser acordado:
//! loader.poll(&mut events);   // Vec<LoadEvent> reaproveitado
//! ```

pub mod gltf_loader;
pub mod ktx2;
pub mod loader;
pub mod meshopt;
pub mod model;
pub mod vault;

#[allow(unused_imports)]
pub use gltf_loader::{parse_model, LoaderConfig};
#[allow(unused_imports)]
pub use loader::{load_model_file, AssetLoader, LoadEvent, Ticket};
#[allow(unused_imports)]
pub use model::{
    Aabb, AlphaMode, AnimPath, AnimationChannel, AnimationClip, AssetError, Interpolation,
    MaterialData, MeshData, ModelAsset, MorphTargetData, NodeData, PrimitiveData, SkinData,
    TexelFormat, TextureAsset, TextureLevel, TextureSlot,
};
#[allow(unused_imports)]
pub use vault::{AssetHash, AssetVault, PathKey, Vault, VaultStats};
