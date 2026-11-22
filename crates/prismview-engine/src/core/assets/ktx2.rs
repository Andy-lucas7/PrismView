//! Texturas KTX2 (`KHR_texture_basisu`): container parseado à mão (cabeçalho de 80 bytes + índice
//! de níveis) e transcodificação UASTC → BC7 (GPU com `TEXTURE_COMPRESSION_BC`) ou RGBA8.
//!
//! Suportado:
//! - `vkFormat` R8G8B8A8 UNORM/SRGB e BC7 UNORM/SRGB (cópia direta dos níveis);
//! - Basis Universal **UASTC** (`vkFormat = 0`, modelo de cor 166), com ou sem supercompressão
//!   Zstandard. Transcodifica para BC7 quando permitido e as dimensões são múltiplas de 4, senão
//!   decodifica para RGBA8.
//!
//! Não suportado (erro `Unsupported`, o material fica sem a textura): ETC1S/BasisLZ (precisa dos
//! codebooks globais), arrays, cubemaps e volumes.

use std::io::Read;
use std::sync::Once;

use super::model::{AssetError, TexelFormat, TextureAsset, TextureLevel};

const KTX2_MAGIC: [u8; 12] = [
    0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];
const HEADER_LEN: usize = 80;
const VK_UNDEFINED: u32 = 0;
const VK_R8G8B8A8_UNORM: u32 = 37;
const VK_R8G8B8A8_SRGB: u32 = 43;
const VK_BC7_UNORM: u32 = 145;
const VK_BC7_SRGB: u32 = 146;
const SUPERCOMPRESSION_NONE: u32 = 0;
const SUPERCOMPRESSION_ZSTD: u32 = 2;
const COLOR_MODEL_ETC1S: u8 = 163;
const COLOR_MODEL_UASTC: u8 = 166;
const TRANSFER_SRGB: u8 = 2;
/// Teto defensivo por textura descomprimida.
const MAX_TEXTURE_BYTES: usize = 512 * 1024 * 1024;

pub fn is_ktx2(bytes: &[u8]) -> bool {
    bytes.len() >= HEADER_LEN && bytes[..12] == KTX2_MAGIC
}

#[derive(Clone, Copy, Debug)]
pub struct Ktx2Options {
    /// O device aceita BC7 (`Features::TEXTURE_COMPRESSION_BC`).
    pub allow_bc7: bool,
    /// Descarta níveis de mip maiores que isto (economia de RAM/VRAM). 0 = sem limite.
    pub max_dim: u32,
}

fn rd_u32(b: &[u8], off: usize) -> Result<u32, AssetError> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| AssetError::Format("KTX2 truncado".into()))
}

fn rd_u64(b: &[u8], off: usize) -> Result<u64, AssetError> {
    b.get(off..off + 8)
        .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
        .ok_or_else(|| AssetError::Format("KTX2 truncado".into()))
}

fn to_usize(v: u64) -> Result<usize, AssetError> {
    usize::try_from(v).map_err(|_| AssetError::Format("KTX2: offset fora do alcance".into()))
}

struct RawLevel<'a> {
    data: &'a [u8],
    uncompressed_len: usize,
}

fn decompress_zstd(input: &[u8], expected: usize) -> Result<Vec<u8>, AssetError> {
    if expected > MAX_TEXTURE_BYTES {
        return Err(AssetError::Format("KTX2: nível grande demais".into()));
    }
    let mut source = input;
    let mut decoder = ruzstd::StreamingDecoder::new(&mut source)
        .map_err(|e| AssetError::Decode(format!("zstd: {e}")))?;
    let mut out = Vec::with_capacity(expected);
    decoder
        .read_to_end(&mut out)
        .map_err(|e| AssetError::Decode(format!("zstd: {e}")))?;
    Ok(out)
}

fn init_transcoder() {
    static INIT: Once = Once::new();
    INIT.call_once(basis_universal::transcoding::transcoder_init);
}

/// Decodifica um arquivo KTX2 completo.
pub fn decode_ktx2(bytes: &[u8], options: Ktx2Options) -> Result<TextureAsset, AssetError> {
    if !is_ktx2(bytes) {
        return Err(AssetError::Format("não é KTX2".into()));
    }
    let vk_format = rd_u32(bytes, 12)?;
    let width = rd_u32(bytes, 20)?;
    let height = rd_u32(bytes, 24)?.max(1);
    let depth = rd_u32(bytes, 28)?;
    let layers = rd_u32(bytes, 32)?;
    let faces = rd_u32(bytes, 36)?;
    let level_count = rd_u32(bytes, 40)?.max(1);
    let supercompression = rd_u32(bytes, 44)?;
    let dfd_offset = to_usize(u64::from(rd_u32(bytes, 48)?))?;

    if width == 0 || depth > 1 || layers > 1 || faces > 1 {
        return Err(AssetError::Unsupported(
            "KTX2: arrays, cubemaps e volumes não são suportados".into(),
        ));
    }
    if supercompression != SUPERCOMPRESSION_NONE && supercompression != SUPERCOMPRESSION_ZSTD {
        return Err(AssetError::Unsupported(
            "KTX2: supercompressão BasisLZ/ZLIB (ETC1S) não suportada; exporte UASTC".into(),
        ));
    }
    if level_count > 16 {
        return Err(AssetError::Format("KTX2: níveis demais".into()));
    }

    // Índice de níveis: byteOffset, byteLength, uncompressedByteLength (u64 cada).
    let mut raw: Vec<RawLevel<'_>> = Vec::with_capacity(level_count as usize);
    for level in 0..level_count as usize {
        let base = HEADER_LEN + level * 24;
        let offset = to_usize(rd_u64(bytes, base)?)?;
        let length = to_usize(rd_u64(bytes, base + 8)?)?;
        let uncompressed = to_usize(rd_u64(bytes, base + 16)?)?;
        let end = offset
            .checked_add(length)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| AssetError::Format("KTX2: nível fora do arquivo".into()))?;
        raw.push(RawLevel {
            data: &bytes[offset..end],
            uncompressed_len: uncompressed,
        });
    }

    // DFD básico: [totalSize u32][descriptor: w0 vendor/type, w1 version/size, w2 model/primaries/
    // transfer/flags]. colorModel = byte 0 de w2, transferFunction = byte 2.
    let (color_model, transfer) = match bytes.get(dfd_offset + 12..dfd_offset + 16) {
        Some(w) => (w[0], w[2]),
        None => (0, 0),
    };

    // Descarta níveis acima do limite de dimensão (nível i tem dimensão `max(1, w >> i)`).
    let mut first_level = 0usize;
    if options.max_dim > 0 {
        while first_level + 1 < raw.len()
            && (width >> first_level).max(1).max((height >> first_level).max(1)) > options.max_dim
        {
            first_level += 1;
        }
    }

    let mut data: Vec<u8> = Vec::new();
    let mut levels: Vec<TextureLevel> = Vec::with_capacity(raw.len() - first_level);
    let (format, srgb);

    match vk_format {
        VK_R8G8B8A8_UNORM | VK_R8G8B8A8_SRGB | VK_BC7_UNORM | VK_BC7_SRGB => {
            let is_bc7 = vk_format == VK_BC7_UNORM || vk_format == VK_BC7_SRGB;
            if is_bc7 && !options.allow_bc7 {
                return Err(AssetError::Unsupported(
                    "KTX2 BC7 em GPU sem TEXTURE_COMPRESSION_BC".into(),
                ));
            }
            format = if is_bc7 { TexelFormat::Bc7 } else { TexelFormat::Rgba8 };
            srgb = vk_format == VK_BC7_SRGB || vk_format == VK_R8G8B8A8_SRGB;
            for (i, level) in raw.iter().enumerate().skip(first_level) {
                let payload = if supercompression == SUPERCOMPRESSION_ZSTD {
                    decompress_zstd(level.data, level.uncompressed_len)?
                } else {
                    level.data.to_vec()
                };
                push_level(&mut data, &mut levels, &payload, (width >> i).max(1), (height >> i).max(1))?;
            }
        }
        VK_UNDEFINED => {
            if color_model == COLOR_MODEL_ETC1S {
                return Err(AssetError::Unsupported(
                    "KTX2 ETC1S/BasisLZ não suportado; reexporte como UASTC".into(),
                ));
            }
            if color_model != COLOR_MODEL_UASTC {
                return Err(AssetError::Unsupported(format!(
                    "KTX2 Basis com modelo de cor {color_model}"
                )));
            }
            srgb = transfer == TRANSFER_SRGB;
            init_transcoder();
            let bc7_ok = options.allow_bc7 && width % 4 == 0 && height % 4 == 0;
            format = if bc7_ok { TexelFormat::Bc7 } else { TexelFormat::Rgba8 };
            for (i, level) in raw.iter().enumerate().skip(first_level) {
                let (w, h) = ((width >> i).max(1), (height >> i).max(1));
                let blocks = if supercompression == SUPERCOMPRESSION_ZSTD {
                    decompress_zstd(level.data, level.uncompressed_len)?
                } else {
                    level.data.to_vec()
                };
                let payload = transcode_uastc(&blocks, w, h, bc7_ok)?;
                push_level(&mut data, &mut levels, &payload, w, h)?;
            }
        }
        other => {
            return Err(AssetError::Unsupported(format!(
                "KTX2 vkFormat {other} não suportado"
            )));
        }
    }

    let (w0, h0) = levels
        .first()
        .map(|l| (l.width, l.height))
        .ok_or_else(|| AssetError::Format("KTX2 sem níveis".into()))?;
    let generate_mips = levels.len() == 1 && (w0 > 1 || h0 > 1) && format == TexelFormat::Rgba8;
    Ok(TextureAsset {
        width: w0,
        height: h0,
        format,
        srgb,
        levels,
        generate_mips,
        data,
    })
}

fn push_level(
    data: &mut Vec<u8>,
    levels: &mut Vec<TextureLevel>,
    payload: &[u8],
    width: u32,
    height: u32,
) -> Result<(), AssetError> {
    if data.len().saturating_add(payload.len()) > MAX_TEXTURE_BYTES {
        return Err(AssetError::Format("KTX2: textura grande demais".into()));
    }
    levels.push(TextureLevel {
        offset: data.len(),
        len: payload.len(),
        width,
        height,
    });
    data.extend_from_slice(payload);
    Ok(())
}

/// Transcodifica um nível UASTC (blocos de 16 B, 4×4) para BC7 ou RGBA8.
fn transcode_uastc(blocks: &[u8], width: u32, height: u32, to_bc7: bool) -> Result<Vec<u8>, AssetError> {
    use basis_universal::transcoding::{
        DecodeFlags, LowLevelUastcTranscoder, SliceParametersUastc, TranscoderBlockFormat,
    };
    let num_blocks_x = width.div_ceil(4);
    let num_blocks_y = height.div_ceil(4);
    let expected = (num_blocks_x as usize)
        .checked_mul(num_blocks_y as usize)
        .and_then(|n| n.checked_mul(16))
        .ok_or_else(|| AssetError::Format("UASTC: tamanho inválido".into()))?;
    if blocks.len() < expected {
        return Err(AssetError::Format("UASTC: nível truncado".into()));
    }
    let transcoder = LowLevelUastcTranscoder::new();
    transcoder
        .transcode_slice(
            &blocks[..expected],
            SliceParametersUastc {
                num_blocks_x,
                num_blocks_y,
                has_alpha: true,
                original_width: width,
                original_height: height,
            },
            DecodeFlags::empty(),
            if to_bc7 {
                TranscoderBlockFormat::BC7
            } else {
                TranscoderBlockFormat::RGBA32
            },
        )
        .map_err(|e| AssetError::Decode(format!("UASTC: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_ktx2() {
        assert!(!is_ktx2(b"\x89PNG\r\n\x1a\n"));
        assert!(decode_ktx2(&[0u8; 100], Ktx2Options { allow_bc7: true, max_dim: 0 }).is_err());
    }
}
