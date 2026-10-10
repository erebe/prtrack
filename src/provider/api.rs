use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::{Remote, Status};
use crate::process;

pub(super) struct Api {
    remote: Remote,
}

impl Api {
    pub(super) fn new(remote: &Remote) -> Self {
        Self {
            remote: remote.clone(),
        }
    }

    pub(super) fn request(
        &self,
        endpoint: &str,
        payload: Option<Value>,
        paginate: bool,
    ) -> Result<Vec<Value>> {
        self.send(
            if payload.is_some() { "POST" } else { "GET" },
            endpoint,
            payload,
            paginate,
        )
    }

    fn send(
        &self,
        method: &str,
        endpoint: &str,
        payload: Option<Value>,
        paginate: bool,
    ) -> Result<Vec<Value>> {
        let mut args = vec![
            "api",
            "--hostname",
            &self.remote.host,
            "--method",
            method,
            endpoint,
        ];
        if paginate {
            args.push("--paginate");
        }
        let input = payload.map(|v| serde_json::to_vec(&v)).transpose()?;
        if input.is_some() {
            // glab treats --input as a raw body and does not infer its type.
            args.extend(["--header", "Content-Type: application/json", "--input", "-"]);
        }
        let raw = process::output(
            self.remote.provider.executable(),
            &args,
            None,
            input.as_deref(),
        )
        .with_context(|| format!("API request to {} failed", self.remote))?;
        // gh emits adjacent page arrays; glab emits a single flattened array.
        let mut items = Vec::new();
        for page in serde_json::Deserializer::from_str(&raw).into_iter::<Value>() {
            match page.context("Invalid provider JSON")? {
                Value::Array(page) => items.extend(page),
                item => items.push(item),
            }
        }
        Ok(items)
    }

    pub(super) fn object(&self, endpoint: &str, payload: Option<Value>) -> Result<Value> {
        self.request(endpoint, payload, false)?
            .into_iter()
            .next()
            .context("Provider returned an empty response")
    }

    pub(super) fn update(&self, method: &str, endpoint: &str, payload: Value) -> Result<Value> {
        self.send(method, endpoint, Some(payload), false)?
            .into_iter()
            .next()
            .context("Provider returned an empty response")
    }

    pub(super) fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let response = self.object(
            "graphql",
            Some(serde_json::json!({"query": query, "variables": variables})),
        )?;
        if let Some(errors) = response.get("errors") {
            ensure!(
                errors.as_array().is_some_and(|errors| errors.is_empty()),
                "Provider GraphQL query failed: {errors}"
            );
        }
        Ok(response)
    }
}

pub(super) fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

pub(super) fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("Provider response is missing {key}"))
}

/// Normalize API timestamps before comparison so offsets and fractions sort correctly.
pub(super) fn timestamp(value: &Value) -> Result<chrono::DateTime<chrono::Utc>> {
    let text = value
        .as_str()
        .context("Provider response has no timestamp")?;
    Ok(chrono::DateTime::parse_from_rfc3339(text)
        .context("Provider returned an invalid timestamp")?
        .with_timezone(&chrono::Utc))
}

/// Read one aliased GraphQL request, retaining neighbors when only this field fails.
/// Root errors invalidate the batch; nested errors invalidate only their own alias.
pub(super) fn graphql_field<'a>(
    response: &'a Value,
    parent: &str,
    alias: &str,
) -> Result<&'a Value> {
    let prefix = alias.trim_end_matches(|c: char| c.is_ascii_digit());
    if let Some(errors) = response.get("errors") {
        for error in errors
            .as_array()
            .context("Invalid GraphQL errors response")?
        {
            let other_request = error["path"].as_array().is_some_and(|path| {
                path.len() >= 2
                    && path[0] == parent
                    && path[1]
                        .as_str()
                        .is_some_and(|field| field.starts_with(prefix) && field != alias)
            });
            ensure!(other_request, "Provider GraphQL query failed: {error}");
        }
    }
    let value = &response["data"][parent][alias];
    ensure!(
        value.is_object(),
        "Provider response is missing request {alias}"
    );
    Ok(value)
}

pub(super) fn status(value: &Value) -> Result<Status> {
    let state = required(value, "state")?;
    Ok(
        if state == "merged"
            || value["merged_at"].is_string()
            || value["merged"].as_bool() == Some(true)
        {
            Status::Merged
        } else if state == "closed" {
            Status::Closed
        } else if value["draft"].as_bool() == Some(true)
            || value["work_in_progress"].as_bool() == Some(true)
        {
            Status::Draft
        } else {
            ensure!(
                state == "open" || state == "opened" || state == "locked",
                "Unknown request state: {state}"
            );
            Status::Open
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merged_takes_precedence_over_closed_and_draft() {
        let mut value = json!({"state":"closed", "merged_at":"2026-01-01", "draft":true});
        assert_eq!(status(&value).unwrap(), Status::Merged);
        value["merged_at"] = Value::Null;
        assert_eq!(status(&value).unwrap(), Status::Closed);
    }
}
