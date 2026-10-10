//! Shared semantic styles. Leave the terminal background and body text untouched.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::provider::{PipelineStatus, Request, Status};

pub(super) const ACCENT: Style = Style::new().fg(Color::LightCyan);
pub(super) const HEADING: Style = ACCENT.add_modifier(Modifier::BOLD);
pub(super) const MUTED: Style = Style::new().add_modifier(Modifier::DIM);
pub(super) const REPOSITORY: Style = Style::new().fg(Color::LightBlue);
pub(super) const HOST: Style = REPOSITORY.add_modifier(Modifier::DIM);
pub(super) const WORKSPACE: Style = Style::new().fg(Color::Cyan);
pub(super) const BRANCH: Style = Style::new().fg(Color::LightMagenta);
pub(super) const LINK: Style = ACCENT.add_modifier(Modifier::UNDERLINED);
pub(super) const SUCCESS: Style = Style::new().fg(Color::Green);
pub(super) const APPROVED: Style = Style::new()
    .fg(Color::LightGreen)
    .add_modifier(Modifier::BOLD);
pub(super) const WARNING: Style = Style::new().fg(Color::Yellow);
pub(super) const ERROR: Style = Style::new().fg(Color::LightRed);
pub(super) const SELECTION: Style = Style::new()
    .bg(Color::Rgb(39, 52, 73))
    .add_modifier(Modifier::BOLD);
// Give otherwise uncolored text a readable foreground on the selection background.
pub(super) const SELECTED_TEXT: Style = Style::new().fg(Color::White);

pub(super) fn status(status: Status) -> Style {
    match status {
        Status::Open => SUCCESS,
        Status::Draft => WARNING,
        Status::Merged => Style::new().fg(Color::Magenta),
        Status::Closed => ERROR,
    }
}

pub(super) fn request_status(request: &Request) -> Style {
    if request.status == Status::Open && request.approved == Some(true) {
        APPROVED
    } else {
        status(request.status)
    }
}

pub(super) fn pipeline_status(status: Option<PipelineStatus>) -> Style {
    match status {
        Some(PipelineStatus::Running) => WARNING,
        Some(PipelineStatus::Failed) => ERROR,
        Some(PipelineStatus::Succeed) => SUCCESS,
        None => MUTED,
    }
}

pub(super) fn unresolved_threads(count: Option<u64>) -> Style {
    match count {
        Some(0) => SUCCESS,
        Some(_) => WARNING,
        None => MUTED,
    }
}

pub(super) fn hints(bindings: &[(&'static str, &'static str, Style)]) -> Line<'static> {
    Line::from(
        bindings
            .iter()
            .flat_map(|&(key, label, style)| {
                [
                    Span::styled(key, style.add_modifier(Modifier::BOLD)),
                    Span::styled(format!(": {label}   "), MUTED),
                ]
            })
            .collect::<Vec<_>>(),
    )
}
