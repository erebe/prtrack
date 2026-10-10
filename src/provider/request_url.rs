//! Recognize hosted request links without contacting a provider.

use anyhow::{Context, Result, bail, ensure};
use percent_encoding::percent_decode_str;
use url::Url;

use super::{Provider, Remote};

pub fn parse_request_url(raw: &str, provider: Option<Provider>) -> Result<(Remote, u64)> {
    let url = Url::parse(raw).context("Invalid pull/merge request URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Request URL must use HTTP or HTTPS"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "Request URL must not contain credentials; authenticate with gh or glab"
    );
    let host = url.host_str().context("Request URL has no hostname")?;
    let parts = url
        .path()
        .trim_start_matches('/')
        .trim_end_matches('/')
        .split('/')
        .map(|part| {
            let part = percent_decode_str(part)
                .decode_utf8()
                .context("Request URL path is not valid UTF-8")?;
            ensure!(
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && !part.contains(['/', '\\'])
                    && !part.chars().any(char::is_control),
                "Invalid request URL path component"
            );
            Ok(part.into_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    let parts: Vec<_> = parts.iter().map(String::as_str).collect();
    let (inferred, project, number) = match parts.as_slice() {
        [owner, repo, "pull", number] => (Provider::Github, vec![*owner, *repo], *number),
        [project @ .., "-", "merge_requests", number] if project.len() >= 2 => {
            (Provider::Gitlab, project.to_vec(), *number)
        }
        _ => bail!(
            "Expected a GitHub /owner/repo/pull/NUMBER or GitLab /namespace/repo/-/merge_requests/NUMBER URL"
        ),
    };
    ensure!(
        (host != "github.com" || inferred == Provider::Github)
            && (host != "gitlab.com" || inferred == Provider::Gitlab),
        "Request URL path does not match its hosting provider"
    );
    ensure!(
        provider.is_none_or(|provider| provider == inferred),
        "--provider conflicts with the request URL"
    );
    ensure!(
        number.bytes().all(|c| c.is_ascii_digit()),
        "Request number must be a positive integer"
    );
    let number: u64 = number.parse().context("Invalid request number")?;
    ensure!(number > 0, "Request number must be a positive integer");
    Ok((
        Remote {
            provider: inferred,
            host: match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            },
            project: project.join("/"),
        },
        number,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_links_and_nested_gitlab_namespaces() {
        for (url, provider, host, project, number) in [
            (
                "https://github.com/owner/repo/pull/7",
                Provider::Github,
                "github.com",
                "owner/repo",
                7,
            ),
            (
                "https://gitlab.com/qovery/backend/q-core/-/merge_requests/4096",
                Provider::Gitlab,
                "gitlab.com",
                "qovery/backend/q-core",
                4096,
            ),
            (
                "https://code.example.test:8443/team/sub/repo/-/merge_requests/12/?view=parallel#note_1",
                Provider::Gitlab,
                "code.example.test:8443",
                "team/sub/repo",
                12,
            ),
            (
                "https://code.example.test/owner/r%65po/pull/9#discussion",
                Provider::Github,
                "code.example.test",
                "owner/repo",
                9,
            ),
        ] {
            let (remote, actual_number) = parse_request_url(url, None).unwrap();
            assert_eq!(
                remote,
                Remote {
                    provider,
                    host: host.into(),
                    project: project.into()
                }
            );
            assert_eq!(actual_number, number);
        }
    }

    #[test]
    fn rejects_invalid_request_links_before_provider_access() {
        for url in [
            "not a URL",
            "file:///owner/repo/pull/7",
            "ssh://github.com/owner/repo/pull/7",
            "https://github.com/owner/repo",
            "https://github.com/owner/repo/issues/7",
            "https://github.com/owner/repo/pull/0",
            "https://github.com/owner/repo/pull/+7",
            "https://github.com/owner/repo/pull/18446744073709551616",
            "https://github.com/owner/repo/pull/nope",
            "https://github.com/owner/repo/pull/7/extra",
            "https://gitlab.com/repo/-/merge_requests/7",
            "https://gitlab.com/owner/repo/pull/7",
            "https://github.com/owner/repo/-/merge_requests/7",
            "https://github.com/owner//pull/7",
            "https://github.com/owner%2Frepo/repo/pull/7",
            "https://github.com/owner/re%00po/pull/7",
            "https://user:password@github.com/owner/repo/pull/7",
        ] {
            assert!(parse_request_url(url, None).is_err(), "Accepted {url}");
        }
        assert!(
            parse_request_url(
                "https://github.com/owner/repo/pull/7",
                Some(Provider::Gitlab)
            )
            .is_err()
        );
    }
}
