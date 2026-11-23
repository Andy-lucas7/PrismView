#![windows_subsystem = "windows"]

// Motor nativo (wgpu + `platform/` + `core/`): `prismview-engine.exe --native`.
// O antigo host legado (WorkerW próprio + ab_glyph + decoder de vídeo em CPU) foi removido.
// `core/` vive em disco com esse nome (PLANO_NATIVO), mas o módulo se chama `engine_core`:
// um `mod core` na raiz sombrearia a crate `core` em todos os `use super::*` dos submódulos.
#[path = "core/mod.rs"]
mod engine_core;
mod ipc_server;
mod native;
#[allow(dead_code)]
mod platform;
mod widgets;

fn main() {
    std::process::exit(native::run());
}
