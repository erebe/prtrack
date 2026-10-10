//! List workflow and repository/workspace filtering.

use anyhow::Result;
use std::{cmp::Reverse, io::IsTerminal};

use super::{Settings, Snapshot, refresh};
use crate::{cli::List, output, store::Tracked, tui};

pub(super) fn run(settings: Settings, args: List) -> Result<()> {
    let records = settings.store.load()?;
    if !args.json && std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
        tui::run(settings, args, records)
    } else {
        let snapshot = if args.offline {
            Snapshot {
                records,
                warnings: Vec::new(),
            }
        } else {
            refresh(&settings)?
        };
        for warning in snapshot.warnings {
            eprintln!("Warning: {}", output::clean(&warning));
        }
        let records = filtered(&snapshot.records, &args);
        if args.json {
            println!("{}", serde_json::to_string_pretty(&records)?);
        } else {
            output::print_table(&records);
        }
        Ok(())
    }
}

/// Apply filters and group visible requests by workspace, with newest updates first.
/// Sorting references gives every output mode the same order without rewriting the cache.
pub fn filtered<'a>(records: &'a [Tracked], args: &List) -> Vec<&'a Tracked> {
    let mut visible: Vec<_> = records
        .iter()
        .filter(|record| {
            args.repo
                .as_ref()
                .is_none_or(|r| r == &record.remote.project || r == &record.remote.to_string())
                && args.workspace.as_ref().is_none_or(|w| {
                    record
                        .origins
                        .iter()
                        .any(|o| o.workspace.as_ref().is_some_and(|s| &s.name == w))
                })
        })
        .collect();
    visible.sort_by_cached_key(|record| {
        let mut workspaces: Vec<_> = record
            .origins
            .iter()
            .filter_map(|origin| origin.workspace.as_ref())
            .map(|workspace| workspace.name.as_str())
            .filter(|name| {
                args.workspace
                    .as_deref()
                    .is_none_or(|filter| filter == *name)
            })
            .collect();
        workspaces.sort_unstable();
        workspaces.dedup();
        (
            workspaces.is_empty(),
            workspaces,
            Reverse(record.request.updated_at.or(record.request.last_comment_at)),
            &record.remote.host,
            &record.remote.project,
            record.request.number,
            record.remote.provider.executable(),
        )
    });
    visible
}
