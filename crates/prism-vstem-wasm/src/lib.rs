//! `prism-vstem-wasm`: o alongamento vetorial por haste do motor nativo (`prism-core::vstem`)
//! rodando no Estúdio, para o texto alongado do canvas sair idêntico ao do desktop.
//!
//! ABI C simples (sem wasm-bindgen): o JS escreve bytes na memória do módulo (`vs_alloc`), registra
//! fontes (`vs_font_add`), pede um layout (`vs_layout`) e lê o resultado como `Float32Array` em
//! `vs_out_ptr()/vs_out_len()`. Um único thread: o estado fica em `thread_local!`.
//!
//! Saída de `vs_layout` (floats): `[largura, altura, nº de linhas, ...comandos]`, comandos como em
//! `layout::OP_*` (`0 = M x y`, `1 = L x y`, `2 = Z`), em px CSS, y para baixo.

#[path = "../../prism-core/src/vstem.rs"]
#[allow(dead_code)]
pub mod vstem;

pub mod layout;

use std::cell::RefCell;
use std::collections::HashMap;

use layout::{Align, Font, Params};

/// Máximo de fontes vivas ao mesmo tempo (o JS reaproveita ids; isto é só um teto de segurança).
const MAX_FONTS: usize = 64;
/// Tamanho máximo aceito para os bytes de uma fonte.
const MAX_FONT_BYTES: usize = 48 * 1024 * 1024;

#[derive(Default)]
struct State {
    fonts: HashMap<u32, Font>,
    next_id: u32,
    out: Vec<f32>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { next_id: 1, ..State::default() });
}

/// Versão da ABI: o JS recusa módulos de outra versão.
#[no_mangle]
pub extern "C" fn vs_abi_version() -> u32 {
    1
}

/// Reserva `len` bytes na memória do módulo para o JS escrever (fonte ou texto).
#[no_mangle]
pub extern "C" fn vs_alloc(len: usize) -> *mut u8 {
    if len == 0 || len > MAX_FONT_BYTES {
        return std::ptr::null_mut();
    }
    let mut v: Vec<u8> = Vec::with_capacity(len);
    let ptr = v.as_mut_ptr();
    std::mem::forget(v);
    ptr
}

/// Libera o que `vs_alloc` reservou (mesmo `len`).
///
/// # Safety
/// `ptr` deve vir de `vs_alloc(len)` e não ter sido liberado antes.
#[no_mangle]
pub unsafe extern "C" fn vs_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        drop(Vec::from_raw_parts(ptr, 0, len));
    }
}

/// Registra uma fonte (copia os bytes). Devolve o id (> 0) ou 0 se inválida/cheia.
///
/// # Safety
/// `ptr..ptr+len` deve ser memória válida e inicializada do módulo.
#[no_mangle]
pub unsafe extern "C" fn vs_font_add(ptr: *const u8, len: usize, face_index: u32) -> u32 {
    if ptr.is_null() || len == 0 || len > MAX_FONT_BYTES {
        return 0;
    }
    let data = std::slice::from_raw_parts(ptr, len).to_vec();
    let Some(font) = Font::new(data, face_index) else { return 0 };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.fonts.len() >= MAX_FONTS {
            return 0;
        }
        let id = s.next_id;
        s.next_id = s.next_id.wrapping_add(1).max(1);
        s.fonts.insert(id, font);
        id
    })
}

#[no_mangle]
pub extern "C" fn vs_font_remove(id: u32) {
    STATE.with(|s| {
        s.borrow_mut().fonts.remove(&id);
    });
}

/// Esvazia os caches de contornos da fonte (ex.: ao mudar de tamanho de tela).
#[no_mangle]
pub extern "C" fn vs_font_clear_caches(id: u32) {
    STATE.with(|s| {
        if let Some(f) = s.borrow_mut().fonts.get_mut(&id) {
            f.clear_caches();
        }
    });
}

/// Faz o layout + alongamento. Devolve o nº de floats da saída (0 = erro: fonte desconhecida,
/// texto inválido ou falha ao ler a fonte). Texto em UTF-8 em `text_ptr..+text_len`.
///
/// # Safety
/// `text_ptr..text_ptr+text_len` deve ser memória válida do módulo.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn vs_layout(
    font_id: u32,
    text_ptr: *const u8,
    text_len: usize,
    size: f32,
    weight: f32,
    letter_spacing_px: f32,
    align: u32,
    max_width: f32,
    stretch: f32,
    compensation: f32,
    device_scale: f32,
) -> usize {
    if text_ptr.is_null() && text_len > 0 {
        return 0;
    }
    let bytes = if text_len == 0 { &[][..] } else { std::slice::from_raw_parts(text_ptr, text_len) };
    let Ok(text) = std::str::from_utf8(bytes) else { return 0 };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let State { fonts, out, .. } = &mut *s;
        let Some(font) = fonts.get_mut(&font_id) else { return 0 };
        let params = Params {
            text,
            size,
            weight,
            letter_spacing_px,
            align: Align::from_u32(align),
            max_width,
            stretch,
            compensation,
            device_scale,
        };
        let Some(l) = layout::layout(font, &params) else { return 0 };
        out.clear();
        out.reserve(3 + l.cmds.len());
        out.extend_from_slice(&[l.width, l.height, l.lines as f32]);
        out.extend_from_slice(&l.cmds);
        out.len()
    })
}

#[no_mangle]
pub extern "C" fn vs_out_ptr() -> *const f32 {
    STATE.with(|s| s.borrow().out.as_ptr())
}

#[no_mangle]
pub extern "C" fn vs_out_len() -> usize {
    STATE.with(|s| s.borrow().out.len())
}
