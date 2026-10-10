//! Review and deploy notifications; Slack configuration and transport stay private.

mod slack;
mod types;

use crate::config::SlackConfig;
use anyhow::Result;

/// Deliver requests without exposing messaging-provider details to callers.
pub trait Notify: Send {
    /// Send one notification for this URL, reporting failure without retrying delivery.
    fn request_review(&self, request_url: &str) -> Result<()>;
    /// Send a deployment request, reporting message delivery rather than deployment success.
    fn request_deploy(&self, request_url: &str) -> Result<()>;
}

impl dyn Notify {
    /// Configure Slack lazily so other commands work without Slack credentials.
    pub fn from_config(config: &SlackConfig) -> Result<Box<dyn Notify>> {
        Ok(Box::new(types::Slack::from_config(config)?))
    }
}
