//! Modo Espectro: exportar um efeito avulso (`.prismfx`, JSON) escolhido pelo usuário.
//! O import é feito no front (input de arquivo), então só a gravação passa por aqui.
use super::*;

/// Tamanho máximo aceito para um `.prismfx` (código WGSL + parâmetros; texturas não vão aqui).
const PRISMFX_MAX_BYTES: usize = 2 * 1024 * 1024;

fn sanitize_file_name(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    if clean.is_empty() { "efeito".to_string() } else { clean }
}

/// Abre o diálogo "Salvar como" filtrado em `.prismfx` e grava `contents` (atômico).
/// `Ok(None)` = usuário cancelou.
#[tauri::command]
pub(crate) async fn export_prismfx(default_file_name: String, contents: String) -> Result<Option<String>, String> {
    if contents.len() > PRISMFX_MAX_BYTES {
        return Err("Efeito grande demais para exportar (limite de 2 MB).".into());
    }
    serde_json::from_str::<serde_json::Value>(&contents).map_err(|e| format!("Efeito inválido: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let _com = desktop_host::ComApartmentGuard::init_sta();
        let filter_w: Vec<u16> = "Efeito Espectro PrismView (*.prismfx)\0*.prismfx\0Todos os arquivos (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();
        let def_ext_w: Vec<u16> = "prismfx\0".encode_utf16().collect();
        let mut file_buf = vec![0u16; 4096];
        let mut default_name = sanitize_file_name(&default_file_name);
        if !default_name.to_ascii_lowercase().ends_with(".prismfx") {
            default_name.push_str(".prismfx");
        }
        let default_w: Vec<u16> = default_name.encode_utf16().collect();
        let copy_len = default_w.len().min(file_buf.len().saturating_sub(1));
        file_buf[..copy_len].copy_from_slice(&default_w[..copy_len]);

        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: HWND::default(),
            lpstrFilter: PCWSTR(filter_w.as_ptr()),
            lpstrFile: PWSTR(file_buf.as_mut_ptr()),
            nMaxFile: file_buf.len() as u32,
            lpstrDefExt: PCWSTR(def_ext_w.as_ptr()),
            Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        if !unsafe { GetSaveFileNameW(&mut ofn).as_bool() } {
            return Ok(None);
        }
        let end = file_buf.iter().position(|&c| c == 0).unwrap_or(file_buf.len());
        let mut path_str = String::from_utf16_lossy(&file_buf[..end]).trim().to_string();
        if path_str.is_empty() {
            return Ok(None);
        }
        if !path_str.to_ascii_lowercase().ends_with(".prismfx") {
            path_str.push_str(".prismfx");
        }
        atomic_fs::write_atomic(Path::new(&path_str), contents.as_bytes(), false)?;
        Ok(Some(path_str))
    })
    .await
    .map_err(|e| format!("Falha ao exportar efeito: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::sanitize_file_name;

    #[test]
    fn file_name_is_sanitized() {
        assert_eq!(sanitize_file_name("  a/b:c  "), "a_b_c");
        assert_eq!(sanitize_file_name("   "), "efeito");
    }
}
