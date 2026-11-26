//! jisuno entry point. The CLI and its subcommands live in [`cli`]; the menu
//! bar app (macOS, `app` feature) lives in [`app`].

#[cfg(all(feature = "app", target_os = "macos"))]
mod app;
mod cli;
mod control;
mod serve;

fn main() {
    cli::run();
}
