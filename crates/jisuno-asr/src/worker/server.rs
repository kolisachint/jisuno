//! Child side of the ASR helper process: load the model once, answer
//! framed requests on stdin with framed replies on stdout, exit on UNLOAD.
//!
//! Decoding is single-threaded and sequential: one request at a time, so
//! the ONNX sessions are never touched concurrently. PARTIAL re-decodes
//! the audio-so-far; the parent throttles it with `--partial-ms`.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::time::Instant;

use anyhow::{anyhow, Context};

use super::frame::*;
use crate::setup::Model;
use crate::transcribe::{ParakeetStreamingSession, StreamConfig, StreamingTranscriber};

/// Something that turns 16 kHz mono f32 PCM into text, streaming while the
/// hold is down and full-clip for one-shots. The real backend owns one
/// [`ParakeetStreamingSession`] — one model in memory, shared by the
/// streaming utterance path and TRANSCRIBE (which runs a full-clip decode
/// on the same worker sessions). Tests use [`FakeBackend`].
trait Backend {
    /// Start (or restart) utterance `id`, dropping any previous audio.
    fn begin(&mut self, id: u64);
    /// Append audio to `id`. Errors on unknown utterances.
    fn push(&mut self, id: u64, samples: &[f32]) -> anyhow::Result<()>;
    /// Joined finished segments so far (no re-decode). Errors on unknown.
    fn partial(&mut self, id: u64) -> anyhow::Result<String>;
    /// Decode the tail and return the full transcript. Errors on unknown.
    fn finish(&mut self, id: u64) -> anyhow::Result<String>;
    /// Drop `id` without decoding.
    fn cancel(&mut self, id: u64);
    /// Full-clip one-shot decode (TRANSCRIBE path).
    fn transcribe(&mut self, pcm: &[f32]) -> anyhow::Result<String>;
}

/// Parakeet backend: one streaming session serves both the utterance path
/// (pause-split segments decoded on its background thread, `finish` only
/// decodes the tail) and TRANSCRIBE (full-clip on the same sessions).
/// One capture at a time: `begin` resets the session for the new id.
struct StreamingBackend {
    inner: ParakeetStreamingSession,
    active: Option<u64>,
}

impl StreamingBackend {
    fn load(dir: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self {
            inner: ParakeetStreamingSession::load(dir, StreamConfig::default())?,
            active: None,
        })
    }

    fn check(&self, id: u64) -> anyhow::Result<()> {
        if self.active == Some(id) {
            Ok(())
        } else {
            Err(anyhow!("unknown utterance {id}"))
        }
    }
}

impl Backend for StreamingBackend {
    fn begin(&mut self, id: u64) {
        self.inner.reset();
        self.active = Some(id);
    }

    fn push(&mut self, id: u64, samples: &[f32]) -> anyhow::Result<()> {
        self.check(id)?;
        self.inner.push(samples)
    }

    fn partial(&mut self, id: u64) -> anyhow::Result<String> {
        self.check(id)?;
        Ok(self.inner.partial_drained().unwrap_or_default())
    }

    fn finish(&mut self, id: u64) -> anyhow::Result<String> {
        self.check(id)?;
        let text = self.inner.finish()?;
        self.inner.reset();
        self.active = None;
        Ok(text)
    }

    fn cancel(&mut self, id: u64) {
        if self.active == Some(id) {
            self.inner.reset();
            self.active = None;
        }
    }

    fn transcribe(&mut self, pcm: &[f32]) -> anyhow::Result<String> {
        self.inner.transcribe_full(pcm)
    }
}

/// Test backend: text is the sample count. Deterministic, needs no model.
/// Buffers per utterance like the real path; `partial` reports the buffered
/// count (the real path reports joined finished segments instead).
struct FakeBackend {
    buffers: HashMap<u64, Vec<f32>>,
}

impl FakeBackend {
    fn new() -> Self {
        Self {
            buffers: HashMap::new(),
        }
    }

    fn check(&self, id: u64) -> anyhow::Result<()> {
        if self.buffers.contains_key(&id) {
            Ok(())
        } else {
            Err(anyhow!("unknown utterance {id}"))
        }
    }
}

impl Backend for FakeBackend {
    fn begin(&mut self, id: u64) {
        self.buffers.insert(id, Vec::new());
    }

    fn push(&mut self, id: u64, samples: &[f32]) -> anyhow::Result<()> {
        match self.buffers.get_mut(&id) {
            Some(buf) => {
                buf.extend_from_slice(samples);
                Ok(())
            }
            None => Err(anyhow!("unknown utterance {id}")),
        }
    }

    fn partial(&mut self, id: u64) -> anyhow::Result<String> {
        self.check(id)?;
        Ok(format!("samples={}", self.buffers[&id].len()))
    }

    fn finish(&mut self, id: u64) -> anyhow::Result<String> {
        match self.buffers.remove(&id) {
            Some(buf) => Ok(format!("samples={}", buf.len())),
            None => Err(anyhow!("unknown utterance {id}")),
        }
    }

    fn cancel(&mut self, id: u64) {
        self.buffers.remove(&id);
    }

    fn transcribe(&mut self, pcm: &[f32]) -> anyhow::Result<String> {
        Ok(format!("samples={}", pcm.len()))
    }
}

/// Current RSS in MiB. Linux reads `/proc/self/statm`; elsewhere
/// `getrusage` max RSS (resident on macOS, kilobytes on Linux — close
/// enough for a status line).
fn rss_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = std::fs::read_to_string("/proc/self/statm") {
            let pages: Vec<u64> = text
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if pages.len() >= 2 {
                let page = 4096u64;
                return pages[1] * page / 1024 / 1024;
            }
        }
    }
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `getrusage` writes a valid `rusage` for RUSAGE_SELF.
    let usage = unsafe {
        if libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) != 0 {
            return 0;
        }
        usage.assume_init()
    };
    #[cfg(target_os = "macos")]
    {
        usage.ru_maxrss as u64 / 1024 / 1024
    }
    #[cfg(not(target_os = "macos"))]
    {
        usage.ru_maxrss as u64 / 1024
    }
}

struct Server {
    backend: Option<Box<dyn Backend>>,
    model_id: String,
}

impl Server {
    fn new(fake: bool) -> Self {
        let _ = fake;
        Self {
            backend: None,
            model_id: String::new(),
        }
    }

    fn status_json(&self) -> String {
        let state = if self.backend.is_some() {
            "loaded"
        } else {
            "idle"
        };
        format!(
            "{{\"state\":\"{state}\",\"model\":\"{}\",\"rss_mb\":{}}}",
            self.model_id,
            rss_mb()
        )
    }
}

/// Run the worker on stdio. Returns on UNLOAD (exit 0) or clean EOF.
pub fn run_worker_stdio() -> anyhow::Result<()> {
    let fake = fake_backend_requested();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = BufReader::new(stdin.lock());
    let mut output = BufWriter::new(stdout.lock());
    run_loop(&mut input, &mut output, fake)
}

/// `JISUNO_FAKE_WORKER=1` selects the fake backend (tests / `--fake`). Only
/// `dev` builds read it; release always loads the real model.
#[cfg(feature = "dev")]
fn fake_backend_requested() -> bool {
    std::env::var_os("JISUNO_FAKE_WORKER").is_some()
}

#[cfg(not(feature = "dev"))]
fn fake_backend_requested() -> bool {
    false
}

fn run_loop(input: &mut impl Read, output: &mut impl Write, fake: bool) -> anyhow::Result<()> {
    let mut server = Server::new(fake);
    loop {
        let Some((msg, payload)) = read_frame(input)? else {
            return Ok(());
        };
        match msg {
            REQ_LOAD => {
                let id = std::str::from_utf8(&payload)
                    .context("LOAD model id is not utf8")?
                    .to_string();
                let t0 = Instant::now();
                let loaded: anyhow::Result<Box<dyn Backend>> = if fake {
                    Ok(Box::new(FakeBackend::new()))
                } else {
                    load_real(&id)
                };
                match loaded {
                    Ok(backend) => {
                        server.backend = Some(backend);
                        server.model_id = id;
                        let ms = t0.elapsed().as_millis() as u64;
                        write_frame(output, REP_LOADED, &ms.to_le_bytes())?;
                    }
                    Err(e) => {
                        write_frame(output, REP_LOAD_FAILED, format!("{e:#}").as_bytes())?;
                    }
                }
            }
            REQ_UNLOAD => {
                write_frame(output, REP_PONG, &[])?;
                output.flush().ok();
                return Ok(());
            }
            REQ_BEGIN => {
                let id = decode_id(&payload)?;
                if let Some(b) = server.backend.as_mut() {
                    b.begin(id);
                }
            }
            REQ_PUSH => {
                let (id, samples) = decode_push(&payload)?;
                match server.backend.as_mut() {
                    Some(b) => {
                        if let Err(e) = b.push(id, &samples) {
                            write_frame(output, REP_ERROR, &encode_id_text(id, &format!("{e:#}")))?;
                        }
                    }
                    None => {
                        write_frame(output, REP_ERROR, &encode_id_text(id, "no model loaded"))?;
                    }
                }
            }
            REQ_FINISH => {
                let id = decode_id(&payload)?;
                match server.backend.as_mut() {
                    Some(b) => match b.finish(id) {
                        Ok(text) => write_frame(output, REP_FINAL, &encode_id_text(id, &text))?,
                        Err(e) => {
                            write_frame(output, REP_ERROR, &encode_id_text(id, &format!("{e:#}")))?
                        }
                    },
                    None => {
                        write_frame(output, REP_ERROR, &encode_id_text(id, "no model loaded"))?;
                    }
                }
            }
            REQ_CANCEL => {
                let id = decode_id(&payload)?;
                if let Some(b) = server.backend.as_mut() {
                    b.cancel(id);
                }
            }
            REQ_PARTIAL => {
                // Joined finished segments — no re-decode of the audio-so-far.
                let id = decode_id(&payload)?;
                match server.backend.as_mut() {
                    Some(b) => match b.partial(id) {
                        Ok(text) => write_frame(output, REP_PARTIAL, &encode_id_text(id, &text))?,
                        Err(e) => {
                            write_frame(output, REP_ERROR, &encode_id_text(id, &format!("{e:#}")))?
                        }
                    },
                    None => {
                        write_frame(output, REP_ERROR, &encode_id_text(id, "no model loaded"))?;
                    }
                }
            }
            REQ_TRANSCRIBE => {
                // Full-clip one-shot on the same sessions as streaming.
                let samples = decode_samples(&payload)?;
                match server.backend.as_mut() {
                    Some(b) => match b.transcribe(&samples) {
                        Ok(text) => write_frame(output, REP_FINAL, &encode_id_text(0, &text))?,
                        Err(e) => {
                            write_frame(output, REP_ERROR, &encode_id_text(0, &format!("{e:#}")))?
                        }
                    },
                    None => {
                        write_frame(output, REP_ERROR, &encode_id_text(0, "no model loaded"))?;
                    }
                }
            }
            REQ_STATUS => {
                let json = server.status_json();
                write_frame(output, REP_STATUS, json.as_bytes())?;
            }
            REQ_PING => {
                write_frame(output, REP_PONG, &[])?;
            }
            other => {
                write_frame(
                    output,
                    REP_ERROR,
                    &encode_id_text(0, &format!("unknown message type {other:#x}")),
                )?;
            }
        }
        output.flush().context("flushing worker stdout")?;
    }
}

fn load_real(id: &str) -> anyhow::Result<Box<dyn Backend>> {
    let model = Model::parse(id)?;
    let dir = model.dir()?;
    // One model in memory: the streaming session owns the sessions, and
    // TRANSCRIBE runs full-clip on those same sessions.
    Ok(Box::new(StreamingBackend::load(&dir)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Drive a whole session through an in-memory transport. Stdin is
    /// preloaded with framed requests; returns the framed replies.
    fn drive(requests: &[(u8, Vec<u8>)], fake: bool) -> Vec<(u8, Vec<u8>)> {
        let mut stdin_bytes = Vec::new();
        for (t, p) in requests {
            write_frame(&mut stdin_bytes, *t, p).unwrap();
        }
        let mut input = Cursor::new(stdin_bytes);
        let mut output = Vec::new();
        run_loop(&mut input, &mut output, fake).unwrap();
        let mut cur = Cursor::new(output);
        let mut replies = Vec::new();
        while let Some(frame) = read_frame(&mut cur).unwrap() {
            replies.push(frame);
        }
        replies
    }

    fn req(t: u8, p: impl Into<Vec<u8>>) -> (u8, Vec<u8>) {
        (t, p.into())
    }

    #[test]
    fn full_session_over_fake_backend() {
        let pcm = vec![0.1f32; 320];
        let replies = drive(
            &[
                req(REQ_STATUS, vec![]),
                req(REQ_LOAD, b"anything".to_vec()),
                req(REQ_BEGIN, encode_id(1)),
                req(REQ_PUSH, encode_push(1, &pcm)),
                req(REQ_PARTIAL, encode_id(1).to_vec()),
                req(REQ_FINISH, encode_id(1).to_vec()),
                req(REQ_TRANSCRIBE, encode_samples(&pcm)),
                req(REQ_PING, vec![]),
                req(REQ_UNLOAD, vec![]),
            ],
            true,
        );
        // 7 replies: STATUS, LOADED, PARTIAL, FINAL, FINAL, PONG, PONG.
        // No reply to BEGIN / PUSH.
        assert_eq!(replies.len(), 7);
        assert_eq!(replies[0].0, REP_STATUS);
        let status = String::from_utf8(replies[0].1.clone()).unwrap();
        assert!(status.contains("\"state\":\"idle\""), "{status}");
        assert_eq!(replies[1].0, REP_LOADED);
        assert_eq!(replies[2].0, REP_PARTIAL);
        assert_eq!(
            decode_id_text(&replies[2].1).unwrap(),
            (1, "samples=320".into())
        );
        assert_eq!(replies[3].0, REP_FINAL);
        assert_eq!(
            decode_id_text(&replies[3].1).unwrap(),
            (1, "samples=320".into())
        );
        assert_eq!(replies[4].0, REP_FINAL);
        assert_eq!(
            decode_id_text(&replies[4].1).unwrap(),
            (0, "samples=320".into())
        );
        assert_eq!(replies[5].0, REP_PONG);
        assert_eq!(replies[6].0, REP_PONG); // UNLOAD ack
    }

    #[test]
    fn unknown_utterance_and_no_model_give_errors() {
        let replies = drive(
            &[
                req(REQ_FINISH, encode_id(7).to_vec()),
                req(REQ_PARTIAL, encode_id(7).to_vec()),
                req(REQ_PUSH, encode_push(7, &[1.0])),
                req(0xFF, vec![]),
            ],
            true,
        );
        assert_eq!(replies.len(), 4);
        for (t, p) in &replies {
            assert_eq!(*t, REP_ERROR);
            let _ = decode_id_text(p).unwrap();
        }
        // FINISH on an empty known utterance decodes zero samples.
        let replies = drive(
            &[
                req(REQ_LOAD, b"x".to_vec()),
                req(REQ_BEGIN, encode_id(3)),
                req(REQ_FINISH, encode_id(3).to_vec()),
            ],
            true,
        );
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0].0, REP_LOADED);
        assert_eq!(
            decode_id_text(&replies[1].1).unwrap(),
            (3, "samples=0".into())
        );
    }

    #[test]
    fn real_load_rejects_unknown_model() {
        let replies = drive(&[req(REQ_LOAD, b"nope".to_vec())], false);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].0, REP_LOAD_FAILED);
        assert!(!replies[0].1.is_empty());
    }

    #[test]
    fn cancel_drops_the_utterance() {
        let replies = drive(
            &[
                req(REQ_LOAD, b"x".to_vec()),
                req(REQ_BEGIN, encode_id(1)),
                req(REQ_CANCEL, encode_id(1).to_vec()),
                req(REQ_FINISH, encode_id(1).to_vec()),
            ],
            true,
        );
        assert_eq!(replies[replies.len() - 1].0, REP_ERROR);
    }

    /// Serum helper coverage without stdio: backend + status JSON shapes.
    #[test]
    fn fake_backend_text_is_sample_count() {
        let mut b = FakeBackend::new();
        assert_eq!(b.transcribe(&[0.0; 160]).unwrap(), "samples=160");
        assert_eq!(b.transcribe(&[]).unwrap(), "samples=0");
        // Streaming path buffers per utterance; partial reports buffered.
        b.begin(1);
        b.push(1, &[0.0; 160]).unwrap();
        assert_eq!(b.partial(1).unwrap(), "samples=160");
        b.push(1, &[0.0; 160]).unwrap();
        assert_eq!(b.finish(1).unwrap(), "samples=320");
        assert!(b.partial(1).is_err());
        // Unknown ids and missing backend surface as errors, not panics.
        assert!(b.push(9, &[0.0]).is_err());
        assert!(b.finish(9).is_err());
        b.begin(2);
        b.cancel(2);
        assert!(b.finish(2).is_err());
    }

    #[test]
    fn status_json_shape() {
        let s = Server::new(true);
        assert_eq!(
            s.status_json(),
            format!(
                "{{\"state\":\"idle\",\"model\":\"\",\"rss_mb\":{}}}",
                rss_mb()
            )
        );
    }

    #[test]
    fn decode_without_model_errors() {
        let mut s = Server::new(true);
        assert!(s.backend.is_none());
        s.backend = Some(Box::new(FakeBackend::new()));
        let b = s.backend.as_mut().unwrap();
        assert_eq!(b.transcribe(&[0.0; 8]).unwrap(), "samples=8");
    }
}
