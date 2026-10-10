//! File-backed settings; credentials are never serialized or included in debug output.

use serde::Deserialize;
use std::{fmt, path::PathBuf};

use crate::provider::Provider;

/// Required directory paths and optional settings loaded from the selected file.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub grove_dir: PathBuf,
    pub data_dir: PathBuf,
    /// Derived from the selected config file, not a TOML option.
    #[serde(skip)]
    pub hook_dir: PathBuf,
    pub provider: Option<Provider>,
    #[serde(default)]
    pub slack: SlackConfig,
}

/// Slack credentials are optional so commands unrelated to notifications still work.
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SlackConfig {
    pub token: Option<String>,
    pub channel_id: Option<String>,
    pub review_message: Option<String>,
    pub deploy_message: Option<String>,
}

impl fmt::Debug for SlackConfig {
    /// Keep credentials out of diagnostics, including the enclosing runtime settings.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlackConfig")
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .field("channel_id", &self.channel_id)
            .field("review_message", &self.review_message)
            .field("deploy_message", &self.deploy_message)
            .finish()
    }
}
