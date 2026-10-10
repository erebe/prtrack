//! Request persistence through a thread-safe trait; the JSON format stays private.

mod json;
mod types;

pub use types::Tracked;

use std::{fmt::Debug, path::Path, sync::Arc};

use anyhow::Result;

pub trait Store: Debug + Send + Sync {
    /// Read tracked requests; a missing cache is empty.
    fn load(&self) -> Result<Vec<Tracked>>;
    /// Verify that tracking data is valid and its directory is writable.
    fn prepare(&self) -> Result<()>;
    /// Merge updates atomically without losing concurrent writers' records.
    fn save(&self, updates: Vec<Tracked>) -> Result<Vec<Tracked>>;
    /// Save a refresh without restoring requests removed since its initial snapshot.
    fn save_refreshed(&self, before: &[Tracked], updates: Vec<Tracked>) -> Result<Vec<Tracked>>;
    /// Remove only the matching repository and request number.
    fn remove(&self, request: &Tracked) -> Result<Vec<Tracked>>;
}

impl dyn Store {
    /// Create a shareable JSON store without reading or writing files yet.
    pub fn open(dir: &Path) -> Arc<dyn Store> {
        Arc::new(json::JsonStore::new(dir))
    }
}
