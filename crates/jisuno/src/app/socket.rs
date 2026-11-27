//! Control socket server: one-line JSON over
//! `~/Library/Application Support/jisuno/jisuno.sock`.
//!
//! The listener runs on its own thread with a [`SocketSnapshot`] refreshed by
//! the tick; `warm`/`unload` travel back as [`HookRequest`] so the tick owns
//! the model bookkeeping. One connection per thread; the socket is `0600`,
//! removed on quit, and a stale file is replaced at startup.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crate::app::command::HookRequest;
use crate::app::model::ModelState;
use crate::ipc::{self, ClientError, Request, Response};

/// Read and write timeout per connection: a client that stalls cannot pin a
/// thread for longer than this.
const CONN_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest request line accepted, in bytes (newline included). A longer line
/// is cut off here and fails to decode.
const MAX_LINE: u64 = 4 * 1024;

/// Tick-refreshed answer for `{"cmd":"status"}`. `rss_mb` is the helper
/// child RSS while the model is warm; `None` falls back to this process.
#[derive(Debug, Clone)]
pub(crate) struct SocketSnapshot {
    pub(crate) model: String,
    pub(crate) state: String,
    pub(crate) rss_mb: Option<u64>,
    /// Recording or transcribing: unload is refused (the hold would lose its audio).
    pub(crate) busy: bool,
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
        Request::Unload if snapshot.busy => (Response::err("busy"), None),
        Request::Unload => (Response::ok(), Some(HookRequest::UnloadModel)),
    }
}

fn handle(stream: UnixStream, snapshot: &SocketSnapshot, hook_tx: &mpsc::Sender<HookRequest>) {
    let mut line = String::new();
    let timeouts = stream
        .set_read_timeout(Some(CONN_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(CONN_TIMEOUT)));
    if timeouts.is_err() {
        return;
    }
    let response = {
        let mut reader = BufReader::new((&stream).take(MAX_LINE));
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

/// Bind the socket, replacing a stale file. Only a refused connect (no
/// listener) counts as stale. Anything else means another instance is alive:
/// [`std::io::ErrorKind::AddrInUse`] is returned so the second instance exits
/// instead of stealing the socket. Returns the path for removal on quit.
pub(crate) fn start(
    snapshot: Arc<Mutex<SocketSnapshot>>,
    hook_tx: mpsc::Sender<HookRequest>,
) -> std::io::Result<PathBuf> {
    let path = ipc::socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        match ipc::send(&Request::Status) {
            Err(ClientError::NotRunning) => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "another Jisuno instance is running",
                ));
            }
            Err(ClientError::Failed(message)) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!(
                        "another Jisuno instance is not answering on the control socket ({message})"
                    ),
                ));
            }
        }
    }
    let _ = std::fs::remove_file(&path);
    // Bind under umask 077 so the socket is never world-accessible, even for
    // the instant before the explicit chmod below.
    let old_umask = unsafe { libc::umask(0o077) };
    let bound = UnixListener::bind(&path);
    unsafe { libc::umask(old_umask) };
    let listener = bound?;
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
