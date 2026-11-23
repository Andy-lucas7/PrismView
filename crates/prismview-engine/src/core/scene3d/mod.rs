//! Cena 3D nativa (Fase 3): glTF + PBR metálico/rugoso + IBL, sem Three.js.
//!
//! - `config` / `graph`: contrato JSON (`Scene3DConfig` v2) e hierarquia por `parentId`;
//! - `animation`: clipes glTF (T/R/S/pesos) amostrados na CPU, pose drift-free;
//! - `gpu_model`: upload de `ModelAsset` (buffers, texturas + mips na GPU, materiais);
//! - `instance`: pose por objeto + skinning/morph em compute (storage buffers);
//! - `environment`: PMREM (especular GGX + irradiância) e LUT do BRDF em compute WGSL;
//! - `lighting`: presets/dia-noite do Estúdio (ambiente, key direcional, rim pontual);
//! - `culling`: frustum × AABB;
//! - `renderer`: `Scene3dRenderer` (passe HDR + tone mapping ACES → textura do compositor).
//!
//! ```ignore
//! let shared = Scene3dShared::new(&gpu);                  // 1 por device
//! let mut loader = AssetLoader::new(vault, shared.loader_config(), wake);
//! let mut scene = Scene3dRenderer::new(shared, &gpu, Scene3dOptions::default());
//! scene.set_viewport(w, h);
//! scene.set_config(&gpu, config, &mut loader);
//! // loop: eventos do loader → scene.handle_load_event(&gpu, &ev);
//! if scene.update(&gpu, dt) { scene.render(&gpu, &mut encoder); }  // depois: output_view()
//! ```

pub mod animation;
pub mod config;
pub mod culling;
pub mod environment;
pub mod gpu_model;
pub mod graph;
pub mod instance;
pub mod lighting;
pub mod renderer;
pub mod sculptures;

#[allow(unused_imports)]
pub use config::{
    Scene3dBackgroundMode, Scene3dCamera, Scene3dConfig, Scene3dEnvPreset, Scene3dObject,
    Scene3dObjectKind, Scene3dPlayback,
};
#[allow(unused_imports)]
pub use graph::SceneGraph;
#[allow(unused_imports)]
pub use renderer::{Scene3dOptions, Scene3dRenderer, Scene3dShared, Scene3dStats};
