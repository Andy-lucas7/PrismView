//! `EXT_meshopt_compression`: decodifica um `bufferView` comprimido para os bytes originais.
//!
//! O JSON da extensão (dentro do `bufferView`) descreve os dados comprimidos:
//! `{ buffer, byteOffset, byteLength, byteStride, count, mode, filter }`. O `bufferView` em si
//! descreve o layout **descomprimido** dentro de um buffer de fallback; o loader grava o resultado
//! naquele buffer, de modo que os accessors leem os dados como se nunca tivessem sido comprimidos.
//!
//! Usa `meshopt::ffi` (strides dinâmicos, sem genéricos) para os decoders de vértice/índice.
//! Os filtros OCTAHEDRAL/QUATERNION/EXPONENTIAL são implementados aqui em Rust puro (port do
//! caminho escalar de `vertexfilter.cpp`): `meshopt` 0.4 não exporta `meshopt_decodeFilter*`,
//! o que gerava LNK2019. Nenhum `unwrap/expect`.

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
        match self.filter {
            MeshoptFilter::None => {}
            MeshoptFilter::Octahedral => match stride {
                4 => decode_filter_oct_i8(data, self.count),
                8 => decode_filter_oct_i16(data, self.count),
                _ => return Err(AssetError::Format("meshopt: OCTAHEDRAL exige stride 4 ou 8".into())),
            },
            MeshoptFilter::Quaternion => {
                if stride != 8 {
                    return Err(AssetError::Format("meshopt: QUATERNION exige stride 8".into()));
                }
                decode_filter_quat(data, self.count);
            }
            MeshoptFilter::Exponential => {
                if stride % 4 != 0 {
                    return Err(AssetError::Format("meshopt: EXPONENTIAL exige stride múltiplo de 4".into()));
                }
                decode_filter_exp(data, self.count * (stride / 4));
            }
        }
        Ok(())
    }
}

/// Arredondamento float→int com sinal, igual ao `int(v + (v >= 0 ? 0.5 : -0.5))` do C++.
#[inline]
fn round_signed(v: f32) -> i32 {
    (v + if v >= 0.0 { 0.5 } else { -0.5 }) as i32
}

/// OCTAHEDRAL, componentes i8 (stride 4): x, y, z(=1.0 codificado), w intacto.
fn decode_filter_oct_i8(data: &mut [u8], count: usize) {
    const MAX: f32 = 127.0;
    for v in data.chunks_exact_mut(4).take(count) {
        let (x, y, z) = oct_decode(
            f32::from(v[0] as i8),
            f32::from(v[1] as i8),
            f32::from(v[2] as i8),
            MAX,
        );
        v[0] = x as i8 as u8;
        v[1] = y as i8 as u8;
        v[2] = z as i8 as u8;
    }
}

/// OCTAHEDRAL, componentes i16 LE (stride 8).
fn decode_filter_oct_i16(data: &mut [u8], count: usize) {
    const MAX: f32 = 32767.0;
    for v in data.chunks_exact_mut(8).take(count) {
        let (x, y, z) = oct_decode(
            f32::from(read_i16(v, 0)),
            f32::from(read_i16(v, 1)),
            f32::from(read_i16(v, 2)),
            MAX,
        );
        write_i16(v, 0, x as i16);
        write_i16(v, 1, y as i16);
        write_i16(v, 2, z as i16);
    }
}

#[inline]
fn oct_decode(mut x: f32, mut y: f32, zin: f32, max: f32) -> (i32, i32, i32) {
    // z codifica 1.0 na mesma escala; reconstrói z e desfaz a dobra do octaedro para z < 0.
    let z = zin - x.abs() - y.abs();
    let t = if z < 0.0 { z } else { 0.0 };
    x += if x >= 0.0 { t } else { -t };
    y += if y >= 0.0 { t } else { -t };
    let l = (x * x + y * y + z * z).sqrt();
    let s = if l > 0.0 { max / l } else { 0.0 };
    (round_signed(x * s), round_signed(y * s), round_signed(z * s))
}

/// QUATERNION (stride 8, i16 LE): 3 componentes + (escala << 2 | índice do maior componente).
fn decode_filter_quat(data: &mut [u8], count: usize) {
    let scale = 1.0 / 2f32.sqrt();
    for v in data.chunks_exact_mut(8).take(count) {
        let c3 = read_i16(v, 3);
        let sf = i32::from(c3) | 3;
        let ss = scale / sf as f32;
        let x = f32::from(read_i16(v, 0)) * ss;
        let y = f32::from(read_i16(v, 1)) * ss;
        let z = f32::from(read_i16(v, 2)) * ss;
        let ww = 1.0 - x * x - y * y - z * z;
        let w = if ww >= 0.0 { ww } else { 0.0 }.sqrt();
        let xf = round_signed(x * 32767.0);
        let yf = round_signed(y * 32767.0);
        let zf = round_signed(z * 32767.0);
        let wf = (w * 32767.0 + 0.5) as i32;
        let qc = (i32::from(c3) & 3) as usize;
        write_i16(v, (qc + 1) & 3, xf as i16);
        write_i16(v, (qc + 2) & 3, yf as i16);
        write_i16(v, (qc + 3) & 3, zf as i16);
        write_i16(v, qc & 3, wf as i16);
    }
}

/// EXPONENTIAL: cada u32 LE = expoente i8 (bits 24..32) + mantissa i24 → `ldexp(m, e)` em f32.
fn decode_filter_exp(data: &mut [u8], words: usize) {
    for w in data.chunks_exact_mut(4).take(words) {
        let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        let m = ((v << 8) as i32) >> 8;
        let e = (v as i32) >> 24;
        let scale = f32::from_bits(((e + 127) as u32).wrapping_shl(23));
        let out = (scale * m as f32).to_bits();
        w.copy_from_slice(&out.to_le_bytes());
    }
}

#[inline]
fn read_i16(v: &[u8], i: usize) -> i16 {
    i16::from_le_bytes([v[i * 2], v[i * 2 + 1]])
}

#[inline]
fn write_i16(v: &mut [u8], i: usize, x: i16) {
    let b = x.to_le_bytes();
    v[i * 2] = b[0];
    v[i * 2 + 1] = b[1];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_filter_matches_ldexp() {
        // m = 3, e = -1 → 1.5
        let raw: u32 = ((-1i32 as u32) << 24) | 3;
        let mut d = raw.to_le_bytes().to_vec();
        decode_filter_exp(&mut d, 1);
        assert_eq!(f32::from_le_bytes([d[0], d[1], d[2], d[3]]), 1.5);
    }

    #[test]
    fn oct_filter_unit_z() {
        // (0, 0, 127) codifica +Z.
        let mut d = vec![0u8, 0, 127, 0];
        decode_filter_oct_i8(&mut d, 1);
        assert_eq!(d, vec![0, 0, 127, 0]);
    }
}
