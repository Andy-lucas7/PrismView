//! Modo Espectro: exportar um efeito avulso (`.prismfx`, JSON) e gravar no disco as texturas
//! embutidas num `.prismfx` importado. O import do JSON em si é feito no front.
use super::*;

/// Tamanho máximo aceito para um `.prismfx` (código WGSL + parâmetros + texturas em base64).
const PRISMFX_MAX_BYTES: usize = 24 * 1024 * 1024;

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
        return Err("Efeito grande demais para exportar (limite de 24 MB).".into());
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

/// Limite de uma textura importada de `.prismfx`.
const TEXTURE_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Decodifica base64 padrão (com ou sem `=`). `None` = caractere inválido.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= val(c)? << (18 - 6 * i as u32);
        }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
        if chunk.len() == 1 {
            return None;
        }
    }
    Some(out)
}

/// Extensão pela assinatura do arquivo (só PNG/JPEG/WebP).
fn image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Grava uma textura embutida num `.prismfx` importado em `%APPDATA%\PrismView\assets\espectro\`
/// (dentro do escopo do asset protocol) e devolve o caminho.
#[tauri::command]
pub(crate) fn save_espectro_texture(file_name: String, data_url: String) -> Result<String, String> {
    let b64 = data_url
        .split_once(";base64,")
        .filter(|(head, _)| head.starts_with("data:image/"))
        .map(|(_, b)| b)
        .ok_or_else(|| "Textura inválida no efeito.".to_string())?;
    if b64.len() / 4 * 3 > TEXTURE_MAX_BYTES {
        return Err("Textura grande demais (limite de 16 MB).".into());
    }
    let bytes = decode_base64(b64).ok_or_else(|| "Textura corrompida no efeito.".to_string())?;
    let ext = image_ext(&bytes).ok_or_else(|| "Textura não é PNG, JPEG ou WebP.".to_string())?;
    let stem: String = sanitize_file_name(&file_name)
        .rsplit_once('.')
        .map_or_else(|| sanitize_file_name(&file_name), |(s, _)| s.to_string())
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .take(40)
        .collect();
    let dir = prismview_root_dir().join("assets").join("espectro");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Não foi possível criar a pasta de texturas: {e}"))?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let path = dir.join(format!("{}-{}.{ext}", &id[..8], if stem.is_empty() { "textura" } else { &stem }));
    atomic_fs::write_atomic(&path, &bytes, false)?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::{decode_base64, image_ext, sanitize_file_name};

    #[test]
    fn base64_and_image_signature() {
        assert_eq!(decode_base64("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
        assert_eq!(decode_base64("aGVsbG8").as_deref(), Some(&b"hello"[..]));
        assert!(decode_base64("a$==").is_none());
        let png = decode_base64("iVBORw0KGgo=").expect("png");
        assert_eq!(image_ext(&png), Some("png"));
        assert_eq!(image_ext(b"GIF89a"), None);
    }

    #[test]
    fn file_name_is_sanitized() {
        assert_eq!(sanitize_file_name("  a/b:c  "), "a_b_c");
        assert_eq!(sanitize_file_name("   "), "efeito");
    }
}
