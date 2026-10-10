//! Workspace origins and private representations of Grove's state file.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Workspace identity used by tracked requests.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Workspace {
    pub name: String,
    pub path: PathBuf,
}

/// Repository, checkout, and workspace associated with a request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Origin {
    pub repository: PathBuf,
    pub checkout: PathBuf,
    pub workspace: Option<Workspace>,
}

/// Private workspace representation in Grove state.json.
#[derive(Debug, Deserialize)]
pub(super) struct GroveWorkspace {
    pub(super) name: String,
    pub(super) path: PathBuf,
    #[serde(default)]
    pub(super) repos: Vec<GroveRepo>,
}

/// Private repository representation in Grove state.json.
#[derive(Debug, Deserialize)]
pub(super) struct GroveRepo {
    pub(super) source_repo: PathBuf,
    pub(super) worktree_path: PathBuf,
}
