//! Parent side of the ASR helper process: spawn the child, send framed
//! requests, route replies.
//!
//! `push` never touches the pipe: it frames the audio and queues it on a
//! bounded channel drained by a dedicated writer thread, so the caller
//! only blocks under backpressure. A reader thread routes FINAL / PARTIAL /
//! ERROR replies to per-utterance waiters; an unexpected child exit fails
//! every pending call with [`WorkerCrashed`].

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context};

use super::frame::*;

/// Error when the helper child is gone. Check with
/// [`is_crashed`] to tell it apart from timeouts and worker-reported
/// `ERROR` replies (which are plain `anyhow` messages).
#[derive(Debug)]
pub struct WorkerCrashed;

impl std::fmt::Display for WorkerCrashed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "asr worker process exited")
    }
}

impl std::error::Error for WorkerCrashed {}

/// True when `err` is (or wraps) a [`WorkerCrashed`).
pub fn is_crashed(err: &anyhow::Error) -> bool {
    err.downcast_ref::<WorkerCrashed>().is_some()
}

type TextWaiter = mpsc::Sender<Result<String, String>>;

#[derive(Default)]
struct Pending {
    finish: HashMap<u64, TextWaiter>,
    partial: HashMap<u64, TextWaiter>,
    load: Option<mpsc::Sender<Result<u64, String>>>,
    status: Option<mpsc::Sender<String>>,
    pong: Option<mpsc::Sender<()>>,
}

/// Parent-side handle to one helper child. `Drop` kills and reaps the child.
pub struct AsrWorker {
    child: Mutex<Option<Child>>,
    /// Framed bytes for the writer thread. Bounded: `push` only blocks
    /// when the child is not draining (256 queued frames of headroom).
    tx: mpsc::SyncSender<Vec<u8>>,
    pending: Arc<Mutex<Pending>>,
}

impl AsrWorker {
    /// Spawn `program args` with piped stdin/stdout (stderr inherited).
    pub fn spawn(program: &Path, args: &[&str]) -> anyhow::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawning asr worker {}", program.display()))?;
        let stdin = child.stdin.take().context("worker stdin not piped")?;
        let stdout = child.stdout.take().context("worker stdout not piped")?;
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(256);
        std::thread::Builder::new()
            .name("asr-worker-writer".to_string())
            .spawn(move || writer_loop(stdin, rx))
            .context("spawning worker writer thread")?;
        let pending = Arc::new(Mutex::new(Pending::default()));
        let reader_pending = Arc::clone(&pending);
        std::thread::Builder::new()
            .name("asr-worker-reader".to_string())
            .spawn(move || reader_loop(stdout, reader_pending))
            .context("spawning worker reader thread")?;
        Ok(Self {
            child: Mutex::new(Some(child)),
            tx,
            pending,
        })
    }

    /// Queue one framed message. Only blocks under backpressure; a dead
    /// writer thread means the child is gone.
    fn send(&self, msg_type: u8, payload: &[u8]) -> anyhow::Result<()> {
        let mut bytes = Vec::with_capacity(5 + payload.len());
        write_frame(&mut bytes, msg_type, payload)?;
        match self.tx.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(bytes)) => {
                // Rare: the child is not draining. Block rather than drop audio.
                self.tx.send(bytes).map_err(|_| WorkerCrashed)?;
                Ok(())
            }
            Err(mpsc::TrySendError::Disconnected(_)) => Err(WorkerCrashed.into()),
        }
    }

    fn lock_child(&self) -> std::sync::MutexGuard<'_, Option<Child>> {
        self.child.lock().expect("child mutex poisoned")
    }

    fn check_alive(&self) -> anyhow::Result<()> {
        let mut guard = self.lock_child();
        let child = guard.as_mut().expect("worker child gone");
        match child.try_wait() {
            Ok(None) => Ok(()),
            _ => Err(WorkerCrashed.into()),
        }
    }

    /// Load `model_id` in the child; returns load milliseconds.
    pub fn load(&self, model_id: &str, timeout: Duration) -> anyhow::Result<u64> {
        self.check_alive()?;
        let (tx, rx) = mpsc::channel::<Result<u64, String>>();
        {
            let mut p = self.pending.lock().expect("pending mutex poisoned");
            if p.load.is_some() {
                return Err(anyhow!("a worker LOAD is already in flight"));
            }
            p.load = Some(tx);
        }
        self.send(REQ_LOAD, model_id.as_bytes())?;
        match rx.recv_timeout(timeout) {
            Ok(Ok(ms)) => Ok(ms),
            Ok(Err(msg)) => Err(anyhow!("worker failed to load model: {msg}")),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(self.timeout_or_crash("LOAD")),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(WorkerCrashed.into()),
        }
    }

    /// Start utterance `id` (replaces any previous buffer for `id`).
    pub fn begin(&self, id: u64) -> anyhow::Result<()> {
        self.check_alive()?;
        self.send(REQ_BEGIN, &encode_id(id))
    }

    /// Append 16 kHz mono samples to utterance `id`. Queued for the
    /// writer thread; does not wait for the child.
    pub fn push(&self, id: u64, samples: &[f32]) -> anyhow::Result<()> {
        self.check_alive()?;
        self.send(REQ_PUSH, &encode_push(id, samples))
    }

    /// End utterance `id` and wait for its transcript.
    pub fn finish(&self, id: u64, timeout: Duration) -> anyhow::Result<String> {
        self.check_alive()?;
        let rx = self.register(&mut |p, tx| {
            p.finish.insert(id, tx);
        });
        self.send(REQ_FINISH, &encode_id(id))?;
        self.wait_text(rx, timeout, "FINISH")
    }

    /// Drop utterance `id` without decoding. A pending `finish` for `id`
    /// fails with "cancelled" instead of hanging.
    pub fn cancel(&self, id: u64) -> anyhow::Result<()> {
        self.check_alive()?;
        let waiters = {
            let mut p = self.pending.lock().expect("pending mutex poisoned");
            [p.finish.remove(&id), p.partial.remove(&id)]
        };
        for tx in waiters.into_iter().flatten() {
            let _ = tx.send(Err("cancelled".to_string()));
        }
        self.send(REQ_CANCEL, &encode_id(id))
    }

    /// Decode the audio-so-far of utterance `id` without ending it.
    pub fn partial(&self, id: u64, timeout: Duration) -> anyhow::Result<String> {
        self.check_alive()?;
        let rx = self.register(&mut |p, tx| {
            p.partial.insert(id, tx);
        });
        self.send(REQ_PARTIAL, &encode_id(id))?;
        self.wait_text(rx, timeout, "PARTIAL")
    }

    /// One-shot transcription without an utterance. Id 0 is reserved for
    /// this path; do not `begin(0)`.
    pub fn transcribe(&self, samples: &[f32], timeout: Duration) -> anyhow::Result<String> {
        self.check_alive()?;
        let rx = self.register(&mut |p, tx| {
            p.finish.insert(0, tx);
        });
        self.send(REQ_TRANSCRIBE, &encode_samples(samples))?;
        self.wait_text(rx, timeout, "TRANSCRIBE")
    }

    /// Ask the child for its `STATUS_REPLY` JSON (`state`/`model`/`rss_mb`).
    pub fn status(&self, timeout: Duration) -> anyhow::Result<String> {
        self.check_alive()?;
        let (tx, rx) = mpsc::channel::<String>();
        {
            let mut p = self.pending.lock().expect("pending mutex poisoned");
            if p.status.is_some() {
                return Err(anyhow!("a worker STATUS is already in flight"));
            }
            p.status = Some(tx);
        }
        self.send(REQ_STATUS, &[])?;
        match rx.recv_timeout(timeout) {
            Ok(json) => Ok(json),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(self.timeout_or_crash("STATUS")),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(WorkerCrashed.into()),
        }
    }

    /// Round-trip check; the child answers PONG.
    pub fn ping(&self, timeout: Duration) -> anyhow::Result<()> {
        self.check_alive()?;
        let (tx, rx) = mpsc::channel::<()>();
        {
            let mut p = self.pending.lock().expect("pending mutex poisoned");
            if p.pong.is_some() {
                return Err(anyhow!("a worker PING is already in flight"));
            }
            p.pong = Some(tx);
        }
        self.send(REQ_PING, &[])?;
        match rx.recv_timeout(timeout) {
            Ok(()) => Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(self.timeout_or_crash("PING")),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(WorkerCrashed.into()),
        }
    }

    /// Ask the child to exit (it replies, then exits 0), wait up to 2 s,
    /// then kill and reap. The OS reclaims the whole child RSS on exit —
    /// that is the point of the helper process.
    pub fn unload(&self) -> anyhow::Result<()> {
        let _ = self.send(REQ_UNLOAD, &[]);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let exited = self
                .lock_child()
                .as_mut()
                .expect("worker child gone")
                .try_wait()
                .context("waiting for worker exit")?
                .is_some();
            if exited {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut guard = self.lock_child();
        let child = guard.as_mut().expect("worker child gone");
        child
            .kill()
            .context("killing worker after UNLOAD timeout")?;
        // Reap so no zombie is left behind.
        child.wait().context("reaping worker after kill")?;
        Ok(())
    }

    /// True while the child has not been observed exited.
    pub fn is_alive(&self) -> bool {
        self.check_alive().is_ok()
    }

    /// OS pid of the helper child (diagnostics and tests).
    pub fn pid(&self) -> u32 {
        self.lock_child().as_ref().expect("worker child gone").id()
    }

    fn register(
        &self,
        slot: &mut dyn FnMut(&mut Pending, TextWaiter),
    ) -> mpsc::Receiver<Result<String, String>> {
        let (tx, rx) = mpsc::channel::<Result<String, String>>();
        slot(
            &mut self.pending.lock().expect("pending mutex poisoned"),
            tx,
        );
        rx
    }

    fn wait_text(
        &self,
        rx: mpsc::Receiver<Result<String, String>>,
        timeout: Duration,
        what: &str,
    ) -> anyhow::Result<String> {
        match rx.recv_timeout(timeout) {
            Ok(Ok(text)) => Ok(text),
            Ok(Err(msg)) => Err(anyhow!("worker {what} failed: {msg}")),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(self.timeout_or_crash(what)),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(WorkerCrashed.into()),
        }
    }

    /// A call that got no reply: blame the child when it is dead, else a
    /// plain timeout (the waiter stays registered; a late reply is dropped).
    fn timeout_or_crash(&self, what: &str) -> anyhow::Error {
        if self.is_alive() {
            anyhow!("worker {what} timed out")
        } else {
            WorkerCrashed.into()
        }
    }
}

impl Drop for AsrWorker {
    fn drop(&mut self) {
        let child = self.child.lock().expect("child mutex poisoned").take();
        if let Some(mut child) = child {
            // Kill and reap off-thread: `wait` blocks and `Drop` must not.
            // The reader thread already ends on the child's stdout EOF.
            std::thread::spawn(move || {
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

/// Move framed bytes into the child stdin. Ends when the parent drops its
/// `SyncSender` (client dropped) or the child closes stdin.
fn writer_loop(mut stdin: impl Write, rx: mpsc::Receiver<Vec<u8>>) {
    for bytes in rx {
        if stdin.write_all(&bytes).is_err() {
            break;
        }
        if stdin.flush().is_err() {
            break;
        }
    }
}

/// Read framed replies and complete the matching waiter. On EOF or a
/// protocol error the thread just ends: every waiter channel disconnects,
/// so pending calls fail with [`WorkerCrashed`].
fn reader_loop(stdout: impl std::io::Read, pending: Arc<Mutex<Pending>>) {
    let mut input = BufReader::new(stdout);
    loop {
        let frame = match read_frame(&mut input) {
            Ok(Some(frame)) => frame,
            _ => break,
        };
        let (msg, payload) = frame;
        let mut p = pending.lock().expect("pending mutex poisoned");
        match msg {
            REP_FINAL => {
                if let Ok((id, text)) = decode_id_text(&payload) {
                    if let Some(tx) = p.finish.remove(&id) {
                        let _ = tx.send(Ok(text));
                    }
                }
            }
            REP_PARTIAL => {
                if let Ok((id, text)) = decode_id_text(&payload) {
                    if let Some(tx) = p.partial.remove(&id) {
                        let _ = tx.send(Ok(text));
                    }
                }
            }
            REP_ERROR => {
                if let Ok((id, msg)) = decode_id_text(&payload) {
                    if let Some(tx) = p.finish.remove(&id) {
                        let _ = tx.send(Err(msg.clone()));
                    }
                    if let Some(tx) = p.partial.remove(&id) {
                        let _ = tx.send(Err(msg));
                    }
                }
            }
            REP_LOADED => {
                if payload.len() == 8 {
                    let ms = u64::from_le_bytes(payload[..8].try_into().expect("checked len"));
                    if let Some(tx) = p.load.take() {
                        let _ = tx.send(Ok(ms));
                    }
                }
            }
            REP_LOAD_FAILED => {
                let msg = String::from_utf8(payload).unwrap_or_else(|_| "bad utf8".to_string());
                if let Some(tx) = p.load.take() {
                    let _ = tx.send(Err(msg));
                }
            }
            REP_STATUS => {
                if let Some(tx) = p.status.take() {
                    let _ = tx.send(String::from_utf8(payload).unwrap_or_default());
                }
            }
            REP_PONG => {
                if let Some(tx) = p.pong.take() {
                    let _ = tx.send(());
                }
            }
            _ => {}
        }
    }
}
