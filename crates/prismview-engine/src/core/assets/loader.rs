//! Loader assíncrono de modelos: threads de trabalho fazem E/S, hash, decodificação (meshopt,
//! KTX2, imagens) e inserem o resultado no `AssetVault`; o thread de render só envia pedidos e
//! consome eventos com `poll`, sem nunca bloquear.
//!
//! Fluxo de um pedido (`request(path)`):
//! 1. `PathKey` (caminho + tamanho + mtime) já conhecido → hash → `Arc` do vault (sem ler o arquivo).
//! 2. Lê o arquivo, calcula o hash do conteúdo (+ configuração de decodificação); já no vault →
//!    reaproveita (arquivos idênticos com nomes diferentes compartilham a mesma entrada).
//! 3. `parse_model` dentro de `catch_unwind` (arquivo corrompido nunca derruba o processo) e
//!    `vault.models.insert`.
//! 4. Evento `Ready`/`Failed` + `wake()` para acordar o loop de eventos (que dorme em `Wait`).
//!
//! Limitação: para `.gltf` com buffers/imagens externos o hash cobre só o JSON (prefira `.glb`,
//! que é o que o Estúdio exporta).
//!
//! Regras: sem `unwrap/expect`; as threads encerram quando o `AssetLoader` é solto.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use super::gltf_loader::{parse_model, LoaderConfig};
use super::model::{AssetError, ModelAsset};
use super::vault::{AssetHash, AssetVault, PathKey};

const MAX_MODEL_BYTES: u64 = 1 << 30;

/// Identifica um pedido (devolvido por `request`, repetido no evento).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ticket(pub u64);

#[derive(Debug)]
pub enum LoadEvent {
    Ready {
        ticket: Ticket,
        path: PathBuf,
        asset: Arc<ModelAsset>,
        /// `true` se veio do vault sem decodificar nada.
        cached: bool,
    },
    Failed {
        ticket: Ticket,
        path: PathBuf,
        error: AssetError,
    },
}

struct Job {
    ticket: Ticket,
    path: PathBuf,
}

pub struct AssetLoader {
    jobs: Option<Sender<Job>>,
    /// Usado só para reportar falha quando nenhum worker está disponível.
    fail_tx: Sender<LoadEvent>,
    events: Receiver<LoadEvent>,
    workers: Vec<JoinHandle<()>>,
    next_ticket: u64,
    vault: Arc<AssetVault>,
}

impl AssetLoader {
    /// `wake` é chamado de um thread de trabalho a cada evento novo; deve apenas acordar o loop
    /// principal (ex.: `LoopProxy::wake`).
    pub fn new(
        vault: Arc<AssetVault>,
        cfg: LoaderConfig,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (event_tx, event_rx) = mpsc::channel::<LoadEvent>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);

        let threads = cfg.worker_threads.clamp(1, 4);
        let mut workers = Vec::with_capacity(threads);
        for index in 0..threads {
            let job_rx = Arc::clone(&job_rx);
            let event_tx = event_tx.clone();
            let vault = Arc::clone(&vault);
            let wake = Arc::clone(&wake);
            let spawned = thread::Builder::new()
                .name(format!("prism-asset-{index}"))
                .spawn(move || worker_loop(&job_rx, &event_tx, &vault, &cfg, wake.as_ref()));
            if let Ok(handle) = spawned {
                workers.push(handle);
            }
        }
        Self {
            jobs: Some(job_tx),
            fail_tx: event_tx,
            events: event_rx,
            workers,
            next_ticket: 1,
            vault,
        }
    }

    pub fn vault(&self) -> &Arc<AssetVault> {
        &self.vault
    }

    /// Enfileira o carregamento de `path`. Nunca bloqueia; o resultado chega em `poll`.
    pub fn request(&mut self, path: impl Into<PathBuf>) -> Ticket {
        let ticket = Ticket(self.next_ticket);
        self.next_ticket += 1;
        let path = path.into();
        let sent = !self.workers.is_empty()
            && self.jobs.as_ref().is_some_and(|tx| {
                tx.send(Job {
                    ticket,
                    path: path.clone(),
                })
                .is_ok()
            });
        if !sent {
            let _ = self.fail_tx.send(LoadEvent::Failed {
                ticket,
                path,
                error: AssetError::Io("nenhuma thread de carregamento disponível".into()),
            });
        }
        ticket
    }

    /// Move os eventos prontos para `out` (reaproveitado: `clear` + `push`). Não bloqueia.
    pub fn poll(&mut self, out: &mut Vec<LoadEvent>) {
        out.clear();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
    }
}

impl Drop for AssetLoader {
    fn drop(&mut self) {
        // Fechar o canal de jobs encerra os workers (terminam o job em andamento).
        self.jobs = None;
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

fn worker_loop(
    jobs: &Mutex<Receiver<Job>>,
    events: &Sender<LoadEvent>,
    vault: &AssetVault,
    cfg: &LoaderConfig,
    wake: &(dyn Fn() + Send + Sync),
) {
    loop {
        // O lock só vale durante `recv`: um worker espera aqui, os outros esperam no mutex.
        let job = {
            let guard = jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.recv()
        };
        let Ok(job) = job else {
            return;
        };
        let event = match load_model_file(&job.path, vault, cfg) {
            Ok((asset, cached)) => LoadEvent::Ready {
                ticket: job.ticket,
                path: job.path,
                asset,
                cached,
            },
            Err(error) => LoadEvent::Failed {
                ticket: job.ticket,
                path: job.path,
                error,
            },
        };
        if events.send(event).is_err() {
            return;
        }
        wake();
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "pânico desconhecido".to_owned()
    }
}

/// Carrega (ou reaproveita do vault) um modelo de forma **síncrona**. Usado pelos workers e por
/// ferramentas/testes. O `bool` indica se veio do vault sem decodificar.
pub fn load_model_file(
    path: &Path,
    vault: &AssetVault,
    cfg: &LoaderConfig,
) -> Result<(Arc<ModelAsset>, bool), AssetError> {
    let meta = std::fs::metadata(path)
        .map_err(|e| AssetError::Io(format!("{}: {e}", path.display())))?;
    if meta.len() > MAX_MODEL_BYTES {
        return Err(AssetError::Io(format!("{}: arquivo grande demais", path.display())));
    }
    let key = PathKey {
        path: path.to_path_buf(),
        len: meta.len(),
        modified: meta.modified().ok(),
    };
    if let Some(hash) = vault.lookup_path(&key) {
        if let Some(asset) = vault.models.get(hash) {
            return Ok((asset, true));
        }
    }

    let bytes = std::fs::read(path).map_err(|e| AssetError::Io(format!("{}: {e}", path.display())))?;
    // A configuração muda o resultado (BC7 vs RGBA8, limite de textura): entra no hash.
    let fingerprint = u64::from(cfg.allow_bc7) | (u64::from(cfg.max_texture_dim) << 8);
    let hash = AssetHash::of_bytes(&bytes).combine(fingerprint);
    if let Some(asset) = vault.models.get(hash) {
        vault.remember_path(key, hash);
        return Ok((asset, true));
    }

    let base = path.parent();
    let parsed = catch_unwind(AssertUnwindSafe(|| parse_model(bytes, hash, base, vault, cfg)))
        .map_err(|payload| AssetError::Panic(panic_message(payload.as_ref())))??;
    let size = parsed.byte_size();
    let asset = vault.models.insert(hash, parsed, size);
    vault.remember_path(key, hash);
    Ok((asset, false))
}
