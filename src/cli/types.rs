//! Clap command-line options and command arguments.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

use crate::provider::Provider;

/// Global command-line options and command selection.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// TOML configuration file [default: platform config directory/pr/config.toml]
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Provider override for self-hosted instances (or set git config pr.provider)
    #[arg(long, global = true, value_enum)]
    pub provider: Option<Provider>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize local configuration
    #[command(subcommand)]
    Init(Init),
    /// Push a branch and create or reuse a pull/merge request
    Push(Push),
    /// Track an existing pull/merge request by URL
    Add(Add),
    /// Show requests across tracked repositories and Grove workspaces
    List(List),
}

/// Initialization tasks that work before application configuration is available.
#[derive(Debug, Subcommand)]
pub enum Init {
    /// Create an example config file and its directory, preserving any existing file
    Config,
}

/// Arguments for tracking an existing request.
#[derive(Debug, Args)]
pub struct Add {
    /// GitHub pull request or GitLab merge request URL
    pub url: String,
}

/// Arguments for publishing a branch and tracking its request.
#[derive(Debug, Args)]
pub struct Push {
    /// Git remote (defaults to push/upstream configuration, then origin)
    #[arg(long)]
    pub remote: Option<String>,
    /// Remote branch to push to (defaults to the current branch name)
    #[arg(long, value_name = "BRANCH")]
    pub target: Option<String>,
    /// PR/MR base branch (defaults to the existing request's base or repository default)
    #[arg(long)]
    pub base: Option<String>,
    /// Request title (defaults to the latest commit's subject)
    #[arg(short, long)]
    pub title: Option<String>,
    /// Request description (defaults to the latest commit's body)
    #[arg(short, long)]
    pub body: Option<String>,
    /// Create a draft request
    #[arg(long)]
    pub draft: bool,
}

/// Arguments for listing tracked requests.
#[derive(Clone, Debug, Default, Args)]
pub struct List {
    /// Filter by owner/repository or host/owner/repository
    #[arg(long)]
    pub repo: Option<String>,
    /// Filter by Grove workspace name
    #[arg(short = 'w', long)]
    pub workspace: Option<String>,
    /// Display cached requests without contacting GitHub or GitLab
    #[arg(long)]
    pub offline: bool,
    /// Print JSON instead of launching the terminal interface
    #[arg(long)]
    pub json: bool,
}
