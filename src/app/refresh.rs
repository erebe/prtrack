//! Discover requests, refresh remote status, and merge results into tracking.

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use super::{
    Settings, Snapshot, now,
    types::{Checkout, RefreshIdentity, RefreshRepositories, RefreshRepository},
};
use crate::{
    git::GitRepository,
    grove::{Grove, Origin},
    provider::{self, GitProvider, PipelineStatus, Remote, Status},
    store::Tracked,
};

const REFRESH_CONCURRENCY: usize = 10;

/// Discover repositories, refresh them concurrently, and persist one combined snapshot.
/// Keeping persistence outside worker tasks protects concurrent untrack/merge operations.
pub fn refresh(settings: &Settings) -> Result<Snapshot> {
    let before = settings.store.load()?;
    let (repositories, warnings) = discover_repositories(settings, &before);
    let results = refresh_repositories(repositories)?;
    let snapshot = collect_results(results, warnings);
    Ok(save_snapshot(settings, &before, snapshot))
}

/// Group cached requests and local checkouts by remote so each repository is queried once.
/// Preserve cached remotes even when their original checkout no longer exists.
fn discover_repositories(
    settings: &Settings,
    records: &[Tracked],
) -> (RefreshRepositories, Vec<String>) {
    let mut warnings = Vec::new();
    let mut grove = <dyn Grove>::new(&settings.grove_dir);
    if let Err(error) = grove.reload() {
        warnings.push(format!("{error:#}"));
    }
    let paths = discover_checkout_paths(settings, grove.as_ref(), records, &mut warnings);
    let mut repositories = RefreshRepositories::new();
    for record in records {
        repositories
            .entry(record.remote.clone())
            .or_default()
            .records
            .push(record.clone());
    }
    for path in paths {
        let inspected = <dyn GitRepository>::discover(&path).and_then(|repo| {
            let (_, remote) = repo.remote(None, settings.provider)?;
            let checkout = Checkout {
                branch: repo.branch().map(str::to_owned),
                origin: grove.origin(repo.as_ref()),
            };
            Ok((remote, checkout))
        });
        match inspected {
            Ok((remote, checkout)) => repositories
                .entry(remote)
                .or_default()
                .checkouts
                .push(checkout),
            Err(error) => warnings.push(format!("{}: {error:#}", path.display())),
        }
    }
    (repositories, warnings)
}

/// Combine Grove, the current repository, and surviving cached checkouts.
/// Deduplication avoids repeated Git inspection while retaining all workspace origins.
fn discover_checkout_paths(
    settings: &Settings,
    grove: &dyn Grove,
    records: &[Tracked],
    warnings: &mut Vec<String>,
) -> Vec<PathBuf> {
    let mut paths = match grove.checkout_paths() {
        Ok(paths) => paths,
        Err(error) => {
            warnings.push(format!("{error:#}"));
            Vec::new()
        }
    };
    if let Ok(repo) = <dyn GitRepository>::discover(&settings.cwd) {
        paths.push(repo.root().to_path_buf());
    }
    paths.extend(
        records
            .iter()
            .flat_map(|r| &r.origins)
            .map(|origin| origin.checkout.clone())
            .filter(|path| path.exists()),
    );
    paths.sort();
    paths.dedup();
    paths
}

/// Run repository work and nested provider batches in one fixed-size pool.
/// Shared identity cells prevent duplicate authentication lookups, including failed ones.
fn refresh_repositories(repositories: RefreshRepositories) -> Result<Vec<Snapshot>> {
    let identities: HashMap<_, _> = repositories
        .keys()
        .map(|remote| {
            (
                (remote.provider, remote.host.clone()),
                RefreshIdentity::new(),
            )
        })
        .collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(REFRESH_CONCURRENCY)
        .build()
        .context("Could not start refresh workers")?;
    Ok(pool.install(|| {
        repositories
            .into_par_iter()
            .map(|(remote, repository)| {
                let identity = &identities[&(remote.provider, remote.host.clone())];
                refresh_repository(remote, repository, identity)
            })
            .collect()
    }))
}

/// Combine worker snapshots after all provider calls finish.
/// Workers own their records, so collection needs neither shared mutation nor store locks.
fn collect_results(results: Vec<Snapshot>, warnings: Vec<String>) -> Snapshot {
    let mut snapshot = Snapshot {
        records: Vec::new(),
        warnings,
    };
    for result in results {
        snapshot.records.extend(result.records);
        snapshot.warnings.extend(result.warnings);
    }
    snapshot
}

/// Save the combined snapshot against the original cache, preserving concurrent edits.
/// Keep fetched results visible if saving fails, and stabilize record/warning ordering.
fn save_snapshot(settings: &Settings, before: &[Tracked], mut snapshot: Snapshot) -> Snapshot {
    Tracked::sort(&mut snapshot.records);
    match settings
        .store
        .save_refreshed(before, snapshot.records.clone())
    {
        Ok(saved) => snapshot.records = saved,
        Err(error) => snapshot
            .warnings
            .push(format!("Could not save tracking data: {error:#}")),
    }
    snapshot.warnings.sort();
    snapshot.warnings.dedup();
    snapshot
}

/// Refresh discovery and cached statuses for one remote without writing to storage.
/// Repository-local snapshots isolate provider failures from other workers' results.
fn refresh_repository(
    remote: Remote,
    repository: RefreshRepository,
    identity: &RefreshIdentity,
) -> Snapshot {
    let mut snapshot = Snapshot {
        records: repository.records,
        warnings: Vec::new(),
    };
    let client = match provider::from_remote(&remote) {
        Ok(client) => client,
        Err(error) => return failed_repository(snapshot, &remote, error),
    };
    // Initialization performs only a blocking API call, never nested pool work.
    let identity = identity.get_or_init(|| client.username().map_err(|e| format!("{e:#}")));
    let refreshed = refresh_discovered_requests(
        client.as_ref(),
        &remote,
        &repository.checkouts,
        identity.as_deref().map_err(String::as_str),
        &mut snapshot,
    );
    refresh_cached_requests(client.as_ref(), &remote, &refreshed, &mut snapshot);
    refresh_pipelines(client.as_ref(), &remote, &mut snapshot);
    snapshot
}

/// Mark every cached record stale when the repository adapter cannot be initialized.
/// Returning its previous data keeps unavailable repositories visible in the list.
fn failed_repository(mut snapshot: Snapshot, remote: &Remote, error: anyhow::Error) -> Snapshot {
    let error = format!("{remote}: {error:#}");
    snapshot.warnings.push(error.clone());
    for record in &mut snapshot.records {
        record.refresh_error = Some(error.clone());
    }
    snapshot
}

/// Discover authored/assigned requests and merge them with their local origins.
/// Return refreshed numbers so the status pass does not fetch those requests again.
fn refresh_discovered_requests(
    client: &dyn GitProvider,
    remote: &Remote,
    checkouts: &[Checkout],
    identity: std::result::Result<&str, &str>,
    snapshot: &mut Snapshot,
) -> HashSet<u64> {
    let requests = identity.map_err(str::to_owned).and_then(|username| {
        client
            .discover_requests(username)
            .map_err(|error| format!("{error:#}"))
    });
    let requests = match requests {
        Ok(requests) => requests,
        Err(error) => {
            snapshot.warnings.push(error);
            return HashSet::new();
        }
    };
    let mut refreshed = HashSet::new();
    for request in requests {
        let origins = request_origins(&request.branch, checkouts, &snapshot.records);
        refreshed.insert(request.number);
        Tracked::merge_into(
            &mut snapshot.records,
            Tracked {
                remote: remote.clone(),
                request,
                origins,
                checked_at: now(),
                refresh_error: None,
                pipeline_error: None,
            },
        );
    }
    refreshed
}

/// Associate a branch with checkouts whose local branch name matches exactly.
/// Fall back to repository origins without a workspace to avoid assigning unrelated work.
fn request_origins(branch: &str, checkouts: &[Checkout], records: &[Tracked]) -> Vec<Origin> {
    let origins: Vec<_> = checkouts
        .iter()
        .filter(|checkout| checkout.branch.as_deref() == Some(branch))
        .map(|checkout| checkout.origin.clone())
        .collect();
    if !origins.is_empty() {
        return origins;
    }
    if let Some(checkout) = checkouts.first() {
        return vec![Origin {
            repository: checkout.origin.repository.clone(),
            checkout: checkout.origin.repository.clone(),
            workspace: None,
        }];
    }
    records
        .first()
        .map(|old| {
            old.origins
                .iter()
                .map(|origin| Origin {
                    repository: origin.repository.clone(),
                    checkout: origin.repository.clone(),
                    workspace: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Batch-refresh tracked requests that were absent from discovery.
/// Leave closed/merged caches untouched unless stale, and preserve individual failed records.
fn refresh_cached_requests(
    client: &dyn GitProvider,
    remote: &Remote,
    refreshed: &HashSet<u64>,
    snapshot: &mut Snapshot,
) {
    let pending: HashMap<_, _> = snapshot
        .records
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            !refreshed.contains(&record.request.number)
                && (!matches!(record.request.status, Status::Merged | Status::Closed)
                    || record.refresh_error.is_some())
        })
        .map(|(index, record)| (record.request.number, index))
        .collect();
    let mut numbers: Vec<_> = pending.keys().copied().collect();
    numbers.sort_unstable();
    for (number, result) in client.get_pull_requests(&numbers) {
        let record = &mut snapshot.records[pending[&number]];
        match result {
            Ok(mut request) => {
                request.pipeline_status = record.request.pipeline_status;
                record.request = request;
                record.checked_at = now();
                record.refresh_error = None;
            }
            Err(error) => {
                let error = format!("{remote} #{number}: {error:#}");
                record.refresh_error = Some(error.clone());
                snapshot.warnings.push(error);
            }
        }
    }
}

/// Initialize missing CI results and retry running/failed pipelines in provider batches.
/// Successful results (including no pipeline) stay cached, even when PR metadata changes.
/// Polling CI independently also avoids refetching closed/merged request details.
fn refresh_pipelines(client: &dyn GitProvider, remote: &Remote, snapshot: &mut Snapshot) {
    let pending: HashMap<_, _> = snapshot
        .records
        .iter()
        .enumerate()
        .filter(|(_, record)| record.request.pipeline_status != Some(PipelineStatus::Succeed))
        .map(|(index, record)| (record.request.number, index))
        .collect();
    let mut numbers: Vec<_> = pending.keys().copied().collect();
    numbers.sort_unstable();
    for (number, result) in client.pipeline_statuses(&numbers) {
        let record = &mut snapshot.records[pending[&number]];
        match result {
            Ok(status) => {
                record.request.pipeline_status = Some(status);
                record.pipeline_error = None;
            }
            Err(error) => {
                let error = format!("{remote} #{number}: pipeline lookup failed: {error:#}");
                record.pipeline_error = Some(error.clone());
                snapshot.warnings.push(error);
            }
        }
        record.checked_at = now();
    }
}
