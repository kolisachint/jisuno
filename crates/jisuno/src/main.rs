//! jisuno entry point. The CLI and its subcommands live in [`cli`].

mod cli;
mod serve;

fn main() {
    cli::run();
}
