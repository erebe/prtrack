use std::collections::HashSet;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde_json::{Value, json};

use super::{
    GitProvider, PipelineStatus, Provider, Remote, Request, Status,
    api::{Api, encode, graphql_field, required, status, timestamp},
    types::RequestRole,
};

// Fetch all data needed for tracking in one request per search page.
const DISCOVERY_QUERY: &str = r#"
query($search: String!, $cursor: String) {
  search(query: $search, type: ISSUE, first: 100, after: $cursor) {
    issueCount
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on PullRequest {
        number title url headRefName baseRefName state isDraft reviewDecision updatedAt
        headRepository { nameWithOwner }
        COMMENT_FIELDS
      }
    }
  }
}
"#;

// Submitted reviews include inline replies, even when the thread is old or resolved.
const REVIEW_STATES: &str = "[APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED]";
const REVIEW_FIELDS: &str = r#"
  nodes {
    body submittedAt
    comments(last: 1) { nodes { createdAt publishedAt } }
  }
  pageInfo { hasNextPage endCursor }
"#;

const THREAD_FIELDS: &str = "nodes { isResolved } pageInfo { hasNextPage endCursor }";

/// Embed comment metadata in existing queries to avoid a subprocess for every PR.
fn comment_fields() -> String {
    format!(
        "comments(last: 1) {{ nodes {{ createdAt }} }} reviews(first: 100, states: {REVIEW_STATES}) {{ {REVIEW_FIELDS} }} reviewThreads(first: 100) {{ {THREAD_FIELDS} }}"
    )
}

pub(super) struct Github {
    api: Api,
    project: String,
    default_branch: Option<String>,
    merge_method: &'static str,
}

impl Github {
    pub(super) fn new(remote: &Remote) -> Result<Self> {
        let api = Api::new(remote);
        let repo = api.object(&project_endpoint(&remote.project), None)?;
        Ok(Self {
            api,
            project: required(&repo, "full_name")?.into(),
            default_branch: repo["default_branch"].as_str().map(str::to_owned),
            merge_method: if repo["allow_merge_commit"].as_bool() != Some(false) {
                "merge"
            } else if repo["allow_squash_merge"].as_bool() == Some(true) {
                "squash"
            } else {
                "rebase"
            },
        })
    }

    fn requests_endpoint(&self) -> String {
        format!("{}/pulls", project_endpoint(&self.project))
    }

    /// Fetch tracking fields and approvals in one query to amortize CLI/API latency.
    /// Retain per-request errors so an inaccessible PR does not invalidate the batch.
    fn get_requests_batch(&self, numbers: &[u64]) -> Vec<(u64, Result<Request>)> {
        let response = (|| {
            let (owner, name) = self
                .project
                .split_once('/')
                .context("Invalid GitHub repository")?;
            let fields = numbers.iter().map(|number| format!(
                "pr{number}: pullRequest(number: {number}) {{ number title url headRefName baseRefName state isDraft reviewDecision updatedAt {} }}", comment_fields()
            )).collect::<Vec<_>>().join(" ");
            let query = format!(
                "query($owner: String!, $name: String!) {{ repository(owner: $owner, name: $name) {{ {fields} }} }}"
            );
            // Preserve partial data: an inaccessible PR must not invalidate its neighbors.
            self.api.object(
                "graphql",
                Some(json!({
                    "query": query, "variables": {"owner": owner, "name": name}
                })),
            )
        })();
        numbers
            .iter()
            .map(|&number| {
                let result = match &response {
                    Ok(response) => parse_batch_entry(response, number).and_then(|mut request| {
                        self.fetch_comment_metadata(
                            &mut request,
                            &response["data"]["repository"][format!("pr{number}")],
                        )?;
                        Ok(request)
                    }),
                    Err(error) => Err(anyhow::anyhow!("{error:#}")),
                };
                (number, result)
            })
            .collect()
    }

    /// Enrich REST results with approval and comment data in bounded GraphQL batches.
    fn fetch_metadata(&self, requests: &mut [Request]) -> Result<()> {
        let (owner, name) = self
            .project
            .split_once('/')
            .context("Invalid GitHub repository")?;
        requests.par_chunks_mut(50).try_for_each(|batch| {
            let fields = batch
                .iter()
                .map(|r| {
                    format!(
                        "pr{}: pullRequest(number: {}) {{ reviewDecision {} }}",
                        r.number, r.number, comment_fields()
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            if fields.is_empty() {
                return Ok(());
            }
            let query = format!(
                "query($owner: String!, $name: String!) {{ repository(owner: $owner, name: $name) {{ {fields} }} }}"
            );
            let response = self
                .api
                .graphql(&query, json!({"owner": owner, "name": name}))?;
            for request in batch {
                let value = &response["data"]["repository"][format!("pr{}", request.number)];
                ensure!(
                    value.is_object() && value.get("reviewDecision").is_some(),
                    "GitHub approval response is missing request #{}",
                    request.number
                );
                request.approved = Some(value["reviewDecision"].as_str() == Some("APPROVED"));
                self.fetch_comment_metadata(request, value)?;
            }
            Ok(())
        })
    }

    /// Follow every review page so recent replies attached to old reviews are included.
    /// The usual case uses the first page already returned by discovery or metadata.
    fn fetch_comment_metadata(&self, request: &mut Request, value: &Value) -> Result<()> {
        let mut latest = value["comments"]["nodes"]
            .as_array()
            .context("GitHub response is missing conversation comments")?
            .iter()
            .map(|comment| timestamp(&comment["createdAt"]))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max();
        let mut reviews = value["reviews"].clone();
        let mut cursors = HashSet::new();
        loop {
            latest = latest.max(review_comment_time(&reviews)?);
            let page = &reviews["pageInfo"];
            if !page["hasNextPage"]
                .as_bool()
                .context("GitHub reviews have no pagination state")?
            {
                request.unresolved_threads =
                    Some(self.fetch_unresolved_threads(request.number, &value["reviewThreads"])?);
                request.last_comment_at = latest;
                return Ok(());
            }
            let cursor = required(page, "endCursor")?;
            ensure!(
                !cursor.is_empty() && cursors.insert(cursor.to_owned()),
                "GitHub reviews returned a repeated or empty cursor"
            );
            let (owner, name) = self
                .project
                .split_once('/')
                .context("Invalid GitHub repository")?;
            let query = format!(
                "query($owner: String!, $name: String!, $number: Int!, $cursor: String!) {{ repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ reviews(first: 100, after: $cursor, states: {REVIEW_STATES}) {{ {REVIEW_FIELDS} }} }} }} }}"
            );
            let response = self.api.graphql(
                &query,
                json!({
                    "owner": owner, "name": name, "number": request.number, "cursor": cursor
                }),
            )?;
            reviews = response["data"]["repository"]["pullRequest"]["reviews"].clone();
        }
    }

    /// Count review threads without fetching bodies or counting replies separately.
    /// Follow later pages, including outdated threads whose resolution is still pending.
    fn fetch_unresolved_threads(&self, number: u64, first_page: &Value) -> Result<u64> {
        let mut threads = first_page.clone();
        let mut count = 0;
        let mut cursors = HashSet::new();
        loop {
            for thread in threads["nodes"]
                .as_array()
                .context("GitHub response is missing review threads")?
            {
                if !thread["isResolved"]
                    .as_bool()
                    .context("GitHub review thread has no resolution state")?
                {
                    count += 1;
                }
            }
            let page = &threads["pageInfo"];
            if !page["hasNextPage"]
                .as_bool()
                .context("GitHub review threads have no pagination state")?
            {
                return Ok(count);
            }
            let cursor = required(page, "endCursor")?;
            ensure!(
                !cursor.is_empty() && cursors.insert(cursor.to_owned()),
                "GitHub review threads returned a repeated or empty cursor"
            );
            let (owner, name) = self
                .project
                .split_once('/')
                .context("Invalid GitHub repository")?;
            let query = format!(
                "query($owner: String!, $name: String!, $number: Int!, $cursor: String!) {{ repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ reviewThreads(first: 100, after: $cursor) {{ {THREAD_FIELDS} }} }} }} }}"
            );
            let response = self.api.graphql(
                &query,
                json!({"owner": owner, "name": name, "number": number, "cursor": cursor}),
            )?;
            threads = response["data"]["repository"]["pullRequest"]["reviewThreads"].clone();
        }
    }

    fn list_pull_requests_rest(
        &self,
        user: Option<(RequestRole, &str)>,
        branch: Option<&str>,
    ) -> Result<Vec<Request>> {
        // Repository issues include PRs and support author/assignee filtering.
        let endpoint = if let Some((role, username)) = user {
            let filter = match role {
                RequestRole::Author => "creator",
                RequestRole::Assignee => "assignee",
            };
            format!(
                "{}/issues?state=open&per_page=100&{filter}={}",
                project_endpoint(&self.project),
                encode(username)
            )
        } else {
            let mut endpoint = format!("{}?state=open&per_page=100", self.requests_endpoint());
            if let Some(branch) = branch {
                let owner = self.project.split('/').next().unwrap_or_default();
                endpoint.push_str(&format!("&head={}", encode(&format!("{owner}:{branch}"))));
            }
            endpoint
        };
        let requests: Result<Vec<_>> = self
            .api
            .request(&endpoint, None, true)?
            .into_par_iter()
            .map(|value| {
                let value = if user.is_some() {
                    if !value["pull_request"].is_object() {
                        return Ok(None);
                    }
                    let number = value["number"]
                        .as_u64()
                        .context("Provider response has no request number")?;
                    // Issue summaries omit head/base branches; fetch only matching PRs.
                    self.api
                        .object(&format!("{}/{number}", self.requests_endpoint()), None)?
                } else {
                    value
                };
                // A fork's branch cannot be associated with this repository's worktrees.
                if !value
                    .pointer("/head/repo/full_name")
                    .and_then(Value::as_str)
                    .is_some_and(|p| p.eq_ignore_ascii_case(&self.project))
                {
                    return Ok(None);
                }
                let request = parse(&value)?;
                Ok(branch
                    .is_none_or(|b| b == request.branch)
                    .then_some(request))
            })
            .collect();
        let mut requests: Vec<_> = requests?.into_iter().flatten().collect();
        self.fetch_metadata(&mut requests)?;
        Ok(requests)
    }

    fn list_user_requests(
        &self,
        role: RequestRole,
        username: &str,
        branch: Option<&str>,
    ) -> Result<Vec<Request>> {
        let filter = match role {
            RequestRole::Author => "author",
            RequestRole::Assignee => "assignee",
        };
        let search = format!("repo:{} is:pr is:open {filter}:{username}", self.project);
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        let mut requests = Vec::new();
        let query = DISCOVERY_QUERY.replace("COMMENT_FIELDS", &comment_fields());
        loop {
            let response = self.api.object(
                "graphql",
                Some(json!({
                    "query": query,
                    "variables": {"search": search, "cursor": cursor},
                })),
            )?;
            if let Some(errors) = response.get("errors") {
                ensure!(
                    errors.as_array().is_some_and(|errors| errors.is_empty()),
                    "GitHub GraphQL query failed: {errors}"
                );
            }
            let results = &response["data"]["search"];
            let count = results["issueCount"]
                .as_u64()
                .context("GitHub search has no result count")?;
            // Search is capped at 1,000 results. Preserve complete discovery for
            // large repositories using the same relationship filter via REST.
            if count > 1_000 {
                return self.list_pull_requests_rest(Some((role, username)), branch);
            }
            for value in results["nodes"]
                .as_array()
                .context("GitHub search has no nodes")?
            {
                // A fork's branch cannot be associated with this repository's worktrees.
                if !value
                    .pointer("/headRepository/nameWithOwner")
                    .and_then(Value::as_str)
                    .is_some_and(|project| project.eq_ignore_ascii_case(&self.project))
                {
                    continue;
                }
                let mut request = parse_graphql(value)?;
                if branch.is_none_or(|branch| branch == request.branch) {
                    self.fetch_comment_metadata(&mut request, value)?;
                    requests.push(request);
                }
            }
            let page = &results["pageInfo"];
            if !page["hasNextPage"]
                .as_bool()
                .context("GitHub search has no pagination state")?
            {
                return Ok(requests);
            }
            let next = required(page, "endCursor")?;
            ensure!(
                !next.is_empty() && seen_cursors.insert(next.to_owned()),
                "GitHub search returned a repeated or empty cursor"
            );
            cursor = Some(next.to_owned());
        }
    }
}

impl GitProvider for Github {
    fn discover_requests(&self, username: &str) -> Result<Vec<Request>> {
        // Both searches share the caller's bounded refresh pool.
        let (authored, assigned) = rayon::join(
            || self.list_user_requests(RequestRole::Author, username, None),
            || self.list_user_requests(RequestRole::Assignee, username, None),
        );
        let mut requests = authored?;
        requests.extend(assigned?);
        let mut seen = HashSet::new();
        requests.retain(|request| seen.insert(request.number));
        Ok(requests)
    }

    fn username(&self) -> Result<String> {
        Ok(required(&self.api.object("user", None)?, "login")?.into())
    }

    fn default_branch(&self) -> Result<String> {
        self.default_branch
            .clone()
            .context("Repository has no default branch; pass --base")
    }

    fn list_pull_requests(
        &self,
        author: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Vec<Request>> {
        match author {
            Some(author) => self.list_user_requests(RequestRole::Author, author, branch),
            None => self.list_pull_requests_rest(None, branch),
        }
    }

    fn get_pull_request(&self, number: u64) -> Result<Request> {
        let mut request = parse(
            &self
                .api
                .object(&format!("{}/{number}", self.requests_endpoint()), None)?,
        )?;
        self.fetch_metadata(std::slice::from_mut(&mut request))?;
        Ok(request)
    }

    /// Schedule bounded-size detail batches in the caller's shared refresh pool.
    /// Keep isolated lookups on their existing REST path without batching overhead.
    fn get_pull_requests(&self, numbers: &[u64]) -> Vec<(u64, Result<Request>)> {
        numbers
            .par_chunks(50)
            .flat_map_iter(|batch| {
                if let [number] = batch {
                    vec![(*number, self.get_pull_request(*number))]
                } else {
                    self.get_requests_batch(batch)
                }
            })
            .collect()
    }

    fn pipeline_statuses(&self, numbers: &[u64]) -> Vec<(u64, Result<PipelineStatus>)> {
        super::pipeline::fetch(&self.api, &self.project, Provider::Github, numbers)
    }

    fn merge_pull_request(&self, number: u64) -> Result<Request> {
        let endpoint = format!("{}/{number}", self.requests_endpoint());
        let value = self.api.object(&endpoint, None)?;
        let mut request = parse(&value)?;
        if request.status == Status::Merged {
            return Ok(request);
        }
        ensure!(
            request.status == Status::Open,
            "Only open, non-draft requests can be merged"
        );
        let result = self.api.update(
            "PUT",
            &format!("{endpoint}/merge"),
            json!({
                "sha": required(&value["head"], "sha")?,
                "merge_method": self.merge_method,
            }),
        )?;
        ensure!(
            result["merged"].as_bool() == Some(true),
            "Provider did not confirm merge: {}",
            result["message"]
                .as_str()
                .unwrap_or("request remains tracked")
        );
        request.status = Status::Merged;
        Ok(request)
    }

    fn close_pull_request(&self, number: u64) -> Result<Request> {
        parse(&self.api.update(
            "PATCH",
            &format!("{}/{number}", self.requests_endpoint()),
            json!({"state": "closed"}),
        )?)
    }

    fn create_pull_request(
        &self,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<Request> {
        let payload =
            json!({"head":branch, "base":base, "title":title, "body":body, "draft":draft});
        parse(&self.api.object(&self.requests_endpoint(), Some(payload))?)
    }
}

fn project_endpoint(project: &str) -> String {
    format!(
        "repos/{}",
        project.split('/').map(encode).collect::<Vec<_>>().join("/")
    )
}

fn parse(value: &Value) -> Result<Request> {
    Ok(Request {
        number: value["number"]
            .as_u64()
            .context("Provider response has no request number")?,
        title: required(value, "title")?.into(),
        url: required(value, "html_url")?.into(),
        branch: required(&value["head"], "ref")?.into(),
        base: required(&value["base"], "ref")?.into(),
        status: status(value)?,
        approved: None,
        last_comment_at: None,
        unresolved_threads: None,
        updated_at: value
            .get("updated_at")
            .filter(|value| !value.is_null())
            .map(timestamp)
            .transpose()?,
        pipeline_status: None,
    })
}

/// Decode one aliased PR, distinguishing its field errors from unrelated failures.
/// Global errors invalidate every entry; successful neighbors remain usable otherwise.
fn parse_batch_entry(response: &Value, number: u64) -> Result<Request> {
    let alias = format!("pr{number}");
    let value = graphql_field(response, "repository", &alias)?;
    ensure!(
        value.get("reviewDecision").is_some(),
        "GitHub approval response is missing request #{number}"
    );
    let request = parse_graphql(value)?;
    ensure!(
        request.number == number,
        "GitHub returned the wrong request for #{number}"
    );
    Ok(request)
}

fn parse_graphql(value: &Value) -> Result<Request> {
    let state = required(value, "state")?;
    let status = match state {
        "MERGED" => Status::Merged,
        "CLOSED" => Status::Closed,
        "OPEN" if value["isDraft"].as_bool() == Some(true) => Status::Draft,
        "OPEN" => Status::Open,
        _ => bail!("Unknown request state: {state}"),
    };
    Ok(Request {
        number: value["number"]
            .as_u64()
            .context("Provider response has no request number")?,
        title: required(value, "title")?.into(),
        url: required(value, "url")?.into(),
        branch: required(value, "headRefName")?.into(),
        base: required(value, "baseRefName")?.into(),
        status,
        approved: Some(value["reviewDecision"].as_str() == Some("APPROVED")),
        last_comment_at: None,
        unresolved_threads: None,
        updated_at: value
            .get("updatedAt")
            .filter(|value| !value.is_null())
            .map(timestamp)
            .transpose()?,
        pipeline_status: None,
    })
}

/// Find the latest written review or inline comment; empty approvals aren't comments.
fn review_comment_time(reviews: &Value) -> Result<Option<DateTime<Utc>>> {
    let mut latest = None;
    for review in reviews["nodes"]
        .as_array()
        .context("GitHub response is missing reviews")?
    {
        if !required(review, "body")?.trim().is_empty() {
            latest = latest.max(Some(timestamp(&review["submittedAt"])?));
        }
        for comment in review["comments"]["nodes"]
            .as_array()
            .context("GitHub response is missing review comments")?
        {
            // Publication can follow creation when comments were part of a pending review.
            let date = comment
                .get("publishedAt")
                .filter(|date| !date.is_null())
                .unwrap_or(&comment["createdAt"]);
            latest = latest.max(Some(timestamp(date)?));
        }
    }
    Ok(latest)
}
