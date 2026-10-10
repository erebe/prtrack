use anyhow::{Context, Result};
use rayon::prelude::*;
use serde_json::{Value, json};
use std::collections::HashSet;

use super::{
    GitProvider, PipelineStatus, Provider, Remote, Request, Status,
    api::{Api, encode, graphql_field, required, status, timestamp},
    types::RequestRole,
};

mod merge;

pub(super) struct Gitlab {
    api: Api,
    project: String,
    default_branch: Option<String>,
}

impl Gitlab {
    pub(super) fn new(remote: &Remote) -> Result<Self> {
        let api = Api::new(remote);
        let repo = api.object(&format!("projects/{}", encode(&remote.project)), None)?;
        Ok(Self {
            api,
            project: required(&repo, "path_with_namespace")?.into(),
            default_branch: repo["default_branch"].as_str().map(str::to_owned),
        })
    }

    fn requests_endpoint(&self) -> String {
        format!("projects/{}/merge_requests", encode(&self.project))
    }

    /// Fetch specified MRs in all states, then batch approval and comment metadata.
    /// Preserve successful closed states and report missing MRs individually.
    fn get_requests_batch(&self, numbers: &[u64]) -> Vec<(u64, Result<Request>)> {
        let mut endpoint = format!(
            "{}?state=all&scope=all&per_page=100",
            self.requests_endpoint()
        );
        for number in numbers {
            endpoint.push_str(&format!("&iids[]={number}"));
        }
        let response = self.api.request(&endpoint, None, true);
        let mut results: Vec<_> = numbers
            .iter()
            .map(|&number| {
                let result = match &response {
                    Ok(values) => values
                        .iter()
                        .find(|value| value["iid"].as_u64() == Some(number))
                        .with_context(|| format!("GitLab response is missing request !{number}"))
                        .and_then(parse),
                    Err(error) => Err(anyhow::anyhow!("{error:#}")),
                };
                (number, result)
            })
            .collect();
        let requests: Vec<_> = results
            .iter()
            .filter_map(|(_, result)| result.as_ref().ok())
            .cloned()
            .collect();
        if requests.is_empty() {
            return results;
        }
        let metadata = self.metadata_response(&requests);
        for (_, result) in &mut results {
            if let Ok(request) = result {
                *result = match &metadata {
                    Ok(response) => parse_metadata(response, request),
                    Err(error) => Err(anyhow::anyhow!("{error:#}")),
                };
            }
        }
        results
    }

    fn list_requests(
        &self,
        user: Option<(RequestRole, &str)>,
        branch: Option<&str>,
    ) -> Result<Vec<Request>> {
        let mut endpoint = format!("{}?state=opened&per_page=100", self.requests_endpoint());
        if let Some(branch) = branch {
            endpoint.push_str(&format!("&source_branch={}", encode(branch)));
        }
        if let Some((role, username)) = user {
            let filter = match role {
                RequestRole::Author => "author_username",
                RequestRole::Assignee => "assignee_username[]",
            };
            endpoint.push_str(&format!("&scope=all&{filter}={}", encode(username)));
        }
        let mut requests = Vec::new();
        for value in self.api.request(&endpoint, None, true)? {
            if !value["source_project_id"].is_number()
                || value["source_project_id"] != value["target_project_id"]
            {
                continue;
            }
            let request = parse(&value)?;
            if branch.is_none_or(|b| b == request.branch) {
                requests.push(request);
            }
        }
        Ok(requests)
    }

    /// Enrich discovered REST records in batches while sharing the refresh worker limit.
    fn fetch_metadata(&self, requests: &mut [Request]) -> Result<()> {
        requests.par_chunks_mut(50).try_for_each(|batch| {
            let response = self.metadata_response(batch)?;
            for request in batch {
                *request = parse_metadata(&response, request)?;
            }
            Ok(())
        })
    }

    /// Fetch approvals and latest user notes together, retaining partial GraphQL data.
    /// Notes are oldest first; ONLY_COMMENTS includes replies and excludes system notes.
    fn metadata_response(&self, requests: &[Request]) -> Result<Value> {
        let fields = requests.iter().map(|request| {
            let approval = if matches!(request.status, Status::Open | Status::Draft) { "approved" } else { "" };
            format!(
                "mr{number}: mergeRequest(iid: \"{number}\") {{ {approval} resolvableDiscussionsCount resolvedDiscussionsCount notes(last: 1, filter: ONLY_COMMENTS) {{ nodes {{ createdAt }} }} }}",
                number = request.number
            )
        }).collect::<Vec<_>>().join(" ");
        let query =
            format!("query($project: ID!) {{ project(fullPath: $project) {{ {fields} }} }}");
        self.api.object(
            "graphql",
            Some(json!({"query": query, "variables": {"project": self.project}})),
        )
    }
}

impl GitProvider for Gitlab {
    fn username(&self) -> Result<String> {
        Ok(required(&self.api.object("user", None)?, "username")?.into())
    }

    fn default_branch(&self) -> Result<String> {
        self.default_branch
            .clone()
            .context("Repository has no default branch; pass --base")
    }

    fn discover_requests(&self, username: &str) -> Result<Vec<Request>> {
        // Both searches share the caller's bounded refresh pool.
        let (authored, assigned) = rayon::join(
            || self.list_requests(Some((RequestRole::Author, username)), None),
            || self.list_requests(Some((RequestRole::Assignee, username)), None),
        );
        let mut requests = authored?;
        requests.extend(assigned?);
        let mut seen = HashSet::new();
        requests.retain(|request| seen.insert(request.number));
        self.fetch_metadata(&mut requests)?;
        Ok(requests)
    }

    fn list_pull_requests(
        &self,
        author: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Vec<Request>> {
        let mut requests =
            self.list_requests(author.map(|name| (RequestRole::Author, name)), branch)?;
        self.fetch_metadata(&mut requests)?;
        Ok(requests)
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
        super::pipeline::fetch(&self.api, &self.project, Provider::Gitlab, numbers)
    }

    fn merge_pull_request(&self, number: u64) -> Result<Request> {
        self.merge_with_recovery(number)
    }

    fn close_pull_request(&self, number: u64) -> Result<Request> {
        parse(&self.api.update(
            "PUT",
            &format!("{}/{number}", self.requests_endpoint()),
            json!({"state_event": "close"}),
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
        let payload = json!({"source_branch":branch, "target_branch":base,
            "title": if draft && !title.starts_with("Draft:") { format!("Draft: {title}") } else { title.to_owned() }, "description":body});
        parse(&self.api.object(&self.requests_endpoint(), Some(payload))?)
    }
}

fn parse(value: &Value) -> Result<Request> {
    Ok(Request {
        number: value["iid"]
            .as_u64()
            .context("Provider response has no request number")?,
        title: required(value, "title")?.into(),
        url: required(value, "web_url")?.into(),
        branch: required(value, "source_branch")?.into(),
        base: required(value, "target_branch")?.into(),
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

/// Apply metadata only when complete, so partial failures retain the cached comment time.
fn parse_metadata(response: &Value, request: &Request) -> Result<Request> {
    let value = graphql_field(response, "project", &format!("mr{}", request.number))?;
    let mut request = request.clone();
    let resolvable = value["resolvableDiscussionsCount"]
        .as_u64()
        .context("GitLab response is missing resolvable discussion count")?;
    let resolved = value["resolvedDiscussionsCount"]
        .as_u64()
        .context("GitLab response is missing resolved discussion count")?;
    request.unresolved_threads = Some(
        resolvable
            .checked_sub(resolved)
            .context("GitLab resolved discussion count exceeds resolvable count")?,
    );
    if matches!(request.status, Status::Open | Status::Draft) {
        request.approved = Some(
            value["approved"]
                .as_bool()
                .context("GitLab response is missing approval state")?,
        );
    }
    request.last_comment_at = value["notes"]["nodes"]
        .as_array()
        .context("GitLab response is missing comment metadata")?
        .iter()
        .map(|note| timestamp(&note["createdAt"]))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .max();
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gitlab_draft_and_encodes_nested_paths_and_branches() {
        let value = json!({"iid":4,"title":"Draft: Test","web_url":"https://gitlab.com/a/b/-/merge_requests/4", "source_branch":"feature/a&b", "target_branch":"main", "state":"opened", "draft":true});
        assert_eq!(
            parse(&value).unwrap().status,
            crate::provider::Status::Draft
        );
        assert_eq!(encode("group/sub/repo"), "group%2Fsub%2Frepo");
        assert_eq!(encode("feature/a&b"), "feature%2Fa%26b");
    }

    #[test]
    fn rejects_missing_or_inconsistent_discussion_totals() {
        let request = parse(&json!({"iid": 4, "title": "Test", "web_url": "https://gitlab.com/a/b/-/merge_requests/4",
            "source_branch": "feature", "target_branch": "main", "state": "opened"})).unwrap();
        for (resolvable, resolved) in [
            (json!(null), json!(0)),
            (json!(1), json!(null)),
            (json!(-1), json!(0)),
            (json!(1), json!(2)),
        ] {
            let response = json!({"data": {"project": {"mr4": {
                "approved": false, "notes": {"nodes": []},
                "resolvableDiscussionsCount": resolvable, "resolvedDiscussionsCount": resolved
            }}}});
            assert!(parse_metadata(&response, &request).is_err());
        }
    }
}
