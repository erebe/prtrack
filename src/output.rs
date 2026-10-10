//! Plain-text formatting shared by CLI output and terminal views.

use crate::{provider::Provider, store::Tracked};
use chrono::{DateTime, Utc};

/// Render comment age compactly, using an explicit clock so redraws and tests agree.
/// Missing timestamps use a dash; small future offsets are treated as clock skew.
pub fn comment_age(timestamp: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(timestamp) = timestamp else {
        return "—".into();
    };
    let seconds = now.signed_duration_since(timestamp).num_seconds().max(0);
    for (duration, unit) in [
        (365 * 86_400, "y"),
        (30 * 86_400, "mo"),
        (7 * 86_400, "w"),
        (86_400, "d"),
        (3_600, "h"),
        (60, "m"),
    ] {
        if seconds >= duration {
            return format!("{}{} ago", seconds / duration, unit);
        }
    }
    "just now".into()
}

/// Keep the precise UTC timestamp available alongside the compact relative age.
pub fn comment_datetime(timestamp: Option<DateTime<Utc>>) -> String {
    timestamp
        .map(|date| date.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "—".into())
}

/// Remote titles and paths are untrusted terminal text.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn workspace_names(record: &Tracked) -> String {
    let mut names: Vec<_> = record
        .origins
        .iter()
        .filter_map(|o| o.workspace.as_ref().map(|w| w.name.as_str()))
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        "—".into()
    } else {
        names.join(", ")
    }
}

pub fn print_table(records: &[&Tracked]) {
    let now = Utc::now();
    println!(
        "STATUS\tPIPELINE\tREPOSITORY\tTITLE\tUNRESOLVED\tLAST COMMENT\tWORKSPACE\tBRANCH\tURL"
    );
    for record in records {
        println!(
            "{}{}\t{}{}\t{}{}{}\t{}\t{}{}\t{}\t{}\t{}\t{}",
            record.request.status_label(),
            if record.refresh_error.is_some() {
                "*"
            } else {
                ""
            },
            record.request.pipeline_label(),
            if record.pipeline_error.is_some() {
                "*"
            } else {
                ""
            },
            clean(&record.remote.to_string()),
            if record.remote.provider == Provider::Github {
                "#"
            } else {
                "!"
            },
            record.request.number,
            clean(&record.request.title),
            record.request.unresolved_threads_label(),
            if record.refresh_error.is_some() {
                "*"
            } else {
                ""
            },
            comment_age(record.request.last_comment_at, now),
            clean(&workspace_names(record)),
            clean(&record.request.branch),
            clean(&record.request.url)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// Pin the clock to check unit boundaries, missing data, and future clock skew.
    #[test]
    fn comment_age_uses_the_largest_elapsed_unit() {
        let now = "2026-10-11T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(comment_age(None, now), "—");
        for (seconds, expected) in [
            (-120, "just now"),
            (0, "just now"),
            (59, "just now"),
            (60, "1m ago"),
            (3599, "59m ago"),
            (3600, "1h ago"),
            (86_400, "1d ago"),
            (7 * 86_400, "1w ago"),
            (30 * 86_400, "1mo ago"),
            (365 * 86_400, "1y ago"),
        ] {
            assert_eq!(
                comment_age(Some(now - Duration::seconds(seconds)), now),
                expected
            );
        }
        assert_eq!(comment_datetime(Some(now)), "2026-10-11 12:00:00 UTC");
        assert_eq!(comment_datetime(None), "—");
    }
}
