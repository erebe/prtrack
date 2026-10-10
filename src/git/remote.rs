use crate::provider::{Provider, Remote};
use anyhow::{Context, Result, bail, ensure};
use url::Url;

pub(super) fn parse_remote(raw: &str, provider: Option<Provider>) -> Result<Remote> {
    let (host, path) = if raw.contains("://") {
        let url = Url::parse(raw).context("Invalid Git remote URL")?;
        ensure!(
            ["https", "http", "ssh", "git"].contains(&url.scheme()),
            "Unsupported Git remote scheme"
        );
        let host = url.host_str().context("Remote has no hostname")?;
        let host = if matches!(url.scheme(), "http" | "https") {
            match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            }
        } else {
            host.to_owned()
        };
        (host, url.path().to_owned())
    } else {
        let (host, path) = raw
            .split_once(':')
            .context("Expected a GitHub/GitLab HTTPS or SSH remote")?;
        (
            host.rsplit('@').next().unwrap_or(host).to_owned(),
            path.to_owned(),
        )
    };
    let host = host.to_ascii_lowercase();
    ensure!(
        !host.is_empty() && !host.starts_with('-') && !host.contains(['/', ' ', '?', '#']),
        "Invalid remote hostname"
    );
    let project = path.trim_matches('/').trim_end_matches(".git").to_owned();
    ensure!(
        project.split('/').count() >= 2
            && project
                .split('/')
                .all(|s| !s.is_empty() && s != "." && s != ".."),
        "Remote must include a namespace and repository"
    );
    let provider = match provider {
        Some(provider) => provider,
        None if host == "github.com" || host.starts_with("github.") => Provider::Github,
        None if host == "gitlab.com" || host.starts_with("gitlab.") => Provider::Gitlab,
        None => bail!(
            "Unknown Git host {host}; use --provider github|gitlab or git config pr.provider github|gitlab"
        ),
    };
    if provider == Provider::Github {
        ensure!(
            project.split('/').count() == 2,
            "GitHub remotes require owner/repository"
        );
    }
    Ok(Remote {
        host,
        project,
        provider,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_ssh_https_and_nested_gitlab_remotes() {
        for raw in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo.git",
            "ssh://git@github.com:22/owner/repo.git",
        ] {
            let remote = parse_remote(raw, None).unwrap();
            assert_eq!(remote.host, "github.com");
            assert_eq!(remote.project, "owner/repo");
        }
        assert_eq!(
            parse_remote("git@gitlab.com:group/sub/repo.git", None)
                .unwrap()
                .project,
            "group/sub/repo"
        );
        assert_eq!(
            parse_remote(
                "https://code.example:8443/team/repo",
                Some(Provider::Gitlab)
            )
            .unwrap()
            .host,
            "code.example:8443"
        );
    }
    #[test]
    fn rejects_local_and_ambiguous_hosts() {
        assert!(parse_remote("/tmp/repo", None).is_err());
        assert!(parse_remote("git@code.example:team/repo", None).is_err());
        assert!(parse_remote("https://github.com/repo", None).is_err());
    }
}
