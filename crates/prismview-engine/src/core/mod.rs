//! Núcleo independente de SO do motor (gpu, compositor 2D, cena, texto; depois glass, scene3d,
//! signals, governor).
//! Regra: nada aqui importa `windows`/Win32; só fala com a plataforma via handles brutos
//! (`raw-window-handle`) e traits de `platform/mod.rs`.

pub mod assets;
pub mod audio;
pub mod bindings;
pub mod compositor;
pub mod glass;
pub mod governor;
pub mod gpu;
pub mod interaction;
pub mod scene2d;
pub mod scene3d;
pub mod signals;
pub mod stem_text;
pub mod text;
