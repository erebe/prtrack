//! Run user-maintained review hooks without depending on a particular review tool.

use anyhow::{Context, Result, ensure};
use std::{fs, io::ErrorKind, path::Path, process::Stdio};

use super::Settings;
use crate::{output, process, provider::Provider, store::Tracked};

/// Discover hooks on each invocation so users can extend a running list session.
pub fn prepare_review(settings: &Settings, record: &Tracked) -> Result<String> {
    let mut hooks = Vec::new();
    match fs::read_dir(&settings.hook_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.context("Cannot read review hook entry")?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("review-") && name.len() > "review-".len() {
                    let path = entry.path();
                    if is_executable_file(&path)? {
                        hooks.push(path);
                    }
                }
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => (),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Cannot read review hooks in {}",
                    settings.hook_dir.display()
                )
            });
        }
    }
    ensure!(
        !hooks.is_empty(),
        "No executable review-* hooks in {}",
        settings.hook_dir.display()
    );
    hooks.sort();

    let mut failures = Vec::new();
    let mut last_output = String::new();
    for hook in &hooks {
        let name = hook.file_name().unwrap().to_string_lossy();
        let result = process::command(hook)
            .current_dir(&settings.cwd)
            .env(
                "PR_PROVIDER",
                match record.remote.provider {
                    Provider::Github => "github",
                    Provider::Gitlab => "gitlab",
                },
            )
            .env("PR_HOST", &record.remote.host)
            .env("PR_PROJECT", &record.remote.project)
            .env("PR_NUMBER", record.request.number.to_string())
            .env("PR_URL", &record.request.url)
            .env("PR_TITLE", &record.request.title)
            .env("PR_BRANCH", &record.request.branch)
            .env("PR_BASE", &record.request.base)
            .env("PR_CONFIG_DIR", settings.hook_dir.parent().unwrap())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .env_remove("GROVE_CD_FILE")
            .stdin(Stdio::null())
            .output();
        match result {
            Ok(result) if result.status.success() => {
                if let Some(line) = last_line(&result.stdout) {
                    last_output = line;
                }
            }
            Ok(result) => failures.push(format!(
                "{name} ({}): {}",
                result.status,
                last_line(&result.stderr).unwrap_or_else(|| "No error details".to_owned())
            )),
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    // Independent hooks all get a turn, even if an earlier hook failed.
    ensure!(
        failures.is_empty(),
        "{}",
        output::clean(&failures.join("; "))
    );
    let suffix = if last_output.is_empty() {
        String::new()
    } else {
        format!(": {last_output}")
    };
    Ok(format!("Review hooks completed ({}){suffix}", hooks.len()))
}

fn last_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| output::clean(line.trim()))
}

fn is_executable_file(path: &Path) -> Result<bool> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("Cannot inspect review hook {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    Ok(metadata.is_file())
}
