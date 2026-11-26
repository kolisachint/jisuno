//! `jisuno-bench` binary: thin CLI over the [`jisuno_bench`] library.
//! The `jisuno bench` subcommand will call the library directly later.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(name = "jisuno-bench", about = "ASR latency/memory/WER harness")]
struct Args {
    /// Model to use (parakeet-v3, parakeet-v2).
    #[arg(long, default_value = "parakeet-v3")]
    model: String,
    /// Directory holding fixture pairs (name.wav + name.txt).
    #[arg(long)]
    fixtures_dir: Option<PathBuf>,
    /// Timed samples per measurement.
    #[arg(long, default_value_t = 5)]
    runs: usize,
    /// Untimed warm-up decodes before measuring.
    #[arg(long, default_value_t = 1)]
    warmup: usize,
    /// Print one JSON object instead of the human table.
    #[arg(long)]
    json: bool,
    /// Exit 1 when a §6 target fails (default: exit 0 always).
    #[arg(long)]
    gate: bool,
}

fn main() {
    let args = Args::parse();
    // Accept the short aliases alongside the full registry ids.
    let model = match args.model.trim().to_lowercase().as_str() {
        "parakeet-v3" => "parakeet-tdt-0.6b-v3-int8".to_string(),
        "parakeet-v2" => "parakeet-tdt-0.6b-v2-int8".to_string(),
        _ => args.model,
    };
    let config = jisuno_bench::BenchConfig {
        model,
        fixtures_dir: args.fixtures_dir,
        runs: args.runs,
        warmup: args.warmup,
        json: args.json,
        gate: args.gate,
    };
    match jisuno_bench::run(&config) {
        Ok(report) => {
            if config.json {
                match serde_json::to_string_pretty(&report) {
                    Ok(json) => println!("{json}"),
                    Err(e) => {
                        eprintln!("error: failed to render JSON: {e:#}");
                        std::process::exit(1);
                    }
                }
            } else {
                print!("{}", jisuno_bench::format_human(&report));
            }
            if config.gate && jisuno_bench::gate_failed(&report) {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}
