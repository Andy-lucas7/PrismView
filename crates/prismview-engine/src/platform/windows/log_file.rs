//! Log em arquivo: o motor roda destacado (stderr = NUL), então todo `eprintln!`/pânico seria
//! perdido. `redirect_stderr_to_file` aponta o STD_ERROR_HANDLE do processo para um arquivo
//! (o `std` do Rust relê o handle a cada escrita). Rotação no início: acima de 1 MB, o atual
//! vira `<nome>.1` (no máximo 2 arquivos). `PRISMVIEW_LOG_STDERR` desliga (depuração no console).

use std::fs::{self, OpenOptions};
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use ::windows::Win32::Foundation::HANDLE;
use ::windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};

const MAX_LOG_BYTES: u64 = 1024 * 1024;

pub fn redirect_stderr_to_file(dir: &Path, name: &str) -> Result<(), String> {
    if std::env::var_os("PRISMVIEW_LOG_STDERR").is_some() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|e| format!("log dir {}: {e}", dir.display()))?;
    let path = dir.join(name);
    if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        fs::rename(&path, dir.join(format!("{name}.1")))
            .map_err(|e| format!("rotacionar {}: {e}", path.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("abrir {}: {e}", path.display()))?;
    // SAFETY: handle válido do arquivo aberto acima; mantido vivo até o fim do processo.
    unsafe { SetStdHandle(STD_ERROR_HANDLE, HANDLE(file.as_raw_handle())) }
        .map_err(|e| format!("SetStdHandle: {e}"))?;
    std::mem::forget(file);
    Ok(())
}
