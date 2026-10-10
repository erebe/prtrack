//! Query only the selected requests' current CI aggregates, independently of PR metadata.

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde_json::{Value, json};

use super::{
    PipelineStatus, Provider,
    api::{Api, graphql_field, required},
};

/// Batch pipeline-only lookups in the caller's shared pool, preserving partial failures.
/// Separate queries let refresh omit successful requests entirely from CI polling.
pub(super) fn fetch(
    api: &Api,
    project: &str,
    provider: Provider,
    numbers: &[u64],
) -> Vec<(u64, Result<PipelineStatus>)> {
    numbers
        .par_chunks(50)
        .flat_map_iter(|batch| {
            let (parent, prefix) = match provider {
                Provider::Github => ("repository", "pr"),
                Provider::Gitlab => ("project", "mr"),
            };
            let response = query(api, project, provider, batch);
            batch
                .iter()
                .map(|&number| {
                    let result = match &response {
                        Ok(response) => {
                            graphql_field(response, parent, &format!("{prefix}{number}"))
                                .and_then(|value| parse(value, provider))
                        }
                        Err(error) => Err(anyhow::anyhow!("{error:#}")),
                    };
                    (number, result)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Ask the provider for its native aggregate, avoiding individual job/check API calls.
/// GitHub includes checks and commit statuses; GitLab aggregates the head pipeline's jobs.
fn query(api: &Api, project: &str, provider: Provider, numbers: &[u64]) -> Result<Value> {
    let (query, variables) = match provider {
        Provider::Github => {
            let (owner, name) = project
                .split_once('/')
                .context("Invalid GitHub repository")?;
            let fields = numbers.iter().map(|number| format!(
                "pr{number}: pullRequest(number: {number}) {{ commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state }} }} }} }} }}"
            )).collect::<Vec<_>>().join(" ");
            (
                format!(
                    "query($owner: String!, $name: String!) {{ repository(owner: $owner, name: $name) {{ {fields} }} }}"
                ),
                json!({"owner": owner, "name": name}),
            )
        }
        Provider::Gitlab => {
            let fields = numbers.iter().map(|number| format!(
                "mr{number}: mergeRequest(iid: \"{number}\") {{ headPipeline {{ status }} }}"
            )).collect::<Vec<_>>().join(" ");
            (
                format!("query($project: ID!) {{ project(fullPath: $project) {{ {fields} }} }}"),
                json!({"project": project}),
            )
        }
    };
    // Do not reject the whole response when GraphQL reports an error for one alias.
    api.object(
        "graphql",
        Some(json!({"query": query, "variables": variables})),
    )
}

/// Normalize active, failed, and successful aggregates without treating bad data as success.
/// Explicit null means no pipeline; blocked/manual work stays active so it is polled again.
fn parse(value: &Value, provider: Provider) -> Result<PipelineStatus> {
    let (pipeline, field) = match provider {
        Provider::Github => {
            let commits = value["commits"]["nodes"]
                .as_array()
                .context("GitHub pipeline response is missing commits")?;
            let Some(commit) = commits.last() else {
                return Ok(PipelineStatus::Succeed);
            };
            (
                commit["commit"]
                    .get("statusCheckRollup")
                    .context("GitHub pipeline response is missing statusCheckRollup")?,
                "state",
            )
        }
        Provider::Gitlab => (
            value
                .get("headPipeline")
                .context("GitLab pipeline response is missing headPipeline")?,
            "status",
        ),
    };
    if pipeline.is_null() {
        return Ok(PipelineStatus::Succeed);
    }
    let state = required(pipeline, field)?;
    match state {
        "SUCCESS" | "SKIPPED" => Ok(PipelineStatus::Succeed),
        "ERROR" | "FAILURE" | "FAILED" | "CANCELED" => Ok(PipelineStatus::Failed),
        "EXPECTED"
        | "PENDING"
        | "CREATED"
        | "PREPARING"
        | "RUNNING"
        | "MANUAL"
        | "SCHEDULED"
        | "WAITING_FOR_RESOURCE"
        | "WAITING_FOR_CALLBACK"
        | "CANCELING" => Ok(PipelineStatus::Running),
        _ => bail!("Unknown pipeline status: {state}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_provider_aggregates_and_absent_pipelines() {
        for (state, expected) in [
            ("PENDING", PipelineStatus::Running),
            ("EXPECTED", PipelineStatus::Running),
            ("RUNNING", PipelineStatus::Running),
            ("MANUAL", PipelineStatus::Running),
            ("WAITING_FOR_RESOURCE", PipelineStatus::Running),
            ("FAILURE", PipelineStatus::Failed),
            ("ERROR", PipelineStatus::Failed),
            ("FAILED", PipelineStatus::Failed),
            ("CANCELED", PipelineStatus::Failed),
            ("SUCCESS", PipelineStatus::Succeed),
            ("SKIPPED", PipelineStatus::Succeed),
        ] {
            assert_eq!(parse(&json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": state}}}]}}), Provider::Github).unwrap(), expected);
            assert_eq!(
                parse(
                    &json!({"headPipeline": {"status": state}}),
                    Provider::Gitlab
                )
                .unwrap(),
                expected
            );
        }
        assert_eq!(
            parse(
                &json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": null}}]}}),
                Provider::Github
            )
            .unwrap(),
            PipelineStatus::Succeed
        );
        assert_eq!(
            parse(&json!({"headPipeline": null}), Provider::Gitlab).unwrap(),
            PipelineStatus::Succeed
        );
    }

    #[test]
    fn rejects_missing_or_unknown_pipeline_data() {
        for provider in [Provider::Github, Provider::Gitlab] {
            for value in [
                json!({}),
                json!({"commits": {"nodes": [{"commit": {}}]}, "headPipeline": {}}),
                json!({"commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "NEW_STATE"}}}]}, "headPipeline": {"status": "NEW_STATE"}}),
            ] {
                assert!(parse(&value, provider).is_err());
            }
        }
    }
}
