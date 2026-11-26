//! Determinism regression test for the ASR threading configuration.
//!
//! Greedy TDT decoding feeds each argmax token back into the predictor, so
//! one flipped token cascades into completely different text. If the
//! session threading ever becomes non-deterministic again (see
//! `build_session` in `parakeet.rs`), this test catches it.
//!
//! Ignored by default: it needs the v3 model installed and takes ~1 min.
//! Run with:
//! `cargo test --release -p jisuno-bench --test determinism -- --ignored --nocapture`
//! Skips quietly when the model is not installed (e.g. Linux CI).

use std::path::Path;

fn fixture_wav(name: &str) -> Option<std::path::PathBuf> {
    let dir = std::env::var("JISUNO_FIXTURES").unwrap_or_else(|_| "/tmp/fixtures".to_string());
    let wav = Path::new(&dir).join(format!("{name}.wav"));
    wav.exists().then_some(wav)
}

#[test]
#[ignore]
fn threaded_decode_is_deterministic() {
    let model = jisuno_asr::setup::Model::ParakeetV3Int8;
    if !model.is_ready() {
        eprintln!("model not installed; skipping determinism test");
        return;
    }
    let wav = match fixture_wav("med").or_else(|| fixture_wav("say")) {
        Some(w) => w,
        None => {
            eprintln!("no fixture wav found; skipping determinism test");
            return;
        }
    };
    let pcm = jisuno_audio::audio::load_wav_16k_mono(&wav).expect("load fixture wav");
    let mut t = jisuno_asr::transcribe::load(model).expect("load transcriber");
    let mut first: Option<String> = None;
    for i in 0..10 {
        let mut parts = Vec::new();
        t.transcribe(&pcm, &mut |s: &str| parts.push(s.to_string()))
            .expect("decode");
        let text = parts.join(" ").trim().to_string();
        println!("run {i}: {text}");
        match &first {
            Some(want) => assert_eq!(&text, want, "run {i} differs: nondeterministic decode"),
            None => first = Some(text),
        }
    }
}
