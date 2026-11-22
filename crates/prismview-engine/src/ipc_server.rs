//! Servidor IPC do Studio: named pipe `IPC_PIPE_NAME`, mensagens JSON (`IpcRequest`/`IpcResponse`).

use prism_core::schema::{IpcRequest, IpcResponse, IPC_PIPE_NAME};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

pub struct IpcEnvelope {
    pub req: IpcRequest,
    pub reply: Sender<IpcResponse>,
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// `wake` acorda o loop do host depois de enfileirar um pedido.
pub fn spawn_named_pipe_server(wake: impl Fn() + Send + 'static, tx: Sender<IpcEnvelope>) {
    thread::spawn(move || {
        let pipe_w = to_wide(IPC_PIPE_NAME);
        loop {
            unsafe {
                let pipe: HANDLE = CreateNamedPipeW(
                    PCWSTR(pipe_w.as_ptr()),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    64 * 1024,
                    64 * 1024,
                    0,
                    None,
                );
                if pipe == INVALID_HANDLE_VALUE {
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }

                let connected = ConnectNamedPipe(pipe, None).is_ok()
                    || GetLastError() == ERROR_PIPE_CONNECTED;
                if connected {
                    handle_pipe_client(pipe, &wake, &tx);
                }
                let _ = DisconnectNamedPipe(pipe);
                let _ = CloseHandle(pipe);
            }
        }
    });
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
            let _ = FlushFileBuffers(pipe);
        }
    }
}
