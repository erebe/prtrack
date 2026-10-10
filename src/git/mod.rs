//! Local Git operations. Callers depend only on this trait, never on gix types.

mod gix_repository;
mod remote;

use std::path::Path;

use anyhow::Result;

use crate::provider::{Provider, Remote};

pub trait GitRepository {
    fn root(&self) -> &Path;
    fn source(&self) -> &Path;
    fn branch(&self) -> Option<&str>;
    /// Select a push remote and return its name and hosted-service identity.
    fn remote(&self, name: Option<&str>, provider: Option<Provider>) -> Result<(String, Remote)>;
    /// Return HEAD's subject and body; fail for an unborn branch.
    fn head_message(&self) -> Result<(String, String)>;
    fn validate_branch(&self, branch: &str) -> Result<()>;
    /// Publish an explicit branch without forcing and set its upstream on success.
    fn push(&self, remote: &str, local_branch: &str, remote_branch: &str) -> Result<()>;
}

impl dyn GitRepository {
    /// Discover a working repository using gix, including linked worktrees.
    pub fn discover(path: &Path) -> Result<Box<dyn GitRepository>> {
        Ok(Box::new(gix_repository::GixRepository::discover(path)?))
    }
}
