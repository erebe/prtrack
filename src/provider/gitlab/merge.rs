//! GitLab merge recovery, kept separate from discovery and metadata fetching.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{Gitlab, Request, Status, encode, parse, required};

const REBASE_TIMEOUT: Duration = Duration::from_secs(60);
const REBASE_POLL_INTERVAL: Duration = Duration::from_millis(500);

impl Gitlab {
    /// Attempt a normal merge first, then perform one recovery pass on failure.
    /// Initial state/authentication errors must not mutate drafts or closed requests.
    pub(super) fn merge_with_recovery(&self, number: u64) -> Result<Request> {
        let endpoint = format!("{}/{number}", self.requests_endpoint());
        let value = self.api.object(&endpoint, None)?;
        let current = mergeable_request(&value)?;
        if current.status == Status::Merged {
            return Ok(current);
        }
        // Validate before attempting a write, so bad provider data cannot trigger recovery.
        let sha = required(&value, "sha")?;
        match self.merge_once(&endpoint, sha) {
            Ok(merged) => Ok(merged),
            Err(initial) => self.recover_and_merge(&endpoint).map_err(|recovery| {
                anyhow::anyhow!("{recovery:#}; initial GitLab merge failure: {initial:#}")
            }),
        }
    }

    /// Merge exactly the inspected commit and require confirmation before updating tracking.
    /// The SHA guard also prevents a concurrent push from silently changing the retry target.
    fn merge_once(&self, endpoint: &str, sha: &str) -> Result<Request> {
        let merged = parse(&self.api.update(
            "PUT",
            &format!("{endpoint}/merge"),
            json!({"sha": sha}),
        )?)?;
        ensure!(
            merged.status == Status::Merged,
            "Provider did not confirm merge; request remains tracked"
        );
        Ok(merged)
    }

    /// Recheck an ambiguous failure, rebase without CI, resolve threads, then retry once.
    /// Recovery errors stop here; repeated rebases and merge loops are never scheduled.
    fn recover_and_merge(&self, endpoint: &str) -> Result<Request> {
        let current = mergeable_request(&self.api.object(endpoint, None)?)
            .context("Could not confirm request state before recovery")?;
        if current.status == Status::Merged {
            return Ok(current);
        }
        self.api
            .update(
                "PUT",
                &format!("{endpoint}/rebase"),
                json!({"skip_ci": true}),
            )
            .context("Could not start GitLab rebase without a pipeline")?;
        let rebased = wait_for_rebase(
            || {
                self.api
                    .object(&format!("{endpoint}?include_rebase_in_progress=true"), None)
            },
            REBASE_TIMEOUT,
            REBASE_POLL_INTERVAL,
        )
        .context("Could not complete GitLab rebase")?;
        let current = mergeable_request(&rebased)?;
        if current.status == Status::Merged {
            return Ok(current);
        }
        let sha = required(&rebased, "sha")?;
        self.resolve_discussions(endpoint)
            .context("Could not resolve GitLab discussions after rebase")?;
        let mut merged = self
            .merge_once(endpoint, sha)
            .context("GitLab merge retry failed")?;
        merged.unresolved_threads = Some(0);
        Ok(merged)
    }

    /// Resolve each open, resolvable thread across all pages, preserving comment text.
    /// Ordinary comments and resolved threads need no mutation; unconfirmed writes stop retry.
    fn resolve_discussions(&self, endpoint: &str) -> Result<()> {
        let discussions =
            self.api
                .request(&format!("{endpoint}/discussions?per_page=100"), None, true)?;
        for discussion in discussions {
            if !has_unresolved_notes(&discussion)? {
                continue;
            }
            let id = required(&discussion, "id")?;
            let updated = self.api.update(
                "PUT",
                &format!("{endpoint}/discussions/{}", encode(id)),
                json!({"resolved": true}),
            )?;
            ensure!(
                required(&updated, "id")? == id && !has_unresolved_notes(&updated)?,
                "GitLab did not confirm discussion {id} resolved"
            );
        }
        Ok(())
    }
}

/// Accept only open requests or a confirmed completed merge, including during recovery races.
/// Draft/closed requests may have changed while the background operation was running.
fn mergeable_request(value: &Value) -> Result<Request> {
    let request = parse(value)?;
    ensure!(
        matches!(request.status, Status::Open | Status::Merged),
        "Only open, non-draft requests can be merged"
    );
    Ok(request)
}

/// Wait for GitLab's asynchronous rebase, failing on errors, malformed data, or a deadline.
/// An injectable fetch and timing policy keep timeout tests independent of live services.
fn wait_for_rebase(
    mut fetch: impl FnMut() -> Result<Value>,
    timeout: Duration,
    interval: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        ensure!(
            Instant::now() < deadline,
            "Timed out waiting for GitLab rebase; request remains tracked"
        );
        let value = fetch()?;
        let running = value["rebase_in_progress"]
            .as_bool()
            .context("GitLab response is missing rebase progress")?;
        if !running {
            let error = value
                .get("merge_error")
                .context("GitLab response is missing rebase result")?;
            ensure!(
                error.is_null() || error.as_str() == Some(""),
                "GitLab rebase failed: {error}"
            );
            return Ok(value);
        }
        std::thread::sleep(interval.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// Inspect every native note to include all resolvable threads.
/// Reject missing resolution data instead of pretending all discussions are closed.
fn has_unresolved_notes(discussion: &Value) -> Result<bool> {
    for note in discussion["notes"]
        .as_array()
        .context("GitLab discussion has no notes")?
    {
        if note["resolvable"].as_bool() == Some(true)
            && !note["resolved"]
                .as_bool()
                .context("GitLab note has no resolution state")?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Completion, error, and timeout paths must never mistake a queued rebase for success.
    #[test]
    fn rebase_polling_waits_for_completion_and_reports_failures() {
        let mut polls = 0;
        let value = wait_for_rebase(
            || {
                polls += 1;
                Ok(json!({"rebase_in_progress": polls < 3, "merge_error": null, "sha": "rebased"}))
            },
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(polls, 3);
        assert_eq!(value["sha"], "rebased");
        for value in [
            json!({"rebase_in_progress": false, "merge_error": "Conflict"}),
            json!({}),
            json!({"rebase_in_progress": false}),
        ] {
            assert!(
                wait_for_rebase(|| Ok(value.clone()), Duration::from_secs(1), Duration::ZERO)
                    .is_err()
            );
        }
        let error = wait_for_rebase(
            || Ok(json!({"rebase_in_progress": true})),
            Duration::from_millis(1),
            Duration::from_millis(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Timed out"));
        assert!(
            wait_for_rebase(
                || anyhow::bail!("Unavailable"),
                Duration::from_secs(1),
                Duration::ZERO
            )
            .unwrap_err()
            .to_string()
            .contains("Unavailable")
        );
    }
}
