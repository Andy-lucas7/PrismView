//! Build do Studio: além do `tauri_build`, compila o motor nativo (`prismview-engine`) e copia o
//! `.exe` para o diretório do binário do Studio (`target/<perfil>/`), onde `native_engine.rs`
//! procura `prismview-engine.exe` (ao lado do `current_exe`).
//!
//! O motor é compilado com `--target-dir target/engine` (dedicado): o cargo externo já segura o
//! lock de `target/`, então reutilizá-lo aqui travaria (deadlock do lock de build).
//! `PRISMVIEW_SKIP_ENGINE_BUILD=1` pula o passo (CI/debug do Studio isolado).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    build_native_engine();
    tauri_build::build();
}

fn build_native_engine() {
    println!("cargo:rerun-if-env-changed=PRISMVIEW_SKIP_ENGINE_BUILD");
    if env::var_os("PRISMVIEW_SKIP_ENGINE_BUILD").is_some_and(|v| v != "0") {
        println!("cargo:warning=PRISMVIEW_SKIP_ENGINE_BUILD: motor nativo não recompilado");
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    // apps/prismview-studio/src-tauri → raiz do workspace.
    let Some(workspace) = manifest_dir.ancestors().nth(3).map(Path::to_path_buf) else {
        println!("cargo:warning=raiz do workspace não encontrada; motor não compilado");
        return;
    };
    for dir in ["crates/prismview-engine/src", "crates/prism-core/src"] {
        println!("cargo:rerun-if-changed={}", workspace.join(dir).display());
    }
    for file in ["crates/prismview-engine/Cargo.toml", "crates/prism-core/Cargo.toml", "Cargo.lock"] {
        println!("cargo:rerun-if-changed={}", workspace.join(file).display());
    }

    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let release = profile == "release";
    let target = env::var("TARGET").unwrap_or_default();
    let host = env::var("HOST").unwrap_or_default();
    let cross = !target.is_empty() && target != host;
    let engine_target_dir = workspace.join("target").join("engine");

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(&workspace)
        .args(["build", "-p", "prismview-engine", "--bin", "prismview-engine", "--target-dir"])
        .arg(&engine_target_dir);
    if release {
        cmd.arg("--release");
    }
    if cross {
        cmd.args(["--target", &target]);
    }
    // Variáveis do cargo externo que não podem vazar para o build aninhado.
    for var in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_MAKEFLAGS",
        "CARGO_PRIMARY_PACKAGE",
        "RUSTC_WORKSPACE_WRAPPER",
        "MAKEFLAGS",
        "MFLAGS",
    ] {
        cmd.env_remove(var);
    }

    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(s) => {
            // Erro de compilação do motor deve quebrar o build: o Studio sem motor não aplica wallpaper.
            panic!("cargo build -p prismview-engine falhou ({s}); veja a saída acima");
        }
        Err(e) => panic!("não foi possível invocar o cargo para o motor nativo: {e}"),
    }

    let mut built = engine_target_dir.clone();
    if cross {
        built.push(&target);
    }
    built.push(if release { "release" } else { "debug" });
    built.push(exe_name());

    // OUT_DIR = target/<perfil>/build/<pkg>-<hash>/out → target/<perfil>/
    let Some(dest_dir) = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .and_then(|p| p.ancestors().nth(3).map(Path::to_path_buf))
    else {
        println!("cargo:warning=OUT_DIR ausente; motor não copiado");
        return;
    };
    let dest = dest_dir.join(exe_name());
    if let Err(e) = copy_if_changed(&built, &dest) {
        // Motor em execução segura o .exe (Windows): avisa em vez de falhar o build.
        println!(
            "cargo:warning=não foi possível copiar {} → {}: {e} (o motor está rodando? feche-o pela bandeja)",
            built.display(),
            dest.display()
        );
    }
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "prismview-engine.exe"
    } else {
        "prismview-engine"
    }
}

fn copy_if_changed(src: &Path, dest: &Path) -> std::io::Result<()> {
    let src_meta = std::fs::metadata(src)?;
    if let Ok(dst_meta) = std::fs::metadata(dest) {
        let same_len = dst_meta.len() == src_meta.len();
        let same_time = match (src_meta.modified(), dst_meta.modified()) {
            (Ok(a), Ok(b)) => b >= a,
            _ => false,
        };
        if same_len && same_time {
            return Ok(());
        }
    }
    std::fs::copy(src, dest).map(|_| ())
}
