//! Shared identities and requests for hosted Git providers.

use chrono::{DateTime, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Supported hosted Git services.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Github,
    Gitlab,
}

/// Server-side relationship filter used during request discovery.
#[derive(Clone, Copy)]
pub(super) enum RequestRole {
    Author,
    Assignee,
}

impl Provider {
    pub fn executable(self) -> &'static str {
        match self {
            Self::Github => "gh",
            Self::Gitlab => "glab",
        }
    }
}

/// A hosted repository identity.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct Remote {
    pub provider: Provider,
    pub host: String,
    pub project: String,
}

impl fmt::Display for Remote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.host, self.project)
    }
}

/// Normalized pull/merge request lifecycle states.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Open,
    Draft,
    Merged,
    Closed,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "OPEN",
            Self::Draft => "DRAFT",
            Self::Merged => "MERGED",
            Self::Closed => "CLOSED",
        })
    }
}

/// Aggregate result of the request's current CI checks or pipeline.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PipelineStatus {
    Running,
    Failed,
    Succeed,
}

impl fmt::Display for PipelineStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Running => "RUNNING",
            Self::Failed => "FAILED",
            Self::Succeed => "SUCCEED",
        })
    }
}

/// Provider-independent pull/merge request data.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Request {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub branch: String,
    pub base: String,
    pub status: Status,
    /// None for older caches or when approval has not been fetched yet.
    #[serde(default)]
    pub approved: Option<bool>,
    /// Latest conversation/review comment; absent for no comments or older caches.
    #[serde(default)]
    pub last_comment_at: Option<DateTime<Utc>>,
    /// Unresolved review threads, counted once per thread; None until fetched.
    #[serde(default)]
    pub unresolved_threads: Option<u64>,
    /// Provider update time used for list ordering; absent in older caches.
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    /// None until the initial pipeline lookup; no attached pipeline is Succeed.
    #[serde(default)]
    pub pipeline_status: Option<PipelineStatus>,
}

impl Request {
    pub fn unresolved_threads_label(&self) -> String {
        self.unresolved_threads
            .map(|count| count.to_string())
            .unwrap_or_else(|| "—".into())
    }

    pub fn pipeline_label(&self) -> String {
        self.pipeline_status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "—".into())
    }

    pub fn status_label(&self) -> String {
        if self.status == Status::Open && self.approved == Some(true) {
            "APPROVED".into()
        } else {
            self.status.to_string()
        }
    }
}
