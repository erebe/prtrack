//! Feedback messages and notification actions for the terminal interface.

use ratatui::{style::Style, text::Span};

use super::theme;
use crate::output::clean;

/// Keep review and deployment dispatch explicit while sharing one background delivery slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NotificationKind {
    Review,
    Deploy,
}

impl NotificationKind {
    /// Identify the selected action in progress and delivery feedback.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Review => "Review",
            Self::Deploy => "Deploy",
        }
    }
}

/// Feedback text paired with its semantic presentation style.
#[derive(Clone, Default)]
pub(super) struct Message {
    text: String,
    style: Style,
}

impl Message {
    pub(super) fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: theme::ACCENT,
        }
    }

    pub(super) fn success(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: theme::SUCCESS,
        }
    }

    pub(super) fn warning(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: theme::WARNING,
        }
    }

    pub(super) fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: theme::ERROR,
        }
    }

    pub(super) fn span(&self) -> Span<'static> {
        Span::styled(clean(&self.text), self.style)
    }
}
