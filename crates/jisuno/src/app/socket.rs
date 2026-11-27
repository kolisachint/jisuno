//! Control socket server: one-line JSON over
//! `~/Library/Application Support/jisuno/jisuno.sock`.
//!
//! The listener runs on its own thread with a [`SocketSnapshot`] refreshed by
//! the tick; `warm`/`unload` travel back as [`HookRequest`] so the tick owns
//! the model bookkeeping. One connection per thread; the socket is `0600`,
//! removed on quit, and a stale file is replaced at startup.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

use crate::app::command::HookRequest;
use crate::app::model::ModelState;
use crate::ipc::{self, Request, Response};

/// Tick-refreshed answer for `{"cmd":"status"}`. `rss_mb` is the helper
/// child RSS while the model is warm; `None` falls back to this process.
#[derive(Debug, Clone)]
pub(crate) struct SocketSnapshot {
    pub(crate) model: String,
    pub(crate) state: String,
    pub(crate) rss_mb: Option<u64>,
}

/// `warm` / `loading` / `unloaded` for the socket and the CLI.
pub(crate) fn state_name(state: &ModelState) -> &'static str {
    match state {
        ModelState::Warm => "warm",
        ModelState::Loading { .. } => "loading",
        ModelState::Unloaded | ModelState::Failed => "unloaded",
    }
}

/// This process's peak RSS in MiB (`ru_maxrss` is bytes on macOS).
pub(crate) fn process_rss_mb() -> u64 {
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return 0;
        }
        usage
    };
    (usage.ru_maxrss.max(0) as u64) / 1024 / 1024
}

fn model_ready(model: &str) -> bool {
    jisuno_asr::setup::Model::parse(model)
        .map(|m| m.is_ready())
        .unwrap_or(false)
}

fn answer(request: &Request, snapshot: &SocketSnapshot) -> (Response, Option<HookRequest>) {
    match request {
        Request::Status => (
            Response::status(
                snapshot.model.clone(),
                &snapshot.state,
                snapshot.rss_mb.unwrap_or_else(process_rss_mb),
            ),
            None,
        ),
        Request::Warm => {
            if model_ready(&snapshot.model) {
                (Response::ok(), Some(HookRequest::WarmModel))
            } else {
                (Response::err("model not installed"), None)
            }
        }
        Request::Unload => (Response::ok(), Some(HookRequest::UnloadModel)),
    }
}

fn handle(stream: UnixStream, snapshot: &SocketSnapshot, hook_tx: &mpsc::Sender<HookRequest>) {
    let mut line = String::new();
    let response = {
        let mut reader = BufReader::new(&stream);
        match reader.read_line(&mut line) {
            Ok(_) => match Request::decode_line(&line) {
                Ok(request) => {
                    let (response, hook) = answer(&request, snapshot);
                    if let Some(hook) = hook {
                        let _ = hook_tx.send(hook);
                    }
                    response
                }
                Err(message) => Response::err(message),
            },
            Err(e) => Response::err(format!("{e}")),
        }
    };
    let mut stream = stream;
    let _ = stream.write_all(response.encode_line().as_bytes());
}

/// Bind the socket, replacing a stale file. Errors with
/// [`std::io::ErrorKind::AddrInUse`] when a live app answers, so a second
/// instance exits instead of stealing the socket. Returns the path for
/// removal on quit.
pub(crate) fn start(
    snapshot: Arc<Mutex<SocketSnapshot>>,
    hook_tx: mpsc::Sender<HookRequest>,
) -> std::io::Result<PathBuf> {
    let path = ipc::socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() && ipc::send(&Request::Status).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "Jisuno is already running",
        ));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(stream) = conn else {
                break;
            };
            let snapshot = snapshot.clone();
            let hook_tx = hook_tx.clone();
            std::thread::spawn(move || {
                let guard = snapshot.lock().unwrap_or_else(|e| e.into_inner()).clone();
                handle(stream, &guard, &hook_tx);
            });
        }
    });
    Ok(path)
}
