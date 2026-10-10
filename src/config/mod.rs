//! Initialize and load TOML configuration independently of CLI parsing and transport.

mod types;

pub use types::{Config, SlackConfig};

use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

impl Config {
    /// Read required paths from the selected file so startup never guesses storage locations.
    pub fn load(explicit_path: Option<&Path>) -> Result<Self> {
        let path = config_path(explicit_path)?;
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(error).with_context(|| {
                    format!(
                        "Cannot read config {}; run `pr init config` (with the same --config option, if used) to create it",
                        path.display()
                    )
                });
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Cannot read config {}", path.display()));
            }
        };
        let mut config: Self = toml::from_str(&source).map_err(|error: toml::de::Error| {
            // TOML errors can quote whole lines or values, including the Slack token.
            let location = error
                .span()
                .and_then(|span| source.get(..span.start))
                .map(|prefix| {
                    let line = prefix.bytes().filter(|&c| c == b'\n').count() + 1;
                    let column = prefix
                        .rsplit('\n')
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .count()
                        + 1;
                    format!(" at line {line}, column {column}")
                })
                .unwrap_or_default();
            anyhow::anyhow!(
                "Invalid config {}{location}; grove_dir and data_dir are required; check TOML syntax, supported keys, and value types",
                path.display()
            )
        })?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        config.hook_dir = std::path::absolute(parent.join("hook"))?;
        for (name, value) in [
            ("grove_dir", &mut config.grove_dir),
            ("data_dir", &mut config.data_dir),
        ] {
            ensure!(
                !value.as_os_str().is_empty(),
                "{name} in {} must not be empty",
                path.display()
            );
            *value = expand_home(value)?;
            if value.is_relative() {
                *value = parent.join(&*value);
            }
        }
        Ok(config)
    }

    /// Publish a private example file without replacing existing configuration or symlinks.
    pub fn init(explicit_path: Option<&Path>) -> Result<PathBuf> {
        let path = std::path::absolute(config_path(explicit_path)?)?;
        let parent = path
            .parent()
            .context("Config path must have a parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create config directory {}", parent.display()))?;
        let mut file = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("Cannot create config in {}", parent.display()))?;
        file.write_all(include_str!("../../config.example.toml").as_bytes())
            .context("Cannot write example configuration")?;
        file.persist_noclobber(&path).map_err(|error| {
            if error.error.kind() == ErrorKind::AlreadyExists {
                anyhow::anyhow!(
                    "Config already exists at {}; leaving it unchanged",
                    path.display()
                )
            } else {
                anyhow::anyhow!("Cannot create config {}: {}", path.display(), error.error)
            }
        })?;
        Ok(path)
    }
}

/// Share path selection between loading and initialization so both target the same file.
fn config_path(explicit_path: Option<&Path>) -> Result<PathBuf> {
    match explicit_path {
        Some(path) => expand_home(path),
        None => Ok(dirs::config_dir()
            .context("Cannot find config directory; select a file with --config")?
            .join("pr/config.toml")),
    }
}

/// Expand a leading `~` component so paths in files work without shell expansion.
fn expand_home(path: &Path) -> Result<PathBuf> {
    match path.strip_prefix("~") {
        Ok(suffix) => Ok(dirs::home_dir()
            .context("Cannot expand ~ in config path: home directory unavailable")?
            .join(suffix)),
        Err(_) => Ok(path.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;

    /// Load nested Slack settings and redact the token when runtime settings are inspected.
    #[test]
    fn reads_slack_settings_and_redacts_credentials() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        fs::write(
            &path,
            "grove_dir = 'grove'\ndata_dir = 'data'\nprovider = 'gitlab'\n[slack]\ntoken = 'secret-token'\nchannel_id = 'C123'\nreview_message = 'Review {pr_url}'\ndeploy_message = 'Release {pr_url}'\n",
        )
        .unwrap();
        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.hook_dir, root.path().join("hook"));
        assert_eq!(config.provider, Some(Provider::Gitlab));
        assert_eq!(config.slack.token.as_deref(), Some("secret-token"));
        assert_eq!(config.slack.channel_id.as_deref(), Some("C123"));
        assert_eq!(
            config.slack.review_message.as_deref(),
            Some("Review {pr_url}")
        );
        assert_eq!(
            config.slack.deploy_message.as_deref(),
            Some("Release {pr_url}")
        );
        assert!(!format!("{config:?}").contains("secret-token"));
    }
}
