//! Client lifecycle tests against the fake example worker
//! (`examples/asr_worker.rs --fake`: text = sample count, no model).
//!
//! The example binary is built by `cargo test` together with the test
//! targets; when it is missing (e.g. a filtered run before any build) the
//! helper below builds it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use jisuno_asr::worker::{is_crashed, AsrWorker};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Path to `target/{debug,release}/examples/asr_worker`.
///
/// Walks up from the test binary to the first `debug`/`release` ancestor
/// (the harness may place test binaries in odd subdirs), then looks in
/// its `examples/` child.
fn example_bin() -> PathBuf {
    let exe = std::env::current_exe().expect("current test binary");
    let profile = exe
        .ancestors()
        .find(|a| {
            a.file_name()
                .map(|n| n == "debug" || n == "release")
                .unwrap_or(false)
        })
        .expect("a debug/release ancestor of the test binary")
        .to_path_buf();
    #[cfg(windows)]
    let bin = profile.join("examples").join("asr_worker.exe");
    #[cfg(not(windows))]
    let bin = profile.join("examples").join("asr_worker");
    if !bin.exists() {
        let status = std::process::Command::new("cargo")
            .args(["build", "--example", "asr_worker", "-p", "jisuno-asr"])
            .status()
            .expect("cargo build --example");
        assert!(status.success(), "building the fake worker example");
    }
    assert!(bin.exists(), "fake worker binary at {}", bin.display());
    bin
}

fn spawn_fake() -> AsrWorker {
    AsrWorker::spawn(&example_bin(), &["--fake"]).expect("spawn fake worker")
}

#[test]
fn lifecycle_against_fake_worker() {
    let w = spawn_fake();
    assert!(w.is_alive());
    w.ping(TIMEOUT).expect("ping");

    let status = w.status(TIMEOUT).expect("status");
    assert!(status.contains("\"state\":\"idle\""), "{status}");

    let ms = w.load("any-model-id", TIMEOUT).expect("load");
    assert!(ms < 60_000, "fake load is instant, got {ms} ms");

    let status = w.status(TIMEOUT).expect("status");
    assert!(status.contains("\"state\":\"loaded\""), "{status}");
    assert!(status.contains("\"rss_mb\""), "{status}");

    // Utterance path: two pushes, partial mid-way, finish at the end.
    w.begin(1).expect("begin");
    w.push(1, &[0.1; 1600]).expect("push");
    assert_eq!(w.partial(1, TIMEOUT).expect("partial"), "samples=1600");
    w.push(1, &[0.1; 1600]).expect("push");
    assert_eq!(w.finish(1, TIMEOUT).expect("finish"), "samples=3200");

    // One-shot path.
    assert_eq!(
        w.transcribe(&[0.2; 800], TIMEOUT).expect("transcribe"),
        "samples=800"
    );

    // Unknown ids fail with worker errors, not crashes.
    assert!(!is_crashed(&w.finish(99, TIMEOUT).unwrap_err()));
    assert!(!is_crashed(&w.partial(99, TIMEOUT).unwrap_err()));

    // Cancel drops the buffer; a later finish reports unknown utterance.
    w.begin(2).expect("begin");
    w.push(2, &[0.1; 100]).expect("push");
    w.cancel(2).expect("cancel");
    assert!(!is_crashed(&w.finish(2, TIMEOUT).unwrap_err()));

    // Unload is prompt and the child is gone afterwards.
    let t0 = Instant::now();
    w.unload().expect("unload");
    assert!(t0.elapsed() < Duration::from_secs(5), "unload is prompt");
    assert!(!w.is_alive());
    assert!(is_crashed(&w.ping(TIMEOUT).unwrap_err()));
}

#[test]
fn real_worker_rejects_unknown_model_and_stays_alive() {
    // No `--fake`: LOAD goes through Model::parse, which fails before any
    // ONNX work, so no model install is needed.
    let w = AsrWorker::spawn(&example_bin(), &[]).expect("spawn worker");
    let err = w.load("nope", TIMEOUT).unwrap_err().to_string();
    assert!(err.contains("unknown model"), "{err}");
    assert!(w.is_alive());
    w.ping(TIMEOUT).expect("still alive after failed load");
    w.unload().expect("unload");
}

#[test]
#[cfg(unix)]
fn crash_mid_utterance_fails_pending_calls() {
    let w = spawn_fake();
    w.load("x", TIMEOUT).expect("load");
    w.begin(7).expect("begin");
    w.push(7, &[0.1; 1600]).expect("push");
    let pid = w.pid();
    // SIGKILL the child mid-utterance: no UNLOAD, no replies.
    let r = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    assert_eq!(r, 0);
    // The pending finish gets no FINAL/ERROR — the reader sees EOF and
    // every waiter fails with WorkerCrashed.
    let err = w.finish(7, TIMEOUT).unwrap_err();
    assert!(is_crashed(&err), "want WorkerCrashed, got {err:#}");
    assert!(!w.is_alive());
    assert!(is_crashed(&w.ping(TIMEOUT).unwrap_err()));
}

/// Real-model parity and overhead (ignored: needs the installed model).
///
/// Run: `cargo test --release -p jisuno-asr --test worker -- --ignored
/// --nocapture`. Skips quietly without the model or fixtures. Compares a
/// 5 s clip decoded in-process vs via the helper child (one PUSH +
/// FINISH), and cold start (spawn + LOAD) vs in-process load.
#[test]
#[ignore = "needs the installed Parakeet model"]
fn helper_process_matches_in_process() {
    use jisuno_asr::setup::Model;
    use std::path::Path;

    let model = Model::parse("parakeet-tdt-0.6b-v3-int8").expect("known model");
    if !model.is_ready() {
        eprintln!("worker parity: model not installed, skipping");
        return;
    }
    // 5 s clip: first fixture sliced/looped to exactly 80k samples, else a
    // 440 Hz sine (equality of the two paths is what matters, not WER).
    let clip = five_second_clip();
    assert_eq!(clip.len(), 80_000);

    // In-process baseline.
    let t0 = Instant::now();
    let mut direct = jisuno_asr::transcribe::load(model).expect("in-process load");
    let direct_load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let t0 = Instant::now();
    let mut direct_parts = Vec::new();
    direct
        .transcribe(&clip, &mut |s: &str| direct_parts.push(s.to_string()))
        .expect("in-process decode");
    let direct_text = direct_parts.join(" ").trim().to_string();
    let direct_ms = t0.elapsed().as_secs_f64() * 1000.0;
    drop(direct);

    // Helper child: cold start, then one PUSH + FINISH of the same clip.
    let t0 = Instant::now();
    let w = AsrWorker::spawn(&example_bin(), &[]).expect("spawn real worker");
    let spawn_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let t0 = Instant::now();
    let load_ms = w
        .load(model.id(), Duration::from_secs(300))
        .expect("worker load");
    let load_wall_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let id = 11;
    w.begin(id).expect("begin");
    w.push(id, &clip).expect("push");
    let t0 = Instant::now();
    let worker_text = w.finish(id, Duration::from_secs(300)).expect("finish");
    let finish_ms = t0.elapsed().as_secs_f64() * 1000.0;
    // PARTIAL path decodes the same audio: spot-check parity too.
    w.begin(id).expect("begin");
    w.push(id, &clip).expect("push");
    let partial_text = w.partial(id, Duration::from_secs(300)).expect("partial");
    w.cancel(id).expect("cancel");
    assert_eq!(partial_text, worker_text, "partial must match finish");
    let t0 = Instant::now();
    w.unload().expect("unload");
    let unload_ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert!(!w.is_alive());

    assert_eq!(worker_text, direct_text, "helper must decode identically");
    eprintln!(
        "worker parity 5 s: in-process load {direct_load_ms:.0} ms, decode {direct_ms:.1} ms"
    );
    eprintln!(
        "worker parity 5 s: spawn {spawn_ms:.0} ms + LOAD {load_ms:.0} ms (wall {load_wall_ms:.0} ms), finish {finish_ms:.1} ms (overhead {:.1} ms), unload {unload_ms:.1} ms",
        finish_ms - direct_ms,
    );
    eprintln!("worker parity 5 s text: {worker_text:?}");
    let _ = Path::new("");
}

/// Exactly 80_000 samples (5 s at 16 kHz) from the first fixture WAV, or a
/// 440 Hz sine when no fixtures are present.
fn five_second_clip() -> Vec<f32> {
    let dir = std::env::var("JISUNO_FIXTURES")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/fixtures"));
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut wavs: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "wav"))
            .collect();
        wavs.sort();
        if let Some(wav) = wavs.first() {
            if let Ok(pcm) = jisuno_audio::audio::load_wav_16k_mono(wav) {
                if !pcm.is_empty() {
                    let mut clip = Vec::with_capacity(80_000);
                    while clip.len() < 80_000 {
                        let take = (80_000 - clip.len()).min(pcm.len());
                        clip.extend_from_slice(&pcm[..take]);
                    }
                    return clip;
                }
            }
        }
    }
    (0..80_000)
        .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 0.1)
        .collect()
}

#[test]
#[cfg(unix)]
fn drop_kills_the_child() {
    let pid = {
        let w = spawn_fake();
        let pid = w.pid();
        w.ping(TIMEOUT).expect("ping");
        pid
        // `w` drops here: no UNLOAD, the child must still die.
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // kill(pid, 0): ESRCH means the process is gone.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        let gone = r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone || Instant::now() >= deadline {
            assert!(gone, "child {pid} still alive after drop");
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
