//! jisuno — local, offline voice-to-text for the terminal.
//!
//! Subcommands:
//!   * `setup`      — download a model (first-run wizard)
//!   * `models`     — list installed models
//!   * `transcribe` — capture from the mic (or `--wav`) and stream text
//!   * `serve`      — load models once, then serve START/CANCEL/SHUTDOWN
//!     commands over stdin (see `serve.rs`)
//!   * `control`    — list/add/remove/validate voice-control commands for agents
//!     (see `control.rs`)
//!
//! With no subcommand, `transcribe` runs. All output the consumer cares about
//! goes to **stdout** as the line protocol in [`protocol`]; logs go to stderr.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use clap::{Args, Parser, Subcommand};
use jisuno_asr::setup::{self, Model};
use jisuno_asr::{tls, transcribe};
use jisuno_audio::{audio, mic};
use jisuno_logic::protocol;
use jisuno_logic::vad::{Vad, VadEvent};

use crate::control;

#[derive(Parser)]
#[command(name = "jisuno", version, about = "Local offline voice-to-text")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// First-run setup: download a model.
    Setup {
        #[arg(long, default_value = "parakeet-tdt-0.6b-v3-int8")]
        model: String,
        /// Extra CA certificate (PEM) to trust, in addition to the OS store.
        /// Repeatable.
        #[arg(long = "ca-cert")]
        ca_cert: Vec<PathBuf>,
        /// Disable TLS certificate verification for downloads. Dangerous —
        /// only use on a trusted network as a last resort.
        #[arg(long)]
        insecure: bool,
    },
    /// List, download, verify, or remove models.
    Models(ModelsArgs),
    /// Transcribe from the microphone (default) or a WAV file.
    Transcribe(TranscribeArgs),
    /// Run as a persistent daemon: load models once, then serve
    /// START/CANCEL/SHUTDOWN commands over stdin.
    Serve(ServeArgs),
    /// Agent CLI for voice-control-config.json: list, add, remove, validate.
    Control {
        #[command(subcommand)]
        command: control::ControlCommand,
    },
}

#[derive(Args, Debug)]
struct ModelsArgs {
    /// Omit for the model list.
    #[command(subcommand)]
    command: Option<ModelsCommand>,
}

#[derive(Subcommand, Debug)]
enum ModelsCommand {
    /// Download a model (resumable, hash-verified, atomic install).
    Download {
        /// Registry id, e.g. parakeet-tdt-0.6b-v3-int8.
        model: String,
        /// Extra trusted CA certificate (PEM). Repeatable.
        #[arg(long = "ca-cert")]
        ca_cert: Vec<PathBuf>,
        /// Disable TLS certificate verification (last resort, insecure).
        #[arg(long)]
        insecure: bool,
    },
    /// Remove an installed model (refuses while it is downloading).
    Remove {
        /// Registry id, e.g. parakeet-tdt-0.6b-v3-int8.
        model: String,
    },
    /// Verify installed files against their pinned sha256 hashes.
    Verify {
        /// Registry id, e.g. parakeet-tdt-0.6b-v3-int8.
        model: String,
    },
}

#[derive(Args, Clone)]
struct ServeArgs {
    /// Model to use (parakeet-tdt-0.6b-v3-int8, parakeet-tdt-0.6b-v2-int8).
    #[arg(
        long,
        env = "JISUNO_MODEL",
        default_value = "parakeet-tdt-0.6b-v3-int8"
    )]
    model: String,
    /// Trailing-silence timeout, in milliseconds, before auto-stop.
    #[arg(long, env = "JISUNO_SILENCE_MS", default_value_t = 600)]
    silence_ms: u64,
    /// Minimum gap between live PARTIAL transcripts while listening, in
    /// milliseconds. Set to 0 to disable partials (emit only the final).
    #[arg(long, env = "JISUNO_PARTIAL_MS", default_value_t = 400)]
    partial_ms: u64,
}

#[derive(Args, Clone)]
struct TranscribeArgs {
    /// Model to use (parakeet-tdt-0.6b-v3-int8, parakeet-tdt-0.6b-v2-int8).
    #[arg(
        long,
        env = "JISUNO_MODEL",
        default_value = "parakeet-tdt-0.6b-v3-int8"
    )]
    model: String,
    /// Trailing-silence timeout, in milliseconds, before auto-stop.
    #[arg(long, env = "JISUNO_SILENCE_MS", default_value_t = 600)]
    silence_ms: u64,
    /// Transcribe this WAV file instead of capturing from the mic.
    #[arg(long)]
    wav: Option<PathBuf>,
}

impl TranscribeArgs {
    /// Defaults used when `transcribe` runs as the implicit command, honoring
    /// the same env vars clap would.
    fn from_env() -> Self {
        let model =
            std::env::var("JISUNO_MODEL").unwrap_or_else(|_| "parakeet-tdt-0.6b-v3-int8".into());
        let silence_ms = std::env::var("JISUNO_SILENCE_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(600);
        Self {
            model,
            silence_ms,
            wav: None,
        }
    }
}

/// Parse the command line and run the selected subcommand.
/// `jisuno models` and its `download`/`remove`/`verify` subcommands.
/// Exit codes: 0 ok, 1 error (mapped by [`run`]).
fn run_models(args: ModelsArgs) -> anyhow::Result<()> {
    match args.command {
        None => setup::list(),
        Some(ModelsCommand::Download {
            model,
            ca_cert,
            insecure,
        }) => {
            let name = Model::parse(&model)?;
            let tls_opts = tls::TlsOptions {
                extra_ca_certs: ca_cert,
                insecure,
            };
            setup::download_with_progress(&name, &tls_opts, &AtomicBool::new(false))?;
            jisuno_logic::protocol::status("ready");
            Ok(())
        }
        Some(ModelsCommand::Remove { model }) => {
            let name = Model::parse(&model)?;
            setup::remove(&name)?;
            println!("{} removed", name.id());
            Ok(())
        }
        Some(ModelsCommand::Verify { model }) => {
            let name = Model::parse(&model)?;
            name.verify()?;
            println!("{} verified", name.id());
            Ok(())
        }
    }
}

pub fn run() {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Setup {
            model,
            ca_cert,
            insecure,
        }) => setup::run(
            &model,
            &tls::TlsOptions {
                extra_ca_certs: ca_cert,
                insecure,
            },
        ),
        Some(Command::Models(args)) => run_models(args),
        Some(Command::Transcribe(args)) => run_transcribe(args),
        Some(Command::Serve(args)) => {
            crate::serve::run(&args.model, args.silence_ms, args.partial_ms)
        }
        Some(Command::Control { command }) => std::process::exit(control::run(command)),
        None => run_transcribe(TranscribeArgs::from_env()),
    };

    if let Err(e) = result {
        protocol::error(&format!("{e:#}"));
        std::process::exit(1);
    }
}

fn run_transcribe(args: TranscribeArgs) -> anyhow::Result<()> {
    let model = Model::parse(&args.model)?;
    if !model.is_ready() {
        protocol::error(&format!(
            "no model found for '{}' — run: jisuno setup --model {}",
            model.id(),
            model.id()
        ));
        std::process::exit(1);
    }

    let mut transcriber = transcribe::load(model)?;
    let mut emit = |seg: &str| protocol::segment(seg);

    // File mode: handy for validating the inference path without a mic.
    if let Some(path) = &args.wav {
        protocol::status("transcribing");
        let pcm = audio::load_wav_16k_mono(path)?;
        transcriber.transcribe(&pcm, &mut emit)?;
        protocol::done();
        return Ok(());
    }

    // Mic mode: capture native-rate chunks, stop on trailing silence, then
    // resample the whole buffer to 16 kHz for transcription.
    let capture = mic::start()?;
    let input_rate = capture.input_rate;
    let mut vad = Vad::new(args.silence_ms);
    let mut raw: Vec<f32> = Vec::new();

    protocol::status("recording");
    // Loop ends on trailing-silence timeout, or when the sender drops (device
    // error / disconnect), which makes `recv` return `Err`.
    while let Ok(chunk) = capture.chunks.recv() {
        let event = vad.push(&chunk);
        raw.extend_from_slice(&chunk);
        if event == VadEvent::SilenceTimeout {
            break;
        }
    }
    capture.stop();

    if !vad.heard_speech() {
        protocol::status("transcribing");
        protocol::done();
        return Ok(());
    }

    protocol::status("transcribing");
    let pcm = audio::resample_to_16k(&raw, input_rate);
    transcriber.transcribe(&pcm, &mut emit)?;
    protocol::done();
    Ok(())
}
