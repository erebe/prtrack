//! Hosted Git services. Concrete providers and API details stay private.

mod api;
mod github;
mod gitlab;
mod pipeline;
mod request_url;
mod types;

pub use request_url::parse_request_url;
pub use types::{PipelineStatus, Provider, Remote, Request, Status};

use anyhow::Result;

/// Common operations supported by every hosted Git provider.
pub trait GitProvider: Send + Sync {
    fn username(&self) -> Result<String>;
    fn default_branch(&self) -> Result<String>;
    /// Discover open requests authored by or assigned to this user, without duplicates.
    /// Both relationship filters are applied by the provider API.
    fn discover_requests(&self, username: &str) -> Result<Vec<Request>>;
    /// List requests from this repository, optionally filtering author and branch.
    /// Author filtering is performed by the provider API.
    fn list_pull_requests(
        &self,
        author: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Vec<Request>>;
    fn get_pull_request(&self, number: u64) -> Result<Request>;
    /// Refresh known requests in batches, returning one independent result per number.
    /// Called within the shared, bounded refresh pool.
    fn get_pull_requests(&self, numbers: &[u64]) -> Vec<(u64, Result<Request>)>;
    /// Fetch aggregate CI results only for selected requests, in bounded batches.
    /// An absent pipeline is successful; lookup errors remain separate from CI failures.
    fn pipeline_statuses(&self, numbers: &[u64]) -> Vec<(u64, Result<PipelineStatus>)>;
    /// Close a pull/merge request and return its confirmed remote state.
    fn close_pull_request(&self, number: u64) -> Result<Request>;
    /// Merge a pull/merge request and return its confirmed remote state.
    fn merge_pull_request(&self, number: u64) -> Result<Request>;
    fn create_pull_request(
        &self,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<Request>;
}

/// Create the GitHub or GitLab provider selected by the remote, resolving any
/// repository rename/transfer before further API calls. Uses gh/glab credentials.
pub fn from_remote(remote: &Remote) -> Result<Box<dyn GitProvider>> {
    match remote.provider {
        Provider::Github => Ok(Box::new(github::Github::new(remote)?)),
        Provider::Gitlab => Ok(Box::new(gitlab::Gitlab::new(remote)?)),
    }
}
