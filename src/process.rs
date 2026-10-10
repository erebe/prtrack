use anyhow::{Context, Result, bail};
use std::{
    ffi::OsStr,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use wait_timeout::ChildExt;

/// Preserve explicit proxy settings and use HTTP_PROXY as an HTTPS fallback.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    let http_proxy = std::env::var_os("HTTP_PROXY")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("http_proxy").filter(|value| !value.is_empty()));
    if let Some(proxy) = http_proxy {
        // Git's HTTP transport accepts the lowercase spelling.
        if std::env::var_os("http_proxy").is_none() {
            command.env("http_proxy", &proxy);
        }
        // Even an explicitly empty HTTPS setting suppresses this fallback.
        if std::env::var_os("HTTPS_PROXY").is_none() && std::env::var_os("https_proxy").is_none() {
            command.env("HTTPS_PROXY", &proxy);
        }
    }
    command
}

/// File-backed output avoids pipe deadlocks while bounding noninteractive commands.
pub fn output(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    input: Option<&[u8]>,
) -> Result<String> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut stdin = tempfile::tempfile()?;
    if let Some(input) = input {
        stdin.write_all(input)?;
    }
    stdin.seek(SeekFrom::Start(0))?;
    let mut command = command(program);
    command
        .args(args)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("Cannot run {program}; install it and ensure it is on PATH"))?;
    let status = match child.wait_timeout(Duration::from_secs(30))? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{program} timed out after 30 seconds");
        }
    };
    let mut text = String::new();
    if !status.success() {
        stderr.seek(SeekFrom::Start(0))?;
        stderr.read_to_string(&mut text)?;
        bail!(
            "{program} failed ({status}): {}",
            crate::output::clean(text.trim())
        );
    }
    stdout.seek(SeekFrom::Start(0))?;
    stdout.read_to_string(&mut text)?;
    Ok(text.trim().to_owned())
}
