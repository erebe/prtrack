//! Tracked records and the private versioned persistence envelope.

use serde::{Deserialize, Serialize};

use crate::{
    grove::Origin,
    provider::{Remote, Request, Status},
};

/// A request plus local origins and refresh metadata.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Tracked {
    pub remote: Remote,
    pub request: Request,
    pub origins: Vec<Origin>,
    pub checked_at: u64,
    pub refresh_error: Option<String>,
    /// Pipeline failures must not cause closed/merged request details to be refetched.
    #[serde(default)]
    pub pipeline_error: Option<String>,
}

impl Tracked {
    pub fn same_request(&self, other: &Self) -> bool {
        self.remote == other.remote && self.request.number == other.request.number
    }

    pub fn merge_into(records: &mut Vec<Tracked>, mut update: Tracked) {
        if let Some(existing) = records.iter_mut().find(|r| r.same_request(&update)) {
            // Creation and mutation responses can omit discussion metadata.
            update.request.unresolved_threads = update
                .request
                .unresolved_threads
                .or(existing.request.unresolved_threads);
            // Ordinary request responses do not fetch CI. Preserve its independent cache.
            if update.request.pipeline_status.is_none() && update.pipeline_error.is_none() {
                update.request.pipeline_status = existing.request.pipeline_status;
                update.pipeline_error = existing.pipeline_error.clone();
            }
            for origin in &existing.origins {
                if !update.origins.contains(origin) {
                    update.origins.push(origin.clone());
                }
            }
            // Merging is irreversible: a refresh fetched before the merge must
            // not restore an open state, even if it finished later.
            let newly_merged = update.request.status == Status::Merged
                && existing.request.status != Status::Merged;
            if newly_merged
                || (update.checked_at >= existing.checked_at
                    && (existing.request.status != Status::Merged
                        || update.request.status == Status::Merged))
            {
                *existing = update;
            } else {
                existing.origins = update.origins;
            }
        } else {
            records.push(update);
        }
    }

    pub fn sort(records: &mut [Tracked]) {
        records.sort_by(|a, b| {
            (&a.remote.host, &a.remote.project, a.request.number).cmp(&(
                &b.remote.host,
                &b.remote.project,
                b.request.number,
            ))
        });
    }
}

/// Private versioned requests.json envelope.
#[derive(Deserialize, Serialize)]
pub(super) struct Data {
    pub(super) version: u32,
    pub(super) requests: Vec<Tracked>,
}
