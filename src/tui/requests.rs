use super::{theme, types::Message};

use crate::{
    output::{self, clean},
    provider::{Provider, Status},
    store::Tracked,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap},
};

pub(super) fn draw(
    frame: &mut Frame,
    records: &[&Tracked],
    state: &mut TableState,
    message: &Message,
    refreshing: bool,
) {
    let now = chrono::Utc::now();
    let areas = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(10),
        Constraint::Length(3),
    ])
    .split(frame.area());
    let mut summary = vec![
        Span::styled(" pr ", theme::HEADING),
        Span::raw(format!(" {} requests", records.len())),
    ];
    for status in [Status::Open, Status::Draft, Status::Merged, Status::Closed] {
        let count = records
            .iter()
            .filter(|r| r.request.status == status)
            .count();
        summary.push(Span::styled(" · ", theme::MUTED));
        summary.push(Span::styled(
            format!("{count} {}", status.to_string().to_lowercase()),
            theme::status(status),
        ));
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(summary),
            Line::styled(if refreshing { " Refreshing…" } else { "" }, theme::ACCENT),
        ]),
        areas[0],
    );
    if records.is_empty() {
        frame.render_widget(Paragraph::new("No matching requests. Run pr push in a feature branch, or refresh to discover your open requests in Grove and the current repository.")
            .style(theme::MUTED).wrap(Wrap { trim: true }).block(Block::bordered().border_style(theme::ACCENT).title(Line::styled(" Pull / Merge Requests ", theme::HEADING))), areas[1]);
    } else {
        let rows = records.iter().enumerate().map(|(index, record)| {
            let request = &record.request;
            let host = match record.remote.host.as_str() {
                "github.com" => "gh",
                "gitlab.com" => "gl",
                host => host,
            };
            let workspace = output::workspace_names(record);
            let workspace_style = if record.origins.iter().any(|o| o.workspace.is_some()) {
                theme::WORKSPACE
            } else if state.selected() == Some(index) {
                theme::SELECTED_TEXT.patch(theme::MUTED)
            } else {
                theme::MUTED
            };
            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(request.status_label(), theme::request_status(request)),
                    Span::styled(
                        if record.refresh_error.is_some() {
                            "*"
                        } else {
                            ""
                        },
                        theme::WARNING,
                    ),
                ])),
                Cell::from(Line::from(vec![
                    Span::styled(
                        request.pipeline_label(),
                        theme::pipeline_status(request.pipeline_status),
                    ),
                    Span::styled(
                        if record.pipeline_error.is_some() {
                            "*"
                        } else {
                            ""
                        },
                        theme::WARNING,
                    ),
                ])),
                Cell::from(Line::from(vec![
                    Span::styled(format!("{}/", clean(host)), theme::HOST),
                    Span::styled(clean(&record.remote.project), theme::REPOSITORY),
                    Span::styled(
                        format!(
                            "{}{}",
                            if record.remote.provider == Provider::Github {
                                "#"
                            } else {
                                "!"
                            },
                            request.number
                        ),
                        theme::ACCENT,
                    ),
                ])),
                Cell::from(clean(&request.title)).style(if state.selected() == Some(index) {
                    theme::SELECTED_TEXT
                } else {
                    Style::default()
                }),
                Cell::from(Line::from(vec![
                    Span::styled(
                        request.unresolved_threads_label(),
                        theme::unresolved_threads(request.unresolved_threads),
                    ),
                    Span::styled(
                        if record.refresh_error.is_some() {
                            "*"
                        } else {
                            ""
                        },
                        theme::WARNING,
                    ),
                ])),
                Cell::from(output::comment_age(request.last_comment_at, now)).style(theme::MUTED),
                Cell::from(clean(&workspace)).style(workspace_style),
                Cell::from(clean(&request.branch)).style(theme::BRANCH),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Percentage(24),
                Constraint::Min(10),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Percentage(12),
                Constraint::Percentage(13),
            ],
        )
        .header(
            Row::new([
                Cell::from("Status").style(theme::ACCENT),
                Cell::from("Pipeline").style(theme::ACCENT),
                Cell::from("Repository").style(theme::REPOSITORY),
                Cell::from("Title"),
                Cell::from("Unresolved").style(theme::ACCENT),
                Cell::from("Last comment").style(theme::MUTED),
                Cell::from("Workspace").style(theme::WORKSPACE),
                Cell::from("Branch").style(theme::BRANCH),
            ])
            .bold(),
        )
        .block(
            Block::bordered()
                .border_style(theme::ACCENT)
                .title(Line::styled(" Pull / Merge Requests ", theme::HEADING)),
        )
        .row_highlight_style(theme::SELECTION)
        .highlight_symbol(Span::styled("› ", theme::HEADING));
        frame.render_stateful_widget(table, areas[1], state);
    }
    let details = state
        .selected()
        .and_then(|i| records.get(i))
        .map(|record| {
            let sources = record
                .origins
                .iter()
                .map(|o| o.repository.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let workspaces = record
                .origins
                .iter()
                .filter_map(|o| o.workspace.as_ref().map(|w| w.path.display().to_string()))
                .collect::<Vec<_>>()
                .join(", ");
            let checkouts = record
                .origins
                .iter()
                .map(|o| o.checkout.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            vec![
                Line::from(clean(&record.request.title)).bold(),
                Line::from(vec![
                    Span::styled("Link: ", theme::MUTED),
                    Span::styled(clean(&record.request.url), theme::LINK),
                ]),
                Line::from(vec![
                    Span::styled("Last comment: ", theme::MUTED),
                    Span::raw(output::comment_datetime(record.request.last_comment_at)),
                ]),
                Line::from(vec![
                    Span::styled(clean(&record.request.branch), theme::BRANCH),
                    Span::styled(" → ", theme::MUTED),
                    Span::styled(clean(&record.request.base), theme::BRANCH),
                ]),
                Line::from(vec![
                    Span::styled("Repository source: ", theme::MUTED),
                    Span::styled(clean(&sources), theme::REPOSITORY),
                ]),
                Line::from(vec![
                    Span::styled("Workspace source: ", theme::MUTED),
                    if workspaces.is_empty() {
                        Span::styled("—", theme::MUTED)
                    } else {
                        Span::styled(clean(&workspaces), theme::WORKSPACE)
                    },
                ]),
                Line::from(vec![
                    Span::styled("Checkout: ", theme::MUTED),
                    Span::styled(clean(&checkouts), theme::REPOSITORY),
                ]),
                Line::styled(
                    [
                        record.refresh_error.as_deref(),
                        record.pipeline_error.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .map(|e| format!("Cached status: {}", clean(e)))
                    .collect::<Vec<_>>()
                    .join(" · "),
                    theme::WARNING,
                ),
            ]
        })
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(details).block(Block::bordered().border_style(theme::ACCENT).title(
            Line::styled(" Selected request · o: browser ", theme::HEADING),
        )),
        areas[2],
    );
    let mut hints = theme::hints(&[
        ("↑/↓ j/k", "select", theme::ACCENT),
        ("n", "notify review", theme::ACCENT),
        ("d", "deploy", theme::WARNING),
        ("o", "browser", theme::ACCENT),
        ("Ctrl+O", "open all OPEN", theme::ACCENT),
        ("m", "merge", theme::SUCCESS),
        ("u", "untrack", theme::WARNING),
        ("Shift+U", "close/remove", theme::ERROR),
        ("g/G", "first/last", theme::ACCENT),
        ("r", "refresh", theme::ACCENT),
        ("Shift+R", "review hooks", theme::ACCENT),
        ("q/Esc", "quit", theme::ACCENT),
    ]);
    hints
        .spans
        .push(Span::styled("* cached after error", theme::WARNING));
    frame.render_widget(
        Paragraph::new(vec![hints, Line::from(message.span())]).wrap(Wrap { trim: true }),
        areas[3],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grove::{Origin, Workspace};
    use crate::provider::{PipelineStatus, Remote, Request};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn empty_and_tiny_terminal_render_without_panicking() {
        for (width, height) in [(120, 30), (20, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| {
                    draw(
                        f,
                        &[],
                        &mut TableState::default(),
                        &Message::warning("Offline"),
                        false,
                    )
                })
                .unwrap();
            if width == 120 {
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                assert!(text.contains("No matching requests"));
            }
        }
    }

    #[test]
    fn table_and_details_show_status_and_source_paths() {
        let mut record = Tracked {
            remote: Remote {
                provider: Provider::Gitlab,
                host: "gitlab.com".into(),
                project: "team/repo".into(),
            },
            request: Request {
                number: 7,
                title: "Add tracking".into(),
                url: "https://gitlab.com/team/repo/-/merge_requests/7".into(),
                branch: "feature".into(),
                base: "main".into(),
                status: Status::Merged,
                approved: None,
                last_comment_at: Some(chrono::Utc::now() - chrono::Duration::minutes(200)),
                unresolved_threads: None,
                updated_at: None,
                pipeline_status: None,
            },
            origins: vec![Origin {
                repository: "/source/repo".into(),
                checkout: "/grove/workspaces/feature/repo".into(),
                workspace: Some(Workspace {
                    name: "feature".into(),
                    path: "/grove/workspaces/feature".into(),
                }),
            }],
            checked_at: 1,
            refresh_error: None,
            pipeline_error: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
        terminal
            .draw(|f| {
                draw(
                    f,
                    &[&record],
                    &mut TableState::default().with_selected(Some(0)),
                    &Message::success("Refreshed"),
                    false,
                )
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        for expected in [
            "MERGED",
            "Pipeline",
            "gl/team/repo!7",
            "Repository source: /source/repo",
            "Workspace source: /grove/workspaces/feature",
            "merge_requests/7",
            "Last comment",
            "Unresolved",
            "3h ago",
            &output::comment_datetime(record.request.last_comment_at),
        ] {
            assert!(text.contains(expected), "Missing {expected}");
        }
        for (status, label, style) in [
            (PipelineStatus::Running, "RUNNING", theme::WARNING),
            (PipelineStatus::Failed, "FAILED", theme::ERROR),
            (PipelineStatus::Succeed, "SUCCEED", theme::SUCCESS),
        ] {
            record.request.pipeline_status = Some(status);
            terminal
                .draw(|f| {
                    draw(
                        f,
                        &[&record],
                        &mut TableState::default(),
                        &Message::success("Refreshed"),
                        false,
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let cells = &buffer.content;
            let start = cells
                .windows(label.len())
                .position(|cells| {
                    cells.iter().map(|cell| cell.symbol()).collect::<String>() == label
                })
                .unwrap();
            assert_eq!(cells[start].fg, style.fg.unwrap());
            let header: String = (0..160).map(|x| buffer[(x, 3)].symbol()).collect();
            assert!(header.find("Pipeline").unwrap() < header.find("Repository").unwrap());
            assert!(header.find("Repository").unwrap() < header.find("Title").unwrap());
        }
        for (count, stale, label, style) in [
            (None, false, "—", theme::MUTED),
            (Some(0), false, "0", theme::SUCCESS),
            (Some(12), false, "12", theme::WARNING),
            (Some(12), true, "12*", theme::WARNING),
        ] {
            record.request.unresolved_threads = count;
            record.refresh_error = stale.then(|| "Request metadata unavailable".into());
            terminal
                .draw(|f| {
                    draw(
                        f,
                        &[&record],
                        &mut TableState::default(),
                        &Message::default(),
                        false,
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let column = (0..150)
                .find(|&x| {
                    (x..x + 10)
                        .map(|x| buffer[(x, 3)].symbol())
                        .collect::<String>()
                        == "Unresolved"
                })
                .unwrap();
            let value = (column..column + 10)
                .map(|x| buffer[(x, 4)].symbol())
                .collect::<String>();
            assert_eq!(value.trim(), label);
            assert_eq!(buffer[(column, 4)].fg, style.fg.unwrap_or_default());
            assert!(buffer[(column, 4)].modifier.contains(style.add_modifier));
        }
    }
}
