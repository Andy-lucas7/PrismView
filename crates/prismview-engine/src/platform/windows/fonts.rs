//! Resolução pontual de fontes do Windows pelo Registro (sem varrer `C:\Windows\Fonts`).
//!
//! Consulta `HKCU\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts` e
//! `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts` e devolve **apenas** os arquivos
//! `.ttf/.otf/.ttc/.otc` correspondentes à família pedida (com todos os pesos verticais da
//! família), evitando mapear centenas de fontes (~350 MB) na RAM.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(
        hkey: isize,
        lpsubkey: *const u16,
        uloptions: u32,
        samdesired: u32,
        phkresult: *mut isize,
    ) -> i32;
    fn RegEnumValueW(
        hkey: isize,
        dwindex: u32,
        lpvaluename: *mut u16,
        lpcchvaluename: *mut u32,
        lpreserved: *mut u32,
        lptype: *mut u32,
        lpdata: *mut u8,
        lpcbdata: *mut u32,
    ) -> i32;
    fn RegCloseKey(hkey: isize) -> i32;
}

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
const KEY_READ: u32 = 0x0002_0019;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const ERROR_SUCCESS: i32 = 0;
const ERROR_NO_MORE_ITEMS: i32 = 259;

/// Sufixos de peso/estilo removidos ao agrupar faces de uma mesma família (ex.: `"Segoe UI Bold"`
/// → `"Segoe UI"`). Não remove nomes de família distintos como `"Emoji"`, `"Symbol"`, `"Variable"`
/// ou `"Historic"`, evitando carregar `seguiemj.ttf` (13 MB) ao pedir `"Segoe UI"`.
const STYLE_SUFFIXES: [&str; 28] = [
    " Bold Italic",
    " SemiBold Italic",
    " Semibold Italic",
    " ExtraBold Italic",
    " ExtraLight Italic",
    " UltraLight Italic",
    " UltraBold Italic",
    " Light Italic",
    " Semilight Italic",
    " SemiLight Italic",
    " Medium Italic",
    " Black Italic",
    " Heavy Italic",
    " Thin Italic",
    " Regular",
    " Bold",
    " Italic",
    " Oblique",
    " Semibold",
    " SemiBold",
    " Semilight",
    " SemiLight",
    " ExtraLight",
    " ExtraBold",
    " UltraLight",
    " UltraBold",
    " Light",
    " Medium",
];

const EXTRA_WEIGHT_SUFFIXES: [&str; 10] = [
    " Black",
    " Heavy",
    " Thin",
    " Hairline",
    " Book",
    " Demibold",
    " DemiBold",
    " Condensed",
    " Narrow",
    " Normal",
];

const FORMAT_SUFFIXES: [&str; 5] = [
    " (TrueType)",
    " (OpenType)",
    " (All res)",
    " (VGA res)",
    " (Type 1)",
];

fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn normalize(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn strip_format_suffix(raw: &str) -> &str {
    let mut s = raw.trim();
    for suf in FORMAT_SUFFIXES {
        if let Some(stripped) = s.strip_suffix(suf) {
            s = stripped.trim();
        }
    }
    s
}

fn strip_style_suffixes(part: &str) -> String {
    let mut name = part.trim().replace(['-', '_'], " ");
    let mut changed = true;
    while changed {
        changed = false;
        let lower = name.to_ascii_lowercase();
        for suf in STYLE_SUFFIXES.iter().chain(EXTRA_WEIGHT_SUFFIXES.iter()) {
            let suf_lower = suf.to_ascii_lowercase();
            if lower.ends_with(&suf_lower) && name.len() > suf.len() {
                let cut = name.len() - suf.len();
                if name.is_char_boundary(cut) {
                    let trimmed = name[..cut].trim();
                    if !trimmed.is_empty() {
                        name = trimmed.to_string();
                        changed = true;
                        break;
                    }
                }
            }
        }
    }
    name
}

fn is_italic_entry(part: &str) -> bool {
    let lower = part.to_ascii_lowercase();
    lower.ends_with(" italic")
        || lower.ends_with(" oblique")
        || lower.ends_with(" italic (truetype)")
        || lower.contains(" italic ")
}

fn is_supported_font_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("ttf" | "otf" | "ttc" | "otc")
    )
}

fn system_fonts_dir() -> PathBuf {
    std::env::var_os("SYSTEMROOT")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(|root| PathBuf::from(root).join("Fonts"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows\Fonts"))
}

fn user_fonts_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|local| PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"))
}

fn app_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::with_capacity(2);
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let dir = PathBuf::from(appdata).join("PrismView").join("fonts");
        if dir.is_dir() {
            dirs.push(dir);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let dir = parent.join("fonts");
            if dir.is_dir() && !dirs.iter().any(|d| d == &dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

fn resolve_registry_path(raw_value: &str, is_hkcu: bool, sys_dir: &Path, user_dir: Option<&Path>) -> Option<PathBuf> {
    let trimmed = raw_value.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return (is_supported_font_file(candidate) && candidate.is_file()).then(|| candidate.to_path_buf());
    }
    if is_hkcu {
        if let Some(dir) = user_dir {
            let p = dir.join(trimmed);
            if is_supported_font_file(&p) && p.is_file() {
                return Some(p);
            }
        }
    }
    let sys = sys_dir.join(trimmed);
    if is_supported_font_file(&sys) && sys.is_file() {
        return Some(sys);
    }
    if !is_hkcu {
        if let Some(dir) = user_dir {
            let p = dir.join(trimmed);
            if is_supported_font_file(&p) && p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Verifica se uma entrada do Registro (ex.: `"Segoe UI Bold (TrueType)"`) corresponde a `needle`.
/// Retorna `Some(is_italic)` quando casa por nome exato, nome-base de família ou stem exato do arquivo.
fn matches_registry_entry(raw_name: &str, raw_value: &str, needle: &str) -> Option<bool> {
    let stripped = strip_format_suffix(raw_name);
    for part in stripped.split(" & ") {
        let part = part.trim();
        if part.is_empty() || part.starts_with('@') {
            continue;
        }
        let italic = is_italic_entry(part);
        if normalize(part) == needle {
            return Some(italic);
        }
        let base = strip_style_suffixes(part);
        if normalize(&base) == needle {
            return Some(italic);
        }
    }
    // Fallback por stem exato do arquivo (ex.: `"impact"` → `impact.ttf`), nunca substring arbitrária.
    let stem = Path::new(raw_value.trim())
        .file_stem()
        .and_then(|s| s.to_str())
        .map(normalize)
        .unwrap_or_default();
    if !stem.is_empty() && stem == needle {
        return Some(is_italic_entry(stripped));
    }
    None
}

fn collect_from_hive(
    hive: isize,
    is_hkcu: bool,
    needle: &str,
    want_italic: bool,
    sys_dir: &Path,
    user_dir: Option<&Path>,
    upright: &mut Vec<PathBuf>,
    italic: &mut Vec<PathBuf>,
) {
    let subkey = wide_null(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts");
    let mut hkey: isize = 0;
    let status = unsafe { RegOpenKeyExW(hive, subkey.as_ptr(), 0, KEY_READ, &mut hkey) };
    if status != ERROR_SUCCESS {
        return;
    }

    let mut index: u32 = 0;
    let mut name_buf = vec![0u16; 512];
    let mut data_buf = vec![0u16; 512];
    loop {
        let mut name_len = name_buf.len() as u32;
        let mut val_type: u32 = 0;
        let mut data_bytes = (data_buf.len() * std::mem::size_of::<u16>()) as u32;
        let res = unsafe {
            RegEnumValueW(
                hkey,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                std::ptr::null_mut(),
                &mut val_type,
                data_buf.as_mut_ptr() as *mut u8,
                &mut data_bytes,
            )
        };
        if res == ERROR_NO_MORE_ITEMS {
            break;
        }
        if res == ERROR_SUCCESS && (val_type == REG_SZ || val_type == REG_EXPAND_SZ) {
            let raw_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            let u16_len = (data_bytes as usize / 2).min(data_buf.len());
            let trimmed_len = data_buf[..u16_len]
                .iter()
                .rposition(|&c| c != 0)
                .map_or(0, |pos| pos + 1);
            let raw_value = String::from_utf16_lossy(&data_buf[..trimmed_len]);

            if let Some(is_ital) = matches_registry_entry(&raw_name, &raw_value, needle) {
                if let Some(path) = resolve_registry_path(&raw_value, is_hkcu, sys_dir, user_dir) {
                    if !is_ital || want_italic {
                        upright.push(path);
                    } else {
                        italic.push(path);
                    }
                }
            }
        }
        index += 1;
        if index > 4096 {
            break;
        }
    }

    unsafe {
        RegCloseKey(hkey);
    }
}

fn is_style_or_weight_suffix(rest: &str) -> bool {
    if rest.is_empty() {
        return true;
    }
    let trimmed = rest
        .strip_suffix("italic")
        .or_else(|| rest.strip_suffix("oblique"))
        .or_else(|| rest.strip_suffix("roman"))
        .unwrap_or(rest);
    if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(weight) = trimmed.parse::<u16>() {
            if (100..=1000).contains(&weight) {
                return true;
            }
        }
    }
    matches!(
        rest,
        "regular"
            | "bold"
            | "semibold"
            | "extrabold"
            | "ultrabold"
            | "demibold"
            | "medium"
            | "light"
            | "extralight"
            | "ultralight"
            | "semilight"
            | "thin"
            | "black"
            | "heavy"
            | "book"
            | "normal"
            | "roman"
            | "variable"
            | "vf"
            | "wght"
            | "italic"
            | "bolditalic"
            | "oblique"
    )
}

fn stem_matches_family(stem: &str, needle: &str) -> bool {
    if stem == needle {
        return true;
    }
    let Some(rest) = stem.strip_prefix(needle) else {
        return false;
    };
    is_style_or_weight_suffix(rest)
}

fn collect_from_local_dirs(needle: &str, out: &mut Vec<PathBuf>) {
    for dir in app_font_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !is_supported_font_file(&path) {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(normalize)
                .unwrap_or_default();
            if stem_matches_family(&stem, needle) {
                out.push(path);
            }
        }
    }
}

/// Resolve os arquivos `.ttf/.otf/.ttc/.otc` de uma família específica (pastas locais do app +
/// Registro HKCU/HKLM do Windows). Nunca varre `C:\Windows\Fonts`.
pub fn resolve_font_files(family: &str) -> Vec<PathBuf> {
    let needle = normalize(family);
    if needle.is_empty() {
        return Vec::new();
    }
    let want_italic = needle.contains("italic") || needle.contains("oblique");
    let sys_dir = system_fonts_dir();
    let user_dir = user_fonts_dir();

    let mut upright = Vec::new();
    let mut italic = Vec::new();

    collect_from_local_dirs(&needle, &mut upright);
    collect_from_hive(
        HKEY_CURRENT_USER,
        true,
        &needle,
        want_italic,
        &sys_dir,
        user_dir.as_deref(),
        &mut upright,
        &mut italic,
    );
    collect_from_hive(
        HKEY_LOCAL_MACHINE,
        false,
        &needle,
        want_italic,
        &sys_dir,
        user_dir.as_deref(),
        &mut upright,
        &mut italic,
    );

    let chosen = if upright.is_empty() { italic } else { upright };
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(chosen.len());
    for path in chosen {
        let key = path.to_string_lossy().to_ascii_lowercase();
        if seen.insert(key) {
            deduped.push(path);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segoe_ui_matches_weights_but_excludes_emoji_and_symbol() {
        let needle = normalize("Segoe UI");
        assert_eq!(matches_registry_entry("Segoe UI (TrueType)", "segoeui.ttf", &needle), Some(false));
        assert_eq!(matches_registry_entry("Segoe UI Bold (TrueType)", "segoeuib.ttf", &needle), Some(false));
        assert_eq!(matches_registry_entry("Segoe UI Semibold (TrueType)", "seguisb.ttf", &needle), Some(false));
        assert_eq!(matches_registry_entry("Segoe UI Light (TrueType)", "segoeuil.ttf", &needle), Some(false));
        assert_eq!(matches_registry_entry("Segoe UI Italic (TrueType)", "segoeuii.ttf", &needle), Some(true));
        assert_eq!(matches_registry_entry("Segoe UI Emoji (TrueType)", "seguiemj.ttf", &needle), None);
        assert_eq!(matches_registry_entry("Segoe UI Symbol (TrueType)", "seguisym.ttf", &needle), None);
        assert_eq!(matches_registry_entry("Segoe UI Variable (TrueType)", "SegUIVar.ttf", &needle), None);
        assert_eq!(matches_registry_entry("Marlett (TrueType)", "marlett.ttf", &needle), None);
    }

    #[test]
    fn impact_and_exact_variants_match() {
        assert_eq!(
            matches_registry_entry("Impact (TrueType)", "impact.ttf", &normalize("Impact")),
            Some(false)
        );
        assert_eq!(
            matches_registry_entry("Arial Black (TrueType)", "ariblk.ttf", &normalize("Arial Black")),
            Some(false)
        );
        assert_eq!(
            matches_registry_entry("Arial Black (TrueType)", "ariblk.ttf", &normalize("Arial")),
            Some(false)
        );
        assert_eq!(
            matches_registry_entry(
                "SamsungSharpSans (TrueType)",
                r"C:\Users\u\AppData\Local\Microsoft\Windows\Fonts\samsungsharpsans.otf",
                &normalize("SamsungSharpSans"),
            ),
            Some(false)
        );
    }

    #[test]
    fn stem_prefix_does_not_match_unrelated_names() {
        assert!(stem_matches_family("outfitbold", "outfit"));
        assert!(stem_matches_family("interregular", "inter"));
        assert!(stem_matches_family("inter400", "inter"));
        assert!(stem_matches_family("inter700italic", "inter"));
        assert!(stem_matches_family("outfit800", "outfit"));
        assert!(stem_matches_family("jetbrainsmono500", "jetbrainsmono"));
        assert!(!stem_matches_family("inter2", "inter"));
        assert!(!stem_matches_family("winterregular", "inter"));
    }
}
