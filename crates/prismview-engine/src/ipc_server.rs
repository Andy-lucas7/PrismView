//! Servidor IPC do Studio: named pipe `IPC_PIPE_NAME`, mensagens JSON (`IpcRequest`/`IpcResponse`).
//!
//! Segurança: a 1ª instância usa `FILE_FLAG_FIRST_PIPE_INSTANCE` (se outro processo já criou o
//! nome, o motor não serve em vez de dividir o pipe), `PIPE_REJECT_REMOTE_CLIENTS` e DACL só com
//! o usuário atual + SYSTEM. Disponibilidade: a instância seguinte é criada assim que um cliente
//! conecta (cada cliente na sua thread), então um pedido lento nunca deixa o pipe "ocupado" a
//! ponto de o Studio concluir que o motor caiu.

use prism_core::schema::{IpcRequest, IpcResponse, IPC_PIPE_NAME};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL,
    INVALID_HANDLE_VALUE,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub struct IpcEnvelope {
    pub req: IpcRequest,
    pub reply: Sender<IpcResponse>,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Instância do pipe (dona do handle). Só é movida entre threads, nunca compartilhada.
struct PipeInstance(HANDLE);

// SAFETY: o HANDLE do pipe pertence a uma única thread por vez (movido para a do cliente).
unsafe impl Send for PipeInstance {}

impl Drop for PipeInstance {
    fn drop(&mut self) {
        // SAFETY: handle válido criado por CreateNamedPipeW; fechado uma única vez aqui.
        unsafe {
            if DisconnectNamedPipe(self.0).is_err() {
                // Cliente já desconectado: nada a fazer.
            }
            if let Err(err) = CloseHandle(self.0) {
                eprintln!("[ipc] CloseHandle: {err}");
            }
        }
    }
}

/// Descritor de segurança `D:P(A;;GA;;;<usuário>)(A;;GA;;;SY)` (liberado com `LocalFree`).
struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
}

// SAFETY: o descritor é imutável após criado e só lido por CreateNamedPipeW.
unsafe impl Send for PipeSecurity {}

impl PipeSecurity {
    fn current_user() -> Result<Self, String> {
        let sid = current_user_sid_string()?;
        let sddl = to_wide(&format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` termina em NUL; o descritor devolvido é liberado no Drop.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|e| format!("descritor de segurança do pipe: {e}"))?;
        Ok(Self { descriptor })
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        // SAFETY: memória alocada por ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            let _ = LocalFree(HLOCAL(self.descriptor.0));
        }
    }
}

/// SID do usuário do processo em texto (`S-1-5-21-...`).
fn current_user_sid_string() -> Result<String, String> {
    // SAFETY: chamadas Win32 com buffers do tamanho informado; o token é fechado no fim.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| format!("OpenProcessToken: {e}"))?;
        let mut needed = 0u32;
        // 1ª chamada só mede (falha com ERROR_INSUFFICIENT_BUFFER por contrato).
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        // `u64` garante o alinhamento de TOKEN_USER.
        let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        let info = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        );
        if let Err(err) = CloseHandle(token) {
            eprintln!("[ipc] CloseHandle(token): {err}");
        }
        info.map_err(|e| format!("GetTokenInformation: {e}"))?;
        let user = &*(buf.as_ptr().cast::<TOKEN_USER>());
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)
            .map_err(|e| format!("ConvertSidToStringSidW: {e}"))?;
        let sid = text.to_string().map_err(|e| format!("SID: {e}"));
        let _ = LocalFree(HLOCAL(text.0.cast()));
        sid
    }
}

fn create_instance(name: &[u16], security: &PipeSecurity, first: bool) -> Result<PipeInstance, String> {
    let attrs = security.attributes();
    let open_mode = if first {
        PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE
    } else {
        PIPE_ACCESS_DUPLEX
    };
    // SAFETY: `name` termina em NUL; `attrs` vive até o fim da chamada.
    let pipe = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            open_mode,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            Some(&attrs),
        )
    };
    if pipe == INVALID_HANDLE_VALUE || pipe.is_invalid() {
        // SAFETY: lido logo após a chamada que falhou.
        let err = unsafe { GetLastError() };
        return Err(format!("CreateNamedPipeW: {err:?}"));
    }
    Ok(PipeInstance(pipe))
}

/// Cria a 1ª instância (falha se o nome já existe) e sobe a thread que aceita clientes.
/// `wake` acorda o loop do host depois de enfileirar um pedido.
pub fn spawn_named_pipe_server(
    wake: impl Fn() + Send + Sync + 'static,
    tx: Sender<IpcEnvelope>,
) -> Result<(), String> {
    let name = to_wide(IPC_PIPE_NAME);
    let security = PipeSecurity::current_user()?;
    let first = create_instance(&name, &security, true)
        .map_err(|e| format!("pipe já em uso por outro processo ou negado ({e})"))?;
    let wake: Waker = Arc::new(wake);
    thread::Builder::new()
        .name("prismview-ipc".into())
        .spawn(move || accept_loop(name, security, first, wake, tx))
        .map_err(|e| format!("thread do IPC: {e}"))?;
    Ok(())
}

fn accept_loop(
    name: Vec<u16>,
    security: PipeSecurity,
    first: PipeInstance,
    wake: Waker,
    tx: Sender<IpcEnvelope>,
) {
    let mut next = Some(first);
    loop {
        let pipe = match next.take() {
            Some(pipe) => pipe,
            None => match create_instance(&name, &security, false) {
                Ok(pipe) => pipe,
                Err(err) => {
                    eprintln!("[ipc] {err}");
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
            },
        };
        // SAFETY: handle de pipe válido; bloqueia até um cliente conectar.
        let connected = unsafe {
            ConnectNamedPipe(pipe.0, None).is_ok() || GetLastError() == ERROR_PIPE_CONNECTED
        };
        if !connected {
            continue; // `pipe` cai e é fechado
        }
        let wake = Arc::clone(&wake);
        let tx = tx.clone();
        let spawned = thread::Builder::new()
            .name("prismview-ipc-client".into())
            .spawn(move || {
                // SAFETY: a instância é exclusiva desta thread até o drop.
                unsafe { handle_pipe_client(pipe.0, &*wake, &tx) };
                drop(pipe);
            });
        if let Err(err) = spawned {
            eprintln!("[ipc] thread do cliente: {err}");
        }
        // Próxima volta cria a instância seguinte já (antes de o cliente atual terminar).
    }
}

unsafe fn handle_pipe_client(pipe: HANDLE, wake: &dyn Fn(), tx: &Sender<IpcEnvelope>) {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let mut bytes_read = 0u32;
        if ReadFile(pipe, Some(&mut buf), Some(&mut bytes_read), None).is_err() || bytes_read == 0 {
            break;
        }
        let payload = &buf[..bytes_read as usize];
        let resp = match serde_json::from_slice::<IpcRequest>(payload) {
            Ok(req) => {
                let (reply_tx, reply_rx) = mpsc::channel();
                if tx.send(IpcEnvelope { req, reply: reply_tx }).is_ok() {
                    wake();
                    reply_rx
                        .recv_timeout(Duration::from_secs(3))
                        .unwrap_or(IpcResponse::Error {
                            message: "engine_timeout".to_string(),
                        })
                } else {
                    IpcResponse::Error {
                        message: "engine_closed".to_string(),
                    }
                }
            }
            Err(e) => IpcResponse::Error {
                message: format!("invalid_json: {e}"),
            },
        };

        if let Ok(mut out) = serde_json::to_vec(&resp) {
            out.push(b'\n');
            let mut written = 0u32;
            if WriteFile(pipe, Some(&out), Some(&mut written), None).is_err() {
                break;
            }
            if let Err(err) = FlushFileBuffers(pipe) {
                eprintln!("[ipc] FlushFileBuffers: {err}");
            }
        }
    }
}
