//! Grove workspace discovery through a trait; state and filesystem details stay private.

mod filesystem;
mod types;

pub use types::{Origin, Workspace};

use std::path::{Path, PathBuf};

use crate::git::GitRepository;
use anyhow::Result;

pub trait Grove {
    /// Reload workspace metadata without modifying Grove's state.
    /// Missing state is empty; a failed reload keeps the previous snapshot.
    fn reload(&mut self) -> Result<()>;
    /// Resolve a checkout's repository and optional workspace from the loaded snapshot.
    fn origin(&self, repo: &dyn GitRepository) -> Origin;
    /// Include registered checkouts and repositories in the standard workspace layout.
    fn checkout_paths(&self) -> Result<Vec<PathBuf>>;
}

impl dyn Grove {
    /// Create a filesystem-backed Grove handle. Call `reload` to read its state.
    pub fn new(dir: &Path) -> Box<dyn Grove> {
        Box::new(filesystem::FilesystemGrove::new(dir))
    }
}
