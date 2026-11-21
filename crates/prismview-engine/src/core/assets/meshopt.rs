//! `EXT_meshopt_compression`: decodifica um `bufferView` comprimido para os bytes originais.
//!
//! O JSON da extensão (dentro do `bufferView`) descreve os dados comprimidos:
//! `{ buffer, byteOffset, byteLength, byteStride, count, mode, filter }`. O `bufferView` em si
//! descreve o layout **descomprimido** dentro de um buffer de fallback; o loader grava o resultado
//! naquele buffer, de modo que os accessors leem os dados como se nunca tivessem sido comprimidos.
//!
//! Usa `meshopt::ffi` (strides dinâmicos, sem genéricos). Os filtros OCTAHEDRAL/QUATERNION/
//! EXPONENTIAL também vêm da biblioteca. Nenhum `unwrap/expect`.

use std::ffi::c_void;

use serde_json::Value;

use super::model::AssetError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshoptMode {
    Attributes,
    Triangles,
    Indices,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshoptFilter {
    None,
    Octahedral,
    Quaternion,
    Exponential,
}

/// Parâmetros da extensão de um `bufferView`.
#[derive(Clone, Copy, Debug)]
pub struct MeshoptView {
    /// Buffer que contém os dados **comprimidos**.
    pub buffer: usize,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub byte_stride: usize,
    pub count: usize,
    pub mode: MeshoptMode,
    pub filter: MeshoptFilter,
}

/// Teto defensivo do tamanho descomprimido de um view (arquivo malicioso/corrompido).
const MAX_DECODED_BYTES: usize = 512 * 1024 * 1024;

impl MeshoptView {
    /// Lê o objeto `EXT_meshopt_compression` de um `bufferView`.
    pub fn from_json(value: &Value) -> Result<Self, AssetError> {
        let uint = |key: &str| -> Result<usize, AssetError> {
            value
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| AssetError::Format(format!("EXT_meshopt_compression: `{key}` ausente")))
        };
        let mode = match value.get("mode").and_then(Value::as_str) {
            Some("ATTRIBUTES") => MeshoptMode::Attributes,
            Some("TRIANGLES") => MeshoptMode::Triangles,
            Some("INDICES") => MeshoptMode::Indices,
            other => {
                return Err(AssetError::Format(format!(
                    "EXT_meshopt_compression: mode inválido {other:?}"
                )))
            }
        };
        let filter = match value.get("filter").and_then(Value::as_str) {
            None | Some("NONE") => MeshoptFilter::None,
            Some("OCTAHEDRAL") => MeshoptFilter::Octahedral,
            Some("QUATERNION") => MeshoptFilter::Quaternion,
            Some("EXPONENTIAL") => MeshoptFilter::Exponential,
            other => {
                return Err(AssetError::Format(format!(
                    "EXT_meshopt_compression: filter inválido {other:?}"
                )))
            }
        };
        Ok(Self {
            buffer: uint("buffer")?,
            byte_offset: value
                .get("byteOffset")
                .and_then(Value::as_u64)
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(0),
            byte_length: uint("byteLength")?,
            byte_stride: uint("byteStride")?,
            count: uint("count")?,
            mode,
            filter,
        })
    }

    /// Tamanho descomprimido (`count * byteStride`).
    pub fn decoded_len(&self) -> Option<usize> {
        self.count
            .checked_mul(self.byte_stride)
            .filter(|&n| n <= MAX_DECODED_BYTES)
    }

    /// Decodifica `source` (o conteúdo do buffer `self.buffer`) e devolve os bytes originais.
    pub fn decode(&self, source: &[u8]) -> Result<Vec<u8>, AssetError> {
        let decoded_len = self
            .decoded_len()
            .ok_or_else(|| AssetError::Format("meshopt: tamanho descomprimido inválido".into()))?;
        let end = self
            .byte_offset
            .checked_add(self.byte_length)
            .filter(|&e| e <= source.len())
            .ok_or_else(|| AssetError::Format("meshopt: bufferView fora do buffer".into()))?;
        let encoded = &source[self.byte_offset..end];
        let mut out = vec![0u8; decoded_len];
        if self.count == 0 {
            return Ok(out);
        }

        match self.mode {
            MeshoptMode::Attributes => {
                if self.byte_stride == 0 || self.byte_stride % 4 != 0 || self.byte_stride > 256 {
                    return Err(AssetError::Format(format!(
                        "meshopt: byteStride {} inválido para ATTRIBUTES",
                        self.byte_stride
                    )));
                }
                // SAFETY: `out` tem exatamente `count * stride` bytes; `encoded` é um slice válido.
                let rc = unsafe {
                    ::meshopt::ffi::meshopt_decodeVertexBuffer(
                        out.as_mut_ptr().cast::<c_void>(),
                        self.count,
                        self.byte_stride,
                        encoded.as_ptr(),
                        encoded.len(),
                    )
                };
                if rc != 0 {
                    return Err(AssetError::Decode(format!(
                        "meshopt: decodeVertexBuffer falhou ({rc})"
                    )));
                }
                self.apply_filter(&mut out)?;
            }
            MeshoptMode::Triangles | MeshoptMode::Indices => {
                if self.byte_stride != 2 && self.byte_stride != 4 {
                    return Err(AssetError::Format(format!(
                        "meshopt: tamanho de índice {} inválido",
                        self.byte_stride
                    )));
                }
                if self.mode == MeshoptMode::Triangles && self.count % 3 != 0 {
                    return Err(AssetError::Format(
                        "meshopt: TRIANGLES com count não múltiplo de 3".into(),
                    ));
                }
                // SAFETY: idem acima (`count` índices de `byte_stride` bytes).
                let rc = unsafe {
                    if self.mode == MeshoptMode::Triangles {
                        ::meshopt::ffi::meshopt_decodeIndexBuffer(
                            out.as_mut_ptr().cast::<c_void>(),
                            self.count,
                            self.byte_stride,
                            encoded.as_ptr(),
                            encoded.len(),
                        )
                    } else {
                        ::meshopt::ffi::meshopt_decodeIndexSequence(
                            out.as_mut_ptr().cast::<c_void>(),
                            self.count,
                            self.byte_stride,
                            encoded.as_ptr(),
                            encoded.len(),
                        )
                    }
                };
                if rc != 0 {
                    return Err(AssetError::Decode(format!(
                        "meshopt: decode de índices falhou ({rc})"
                    )));
                }
            }
        }
        Ok(out)
    }

    fn apply_filter(&self, data: &mut [u8]) -> Result<(), AssetError> {
        let stride = self.byte_stride;
        let ptr = data.as_mut_ptr().cast::<c_void>();
        match self.filter {
            MeshoptFilter::None => {}
            MeshoptFilter::Octahedral => {
                if stride != 4 && stride != 8 {
                    return Err(AssetError::Format("meshopt: OCTAHEDRAL exige stride 4 ou 8".into()));
                }
                // SAFETY: `data` tem `count * stride` bytes.
                unsafe { ::meshopt::ffi::meshopt_decodeFilterOct(ptr, self.count, stride) };
            }
            MeshoptFilter::Quaternion => {
                if stride != 8 {
                    return Err(AssetError::Format("meshopt: QUATERNION exige stride 8".into()));
                }
                // SAFETY: idem.
                unsafe { ::meshopt::ffi::meshopt_decodeFilterQuat(ptr, self.count, stride) };
            }
            MeshoptFilter::Exponential => {
                if stride % 4 != 0 {
                    return Err(AssetError::Format("meshopt: EXPONENTIAL exige stride múltiplo de 4".into()));
                }
                // SAFETY: idem.
                unsafe { ::meshopt::ffi::meshopt_decodeFilterExp(ptr, self.count, stride) };
            }
        }
        Ok(())
    }
}
