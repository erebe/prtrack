mod app;
mod cli;
mod config;
mod git;
mod grove;
mod notify;
mod output;
mod process;
mod provider;
mod store;
mod tui;

use clap::Parser;

fn main() {
    if let Err(error) = app::run(cli::Cli::parse()) {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}
