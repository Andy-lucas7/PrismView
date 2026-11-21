//! AssetVault: cache de assets por hash de conteúdo com refcount `Arc`.
//!
//! - `Vault<T>`: mapa `AssetHash → Arc<T>` protegido por `Mutex` (leituras curtas; o trabalho
//!   pesado de decodificação acontece fora do lock). O vault mantém **uma** referência forte por
//!   entrada; quem usa o asset recebe um `Arc` clonado. `Arc::strong_count == 1` significa "só o
//!   vault segura": a entrada pode ser despejada por `trim` (LRU) sem invalidar ninguém.
//! - `AssetVault`: um `Vault` de modelos e outro de texturas (modelos seguram `Arc<TextureAsset>`,
//!   então uma textura compartilhada por 2 modelos existe uma única vez) + índice
//!   `caminho/tamanho/mtime → hash` que evita reler e re-hashear arquivos já carregados.
//!
//! Regras: sem `unwrap/expect` (lock envenenado é recuperado com `into_inner`); nenhum código de SO.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use super::model::{ModelAsset, TextureAsset};

// ───────────────────────── Hash ─────────────────────────

/// Hash de 64 bits do conteúdo de um asset (não criptográfico).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetHash(pub u64);

impl AssetHash {
    /// Hash rápido de bytes (8 bytes por iteração + avalanche final).
    pub fn of_bytes(bytes: &[u8]) -> Self {
        const K: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut h: u64 = (bytes.len() as u64).wrapping_mul(K) ^ 0x243f_6a88_85a3_08d3;
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let mut word = [0u8; 8];
            word.copy_from_slice(chunk);
            h = (h.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(K);
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            h = (h.rotate_left(5) ^ u64::from_le_bytes(word)).wrapping_mul(K);
        }
        Self(Self::avalanche(h))
    }

    /// Combina com outro valor (ex.: espaço de cor da textura).
    pub fn combine(self, other: u64) -> Self {
        Self(Self::avalanche(
            self.0 ^ other.wrapping_add(0x9e37_79b9_7f4a_7c15).wrapping_add(self.0 << 6),
        ))
    }

    fn avalanche(mut x: u64) -> u64 {
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^ (x >> 31)
    }
}

// ───────────────────────── Vault<T> ─────────────────────────

struct Slot<T> {
    value: Arc<T>,
    bytes: usize,
    used: u64,
}

struct Inner<T> {
    map: HashMap<AssetHash, Slot<T>>,
    bytes: usize,
    clock: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VaultStats {
    pub entries: usize,
    pub bytes: usize,
    /// Entradas também seguras por alguém fora do vault (não despejáveis).
    pub in_use: usize,
}

pub struct Vault<T> {
    inner: Mutex<Inner<T>>,
}

impl<T> Default for Vault<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Vault<T> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                bytes: 0,
                clock: 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Referência ao asset (clona o `Arc`) e marca como usado recentemente.
    pub fn get(&self, hash: AssetHash) -> Option<Arc<T>> {
        let mut inner = self.lock();
        inner.clock += 1;
        let now = inner.clock;
        inner.map.get_mut(&hash).map(|slot| {
            slot.used = now;
            Arc::clone(&slot.value)
        })
    }

    pub fn contains(&self, hash: AssetHash) -> bool {
        self.lock().map.contains_key(&hash)
    }

    /// Insere `value` (`bytes` = custo estimado em memória). Se outro thread já inseriu o mesmo
    /// hash, devolve a entrada existente e descarta `value` (sem duplicar).
    pub fn insert(&self, hash: AssetHash, value: T, bytes: usize) -> Arc<T> {
        let mut inner = self.lock();
        inner.clock += 1;
        let now = inner.clock;
        if let Some(slot) = inner.map.get_mut(&hash) {
            slot.used = now;
            return Arc::clone(&slot.value);
        }
        let value = Arc::new(value);
        inner.bytes += bytes;
        inner.map.insert(
            hash,
            Slot {
                value: Arc::clone(&value),
                bytes,
                used: now,
            },
        );
        value
    }

    /// Despeja entradas **sem uso externo** (só o vault as segura), das menos recentes para as
    /// mais recentes, até o total ficar em `budget_bytes`. Devolve os bytes liberados.
    /// `trim(0)` esvazia tudo que está ocioso.
    pub fn trim(&self, budget_bytes: usize) -> usize {
        let mut inner = self.lock();
        if inner.bytes <= budget_bytes {
            return 0;
        }
        let mut idle: Vec<(u64, AssetHash)> = inner
            .map
            .iter()
            .filter(|(_, slot)| Arc::strong_count(&slot.value) == 1)
            .map(|(hash, slot)| (slot.used, *hash))
            .collect();
        idle.sort_unstable();
        let mut freed = 0usize;
        for (_, hash) in idle {
            if inner.bytes <= budget_bytes {
                break;
            }
            if let Some(slot) = inner.map.remove(&hash) {
                inner.bytes = inner.bytes.saturating_sub(slot.bytes);
                freed += slot.bytes;
            }
        }
        freed
    }

    /// Remove uma entrada ociosa (p.ex. asset trocado no Estúdio). `false` se está em uso/ausente.
    pub fn evict(&self, hash: AssetHash) -> bool {
        let mut inner = self.lock();
        let idle = inner
            .map
            .get(&hash)
            .is_some_and(|slot| Arc::strong_count(&slot.value) == 1);
        if !idle {
            return false;
        }
        if let Some(slot) = inner.map.remove(&hash) {
            inner.bytes = inner.bytes.saturating_sub(slot.bytes);
        }
        true
    }

    pub fn stats(&self) -> VaultStats {
        let inner = self.lock();
        VaultStats {
            entries: inner.map.len(),
            bytes: inner.bytes,
            in_use: inner
                .map
                .values()
                .filter(|slot| Arc::strong_count(&slot.value) > 1)
                .count(),
        }
    }
}

// ───────────────────────── AssetVault ─────────────────────────

/// Identidade barata de um arquivo em disco (para pular leitura + hash se nada mudou).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathKey {
    pub path: PathBuf,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

pub struct AssetVault {
    pub models: Vault<ModelAsset>,
    pub textures: Vault<TextureAsset>,
    paths: Mutex<HashMap<PathKey, AssetHash>>,
}

impl AssetVault {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            models: Vault::new(),
            textures: Vault::new(),
            paths: Mutex::new(HashMap::new()),
        })
    }

    fn paths(&self) -> MutexGuard<'_, HashMap<PathKey, AssetHash>> {
        self.paths.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Hash conhecido para este arquivo (mesmo caminho, tamanho e mtime).
    pub fn lookup_path(&self, key: &PathKey) -> Option<AssetHash> {
        self.paths().get(key).copied()
    }

    pub fn remember_path(&self, key: PathKey, hash: AssetHash) {
        let mut paths = self.paths();
        if paths.len() > 4096 {
            paths.clear();
        }
        paths.insert(key, hash);
    }

    /// Despeja modelos e texturas ociosos até caberem nos orçamentos. Texturas só ficam ociosas
    /// quando o modelo que as segura é despejado, então os modelos são aparados primeiro.
    pub fn trim(&self, model_budget: usize, texture_budget: usize) -> usize {
        let freed = self.models.trim(model_budget);
        freed + self.textures.trim(texture_budget)
    }

    /// Total em memória de CPU (modelos + texturas).
    pub fn total_bytes(&self) -> usize {
        self.models.stats().bytes + self.textures.stats().bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_sensitive() {
        let a = AssetHash::of_bytes(b"prismview");
        assert_eq!(a, AssetHash::of_bytes(b"prismview"));
        assert_ne!(a, AssetHash::of_bytes(b"prismvieW"));
        assert_ne!(a, AssetHash::of_bytes(b"prismview "));
        assert_ne!(a.combine(1), a.combine(2));
    }

    #[test]
    fn trim_only_evicts_idle_entries() {
        let vault: Vault<Vec<u8>> = Vault::new();
        let kept = vault.insert(AssetHash(1), vec![0; 10], 10);
        let _ = vault.insert(AssetHash(2), vec![0; 10], 10); // ocioso (Arc descartado)
        assert_eq!(vault.trim(0), 10);
        assert!(vault.contains(AssetHash(1)));
        assert!(!vault.contains(AssetHash(2)));
        drop(kept);
        assert_eq!(vault.trim(0), 10);
        assert_eq!(vault.stats().entries, 0);
    }

    #[test]
    fn duplicate_insert_returns_existing() {
        let vault: Vault<u32> = Vault::new();
        let a = vault.insert(AssetHash(7), 1, 4);
        let b = vault.insert(AssetHash(7), 2, 4);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(vault.stats().bytes, 4);
    }
}
