//! Fetch an existing request by URL and merge it into local tracking.

use anyhow::{Context, Result, ensure};

use super::{Settings, now};
use crate::{cli::Add, output, provider, store::Tracked};

pub(super) fn run(settings: &Settings, args: Add) -> Result<()> {
    let (remote, number) = provider::parse_request_url(&args.url, settings.provider)?;
    settings.store.prepare()?;
    let client = provider::from_remote(&remote)?;
    let request = client.get_pull_request(number)?;
    ensure!(
        request.number == number,
        "Provider returned request #{} instead of #{number}",
        request.number
    );
    let url = request.url.clone();
    settings
        .store
        .save(vec![Tracked {
            remote,
            request,
            // The URL identifies a hosted repository, not a local checkout.
            // Store merging preserves known origins; discovery can add more later.
            origins: Vec::new(),
            checked_at: now(),
            refresh_error: None,
            pipeline_error: None,
        }])
        .context("Could not save tracking data; rerun pr add to retry")?;
    println!("Tracking request: {}", output::clean(&url));
    Ok(())
}
