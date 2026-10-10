//! Publish a branch and create or reuse its pull/merge request.

use anyhow::{Context, Result, ensure};

use super::{Settings, now};
use crate::{cli::Push, git::GitRepository, grove::Grove, output, provider, store::Tracked};

pub(super) fn run(settings: &Settings, args: Push) -> Result<()> {
    let repo = <dyn GitRepository>::discover(&settings.cwd)?;
    let (remote_name, remote) = repo.remote(args.remote.as_deref(), settings.provider)?;
    let branch = repo
        .branch()
        .context("Cannot push a detached HEAD; check out a branch first")?;
    let (commit_title, commit_body) = repo.head_message()?;
    let mut grove = <dyn Grove>::new(&settings.grove_dir);
    grove.reload()?;
    let origin = grove.origin(repo.as_ref());
    settings.store.prepare()?;
    let client = provider::from_remote(&remote)?;
    let remote_branch = args.target.as_deref().unwrap_or(branch);
    repo.validate_branch(remote_branch)
        .context("Invalid remote branch name")?;
    // Authenticate and check for an existing request before publishing any commits.
    let mut existing = client
        .list_pull_requests(None, Some(remote_branch))?
        .into_iter()
        .filter(|r| args.base.as_ref().is_none_or(|base| base == &r.base));
    let existing_request = existing.next();
    ensure!(
        existing.next().is_none(),
        "Multiple open requests use this branch; select one with --base"
    );
    let base = match args.base {
        Some(base) => base,
        None if existing_request.is_some() => existing_request.as_ref().unwrap().base.clone(),
        None => client.default_branch()?,
    };
    ensure!(
        !base.is_empty() && base != remote_branch,
        "Source and target branches must differ; check out a feature branch or choose another --target or --base"
    );
    let title = args.title.unwrap_or(commit_title);
    ensure!(!title.trim().is_empty(), "Request title cannot be empty");
    let body = args.body.unwrap_or(commit_body);
    repo.push(&remote_name, branch, remote_branch)?;
    let reused = existing_request.is_some();
    let request = match existing_request {
        Some(request) => request,
        None => client
            .create_pull_request(remote_branch, &base, &title, &body, args.draft)
            .context("Branch was pushed, but request creation failed; rerun pr push with the same options to retry")?,
    };
    let url = request.url.clone();
    settings
        .store
        .save(vec![Tracked {
            remote,
            request,
            origins: vec![origin],
            checked_at: now(),
            refresh_error: None,
            pipeline_error: None,
        }])
        .with_context(|| {
            format!("Request exists at {url}, but tracking could not be saved; rerun pr push with the same options")
        })?;
    println!(
        "{} {}",
        if reused {
            "Tracking existing request:"
        } else {
            "Created request:"
        },
        output::clean(&url)
    );
    Ok(())
}
