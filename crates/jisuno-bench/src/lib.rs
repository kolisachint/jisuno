//! `jisuno-bench`: an ASR latency / memory / WER harness.
//!
//! Measures what §6 of `docs/design/jisuno.md` requires: cold load time,
//! warm decode for 2/5/15 s clips, simulated key-up → text for a 5 s clip,
//! process RSS around the model lifecycle, and word error rate over
//! `name.wav` + `name.txt` fixture pairs. Checked against the §6 targets
//! (final decode of a 5 s clip ≤ 100 ms, key-up → text ≤ 150 ms).
//!
//! The `jisuno bench` subcommand will call [`run`] later; the binary in this
//! crate (`src/main.rs`) is a thin CLI over the same API.

use std::path::PathBuf;
use std::time::Instant;

use serde::Serialize;

/// Sample rate every backend consumes.
const SAMPLE_RATE: u32 = 16_000;

/// §6 target: final decode of a 5 s clip, warm model.
pub const TARGET_DECODE_5S_MS: f64 = 100.0;
/// §6 target: key-up → text, warm model.
pub const TARGET_KEYUP_MS: f64 = 150.0;

/// How to run the harness. Defaults match the task spec.
#[derive(Debug, Clone)]
pub struct BenchConfig {
    /// Model id as accepted by `jisuno_asr::setup::Model::parse`.
    pub model: String,
    /// Directory holding fixture pairs (`name.wav` + `name.txt`).
    pub fixtures_dir: Option<PathBuf>,
    /// Timed samples per measurement.
    pub runs: usize,
    /// Untimed warm-up decodes before measuring.
    pub warmup: usize,
    /// Print one JSON object instead of the human table.
    pub json: bool,
    /// Exit non-zero when a §6 target fails (CI artifact mode keeps 0).
    pub gate: bool,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            model: "parakeet-v3".to_string(),
            fixtures_dir: None,
            runs: 5,
            warmup: 1,
            json: false,
            gate: false,
        }
    }
}

/// Median / p90 / min / max over timed samples, in milliseconds.
#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub n: usize,
    pub min_ms: f64,
    pub max_ms: f64,
    pub median_ms: f64,
    pub p90_ms: f64,
}

/// Summarize `samples_ms`. Returns `None` when there are no samples.
pub fn summarize(mut samples_ms: Vec<f64>) -> Option<Stats> {
    if samples_ms.is_empty() {
        return None;
    }
    samples_ms.sort_by(|a, b| a.total_cmp(b));
    let n = samples_ms.len();
    let median_ms = if n % 2 == 1 {
        samples_ms[n / 2]
    } else {
        (samples_ms[n / 2 - 1] + samples_ms[n / 2]) / 2.0
    };
    // Nearest-rank percentile: rank = ceil(p/100 * n), 1-based.
    let rank = (0.9 * n as f64).ceil() as usize;
    let p90_ms = samples_ms[rank.saturating_sub(1).min(n - 1)];
    Some(Stats {
        n,
        min_ms: samples_ms[0],
        max_ms: samples_ms[n - 1],
        median_ms,
        p90_ms,
    })
}

/// Lowercase, strip punctuation, split on whitespace. Punctuation is any
/// character that is neither alphanumeric nor whitespace.
pub fn normalize_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Word-level Levenshtein result between a reference and a hypothesis.
#[derive(Debug, Clone, Serialize)]
pub struct WerScore {
    /// `errors / ref_words`, or 0/1 for the empty-reference edge cases.
    pub wer: f64,
    pub errors: usize,
    pub ref_words: usize,
    pub hyp_words: usize,
}

/// Word error rate after [`normalize_words`]. An empty reference scores 0.0
/// against an empty hypothesis and 1.0 otherwise.
pub fn wer(reference: &str, hypothesis: &str) -> WerScore {
    let r = normalize_words(reference);
    let h = normalize_words(hypothesis);
    if r.is_empty() {
        return WerScore {
            wer: if h.is_empty() { 0.0 } else { 1.0 },
            errors: h.len(),
            ref_words: 0,
            hyp_words: h.len(),
        };
    }
    let errors = levenshtein(&r, &h);
    WerScore {
        wer: errors as f64 / r.len() as f64,
        errors,
        ref_words: r.len(),
        hyp_words: h.len(),
    }
}

/// Edit distance over word slices (substitution/insertion/deletion = 1).
fn levenshtein(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, wa) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, wb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(wa != wb);
            curr[j + 1] = sub.min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut core::ffi::c_void, bytes: usize);
}

/// Current process resident set size, in bytes. `None` where unmeasurable.
pub fn rss_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // Current resident size via mach task_info (libc only). Falls back
        // to the getrusage peak — bytes on macOS, unlike Linux KiB — when
        // the task call fails.
        if let Some(rss) = mach_resident_bytes() {
            return Some(rss);
        }
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        if ret == 0 {
            Some(usage.ru_maxrss as u64)
        } else {
            None
        }
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb * 1024);
            }
        }
        return None;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Current process resident size on macOS, via mach `task_info`.
// The harness is `libc`-only by design (no `mach2` dep), hence the
// deprecated `mach_task_self` shim.
#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn mach_resident_bytes() -> Option<u64> {
    let mut info: libc::mach_task_basic_info = unsafe { std::mem::zeroed() };
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    let ret = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as libc::task_info_t,
            &mut count,
        )
    };
    if ret == libc::KERN_SUCCESS {
        Some(info.resident_size)
    } else {
        None
    }
}

/// Machine description for the JSON artifact (CI stores it, §6.1).
#[derive(Debug, Clone, Serialize)]
pub struct MachineInfo {
    pub os: String,
    pub arch: String,
    pub cpu_brand: Option<String>,
    pub total_ram_bytes: Option<u64>,
}

#[cfg(target_os = "macos")]
fn sysctl_value(key: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/sbin/sysctl")
        .arg("-n")
        .arg(key)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Describe this machine. `cpu_brand` / `total_ram_bytes` are `None` where
/// the platform offers no easy query.
pub fn machine_info() -> MachineInfo {
    #[cfg(target_os = "macos")]
    {
        MachineInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_brand: sysctl_value("machdep.cpu.brand_string"),
            total_ram_bytes: sysctl_value("hw.memsize").and_then(|s| s.parse().ok()),
        }
    }
    #[cfg(target_os = "linux")]
    {
        MachineInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_brand: std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("model name"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, v)| v.trim().to_string())
            }),
            total_ram_bytes: std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| {
                t.lines().find_map(|l| {
                    let rest = l.strip_prefix("MemTotal:")?;
                    rest.split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                        .map(|kb| kb * 1024)
                })
            }),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        MachineInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_brand: None,
            total_ram_bytes: None,
        }
    }
}

/// One decoded fixture pair.
#[derive(Debug, Clone, Serialize)]
pub struct WerCase {
    pub name: String,
    pub wer: f64,
    pub errors: usize,
    pub ref_words: usize,
    pub hyp_words: usize,
    pub hypothesis: String,
}

/// RSS before load, with the model warm, and after dropping it.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryNumbers {
    pub rss_before_bytes: Option<u64>,
    pub rss_warm_bytes: Option<u64>,
    pub rss_unloaded_bytes: Option<u64>,
}

/// A §6 target verdict for one table row. `passed` is `None` for rows with
/// no target (informational only).
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub target: String,
    pub passed: Option<bool>,
}

/// Everything [`run`] measured, serializable as the CI JSON artifact.
#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub model: String,
    pub machine: MachineInfo,
    pub runs: usize,
    pub warmup: usize,
    pub audio_source: String,
    pub synthetic_audio: bool,
    pub cold_load_ms: Option<Stats>,
    pub cold_load_note: String,
    pub decode_2s_ms: Option<Stats>,
    pub decode_5s_ms: Option<Stats>,
    pub decode_15s_ms: Option<Stats>,
    pub keyup_5s_ms: Option<Stats>,
    pub flush_ms: Option<Stats>,
    pub flush_note: String,
    pub insert_ms: Option<Stats>,
    pub insert_note: String,
    pub memory: MemoryNumbers,
    pub wer_cases: Vec<WerCase>,
    pub wer_mean: Option<f64>,
    pub wer_note: String,
    pub checks: Vec<Check>,
}

/// True when any §6-targeted row failed.
pub fn gate_failed(report: &BenchReport) -> bool {
    report.checks.iter().any(|c| c.passed == Some(false))
}

/// Render `report` as the human table (default output).
pub fn format_human(report: &BenchReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "jisuno-bench  model={}  {}/{}  runs={} warmup={}\n",
        report.model, report.machine.os, report.machine.arch, report.runs, report.warmup
    ));
    out.push_str(&format!("audio: {}\n\n", report.audio_source));
    out.push_str(&format!(
        "{:<22} {:>9} {:>9} {:>9} {:>9}  {:<10} {}\n",
        "measurement", "median", "p90", "min", "max", "target", "result"
    ));
    let row = |name: &str, s: &Option<Stats>, target: &str, check: Option<bool>| {
        format!(
            "{:<22} {:>9} {:>9} {:>9} {:>9}  {:<10} {}\n",
            name,
            fmt_ms(s.as_ref().map(|v| v.median_ms)),
            fmt_ms(s.as_ref().map(|v| v.p90_ms)),
            fmt_ms(s.as_ref().map(|v| v.min_ms)),
            fmt_ms(s.as_ref().map(|v| v.max_ms)),
            target,
            match check {
                Some(true) => "PASS",
                Some(false) => "FAIL",
                None => "n/a",
            }
        )
    };
    let check = |name: &str| {
        report
            .checks
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| c.passed)
    };
    out.push_str(&row(
        "cold load *",
        &report.cold_load_ms,
        "—",
        check("cold load"),
    ));
    out.push_str(&row(
        "warm decode, 2 s",
        &report.decode_2s_ms,
        "—",
        check("warm decode, 2 s"),
    ));
    out.push_str(&row(
        "warm decode, 5 s",
        &report.decode_5s_ms,
        "≤ 100 ms",
        check("warm decode, 5 s"),
    ));
    out.push_str(&row(
        "warm decode, 15 s",
        &report.decode_15s_ms,
        "—",
        check("warm decode, 15 s"),
    ));
    out.push_str(&row(
        "key-up -> text (5 s)",
        &report.keyup_5s_ms,
        "≤ 150 ms",
        check("key-up -> text (5 s)"),
    ));
    out.push_str(&row(
        "flush (mic drain)",
        &report.flush_ms,
        "≤ 15 ms",
        check("flush (mic drain)"),
    ));
    out.push_str(&row(
        "insert (paste)",
        &report.insert_ms,
        "≤ 10 ms",
        check("insert (paste)"),
    ));
    out.push('\n');
    out.push_str(&format!(
        "memory RSS: before {}  warm {}  unloaded {}\n",
        fmt_mib(report.memory.rss_before_bytes),
        fmt_mib(report.memory.rss_warm_bytes),
        fmt_mib(report.memory.rss_unloaded_bytes),
    ));
    match report.wer_mean {
        Some(mean) => {
            out.push_str(&format!(
                "WER: mean {:.3} over {} case(s)\n",
                mean,
                report.wer_cases.len()
            ));
            for case in &report.wer_cases {
                out.push_str(&format!(
                    "  {:<20} WER {:.3} (errors {} / {} ref words)\n",
                    case.name, case.wer, case.errors, case.ref_words
                ));
            }
        }
        None => out.push_str("WER: n/a\n"),
    }
    out.push_str("notes:\n");
    out.push_str(&format!("  * {}\n", report.cold_load_note));
    out.push_str(&format!("  * {}\n", report.flush_note));
    out.push_str(&format!("  * {}\n", report.insert_note));
    out.push_str(&format!("  * {}\n", report.wer_note));
    out
}

fn fmt_ms(v: Option<f64>) -> String {
    match v {
        Some(ms) => format!("{ms:.1} ms"),
        None => "n/a".to_string(),
    }
}

fn fmt_mib(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) => format!("{:.0} MiB", b as f64 / (1024.0 * 1024.0)),
        None => "n/a".to_string(),
    }
}

/// Run the full harness. Never downloads: an uninstalled model is an error
/// telling the user to run `jisuno setup`.
pub fn run(config: &BenchConfig) -> anyhow::Result<BenchReport> {
    use anyhow::Context;
    use jisuno_asr::setup::Model;

    let model = Model::parse(&config.model)?;
    if !model.is_ready() {
        anyhow::bail!(
            "no model '{}' installed — run: jisuno setup --model {} (bench never downloads)",
            model.id(),
            model.id()
        );
    }
    let runs = config.runs.max(1);

    let rss_before = rss_bytes();

    // a. Cold load, timed in-process: each sample constructs a fresh
    // transcriber and drops it. Note the caveat in the report.
    let mut cold_samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let transcriber =
            jisuno_asr::transcribe::load(model).context("loading transcriber (cold load)")?;
        cold_samples.push(start.elapsed().as_secs_f64() * 1000.0);
        drop(transcriber);
    }
    let cold_load_ms = summarize(cold_samples);

    // Keep one warm transcriber for the decode measurements.
    let mut transcriber =
        jisuno_asr::transcribe::load(model).context("loading transcriber (warm)")?;
    let rss_warm = rss_bytes();

    // b. Clips: slice or loop a fixture WAV; fall back to synthetic noise.
    let (base, audio_source, synthetic_audio) = load_base_audio(config)?;
    let clip_2s = make_clip(&base, 2);
    let clip_5s = make_clip(&base, 5);
    let clip_15s = make_clip(&base, 15);

    for _ in 0..config.warmup {
        let _ = decode_to_string(&mut *transcriber, &clip_5s)?;
    }

    let mut d2 = Vec::with_capacity(runs);
    let mut d5 = Vec::with_capacity(runs);
    let mut d15 = Vec::with_capacity(runs);
    let mut keyup = Vec::with_capacity(runs);
    for _ in 0..runs {
        d2.push(decode_to_string(&mut *transcriber, &clip_2s)?.1);
        d5.push(decode_to_string(&mut *transcriber, &clip_5s)?.1);
        d15.push(decode_to_string(&mut *transcriber, &clip_15s)?.1);
        // c. Simulated key-up → text: all audio is already buffered, so time
        // the final decode plus post-process (segment join + trim), exactly
        // like serve.rs builds its FINAL. Mic flush and app insert are
        // platform work outside this process; recorded as null with a note.
        keyup.push(decode_to_string(&mut *transcriber, &clip_5s)?.1);
    }
    let decode_2s_ms = summarize(d2);
    let decode_5s_ms = summarize(d5);
    let decode_15s_ms = summarize(d15);
    let keyup_5s_ms = summarize(keyup);

    // e. WER over fixture pairs, decoded with the same warm transcriber.
    let (wer_cases, wer_mean, wer_note) = measure_wer(config, &mut *transcriber)?;

    drop(transcriber);
    // Experiment 5: give freed allocator pages back to the OS before
    // measuring "unloaded", so the number reflects what *can* be returned.
    // The product needs the same call on its unload path (see PERF.md).
    #[cfg(target_os = "macos")]
    unsafe {
        malloc_zone_pressure_relief(core::ptr::null_mut(), 0);
    }
    let rss_unloaded = rss_bytes();

    let pass_5s = decode_5s_ms
        .as_ref()
        .is_some_and(|s| s.median_ms <= TARGET_DECODE_5S_MS);
    let pass_keyup = keyup_5s_ms
        .as_ref()
        .is_some_and(|s| s.median_ms <= TARGET_KEYUP_MS);
    let checks = vec![
        Check {
            name: "cold load".to_string(),
            target: "—".to_string(),
            passed: None,
        },
        Check {
            name: "warm decode, 2 s".to_string(),
            target: "—".to_string(),
            passed: None,
        },
        Check {
            name: "warm decode, 5 s".to_string(),
            target: "≤ 100 ms (median)".to_string(),
            passed: Some(pass_5s),
        },
        Check {
            name: "warm decode, 15 s".to_string(),
            target: "—".to_string(),
            passed: None,
        },
        Check {
            name: "key-up -> text (5 s)".to_string(),
            target: "≤ 150 ms (median)".to_string(),
            passed: Some(pass_keyup),
        },
        Check {
            name: "flush (mic drain)".to_string(),
            target: "≤ 15 ms".to_string(),
            passed: None,
        },
        Check {
            name: "insert (paste)".to_string(),
            target: "≤ 10 ms".to_string(),
            passed: None,
        },
    ];

    Ok(BenchReport {
        model: model.id().to_string(),
        machine: machine_info(),
        runs,
        warmup: config.warmup,
        audio_source,
        synthetic_audio,
        cold_load_ms,
        cold_load_note: "cold load is measured in-process (page cache warms after \
            the first sample); a fresh-process load may be slower."
            .to_string(),
        decode_2s_ms,
        decode_5s_ms,
        decode_15s_ms,
        keyup_5s_ms,
        flush_ms: None,
        flush_note: "flush (≤ 15 ms target) is platform work — draining the last \
            mic chunk in the app — and is not measured by this harness."
            .to_string(),
        insert_ms: None,
        insert_note: "insert (≤ 10 ms target) is platform work — paste plus \
            clipboard restore in the app — and is not measured by this harness."
            .to_string(),
        memory: MemoryNumbers {
            rss_before_bytes: rss_before,
            rss_warm_bytes: rss_warm,
            rss_unloaded_bytes: rss_unloaded,
        },
        wer_cases,
        wer_mean,
        wer_note,
        checks,
    })
}

/// Decode `pcm` the way `serve.rs` builds a FINAL: stream segments, join
/// with spaces, trim. Returns the text and the elapsed milliseconds.
fn decode_to_string(
    transcriber: &mut dyn jisuno_asr::transcribe::Transcriber,
    pcm: &[f32],
) -> anyhow::Result<(String, f64)> {
    let start = Instant::now();
    let mut parts: Vec<String> = Vec::new();
    transcriber.transcribe(pcm, &mut |seg: &str| parts.push(seg.to_string()))?;
    let text = parts.join(" ").trim().to_string();
    Ok((text, start.elapsed().as_secs_f64() * 1000.0))
}

/// First sorted `*.wav` in `fixtures_dir`, loaded as 16 kHz mono; otherwise
/// deterministic speech-free noise (decode time is still meaningful).
fn load_base_audio(config: &BenchConfig) -> anyhow::Result<(Vec<f32>, String, bool)> {
    if let Some(dir) = &config.fixtures_dir {
        if let Some(wav) = first_wav(dir)? {
            let pcm = jisuno_audio::audio::load_wav_16k_mono(&wav)?;
            if pcm.is_empty() {
                anyhow::bail!("fixture {} decoded to 0 samples", wav.display());
            }
            let secs = pcm.len() as f64 / SAMPLE_RATE as f64;
            return Ok((
                pcm,
                format!(
                    "fixture {} ({:.1}s base; 2/5/15 s clips sliced/looped)",
                    wav.display(),
                    secs
                ),
                false,
            ));
        }
    }
    // No fixture: low-amplitude deterministic noise via xorshift32.
    let mut state: u32 = 0x243F_6A88;
    let noise: Vec<f32> = (0..15 * SAMPLE_RATE as usize)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) * 0.02
        })
        .collect();
    Ok((
        noise,
        "synthetic speech-free noise (no fixture WAV found; decode time is still meaningful)"
            .to_string(),
        true,
    ))
}

/// Earliest `*.wav` (sorted by file name) directly inside `dir`.
fn first_wav(dir: &std::path::Path) -> anyhow::Result<Option<PathBuf>> {
    let mut wavs: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
        {
            wavs.push(path);
        }
    }
    wavs.sort();
    Ok(wavs.into_iter().next())
}

/// Build a `secs`-long 16 kHz clip: slice when the base is longer, loop when
/// shorter.
fn make_clip(base: &[f32], secs: u64) -> Vec<f32> {
    let want = secs as usize * SAMPLE_RATE as usize;
    if base.len() >= want {
        return base[..want].to_vec();
    }
    let mut clip = Vec::with_capacity(want);
    while clip.len() < want {
        let take = (want - clip.len()).min(base.len());
        clip.extend_from_slice(&base[..take]);
    }
    clip
}

/// Decode every `name.wav` with a sibling `name.txt` and score WER.
fn measure_wer(
    config: &BenchConfig,
    transcriber: &mut dyn jisuno_asr::transcribe::Transcriber,
) -> anyhow::Result<(Vec<WerCase>, Option<f64>, String)> {
    let Some(dir) = &config.fixtures_dir else {
        return Ok((
            Vec::new(),
            None,
            "no --fixtures-dir given; WER skipped.".to_string(),
        ));
    };
    let mut wavs: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
        {
            wavs.push(path);
        }
    }
    wavs.sort();
    let mut cases = Vec::new();
    for wav in &wavs {
        let txt = wav.with_extension("txt");
        if !txt.exists() {
            continue;
        }
        let reference = std::fs::read_to_string(&txt)?;
        let pcm = jisuno_audio::audio::load_wav_16k_mono(wav)?;
        let (hypothesis, _) = decode_to_string(transcriber, &pcm)?;
        let score = wer(&reference, &hypothesis);
        let name = wav
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| wav.display().to_string());
        cases.push(WerCase {
            name,
            wer: score.wer,
            errors: score.errors,
            ref_words: score.ref_words,
            hyp_words: score.hyp_words,
            hypothesis,
        });
    }
    if cases.is_empty() {
        return Ok((
            Vec::new(),
            None,
            format!(
                "no name.wav + name.txt pairs in {}; WER skipped.",
                dir.display()
            ),
        ));
    }
    let mean = cases.iter().map(|c| c.wer).sum::<f64>() / cases.len() as f64;
    let n = cases.len();
    Ok((cases, Some(mean), format!("{n} pair(s) decoded.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_scores_zero() {
        let s = wer("hello world", "hello world");
        assert_eq!(s.wer, 0.0);
        assert_eq!(s.errors, 0);
        assert_eq!(s.ref_words, 2);
    }

    #[test]
    fn normalization_ignores_case_and_punctuation() {
        let s = wer("Hello, WORLD!", "hello world");
        assert_eq!(s.wer, 0.0);
    }

    #[test]
    fn one_substitution() {
        let s = wer("the cat sat", "the dog sat");
        assert_eq!(s.errors, 1);
        assert!((s.wer - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn one_insertion() {
        let s = wer("the cat", "the big cat");
        assert_eq!(s.errors, 1);
        assert!((s.wer - 0.5).abs() < 1e-9);
    }

    #[test]
    fn one_deletion() {
        let s = wer("the big cat", "the cat");
        assert_eq!(s.errors, 1);
        assert!((s.wer - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn empty_reference() {
        let both_empty = wer("", "");
        assert_eq!(both_empty.wer, 0.0);
        let hyp_only = wer("", "hello");
        assert_eq!(hyp_only.wer, 1.0);
        assert_eq!(hyp_only.hyp_words, 1);
    }

    #[test]
    fn empty_hypothesis_counts_all_ref_words() {
        let s = wer("a b c", "");
        assert_eq!(s.errors, 3);
        assert_eq!(s.wer, 1.0);
    }

    #[test]
    fn stats_median_and_p90() {
        let s = summarize(vec![3.0, 1.0, 2.0, 4.0]).expect("non-empty");
        assert_eq!(s.n, 4);
        assert_eq!(s.min_ms, 1.0);
        assert_eq!(s.max_ms, 4.0);
        assert!((s.median_ms - 2.5).abs() < 1e-9);
        // Nearest-rank p90 over 4 samples: ceil(0.9*4) = 4th value.
        assert_eq!(s.p90_ms, 4.0);
    }

    #[test]
    fn stats_p90_over_ten_samples() {
        let v: Vec<f64> = (1..=10).map(|x| x as f64).collect();
        let s = summarize(v).expect("non-empty");
        assert_eq!(s.median_ms, 5.5);
        assert_eq!(s.p90_ms, 9.0);
    }

    #[test]
    fn stats_empty_is_none() {
        assert!(summarize(Vec::new()).is_none());
    }

    #[test]
    fn json_shape_has_machine_model_and_numbers() {
        let report = BenchReport {
            model: "parakeet-v3".to_string(),
            machine: MachineInfo {
                os: "macos".to_string(),
                arch: "aarch64".to_string(),
                cpu_brand: None,
                total_ram_bytes: None,
            },
            runs: 5,
            warmup: 1,
            audio_source: "synthetic".to_string(),
            synthetic_audio: true,
            cold_load_ms: summarize(vec![10.0]),
            cold_load_note: String::new(),
            decode_2s_ms: summarize(vec![20.0]),
            decode_5s_ms: summarize(vec![50.0]),
            decode_15s_ms: summarize(vec![150.0]),
            keyup_5s_ms: summarize(vec![60.0]),
            flush_ms: None,
            flush_note: String::new(),
            insert_ms: None,
            insert_note: String::new(),
            memory: MemoryNumbers {
                rss_before_bytes: Some(1),
                rss_warm_bytes: Some(2),
                rss_unloaded_bytes: Some(3),
            },
            wer_cases: Vec::new(),
            wer_mean: None,
            wer_note: String::new(),
            checks: vec![Check {
                name: "warm decode, 5 s".to_string(),
                target: "≤ 100 ms (median)".to_string(),
                passed: Some(true),
            }],
        };
        let v = serde_json::to_value(&report).expect("serializable");
        assert_eq!(v["model"], "parakeet-v3");
        assert_eq!(v["machine"]["os"], "macos");
        assert_eq!(v["machine"]["cpu_brand"], serde_json::Value::Null);
        assert_eq!(v["decode_5s_ms"]["median_ms"], 50.0);
        assert_eq!(v["flush_ms"], serde_json::Value::Null);
        assert_eq!(v["checks"][0]["passed"], true);
        assert!(!gate_failed(&report));
        let failing = BenchReport {
            checks: vec![Check {
                name: "warm decode, 5 s".to_string(),
                target: "≤ 100 ms (median)".to_string(),
                passed: Some(false),
            }],
            ..report
        };
        assert!(gate_failed(&failing));
    }

    #[test]
    fn clips_slice_and_loop() {
        let base: Vec<f32> = (0..32_000).map(|i| i as f32).collect();
        assert_eq!(make_clip(&base, 1).len(), 16_000);
        let short = vec![1.0, 2.0];
        let looped = make_clip(&short, 1);
        assert_eq!(looped.len(), 16_000);
        assert_eq!(looped[0], 1.0);
        assert_eq!(looped[2], 1.0);
    }
}
