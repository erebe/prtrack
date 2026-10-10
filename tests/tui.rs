#![cfg(unix)]

use std::process::Command;

fn check(provider: &str, scenario: &str) {
    let output = Command::new("python3")
        .arg("-B")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/tui.py"
        ))
        .arg(env!("CARGO_BIN_EXE_pr"))
        .arg(provider)
        .arg(scenario)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn github_refreshes_on_focus_without_overlapping_or_refreshing_offline() {
    check("github", "focus");
}

#[test]
fn gitlab_refreshes_on_focus_without_overlapping_or_refreshing_offline() {
    check("gitlab", "focus");
}

#[test]
fn github_shift_r_prepares_review_and_preserves_existing_work() {
    check("github", "review");
}

#[test]
fn gitlab_shift_r_prepares_review_and_preserves_existing_work() {
    check("gitlab", "review");
}

#[test]
fn shift_r_discovers_review_hooks_and_runs_all_with_selected_request_context() {
    check("github", "review_hooks");
}

#[test]
fn github_u_untracks_and_shift_u_closes_then_removes_tracking() {
    check("github", "close");
}

#[test]
fn gitlab_u_untracks_and_shift_u_closes_then_removes_tracking() {
    check("gitlab", "close");
}

#[test]
fn github_m_merges_and_preserves_tracking() {
    check("github", "merge");
}

#[test]
fn gitlab_m_merges_and_preserves_tracking() {
    check("gitlab", "merge");
}

/// Validate skip-CI rebases, paginated thread resolution, retry limits, and failure recovery.
#[test]
fn gitlab_merge_rebases_resolves_discussions_and_retries_once() {
    check("gitlab", "merge_recovery");
}

#[test]
fn github_ctrl_o_opens_only_filtered_open_requests() {
    check("github", "browser");
}

#[test]
fn gitlab_ctrl_o_opens_only_filtered_open_requests() {
    check("gitlab", "browser");
}

/// Verify list-only notification routing, configuration feedback, and offline behavior.
#[test]
fn github_notifications_validate_configuration_and_respect_offline() {
    check("github", "notify");
}

/// Keep the same notification behavior when the selected request belongs to GitLab.
#[test]
fn gitlab_notifications_validate_configuration_and_respect_offline() {
    check("gitlab", "notify");
}

#[test]
fn github_groups_workspaces_and_keeps_selection_when_updates_reorder_rows() {
    check("github", "ordering");
}

#[test]
fn gitlab_groups_workspaces_and_keeps_selection_when_updates_reorder_rows() {
    check("gitlab", "ordering");
}
