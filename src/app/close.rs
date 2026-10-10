//! Close a remote request, then remove local tracking.

use anyhow::{Context, Result, ensure};

use crate::{
    provider::{self, Status},
    store::{Store, Tracked},
};

/// Confirm remote closure before removing local tracking. A retry after a cache
/// write failure also works for requests that have already closed or merged.
pub fn close_request(store: &dyn Store, record: &Tracked) -> Result<()> {
    let client = provider::from_remote(&record.remote)?;
    let current = client.get_pull_request(record.request.number)?;
    if matches!(current.status, Status::Open | Status::Draft) {
        let closed = client.close_pull_request(record.request.number)?;
        ensure!(
            matches!(closed.status, Status::Closed | Status::Merged),
            "Provider did not confirm closure; request remains tracked"
        );
    }
    store.remove(record).context("Request is closed or merged, but removing it from requests.json failed; press Shift+U to retry")?;
    Ok(())
}
