//! Application workflows; adapters are accessed through their public interfaces.

mod add;
mod close;
mod list;
mod merge;
mod push;
mod refresh;
mod review;
mod types;

pub use close::close_request;
pub use list::filtered;
pub use merge::merge_request;
pub use refresh::refresh;
pub use review::prepare_review;
pub use types::{Settings, Snapshot};

use anyhow::Result;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    cli::{Cli, Command, Init},
    config::Config,
};

/// Dispatch initialization without loading a config; other workflows require its settings.
pub fn run(cli: Cli) -> Result<()> {
    let settings = || Settings::from_config(cli.config.as_deref(), cli.provider);
    match cli.command {
        Command::Init(Init::Config) => {
            let path = Config::init(cli.config.as_deref())?;
            println!("Created config at {}", path.display());
            Ok(())
        }
        Command::Push(args) => push::run(&settings()?, args),
        Command::Add(args) => add::run(&settings()?, args),
        Command::List(args) => list::run(settings()?, args),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
