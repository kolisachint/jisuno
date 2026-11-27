//! jisuno entry point. The CLI and its subcommands live in [`cli`]; the menu
//! bar app (macOS, `app` feature) lives in [`app`].

#[cfg(all(feature = "app", target_os = "macos"))]
mod app;
mod cli;
mod control;
// The server half (Response constructors, Request parsing) is macOS-app
// only; the CLI uses the client half. Silence those on other builds.
#[cfg_attr(not(all(feature = "app", target_os = "macos")), allow(dead_code))]
mod ipc;
// App-support helpers: the menu bar app (its only non-test caller) is
// macOS-only, so allow dead code on builds without it.
#[cfg_attr(not(all(feature = "app", target_os = "macos")), allow(dead_code))]
mod runtime;
mod serve;
// Like `runtime`: only the macOS app (plus tests) uses this.
#[cfg_attr(not(all(feature = "app", target_os = "macos")), allow(dead_code))]
mod watch;

fn main() {
    cli::run();
}
