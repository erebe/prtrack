use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use gix::{bstr::ByteSlice, remote::Direction};

use super::{GitRepository, remote::parse_remote};
use crate::provider::{Provider, Remote};

pub(super) struct GixRepository {
    repo: gix::Repository,
    root: PathBuf,
    source: PathBuf,
    branch: Option<String>,
}

impl GixRepository {
    pub(super) fn discover(path: &Path) -> Result<Self> {
        let repo = gix::discover(path).context("Run this command inside a Git repository")?;
        let root = repo
            .workdir()
            .context("Run this command inside a Git working tree")?
            .canonicalize()?;
        let common = repo.common_dir().canonicalize()?;
        let source = if common.file_name().is_some_and(|s| s == ".git") {
            common.parent().unwrap_or(&root).to_path_buf()
        } else {
            root.clone()
        };
        let branch = repo
            .head_name()?
            .map(|name| name.shorten().to_str().map(str::to_owned))
            .transpose()
            .context("Branch name is not valid UTF-8")?;
        Ok(Self {
            repo,
            root,
            source,
            branch,
        })
    }

    fn config(&self, key: &str) -> Result<Option<String>> {
        self.repo
            .config_snapshot()
            .string(key)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .context("Git configuration is not valid UTF-8")
            .map(|value| value.filter(|value| !value.is_empty()))
    }
}

impl GitRepository for GixRepository {
    fn root(&self) -> &Path {
        &self.root
    }
    fn source(&self) -> &Path {
        &self.source
    }
    fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    fn remote(&self, name: Option<&str>, provider: Option<Provider>) -> Result<(String, Remote)> {
        let remote_name = match name {
            Some(name) => name.to_owned(),
            None => {
                let push_remote = self
                    .branch
                    .as_ref()
                    .map(|branch| self.config(&format!("branch.{branch}.pushRemote")))
                    .transpose()?
                    .flatten();
                let upstream = self
                    .branch
                    .as_ref()
                    .map(|branch| self.config(&format!("branch.{branch}.remote")))
                    .transpose()?
                    .flatten()
                    .filter(|r| r != ".");
                match push_remote
                    .or(self.config("remote.pushDefault")?)
                    .or(upstream)
                {
                    Some(name) => name,
                    None => {
                        let names: Vec<_> = self
                            .repo
                            .remote_names()
                            .into_iter()
                            .map(|name| name.to_str().map(str::to_owned))
                            .collect::<std::result::Result<_, _>>()?;
                        if names.iter().any(|name| name == "origin") {
                            "origin".into()
                        } else if names.len() == 1 {
                            names[0].clone()
                        } else {
                            bail!(
                                "No unambiguous remote; pass --remote NAME or configure remote.pushDefault"
                            );
                        }
                    }
                }
            }
        };
        ensure!(
            !remote_name.starts_with('-') && !remote_name.is_empty(),
            "Invalid remote name"
        );
        let remote = self.repo.find_remote(remote_name.as_str())?;
        let mut urls = remote.urls(Direction::Push);
        let url = urls.next().context("Remote has no push URL")?;
        ensure!(
            urls.next().is_none(),
            "Remote has multiple push URLs; select a remote with one destination"
        );
        let configured_provider = self
            .config("pr.provider")?
            .or(self.config("ptrack.provider")?);
        let provider = provider.or(match configured_provider.as_deref() {
            Some("github") => Some(Provider::Github),
            Some("gitlab") => Some(Provider::Gitlab),
            _ => None,
        });
        Ok((
            remote_name,
            parse_remote(url.to_bstring().to_str()?, provider)?,
        ))
    }

    fn head_message(&self) -> Result<(String, String)> {
        let commit = self
            .repo
            .head_commit()
            .context("Create a commit before pushing a request")?;
        let message = commit.message()?;
        let title = message.summary().to_str()?.trim().to_owned();
        let body = message
            .body
            .map(|body| body.to_str().map(|body| body.trim().to_owned()))
            .transpose()?
            .unwrap_or_default();
        Ok((title, body))
    }

    fn validate_branch(&self, branch: &str) -> Result<()> {
        gix::validate::reference::name(format!("refs/heads/{branch}").as_bytes().as_bstr())?;
        Ok(())
    }

    fn push(&self, remote: &str, local_branch: &str, remote_branch: &str) -> Result<()> {
        // gix 0.89 has no send-pack API. Keep only push in Git to preserve
        // authentication, hooks, non-fast-forward checks, and upstream setup.
        let refspec = format!("refs/heads/{local_branch}:refs/heads/{remote_branch}");
        let status = crate::process::command("git")
            .current_dir(&self.root)
            .args(["push", "-f", "--set-upstream", "--", remote, &refspec])
            .status()?;
        ensure!(status.success(), "Git push failed; no request was created");
        Ok(())
    }
}
