//! Exercise configuration through the real CLI without provider or Slack calls.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

/// Isolate platform defaults from the developer's configuration and tracked requests.
fn command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pr"));
    command
        .current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .args(args);
    command
}

/// Report CLI diagnostics on failure so file-loading regressions are easy to diagnose.
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Seed distinct records so tests verify which cache was read, without relying on writes.
fn write_cache(path: &Path, number: u64) {
    fs::create_dir_all(path).unwrap();
    let data = serde_json::json!({
        "version": 1,
        "requests": [{
            "remote": {"provider": "github", "host": "github.com", "project": "owner/repo"},
            "request": {
                "number": number, "title": "Config fixture", "status": "open",
                "url": format!("https://github.com/owner/repo/pull/{number}"),
                "branch": "feature", "base": "main"
            },
            "origins": [], "checked_at": 0, "refresh_error": null
        }]
    });
    fs::write(
        path.join("requests.json"),
        serde_json::to_vec(&data).unwrap(),
    )
    .unwrap();
}

/// Match the record selected by the CLI after resolving configuration and overrides.
fn assert_request(output: Output, number: u64) {
    let records: serde_json::Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["request"]["number"], number);
}

/// Relative locations belong to the selected file even when invoked from another directory.
#[test]
fn selected_config_controls_directory_paths() {
    let root = tempfile::tempdir().unwrap();
    let settings = root.path().join("settings");
    fs::create_dir(&settings).unwrap();
    fs::write(
        settings.join("pr.toml"),
        "data_dir = 'tracked'\ngrove_dir = 'workspaces'\n",
    )
    .unwrap();
    write_cache(&settings.join("tracked"), 1);
    let output = command(
        root.path(),
        &[
            "--config",
            "settings/pr.toml",
            "list",
            "--offline",
            "--json",
        ],
    )
    .output()
    .unwrap();
    assert_request(output, 1);
    assert!(!root.path().join("tracked").exists());

    // Selecting another config switches caches, even if the previous cache is corrupt.
    fs::write(settings.join("tracked/requests.json"), "bad cache").unwrap();
    fs::write(
        settings.join("other.toml"),
        "data_dir = 'other'\ngrove_dir = 'other-workspaces'\n",
    )
    .unwrap();
    write_cache(&settings.join("other"), 2);
    let output = command(
        root.path(),
        &[
            "list",
            "--config",
            "settings/other.toml",
            "--offline",
            "--json",
        ],
    )
    .output()
    .unwrap();
    assert_request(output, 2);
}

/// Missing explicit files and invalid content must fail before touching the cache or exposing secrets.
#[test]
fn rejects_bad_configs_without_exposing_contents_or_modifying_tracking() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let original = r#"{"version":1,"requests":[]}"#;
    fs::write(cache.join("requests.json"), original).unwrap();
    let args = ["--config", "pr.toml", "list", "--offline", "--json"];
    let output = command(root.path(), &args).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot read config pr.toml"));
    for invalid in [
        "[slack]\ntoken = 'secret-token' broken",
        "provider = 'secret-token'",
        "secret-token = 1",
        "[slack]\nchannel_id = ['secret-token']",
        "[slack]\nreview_message = ['secret-token']",
        "[slack]\ndeploy_message = ['secret-token']",
        "[slack]\nsecret-token = 'wrong-key'",
    ] {
        fs::write(
            root.path().join("pr.toml"),
            format!("data_dir = 'cache'\ngrove_dir = 'grove'\n{invalid}"),
        )
        .unwrap();
        let output = command(root.path(), &args).output().unwrap();
        assert!(!output.status.success(), "accepted {invalid}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("config") || error.contains("must not be empty"),
            "{error}"
        );
        assert!(!error.contains("secret-token"), "{error}");
        assert_eq!(
            fs::read_to_string(cache.join("requests.json")).unwrap(),
            original
        );
    }
}

/// Config discovery respects XDG, requires a file, and expands home-relative paths.
#[cfg(target_os = "linux")]
#[test]
fn default_config_is_required_and_respects_xdg_and_home_paths() {
    let root = tempfile::tempdir().unwrap();
    let args = ["list", "--offline", "--json"];
    write_cache(&root.path().join("data/pr"), 1);
    let output = command(root.path(), &args).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("pr init config"));

    fs::create_dir_all(root.path().join("config/pr")).unwrap();
    fs::write(
        root.path().join("config/pr/config.toml"),
        "data_dir = '~/tracked'\ngrove_dir = '~/.grove'\n",
    )
    .unwrap();
    write_cache(&root.path().join("tracked"), 2);
    assert_request(command(root.path(), &args).output().unwrap(), 2);

    fs::write(root.path().join("config/pr/config.toml"), "invalid TOML").unwrap();
    assert!(
        !command(root.path(), &args)
            .output()
            .unwrap()
            .status
            .success()
    );
    // An explicit file replaces the default file instead of merging with it.
    fs::write(
        root.path().join("custom.toml"),
        "data_dir = '~/other'\ngrove_dir = '~/.grove'\n",
    )
    .unwrap();
    write_cache(&root.path().join("other"), 3);
    let output = command(
        root.path(),
        &["--config", "~/custom.toml", "list", "--offline", "--json"],
    )
    .output()
    .unwrap();
    assert_request(output, 3);
}

/// The documented template works without Slack credentials; help never needs configuration.
#[test]
fn example_config_works_and_help_does_not_require_a_valid_config() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("pr.toml"),
        include_str!("../config.example.toml"),
    )
    .unwrap();
    let output = command(
        root.path(),
        &["--config", "pr.toml", "list", "--offline", "--json"],
    )
    .output()
    .unwrap();
    assert_eq!(success(output).trim(), "[]");
    let help = success(
        command(root.path(), &["--config", "missing.toml", "--help"])
            .output()
            .unwrap(),
    );
    assert!(help.contains("--config"));
    assert!(!help.contains("PR_DATA_DIR"));
    for flag in ["--grove-dir", "--data-dir"] {
        assert!(!help.contains(flag));
        for args in [
            vec![flag, "other", "list", "--offline"],
            vec!["list", flag, "other", "--offline"],
        ] {
            let output = command(root.path(), &args).output().unwrap();
            assert_eq!(output.status.code(), Some(2));
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains(&format!("unexpected argument '{flag}'"))
            );
        }
    }
}

/// Reject incomplete or empty directory settings instead of silently choosing another cache.
#[test]
fn directory_paths_are_mandatory_and_nonempty() {
    let root = tempfile::tempdir().unwrap();
    write_cache(&root.path().join("cache"), 1);
    let original = fs::read(root.path().join("cache/requests.json")).unwrap();
    for (source, expected) in [
        ("", "grove_dir and data_dir are required"),
        ("grove_dir = 'grove'", "grove_dir and data_dir are required"),
        ("data_dir = 'cache'", "grove_dir and data_dir are required"),
        ("grove_dir = ''\ndata_dir = 'cache'", "grove_dir"),
        ("grove_dir = 'grove'\ndata_dir = ''", "data_dir"),
    ] {
        fs::write(root.path().join("pr.toml"), source).unwrap();
        let output = command(
            root.path(),
            &["--config", "pr.toml", "list", "--offline", "--json"],
        )
        .output()
        .unwrap();
        assert!(!output.status.success(), "accepted {source}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
        assert_eq!(
            fs::read(root.path().join("cache/requests.json")).unwrap(),
            original
        );
    }
}

/// Initialize the same default path that startup reads, without creating tracking data.
#[cfg(target_os = "linux")]
#[test]
fn init_creates_private_default_config_and_prints_its_path() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config/pr/config.toml");
    let output = success(command(root.path(), &["init", "config"]).output().unwrap());
    assert!(output.contains(&path.display().to_string()), "{output}");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        include_str!("../config.example.toml")
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!root.path().join(".local/share/pr").exists());
    assert_eq!(
        success(
            command(root.path(), &["list", "--offline", "--json"])
                .output()
                .unwrap()
        )
        .trim(),
        "[]"
    );
}

/// Custom paths work before loading config and accept the global flag before or after subcommands.
#[test]
fn init_creates_explicit_config_and_missing_parent_directories() {
    let root = tempfile::tempdir().unwrap();
    for (args, relative) in [
        (
            ["--config", "nested/settings/pr.toml", "init", "config"],
            "nested/settings/pr.toml",
        ),
        (["init", "config", "--config", "local.toml"], "local.toml"),
        (["init", "config", "--config", "~/home.toml"], "home.toml"),
    ] {
        let output = success(command(root.path(), &args).output().unwrap());
        let path = root.path().join(relative);
        assert!(output.contains(&path.display().to_string()), "{output}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            include_str!("../config.example.toml")
        );
        assert_eq!(
            success(
                command(
                    root.path(),
                    &["--config", relative, "list", "--offline", "--json"]
                )
                .output()
                .unwrap()
            )
            .trim(),
            "[]"
        );
    }
}

/// Initialization never parses or overwrites existing files and reports filesystem failures.
#[test]
fn init_preserves_existing_config_and_reports_unwritable_paths() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pr.toml");
    let original = "invalid TOML containing secret-token";
    fs::write(&path, original).unwrap();
    let output = command(root.path(), &["--config", "pr.toml", "init", "config"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("already exists"), "{error}");
    assert!(error.contains(&path.display().to_string()), "{error}");
    assert!(!error.contains("secret-token"));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    let output = command(
        root.path(),
        &["--config", "pr.toml/config.toml", "init", "config"],
    )
    .output()
    .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot create config directory"));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

/// Existing config symlinks must survive initialization, including links to missing files.
#[cfg(unix)]
#[test]
fn init_preserves_config_symlinks_and_their_targets() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target.toml");
    let link = root.path().join("pr.toml");
    symlink(&target, &link).unwrap();
    for existing in [false, true] {
        if existing {
            fs::write(&target, "existing config").unwrap();
        }
        let output = command(root.path(), &["--config", "pr.toml", "init", "config"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read_link(&link).unwrap(), target);
        if existing {
            assert_eq!(fs::read_to_string(&target).unwrap(), "existing config");
        } else {
            assert!(!target.exists());
        }
    }
}
