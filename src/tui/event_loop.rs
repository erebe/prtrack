use super::{
    requests,
    types::{Message, NotificationKind},
};

use crate::{
    app::{self, Settings, Snapshot},
    cli::List,
    notify::Notify,
    provider::Status,
    store::Tracked,
};
use anyhow::Result;
use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{
            self, DisableFocusChange, EnableFocusChange, Event, KeyCode, KeyEventKind, KeyModifiers,
        },
        execute,
    },
    widgets::TableState,
};
use std::{
    collections::HashSet,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

pub fn run(settings: Settings, args: List, records: Vec<Tracked>) -> Result<()> {
    let mut terminal = ratatui::init();
    scopeguard::defer! {
        let _ = execute!(std::io::stdout(), DisableFocusChange);
        ratatui::restore();
    }
    execute!(std::io::stdout(), EnableFocusChange)?;
    run_loop(&mut terminal, settings, args, records)
}

fn start_refresh(settings: Settings) -> Receiver<Result<Snapshot>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(app::refresh(&settings));
    });
    receiver
}

/// Send the selected URL off the event loop so Slack latency never blocks navigation.
fn start_notification(
    notifier: Box<dyn Notify>,
    url: String,
    kind: NotificationKind,
) -> Receiver<Result<()>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = match kind {
            NotificationKind::Review => notifier.request_review(&url),
            NotificationKind::Deploy => notifier.request_deploy(&url),
        };
        let _ = sender.send(result);
    });
    receiver
}

fn start_browser(urls: Vec<String>) -> Receiver<Message> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let total = urls.len();
        let mut failures = Vec::new();
        for url in urls {
            if let Err(error) = webbrowser::open(&url) {
                failures.push(format!("{url}: {error}"));
            }
        }
        let message = if failures.is_empty() {
            Message::success(if total == 1 {
                "Opened request in browser".into()
            } else {
                format!("Opened {total} requests in browser")
            })
        } else {
            let opened = total - failures.len();
            let text = format!(
                "Opened {opened} of {total} requests; could not open browser for {}. Open failed links manually.",
                failures.join(" | ")
            );
            if opened == 0 {
                Message::error(text)
            } else {
                Message::warning(text)
            }
        };
        let _ = sender.send(message);
    });
    receiver
}

fn run_loop(
    terminal: &mut DefaultTerminal,
    settings: Settings,
    args: List,
    mut records: Vec<Tracked>,
) -> Result<()> {
    let mut refresh = if args.offline {
        None
    } else {
        Some(start_refresh(settings.clone()))
    };
    let mut message = if args.offline {
        Message::warning("Offline: showing cached statuses")
    } else {
        Message::info("Refreshing requests…")
    };
    let mut state = TableState::default().with_selected(Some(0));
    let mut browser: Option<Receiver<Message>> = None;
    let mut closing: Option<(Tracked, Receiver<Result<()>>)> = None;
    let mut merging: Option<(Tracked, Receiver<Result<Tracked>>)> = None;
    let mut notifying: Option<(NotificationKind, String, Receiver<Result<()>>)> = None;
    let mut reviewing: Option<Receiver<Result<String>>> = None;
    let mut merged_during_refresh: Vec<Tracked> = Vec::new();
    let mut quit_after_action = false;
    let mut removed_during_refresh: Vec<Tracked> = Vec::new();
    loop {
        // Background updates can move a row between workspace/date groups. Keep actions
        // attached to the selected request instead of whichever row takes its old index.
        let selected = state
            .selected()
            .filter(|_| refresh.is_some() || merging.is_some() || closing.is_some())
            .and_then(|index| {
                app::filtered(&records, &args)
                    .get(index)
                    .map(|record| (record.remote.clone(), record.request.number))
            });
        if let Some(receiver) = &browser {
            match receiver.try_recv() {
                Ok(result) => {
                    message = result;
                    browser = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    message = Message::error("Browser launcher stopped; press o to retry");
                    browser = None;
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        if let Some(receiver) = &refresh {
            match receiver.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(snapshot) => {
                            records = snapshot.records;
                            for merged in &merged_during_refresh {
                                Tracked::merge_into(&mut records, merged.clone());
                            }
                            // The worker may have saved its result before the deletion,
                            // but delivered it afterwards. Keep that stale row hidden.
                            records.retain(|record| {
                                !removed_during_refresh
                                    .iter()
                                    .any(|removed| removed.same_request(record))
                            });
                            message = if snapshot.warnings.is_empty() {
                                Message::success("Statuses refreshed")
                            } else {
                                Message::warning(format!(
                                    "{} warning(s): {}",
                                    snapshot.warnings.len(),
                                    snapshot.warnings.join(" | ")
                                ))
                            };
                        }
                        Err(error) => {
                            message = Message::error(format!(
                                "Refresh failed; showing cached statuses: {error:#}"
                            ))
                        }
                    }
                    refresh = None;
                    removed_during_refresh.clear();
                    merged_during_refresh.clear();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    message = Message::error("Refresh worker stopped; press r to retry");
                    refresh = None;
                    removed_during_refresh.clear();
                    merged_during_refresh.clear();
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        if let Some((record, receiver)) = &closing {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                    "Close worker stopped; press Shift+U to retry"
                ))),
            };
            if let Some(result) = result {
                match result {
                    Ok(()) => {
                        records.retain(|item| !item.same_request(record));
                        if refresh.is_some() {
                            removed_during_refresh.push(record.clone());
                        }
                        message = Message::success(format!(
                            "Closed and stopped tracking {} #{}",
                            record.remote, record.request.number
                        ));
                    }
                    Err(error) => {
                        if quit_after_action {
                            return Err(error);
                        }
                        message = Message::error(format!(
                            "Close/remove failed for {} #{}: {error:#}",
                            record.remote, record.request.number
                        ))
                    }
                }
                closing = None;
            }
        }
        if let Some((record, receiver)) = &merging {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                    "Merge worker stopped; press m to retry"
                ))),
            };
            if let Some(result) = result {
                match result {
                    Ok(merged) => {
                        Tracked::merge_into(&mut records, merged.clone());
                        if refresh.is_some() {
                            merged_during_refresh.push(merged.clone());
                        }
                        message = Message::success(format!(
                            "Merged {} #{}",
                            record.remote, record.request.number
                        ));
                    }
                    Err(error) => {
                        if quit_after_action {
                            return Err(error);
                        }
                        message = Message::error(format!(
                            "Merge failed for {} #{}: {error:#}",
                            record.remote, record.request.number
                        ));
                    }
                }
                merging = None;
            }
        }
        if let Some((kind, url, receiver)) = &notifying {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                    "Notification worker stopped; check the channel before retrying"
                ))),
            };
            if let Some(result) = result {
                message = match result {
                    Ok(()) => {
                        Message::success(format!("{} notification sent for {url}", kind.label()))
                    }
                    Err(error) => {
                        if quit_after_action {
                            return Err(error);
                        }
                        Message::error(format!("Notification failed: {error:#}"))
                    }
                };
                notifying = None;
            }
        }
        if let Some(receiver) = &reviewing {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                    "Review worker stopped; check hook effects before retrying Shift+R"
                ))),
            };
            if let Some(result) = result {
                message = match result {
                    Ok(text) => Message::success(text),
                    Err(error) => {
                        if quit_after_action {
                            return Err(error);
                        }
                        Message::error(format!("Review preparation failed: {error:#}"))
                    }
                };
                reviewing = None;
            }
        }
        if quit_after_action
            && closing.is_none()
            && merging.is_none()
            && notifying.is_none()
            && reviewing.is_none()
        {
            return Ok(());
        }
        let visible = app::filtered(&records, &args);
        if visible.is_empty() {
            state.select(None);
        } else {
            let restored = selected.and_then(|(remote, number)| {
                visible
                    .iter()
                    .position(|record| record.remote == remote && record.request.number == number)
            });
            state.select(Some(restored.unwrap_or_else(|| {
                state.selected().unwrap_or(0).min(visible.len() - 1)
            })));
        }
        terminal.draw(|frame| {
            requests::draw(frame, &visible, &mut state, &message, refresh.is_some());
        })?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let event = event::read()?;
        if event == Event::FocusGained && !args.offline && refresh.is_none() && !quit_after_action {
            message = Message::info("Refreshing requests…");
            refresh = Some(start_refresh(settings.clone()));
        }
        if let Event::Key(key) = event {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let quit = key.code == KeyCode::Char('c')
                && key.modifiers.contains(KeyModifiers::CONTROL)
                || matches!(key.code, KeyCode::Char('q') | KeyCode::Esc);
            if quit {
                if closing.is_none()
                    && merging.is_none()
                    && notifying.is_none()
                    && reviewing.is_none()
                {
                    return Ok(());
                }
                quit_after_action = true;
                message = Message::info("Waiting for request action to finish before quitting…");
                continue;
            }
            if key.code == KeyCode::Char('R')
                || (key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::SHIFT))
            {
                if args.offline {
                    message = Message::warning(
                        "Cannot prepare review offline; run pr list without --offline",
                    );
                } else if reviewing.is_some()
                    || closing.is_some()
                    || merging.is_some()
                    || notifying.is_some()
                    || quit_after_action
                {
                    message = Message::warning("A request action is already in progress");
                } else if let Some(record) = state.selected().and_then(|i| visible.get(i)) {
                    let record = (*record).clone();
                    message = Message::info(format!(
                        "Preparing review for {} #{}…",
                        record.remote, record.request.number
                    ));
                    let review_settings = settings.clone();
                    let (sender, receiver) = mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(app::prepare_review(&review_settings, &record));
                    });
                    reviewing = Some(receiver);
                } else {
                    message = Message::info("No request selected");
                }
                continue;
            }
            if key.code == KeyCode::Char('U')
                || (key.code == KeyCode::Char('u') && key.modifiers.contains(KeyModifiers::SHIFT))
            {
                let record = state.selected().and_then(|i| visible.get(i)).copied();
                if args.offline {
                    message = Message::warning(
                        "Cannot close requests offline; run pr list without --offline",
                    );
                } else if closing.is_some()
                    || merging.is_some()
                    || notifying.is_some()
                    || reviewing.is_some()
                {
                    message = Message::warning("A request action is already in progress");
                } else if let Some(record) = record.cloned() {
                    message = Message::info(format!(
                        "Closing {} #{}…",
                        record.remote, record.request.number
                    ));
                    let store = settings.store.clone();
                    let target = record.clone();
                    let (sender, receiver) = mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(app::close_request(store.as_ref(), &target));
                    });
                    closing = Some((record, receiver));
                }
                continue;
            }
            if key.code == KeyCode::Char('m') && key.modifiers.is_empty() {
                let record = state.selected().and_then(|i| visible.get(i)).copied();
                if args.offline {
                    message = Message::warning(
                        "Cannot merge requests offline; run pr list without --offline",
                    );
                } else if closing.is_some()
                    || merging.is_some()
                    || notifying.is_some()
                    || reviewing.is_some()
                {
                    message = Message::warning("A request action is already in progress");
                } else if let Some(record) = record.cloned() {
                    message = Message::info(format!(
                        "Merging {} #{}…",
                        record.remote, record.request.number
                    ));
                    let store = settings.store.clone();
                    let target = record.clone();
                    let (sender, receiver) = mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(app::merge_request(store.as_ref(), &target));
                    });
                    merging = Some((record, receiver));
                } else {
                    message = Message::info("No request selected");
                }
                continue;
            }
            if key.code == KeyCode::Char('o') && browser.is_none() {
                let all_open = key.modifiers.contains(KeyModifiers::CONTROL);
                let urls: Vec<_> = if all_open {
                    let mut seen = HashSet::new();
                    visible
                        .iter()
                        .filter(|record| record.request.status == Status::Open)
                        .map(|record| record.request.url.clone())
                        .filter(|url| seen.insert(url.clone()))
                        .collect()
                } else {
                    state
                        .selected()
                        .and_then(|i| visible.get(i))
                        .map(|record| record.request.url.clone())
                        .into_iter()
                        .collect()
                };
                if urls.is_empty() {
                    message = Message::info(if all_open {
                        "No OPEN requests in the current list"
                    } else {
                        "No request selected"
                    });
                } else {
                    message = Message::info(if all_open {
                        format!("Opening {} OPEN requests in browser…", urls.len())
                    } else {
                        "Opening request in browser…".into()
                    });
                    browser = Some(start_browser(urls));
                }
                continue;
            }
            match key.code {
                KeyCode::Char('n' | 'd') if key.modifiers.is_empty() => {
                    let kind = if key.code == KeyCode::Char('d') {
                        NotificationKind::Deploy
                    } else {
                        NotificationKind::Review
                    };
                    if args.offline {
                        message = Message::warning(
                            "Cannot notify offline; run pr list without --offline",
                        );
                    } else if notifying.is_some() {
                        message = Message::info("A notification is already in progress");
                    } else if closing.is_some()
                        || merging.is_some()
                        || reviewing.is_some()
                        || quit_after_action
                    {
                        message = Message::warning("A request action is already in progress");
                    } else if let Some(record) = state.selected().and_then(|i| visible.get(i)) {
                        match <dyn Notify>::from_config(&settings.slack) {
                            Ok(notifier) => {
                                let url = record.request.url.clone();
                                message = Message::info(format!(
                                    "{} notification: sending {url}…",
                                    kind.label()
                                ));
                                let receiver = start_notification(notifier, url.clone(), kind);
                                notifying = Some((kind, url, receiver));
                            }
                            Err(error) => {
                                message = Message::error(format!("Notification failed: {error:#}"))
                            }
                        }
                    } else {
                        message = Message::info("No request selected");
                    }
                }
                KeyCode::Down | KeyCode::Char('j') if !visible.is_empty() => state.select(Some(
                    (state.selected().unwrap_or(0) + 1).min(visible.len() - 1),
                )),
                KeyCode::Up | KeyCode::Char('k') => {
                    state.select(Some(state.selected().unwrap_or(0).saturating_sub(1)))
                }
                KeyCode::Home | KeyCode::Char('g') => state.select(Some(0)),
                KeyCode::End | KeyCode::Char('G') => state.select(visible.len().checked_sub(1)),
                KeyCode::Char('u')
                    if key.modifiers.is_empty() && merging.is_none() && reviewing.is_none() =>
                {
                    if let Some(record) = state
                        .selected()
                        .and_then(|i| visible.get(i))
                        .copied()
                        .cloned()
                    {
                        match settings.store.remove(&record) {
                            Ok(saved) => {
                                message = Message::success(format!(
                                    "Stopped tracking {} #{}",
                                    record.remote, record.request.number
                                ));
                                records = saved;
                                if refresh.is_some() {
                                    removed_during_refresh.push(record);
                                }
                            }
                            Err(error) => {
                                message = Message::error(format!(
                                    "Could not remove request from tracking: {error:#}"
                                ))
                            }
                        }
                    }
                }
                KeyCode::Char('r')
                    if key.modifiers.is_empty()
                        && refresh.is_none()
                        && !args.offline
                        && !quit_after_action =>
                {
                    message = Message::info("Refreshing requests…");
                    refresh = Some(start_refresh(settings.clone()));
                }
                _ => (),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestNotify {
        captured: mpsc::Sender<(NotificationKind, String)>,
        release: mpsc::Receiver<bool>,
    }

    impl TestNotify {
        /// Hold delivery until released to verify the event loop can keep running.
        fn deliver(&self, kind: NotificationKind, url: &str) -> Result<()> {
            self.captured.send((kind, url.to_owned()))?;
            anyhow::ensure!(
                self.release.recv_timeout(Duration::from_secs(3))?,
                "delivery failed"
            );
            Ok(())
        }
    }

    impl Notify for TestNotify {
        /// Capture review dispatch separately so deployment cannot silently use it.
        fn request_review(&self, url: &str) -> Result<()> {
            self.deliver(NotificationKind::Review, url)
        }

        /// Capture deployment dispatch through the same asynchronous delivery path.
        fn request_deploy(&self, url: &str) -> Result<()> {
            self.deliver(NotificationKind::Deploy, url)
        }
    }

    /// Capture the selected URL once and report background success or failure to the UI.
    #[test]
    fn notification_worker_returns_immediately_and_preserves_result() {
        for (kind, succeed) in [
            (NotificationKind::Review, true),
            (NotificationKind::Review, false),
            (NotificationKind::Deploy, true),
            (NotificationKind::Deploy, false),
        ] {
            let (captured, urls) = mpsc::channel();
            let (release, waiting) = mpsc::channel();
            let receiver = start_notification(
                Box::new(TestNotify {
                    captured,
                    release: waiting,
                }),
                "https://github.com/owner/repo/pull/7".into(),
                kind,
            );
            assert_eq!(
                urls.recv_timeout(Duration::from_secs(3)).unwrap(),
                (kind, "https://github.com/owner/repo/pull/7".into())
            );
            assert!(matches!(
                receiver.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
            release.send(succeed).unwrap();
            assert_eq!(
                receiver
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .is_ok(),
                succeed
            );
        }
    }
}
