use super::{Grove, Origin, Workspace, types::GroveWorkspace};
use crate::git::GitRepository;
use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) struct FilesystemGrove {
    dir: PathBuf,
    workspaces: Vec<GroveWorkspace>,
}

impl FilesystemGrove {
    pub(super) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            workspaces: Vec::new(),
        }
    }
}

impl Grove for FilesystemGrove {
    fn reload(&mut self) -> Result<()> {
        self.workspaces = read(&self.dir)?;
        Ok(())
    }

    fn origin(&self, repo: &dyn GitRepository) -> Origin {
        origin(repo, &self.workspaces, &self.dir)
    }

    fn checkout_paths(&self) -> Result<Vec<PathBuf>> {
        checkout_paths(&self.dir, &self.workspaces)
    }
}

/// Read Grove's public state format without modifying Grove's own files.
fn read(dir: &Path) -> Result<Vec<GroveWorkspace>> {
    let path = dir.join("state.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("Invalid Grove state in {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn origin(repo: &dyn GitRepository, workspaces: &[GroveWorkspace], grove_dir: &Path) -> Origin {
    let root = canonical(repo.root());
    for workspace in workspaces {
        for member in &workspace.repos {
            if root == canonical(&member.worktree_path) {
                return Origin {
                    repository: canonical(&member.source_repo),
                    checkout: root,
                    workspace: Some(Workspace {
                        name: workspace.name.clone(),
                        path: canonical(&workspace.path),
                    }),
                };
            }
        }
    }
    let workspace = root
        .strip_prefix(canonical(&grove_dir.join("workspaces")))
        .ok()
        .and_then(|relative| {
            let mut components = relative.components();
            let name = components
                .next()?
                .as_os_str()
                .to_string_lossy()
                .into_owned();
            Some(Workspace {
                path: canonical(&grove_dir.join("workspaces").join(&name)),
                name,
            })
        });
    Origin {
        repository: repo.source().to_path_buf(),
        checkout: root,
        workspace,
    }
}

/// Include unregistered checkouts in the standard workspaces/<name>/<repo> layout.
fn checkout_paths(dir: &Path, workspaces: &[GroveWorkspace]) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<_> = workspaces
        .iter()
        .flat_map(|w| w.repos.iter().map(|r| r.worktree_path.clone()))
        .collect();
    let root = dir.join("workspaces");
    match fs::read_dir(&root) {
        Ok(entries) => {
            for entry in entries {
                let workspace = entry?.path();
                if !workspace.is_dir() {
                    continue;
                }
                if workspace.join(".git").exists() {
                    paths.push(workspace.clone());
                }
                for repo in fs::read_dir(workspace)? {
                    let repo = repo?.path();
                    if repo.join(".git").exists() {
                        paths.push(repo);
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e).context("Cannot scan Grove workspaces"),
    }
    paths = paths.into_iter().map(|p| canonical(&p)).collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}
