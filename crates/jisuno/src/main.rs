//! jisuno entry point. The CLI and its subcommands live in [`cli`].

mod cli;
mod control;
mod serve;

fn main() {
    cli::run();
}
