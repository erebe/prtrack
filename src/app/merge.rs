use anyhow::{Context, Result, ensure};

use super::now;
use crate::{
    provider::{self, Status},
    store::{Store, Tracked},
};

/// Merge remotely, then retain the confirmed merged request in the cache.
pub fn merge_request(store: &dyn Store, record: &Tracked) -> Result<Tracked> {
    let client = provider::from_remote(&record.remote)?;
    let request = client.merge_pull_request(record.request.number)?;
    ensure!(
        request.status == Status::Merged,
        "Provider did not confirm merge"
    );
    let mut merged = record.clone();
    merged.request = request;
    // Preserve cached metadata when the merge response does not include it.
    merged.request.last_comment_at = record.request.last_comment_at;
    merged.request.unresolved_threads = merged
        .request
        .unresolved_threads
        .or(record.request.unresolved_threads);
    merged.request.pipeline_status = record.request.pipeline_status;
    merged.checked_at = now();
    merged.refresh_error = None;
    store
        .save_refreshed(std::slice::from_ref(record), vec![merged.clone()])
        .context("Request was merged, but updating requests.json failed; press m to retry")?;
    Ok(merged)
}
