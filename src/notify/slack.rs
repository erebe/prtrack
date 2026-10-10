use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use super::{Notify, types::Slack};
use crate::config::SlackConfig;

const ENDPOINT: &str = "https://slack.com/api/chat.postMessage";

impl Slack {
    /// Validate Slack settings only when needed so other commands require no credentials.
    pub(super) fn from_config(config: &SlackConfig) -> Result<Self> {
        let token = setting("slack.token", config.token.as_deref())?;
        let channel_id = setting("slack.channel_id", config.channel_id.as_deref())?;
        ensure!(
            matches!(channel_id.as_bytes().first(), Some(b'C' | b'G'))
                && channel_id.len() > 1
                && channel_id.bytes().all(|c| c.is_ascii_alphanumeric()),
            "slack.channel_id must be a Slack channel ID (for example C0123456789)"
        );
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .new_agent();
        Ok(Self {
            token: token.to_owned(),
            channel_id: channel_id.to_owned(),
            review_message: config.review_message.clone(),
            deploy_message: config.deploy_message.clone(),
            agent,
            endpoint: ENDPOINT.into(),
        })
    }
    /// Send the configured template with the request URL substituted into each placeholder.
    /// Suppress unfurls and check Slack's JSON result to confirm message delivery.
    /// A POST is never automatically retried because it may already have been delivered.
    fn send(&self, template: Option<&str>, setting_name: &str, request_url: &str) -> Result<()> {
        // Validate only the selected template so each notification can be configured independently.
        let template = template.with_context(|| {
            format!("Set {setting_name} in the config file to enable Slack notifications")
        })?;
        ensure!(
            template.contains("{pr_url}"),
            "{setting_name} must contain {{pr_url}}"
        );
        // Validate cached URLs too so they cannot inject Slack mentions or link markup.
        let url = url::Url::parse(request_url).context("Invalid request URL for notification")?;
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && !request_url.chars().any(char::is_whitespace)
                && !request_url.contains(['<', '>', '|']),
            "Notification requires an HTTP(S) request URL without credentials, whitespace, or Slack markup"
        );
        // Escape URL text while preserving the template's intentional Slack markup.
        let request_url = request_url.replace('&', "&amp;");
        let mut response = self
            .agent
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.token))
            .send_json(json!({
                "channel": self.channel_id,
                "text": template.replace("{pr_url}", &request_url),
                "mrkdwn": true,
                "unfurl_links": false,
                "unfurl_media": false,
            }))
            // Transport errors may contain headers/URLs: keep credentials out of the TUI.
            .map_err(|_| {
                anyhow::anyhow!(
                    "Slack request failed or timed out; check the channel before retrying"
                )
            })?;
        let status = response.status();
        if status.as_u16() == 429 {
            let delay = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            bail!(
                "Slack rate limit reached{}",
                delay
                    .map(|s| format!("; retry after {s}s"))
                    .unwrap_or_default()
            );
        }
        ensure!(status.is_success(), "Slack returned HTTP {status}");
        let response: Value = response
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_json()
            .map_err(|_| {
                anyhow::anyhow!(
                    "Slack returned an invalid response; check the channel before retrying"
                )
            })?;
        if response["ok"].as_bool() != Some(true) {
            // Slack error codes are identifiers. Never display arbitrary response text.
            let code = response["error"]
                .as_str()
                .filter(|code| {
                    !code.is_empty()
                        && code.len() <= 80
                        && code.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
                })
                .unwrap_or("unknown_error");
            bail!("Slack rejected the message: {code}");
        }
        ensure!(
            response["channel"].as_str() == Some(&self.channel_id)
                && response["ts"].as_str().is_some_and(|ts| !ts.is_empty()),
            "Slack did not confirm message delivery; check the channel before retrying"
        );
        Ok(())
    }
}

impl Notify for Slack {
    /// Use the review template while sharing the configured Slack channel and transport.
    fn request_review(&self, request_url: &str) -> Result<()> {
        self.send(
            self.review_message.as_deref(),
            "slack.review_message",
            request_url,
        )
    }

    /// Use the deployment template without falling back to review text or confirming a deploy.
    fn request_deploy(&self, request_url: &str) -> Result<()> {
        self.send(
            self.deploy_message.as_deref(),
            "slack.deploy_message",
            request_url,
        )
    }
}

/// Require nonempty settings without echoing secret values in configuration errors.
fn setting<'a>(name: &str, value: Option<&'a str>) -> Result<&'a str> {
    let value = value
        .with_context(|| format!("Set {name} in the config file to enable Slack notifications"))?;
    ensure!(
        !value.trim().is_empty() && !value.chars().any(char::is_whitespace),
        "{name} must be nonempty and contain no whitespace"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread::{self, JoinHandle},
    };

    /// Serve one local response and capture the HTTP request without contacting Slack.
    fn server(status: &str, body: &str, headers: &str) -> (String, JoinHandle<(String, Value)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/api/chat.postMessage",
            listener.local_addr().unwrap()
        );
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        );
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut headers = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
                headers.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            socket.write_all(response.as_bytes()).unwrap();
            (headers, serde_json::from_slice(&body).unwrap())
        });
        (endpoint, worker)
    }

    /// Use a private endpoint and disable environment proxies to isolate transport tests.
    fn notifier(endpoint: String) -> Slack {
        let mut notifier = Slack::from_config(&SlackConfig {
            token: Some("test-slack-token".into()),
            channel_id: Some("C0123456789".into()),
            review_message: Some("<@U07F73SKT0V> review {pr_url} in <#C02NQ0LC8M9>".into()),
            deploy_message: Some("<@U07F73SKT0V> release {pr_url}".into()),
        })
        .unwrap();
        notifier.endpoint = endpoint;
        notifier.agent = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(3)))
            .build()
            .new_agent();
        notifier
    }

    /// Verify a real Homer mention for both providers and credential transport in a header.
    #[test]
    fn posts_homer_mentions_and_checks_delivery() {
        for url in [
            "https://github.com/Qovery/documentation/pull/416",
            "https://gitlab.com/qovery/backend/q-core/-/merge_requests/4096",
        ] {
            let (endpoint, worker) = server(
                "200 OK",
                r#"{"ok":true,"channel":"C0123456789","ts":"123.456"}"#,
                "",
            );
            notifier(endpoint).request_review(url).unwrap();
            let (headers, body) = worker.join().unwrap();
            assert!(headers.starts_with("POST /api/chat.postMessage HTTP/1.1\r\n"));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-slack-token\r\n")
            );
            assert_eq!(
                body,
                json!({
                    "channel": "C0123456789", "text": format!("<@U07F73SKT0V> review {url} in <#C02NQ0LC8M9>"),
                    "mrkdwn": true, "unfurl_links": false, "unfurl_media": false
                })
            );
        }
    }

    /// Preserve query parameters without treating their ampersands as Slack control text.
    #[test]
    fn escapes_url_text_in_mentions() {
        let (endpoint, worker) = server(
            "200 OK",
            r#"{"ok":true,"channel":"C0123456789","ts":"123.456"}"#,
            "",
        );
        notifier(endpoint)
            .request_review("https://github.com/owner/repo/pull/7?a=1&b=2")
            .unwrap();
        let (_, body) = worker.join().unwrap();
        assert_eq!(
            body["text"],
            "<@U07F73SKT0V> review https://github.com/owner/repo/pull/7?a=1&amp;b=2 in <#C02NQ0LC8M9>"
        );
    }

    /// Honor custom text, Slack markup, newlines, and every occurrence of the URL marker.
    #[test]
    fn substitutes_urls_in_a_configured_message() {
        let (endpoint, worker) = server(
            "200 OK",
            r#"{"ok":true,"channel":"C0123456789","ts":"123.456"}"#,
            "",
        );
        let mut slack = notifier(endpoint);
        slack.review_message =
            Some("Review needed: <{pr_url}|request>\n<@UOTHER> {pr_url} {unchanged}".into());
        slack
            .request_review("https://github.com/team/repo/pull/42")
            .unwrap();
        let (_, body) = worker.join().unwrap();
        assert_eq!(
            body["text"],
            "Review needed: <https://github.com/team/repo/pull/42|request>\n<@UOTHER> https://github.com/team/repo/pull/42 {unchanged}"
        );
    }

    /// Deployments use their own template and the existing channel, even without review settings.
    #[test]
    fn deploy_message_uses_the_shared_channel_and_substitutes_each_url() {
        let (endpoint, worker) = server(
            "200 OK",
            r#"{"ok":true,"channel":"C0123456789","ts":"123.456"}"#,
            "",
        );
        let mut slack = notifier(endpoint);
        slack.review_message = None;
        slack.deploy_message = Some("<@UDEPLOY> release {pr_url}\n<{pr_url}|changes>".into());
        slack
            .request_deploy("https://gitlab.com/team/repo/-/merge_requests/7?a=1&b=2")
            .unwrap();
        let (headers, body) = worker.join().unwrap();
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("authorization: bearer test-slack-token\r\n")
        );
        assert_eq!(
            body,
            json!({
                "channel": "C0123456789",
                "text": "<@UDEPLOY> release https://gitlab.com/team/repo/-/merge_requests/7?a=1&amp;b=2\n<https://gitlab.com/team/repo/-/merge_requests/7?a=1&amp;b=2|changes>",
                "mrkdwn": true, "unfurl_links": false, "unfurl_media": false
            })
        );
    }

    /// Review-only configs stay usable and deploy errors identify the missing template before I/O.
    #[test]
    fn validates_only_the_requested_message() {
        for template in [None, Some(""), Some("release without a URL")] {
            let (endpoint, worker) = server(
                "200 OK",
                r#"{"ok":true,"channel":"C0123456789","ts":"123.456"}"#,
                "",
            );
            let mut slack = notifier(endpoint);
            slack.deploy_message = template.map(str::to_owned);
            let error = slack
                .request_deploy("https://github.com/team/repo/pull/7")
                .unwrap_err();
            assert!(error.to_string().contains("slack.deploy_message"));
            // The failed deploy must not consume this server's single request with review text.
            slack
                .request_review("https://github.com/team/repo/pull/7")
                .unwrap();
            let (_, body) = worker.join().unwrap();
            assert!(body["text"].as_str().unwrap().contains(" review "));
        }
    }

    /// Slack can fail with HTTP 200; failures must neither claim success nor leak tokens.
    #[test]
    fn reports_slack_http_json_and_delivery_errors_without_retrying() {
        for (status, body, headers, expected) in [
            (
                "200 OK",
                r#"{"ok":false,"error":"missing_scope"}"#,
                "",
                "missing_scope",
            ),
            (
                "200 OK",
                r#"{"ok":false,"error":"invalid_auth"}"#,
                "",
                "invalid_auth",
            ),
            (
                "200 OK",
                r#"{"ok":false,"error":"test-slack-token"}"#,
                "",
                "unknown_error",
            ),
            (
                "429 Too Many Requests",
                "",
                "Retry-After: 42\r\n",
                "retry after 42s",
            ),
            ("503 Unavailable", "test-slack-token", "", "HTTP 503"),
            (
                "302 Found",
                "",
                "Location: http://127.0.0.1:1/redirect\r\n",
                "HTTP 302",
            ),
            ("200 OK", "test-slack-token", "", "invalid response"),
            ("200 OK", r#"{"ok":true}"#, "", "did not confirm"),
            (
                "200 OK",
                r#"{"ok":true,"channel":"COTHER","ts":"123.456"}"#,
                "",
                "did not confirm",
            ),
        ] {
            let (endpoint, worker) = server(status, body, headers);
            let error = format!(
                "{:#}",
                notifier(endpoint)
                    .request_review("https://github.com/owner/repo/pull/7")
                    .unwrap_err()
            );
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("test-slack-token"), "{error}");
            worker.join().unwrap();
        }
    }

    /// Reject malformed cached URLs before attempting delivery or exposing their contents.
    #[test]
    fn rejects_invalid_notification_urls_before_network_access() {
        for url in [
            "file:///tmp/test",
            "https://token@github.com/owner/repo/pull/7",
            "https://github.com/owner/repo/pull/7\nextra",
            "https://github.com/owner/repo/pull/7#<@UOTHER>",
            "https://github.com/owner/repo/pull/7|extra",
        ] {
            let error = notifier("http://127.0.0.1:1".into())
                .request_review(url)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("Notification requires") || error.contains("Invalid request URL"),
                "{error}"
            );
        }
    }
}
