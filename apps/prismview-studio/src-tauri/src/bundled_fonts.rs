//! Fontes padrão do Estúdio (Outfit, Inter, JetBrains Mono; OFL, subset latin) embutidas no
//! executável e instaladas em `%APPDATA%\PrismView\fonts\`, pasta que o motor nativo registra.
//! Assim o desktop desenha as mesmas fontes do preview mesmo offline e sem elas no sistema.
//! A UI usa os mesmos desenhos em WOFF2 (`public/fonts`, `@font-face` em `styles/prism.css`).

use std::fs;
use std::path::Path;

macro_rules! font_files {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../fonts/", $name)))),*]
    };
}

const FILES: &[(&str, &[u8])] = font_files![
    "Outfit-300.ttf",
    "Outfit-400.ttf",
    "Outfit-500.ttf",
    "Outfit-600.ttf",
    "Outfit-700.ttf",
    "Outfit-800.ttf",
    "Inter-300.ttf",
    "Inter-400.ttf",
    "Inter-500.ttf",
    "Inter-600.ttf",
    "Inter-700.ttf",
    "JetBrainsMono-300.ttf",
    "JetBrainsMono-400.ttf",
    "JetBrainsMono-500.ttf",
    "JetBrainsMono-600.ttf",
    "OFL-outfit.txt",
    "OFL-inter.txt",
    "OFL-jetbrainsmono.txt",
];

/// Grava os arquivos ausentes ou com tamanho diferente (atualização). Falhas só vão para o log.
pub fn install(dir: &Path) {
    if let Err(e) = fs::create_dir_all(dir) {
        eprintln!("[prismview-studio] fontes: {}: {e}", dir.display());
        return;
    }
    for (name, bytes) in FILES {
        let path = dir.join(name);
        if fs::metadata(&path).map(|m| m.len()).ok() == Some(bytes.len() as u64) {
            continue;
        }
        if let Err(e) = crate::atomic_fs::write_atomic(&path, bytes, false) {
            eprintln!("[prismview-studio] fonte {name}: {e}");
        }
    }
}
