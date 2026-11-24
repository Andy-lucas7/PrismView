fn main() {
    #[cfg(target_os = "windows")]
    {
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/prismview-studio/src-tauri/icons/icon.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let mut res = tauri_winres::WindowsResource::new();
        if icon.is_file() {
            res.set_icon(&icon.to_string_lossy());
        }
        res.set("FileDescription", "PrismView Engine");
        res.set("ProductName", "PrismView Engine");
        res.set("InternalName", "PrismView Engine");
        res.set("OriginalFilename", "prismview-engine.exe");
        if let Err(err) = res.compile() {
            panic!("falha ao compilar recursos Windows do prismview-engine: {err}");
        }
    }
}
