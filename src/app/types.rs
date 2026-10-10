//! Runtime settings and data shared by application workflows.

use anyhow::Result;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use crate::{
    config::{Config, SlackConfig},
    grove::Origin,
    provider::{Provider, Remote},
    store::{Store, Tracked},
};

/// Runtime settings using file-defined paths and an optional command-line provider override.
#[derive(Debug, Clone)]
pub struct Settings {
    pub cwd: PathBuf,
    pub grove_dir: PathBuf,
    pub hook_dir: PathBuf,
    pub store: Arc<dyn Store>,
    pub provider: Option<Provider>,
    pub slack: SlackConfig,
}

impl Settings {
    /// Load paths from configuration before opening the cache; CLI may override the provider.
    pub(super) fn from_config(path: Option<&Path>, provider: Option<Provider>) -> Result<Self> {
        let config = Config::load(path)?;
        Ok(Self {
            cwd: std::env::current_dir()?,
            grove_dir: config.grove_dir,
            hook_dir: config.hook_dir,
            store: <dyn Store>::open(&config.data_dir),
            provider: provider.or(config.provider),
            slack: config.slack,
        })
    }
}

/// Refresh results passed from workers to the CLI or TUI.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub records: Vec<Tracked>,
    pub warnings: Vec<String>,
}

/// A discovered checkout used to associate requests with local origins.
#[derive(Clone)]
pub(super) struct Checkout {
    pub(super) branch: Option<String>,
    pub(super) origin: Origin,
}

/// Repository-local state owned by one refresh task.
#[derive(Default)]
pub(super) struct RefreshRepository {
    pub(super) checkouts: Vec<Checkout>,
    pub(super) records: Vec<Tracked>,
}

/// Repository work grouped by remote to avoid duplicate provider queries.
pub(super) type RefreshRepositories = HashMap<Remote, RefreshRepository>;

/// Cache both successful and failed identity lookups across repository workers.
pub(super) type RefreshIdentity = OnceLock<std::result::Result<String, String>>;
