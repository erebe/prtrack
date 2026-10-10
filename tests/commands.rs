#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    dir: tempfile::TempDir,
    git: PathBuf,
    repo: PathBuf,
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

impl Fixture {
    fn new(gitlab: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "grove_dir = 'grove'\ndata_dir = 'data'\n",
        )
        .unwrap();
        let git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("git"))
            .find(|p| p.is_file())
            .unwrap();
        let repo = dir.path().join("source");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(dir.path().join("bin")).unwrap();
        for name in ["git", "gh", "glab"] {
            let path = dir.path().join("bin").join(name);
            fs::write(&path, include_str!("fixtures/host.py")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let fixture = Self { dir, git, repo };
        let (host, project) = if gitlab {
            ("code.example.test", "group/sub/repo")
        } else {
            ("github.com", "owner/repo")
        };
        fixture.configure(json!({"host":host,"project":project,"existing":false}));
        fixture.git(&fixture.repo, &["init", "-b", "main"]);
        fixture.git(
            &fixture.repo,
            &["config", "user.email", "test@example.test"],
        );
        fixture.git(&fixture.repo, &["config", "user.name", "Test User"]);
        fixture.git(&fixture.repo, &["config", "commit.gpgsign", "false"]);
        fixture.git(&fixture.repo, &["commit", "--allow-empty", "-m", "Initial"]);
        fixture.git(&fixture.repo, &["checkout", "-b", "feature/topic"]);
        fixture.git(
            &fixture.repo,
            &[
                "commit",
                "--allow-empty",
                "-m",
                "Add feature",
                "-m",
                "Commit description",
            ],
        );
        fixture.git(
            &fixture.repo,
            &[
                "remote",
                "add",
                "origin",
                &format!("git@{host}:{project}.git"),
            ],
        );
        if gitlab {
            fixture.git(&fixture.repo, &["config", "pr.provider", "gitlab"]);
        }
        fixture.git(fixture.dir.path(), &["init", "--bare", "remote.git"]);
        fixture
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        success(
            Command::new(&self.git)
                .current_dir(cwd)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap(),
        )
    }

    fn configure(&self, update: Value) {
        let path = self.dir.path().join("provider.json");
        let mut value: Value = fs::read(&path)
            .ok()
            .map(|v| serde_json::from_slice(&v).unwrap())
            .unwrap_or(json!({}));
        value
            .as_object_mut()
            .unwrap()
            .extend(update.as_object().unwrap().clone());
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let path = std::env::join_paths(
            std::iter::once(self.dir.path().join("bin"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pr"));
        if !args.contains(&"--config") {
            command
                .arg("--config")
                .arg(self.dir.path().join("config.toml"));
        }
        command
            .current_dir(cwd)
            .args(args)
            .env("PATH", path)
            .env("PR_TEST_ROOT", self.dir.path())
            .env("PR_TEST_GIT", &self.git)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        command
    }

    fn run_at(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd, args).output().unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_at(&self.repo, args)
    }
    fn records(&self, args: &[&str]) -> Value {
        serde_json::from_str(&success(self.run(args))).unwrap()
    }
    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.dir.path().join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// Inspect CI queries separately from discovery/approval requests.
    fn pipeline_queries(&self) -> Vec<Value> {
        fs::read_to_string(self.dir.path().join("graphql.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|payload| {
                let query = payload["query"].as_str().unwrap();
                query.contains("statusCheckRollup") || query.contains("headPipeline")
            })
            .collect()
    }

    fn workspace(&self) -> PathBuf {
        self.git(&self.repo, &["checkout", "main"]);
        let checkout = self
            .dir
            .path()
            .join("custom-workspaces")
            .join("feature")
            .join("repo");
        self.git(
            &self.repo,
            &[
                "worktree",
                "add",
                checkout.to_str().unwrap(),
                "feature/topic",
            ],
        );
        fs::create_dir_all(self.dir.path().join("grove")).unwrap();
        fs::write(self.dir.path().join("grove/state.json"), serde_json::to_vec(&json!([{
            "name":"feature", "path":checkout.parent().unwrap(), "repos":[{
                "source_repo":self.repo, "worktree_path":checkout, "branch":"stale-branch-name", "repo_name":"repo"
            }]
        }])).unwrap()).unwrap();
        checkout
    }
}

#[test]
fn add_tracks_request_urls_without_a_checkout() {
    for gitlab in [false, true] {
        for status in ["open", "draft", "closed", "merged"] {
            let fixture = Fixture::new(gitlab);
            let (host, project, path) = if gitlab {
                ("gitlab.com", "qovery/backend/q-core", "-/merge_requests")
            } else {
                ("github.com", "owner/repo", "pull")
            };
            let url = format!("https://{host}/{project}/{path}/4096");
            let state = match status {
                "merged" if !gitlab => "closed",
                "open" | "draft" if gitlab => "opened",
                "draft" => "open",
                status => status,
            };
            let mut request = json!({
                "number": 4096, "iid": 4096, "title": "Added by URL",
                "html_url": url, "web_url": url,
                "head": {"ref": "someone/feature"}, "base": {"ref": "main"},
                "source_branch": "someone/feature", "target_branch": "main",
                "state": state, "draft": status == "draft",
                "merged_at": if status == "merged" { json!("2026-01-01") } else { Value::Null },
                "issue_comments": [{"created_at": "2026-01-02T12:30:00Z"}],
                "discussions": [{"notes": [{"created_at": "2026-01-02T12:30:00Z", "system": false}]}]
            });
            // Explicit addition does not filter by author or discover other requests.
            fixture.configure(json!({
                "host": host, "project": project, "extra_requests": [request],
                "fail_user": true, "fail_discovery": true
            }));
            let message = success(fixture.run_at(fixture.dir.path(), &["add", &url]));
            assert!(message.contains(&format!("Tracking request: {url}")));
            let records = fixture.records(&["list", "--offline", "--json"]);
            assert_eq!(records.as_array().unwrap().len(), 1);
            assert_eq!(records[0]["remote"]["host"], host);
            assert_eq!(records[0]["remote"]["project"], project);
            assert_eq!(records[0]["request"]["number"], 4096);
            assert_eq!(records[0]["request"]["status"], status);
            assert_eq!(records[0]["request"]["branch"], "someone/feature");
            assert_eq!(
                records[0]["request"]["last_comment_at"],
                "2026-01-02T12:30:00Z"
            );
            assert_eq!(records[0]["origins"], json!([]));
            assert!(records[0]["checked_at"].as_u64().unwrap() > 0);

            request["title"] = json!("Updated by URL");
            fixture.configure(json!({"extra_requests": [request]}));
            success(fixture.run_at(
                fixture.dir.path(),
                &["add", &format!("{url}/?view=parallel#discussion")],
            ));
            let records = fixture.records(&["list", "--offline", "--json"]);
            assert_eq!(records.as_array().unwrap().len(), 1);
            assert_eq!(records[0]["request"]["title"], "Updated by URL");
            assert!(records[0]["refresh_error"].is_null());
            let calls = fixture.calls();
            // Both additions fetch comment metadata, including closed requests.
            assert_eq!(calls.len(), 6);
            assert!(
                calls
                    .iter()
                    .all(|call| call[5] == "GET" || (call[5] == "POST" && call[6] == "graphql"))
            );
        }
    }
}

#[test]
fn add_preserves_existing_workspace_origins_and_follows_repository_renames() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        fixture.configure(json!({"canonical_project": "renamed/current-repo"}));
        let checkout = fixture.workspace();
        success(fixture.run_at(&checkout, &["push"]));
        let before = fixture.records(&["list", "--offline", "--json"]);
        let remote = &before[0]["remote"];
        let url = format!(
            "https://{}/{}/{}/7",
            remote["host"].as_str().unwrap(),
            remote["project"].as_str().unwrap(),
            if gitlab { "-/merge_requests" } else { "pull" }
        );
        let calls = fixture.calls().len();
        fixture.configure(json!({"title": "Updated by URL"}));
        success(fixture.run_at(fixture.dir.path(), &["add", &url]));
        let after = fixture.records(&["list", "--offline", "--json"]);
        assert_eq!(after.as_array().unwrap().len(), 1);
        assert_eq!(after[0]["origins"], before[0]["origins"]);
        assert_eq!(after[0]["origins"][0]["workspace"]["name"], "feature");
        assert_eq!(after[0]["request"]["title"], "Updated by URL");
        assert!(
            after[0]["request"]["url"]
                .as_str()
                .unwrap()
                .contains("renamed/current-repo")
        );
        assert!(
            fixture.calls()[calls..]
                .iter()
                .all(|call| call[5] == "GET" || (call[5] == "POST" && call[6] == "graphql"))
        );
    }
}

#[test]
fn add_failures_preserve_tracking_data() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let url = if gitlab {
            "https://code.example.test/group/sub/repo/-/merge_requests/7"
        } else {
            "https://github.com/owner/repo/pull/7"
        };
        assert!(!fixture.run(&["add"]).status.success());
        assert!(
            !fixture
                .run(&["add", "https://github.com/owner/repo/issues/7"])
                .status
                .success()
        );
        assert!(fixture.calls().is_empty());
        assert!(!fixture.dir.path().join("data/requests.json").exists());

        fixture.configure(json!({"fail_detail": true}));
        assert!(!fixture.run(&["add", url]).status.success());
        assert!(!fixture.dir.path().join("data/requests.json").exists());
        fixture.configure(json!({"fail_detail": false}));
        success(fixture.run(&["add", url]));
        let path = fixture.dir.path().join("data/requests.json");
        let before = fs::read(&path).unwrap();
        for failure in ["fail_api", "fail_detail"] {
            fixture.configure(json!({failure: true}));
            assert!(!fixture.run(&["add", url]).status.success());
            assert_eq!(fs::read(&path).unwrap(), before);
            fixture.configure(json!({failure: false}));
        }
        fs::write(&path, "corrupted cache").unwrap();
        let calls = fixture.calls().len();
        assert!(!fixture.run(&["add", url]).status.success());
        assert_eq!(fixture.calls().len(), calls);
        assert_eq!(fs::read_to_string(path).unwrap(), "corrupted cache");
    }
}

#[test]
fn proxy_settings_reach_provider_calls_and_git_push() {
    let cases = [
        (json!({}), None, None),
        (
            json!({"HTTP_PROXY": "http://proxy.test:3128"}),
            Some("http://proxy.test:3128"),
            Some("http://proxy.test:3128"),
        ),
        (
            json!({"http_proxy": "http://lower.test:3128"}),
            Some("http://lower.test:3128"),
            Some("http://lower.test:3128"),
        ),
        (
            json!({"HTTP_PROXY": "http://proxy.test:3128", "HTTPS_PROXY": "http://secure.test:8080"}),
            Some("http://proxy.test:3128"),
            Some("http://secure.test:8080"),
        ),
        (
            json!({"HTTP_PROXY": "http://proxy.test:3128", "https_proxy": "http://secure.test:8080"}),
            Some("http://proxy.test:3128"),
            None,
        ),
        (
            json!({"HTTP_PROXY": "http://proxy.test:3128", "HTTPS_PROXY": ""}),
            Some("http://proxy.test:3128"),
            Some(""),
        ),
        (
            json!({"HTTP_PROXY": "http://proxy.test:3128", "https_proxy": ""}),
            Some("http://proxy.test:3128"),
            None,
        ),
        (json!({"HTTP_PROXY": ""}), None, None),
        (
            json!({"HTTP_PROXY": "http://proxy.test:3128", "http_proxy": "http://lower.test:3128"}),
            Some("http://lower.test:3128"),
            Some("http://proxy.test:3128"),
        ),
    ];
    let names = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "NO_PROXY",
        "no_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    for gitlab in [false, true] {
        for (input, http, https) in &cases {
            let fixture = Fixture::new(gitlab);
            let mut expected = json!({});
            let mut command = fixture.command(&fixture.repo, &["push"]);
            for name in names {
                command.env_remove(name);
                expected[name] = input[name].clone();
            }
            for (name, value) in input.as_object().unwrap() {
                command.env(name, value.as_str().unwrap());
            }
            // Preserve both bypass spellings and unrelated proxy settings.
            for (name, value) in [
                ("NO_PROXY", "private.test,localhost"),
                ("no_proxy", "internal.test"),
                ("ALL_PROXY", "socks5://all.test:1080"),
            ] {
                command.env(name, value);
                expected[name] = json!(value);
            }
            expected["http_proxy"] = json!(http);
            expected["HTTPS_PROXY"] = json!(https);
            fixture.configure(json!({"expected_proxy": expected}));
            success(command.output().unwrap());
            assert!(fixture.calls().iter().any(|call| call[0] == "git"));
            assert!(
                fixture
                    .calls()
                    .iter()
                    .any(|call| call[0] == if gitlab { "glab" } else { "gh" })
            );

            let mut list = fixture.command(&fixture.repo, &["list", "--json"]);
            for name in names {
                list.env_remove(name);
            }
            for (name, value) in command.get_envs() {
                if let Some(value) = value {
                    list.env(name, value);
                }
            }
            let records: Value = serde_json::from_str(&success(list.output().unwrap())).unwrap();
            assert!(records[0]["refresh_error"].is_null());
        }
    }
}

#[test]
fn renamed_repositories_use_current_api_paths_for_creation_and_reuse() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let project = if gitlab {
            "renamed/group/repo"
        } else {
            "renamed-owner/current-repo"
        };
        fixture.configure(json!({"canonical_project": project}));
        let original_remote = fixture.git(&fixture.repo, &["remote", "get-url", "origin"]);

        let output =
            success(fixture.run(&["push", "--base", "release", "--body", "Body with\nnewlines"]));
        assert!(output.contains(project));
        let payload: Value =
            serde_json::from_slice(&fs::read(fixture.dir.path().join("payload.json")).unwrap())
                .unwrap();
        assert_eq!(
            payload[if gitlab { "target_branch" } else { "base" }],
            "release"
        );
        assert_eq!(
            payload[if gitlab { "description" } else { "body" }],
            "Body with\nnewlines"
        );
        assert!(success(fixture.run(&["push"])).contains("Tracking existing request:"));

        let calls = fixture.calls();
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.as_array().unwrap().contains(&json!("POST")) && c[6] != "graphql")
                .count(),
            1
        );
        let endpoint = if gitlab {
            "projects/renamed%2Fgroup%2Frepo/merge_requests"
        } else {
            "repos/renamed-owner/current-repo/pulls"
        };
        assert!(
            calls
                .iter()
                .any(|c| c.as_array().unwrap().contains(&json!(endpoint)))
        );
        if !gitlab {
            assert!(calls.iter().any(|c| {
                c.as_array().unwrap().iter().any(|arg| {
                    arg.as_str()
                        .is_some_and(|arg| arg.contains("head=renamed-owner%3Afeature%2Ftopic"))
                })
            }));
        }
        assert_eq!(
            fixture.git(&fixture.repo, &["remote", "get-url", "origin"]),
            original_remote
        );
        assert_eq!(
            fixture
                .records(&["list", "--offline", "--json"])
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn renamed_repositories_discover_existing_requests_and_refresh_merged_status() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        fixture.configure(json!({"canonical_project":"renamed/current-repo", "existing":true}));
        let discovered = fixture.records(&["list", "--json"]);
        assert_eq!(discovered.as_array().unwrap().len(), 1);
        assert!(
            discovered[0]["request"]["url"]
                .as_str()
                .unwrap()
                .contains("renamed/current-repo")
        );
        fixture.configure(json!({"status":"merged"}));
        assert_eq!(
            fixture.records(&["list", "--json"])[0]["request"]["status"],
            "merged"
        );
    }
}

#[test]
fn push_target_publishes_exact_branch_and_reuses_request() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let checkout = fixture.workspace();
        let url = fixture.git(&fixture.repo, &["remote", "get-url", "origin"]);
        fixture.git(&fixture.repo, &["remote", "add", "publish", url.trim()]);
        // An explicit destination does not need a username to construct its name.
        fixture.configure(json!({"fail_user": true}));
        let output = success(fixture.run_at(
            &checkout,
            &[
                "push", "--remote", "publish", "--target", "toto", "--base", "release",
            ],
        ));
        assert!(output.contains("Created request:"));
        let remote = fixture.dir.path().join("remote.git");
        assert_eq!(
            fixture
                .git(
                    &remote,
                    &["for-each-ref", "--format=%(refname)", "refs/heads"]
                )
                .trim(),
            "refs/heads/toto"
        );
        assert_eq!(
            fixture.git(&checkout, &["rev-parse", "HEAD"]),
            fixture.git(&remote, &["rev-parse", "refs/heads/toto"])
        );
        assert_eq!(
            fixture.git(&checkout, &["branch", "--show-current"]).trim(),
            "feature/topic"
        );
        assert_eq!(
            fixture
                .git(&checkout, &["config", "branch.feature/topic.remote"])
                .trim(),
            "publish"
        );
        assert_eq!(
            fixture
                .git(&checkout, &["config", "branch.feature/topic.merge"])
                .trim(),
            "refs/heads/toto"
        );
        let payload: Value =
            serde_json::from_slice(&fs::read(fixture.dir.path().join("payload.json")).unwrap())
                .unwrap();
        assert_eq!(
            payload[if gitlab { "source_branch" } else { "head" }],
            "toto"
        );
        assert_eq!(
            payload[if gitlab { "target_branch" } else { "base" }],
            "release"
        );

        fixture.git(
            &checkout,
            &["commit", "--allow-empty", "-m", "Update feature"],
        );
        let output = success(fixture.run_at(&checkout, &["push", "--target", "toto"]));
        assert!(output.contains("Tracking existing request:"));
        assert_eq!(
            fixture.git(&checkout, &["rev-parse", "HEAD"]),
            fixture.git(&remote, &["rev-parse", "refs/heads/toto"])
        );
        let creations = fixture
            .calls()
            .into_iter()
            .filter(|call| call[0] != "git" && call[5] == "POST" && call[6] != "graphql")
            .count();
        assert_eq!(creations, 1);
        fixture.configure(json!({"fail_user": false}));
        // Refreshes retain the workspace association recorded by the explicit push.
        let records = fixture.records(&["list", "--json", "--workspace", "feature"]);
        assert_eq!(records.as_array().unwrap().len(), 1);
        assert_eq!(records[0]["request"]["branch"], "toto");
        assert_eq!(records[0]["request"]["base"], "release");
        assert!(
            records[0]["origins"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["checkout"] == checkout.to_str().unwrap())
        );
    }
}

#[test]
fn push_target_accepts_nested_names_from_the_local_base_branch() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        fixture.git(&fixture.repo, &["checkout", "main"]);
        fixture.git(
            &fixture.repo,
            &["commit", "--allow-empty", "-m", "Local change"],
        );
        success(fixture.run(&["push", "--target", "feature/toto"]));
        assert_eq!(
            fixture.git(&fixture.repo, &["rev-parse", "HEAD"]),
            fixture.git(
                &fixture.dir.path().join("remote.git"),
                &["rev-parse", "refs/heads/feature/toto"]
            )
        );
        let records = fixture.records(&["list", "--offline", "--json"]);
        assert_eq!(records[0]["request"]["branch"], "feature/toto");
        assert_eq!(records[0]["request"]["base"], "main");
    }
}

#[test]
fn push_target_rejects_invalid_and_identical_base_branches_before_publishing() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        for target in ["", "bad branch", "bad..branch", "bad:branch", "bad\nbranch"] {
            let output = fixture.run(&["push", "--target", target]);
            assert!(!output.status.success(), "Accepted {target:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("Invalid remote branch name"));
        }
        for args in [
            vec!["push", "--target", "main"],
            vec!["push", "--target", "toto", "--base", "toto"],
        ] {
            let output = fixture.run(&args);
            assert!(!output.status.success());
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("Source and target branches must differ")
            );
        }
        assert!(!fixture.calls().iter().any(|c| c[0] == "git"));
        assert!(!fixture.dir.path().join("payload.json").exists());
        assert_eq!(
            fixture
                .git(
                    &fixture.dir.path().join("remote.git"),
                    &["for-each-ref", "--format=%(refname)", "refs/heads"]
                )
                .trim(),
            ""
        );
    }
}

#[test]
fn push_uses_the_current_branch_name_without_a_username_lookup() {
    for gitlab in [false, true] {
        for branch in ["feature/topic", "erebe/feature/topic"] {
            let fixture = Fixture::new(gitlab);
            fixture.configure(json!({"username": "erebe", "fail_user": true}));
            if branch != "feature/topic" {
                fixture.git(&fixture.repo, &["branch", "-m", branch]);
            }
            success(fixture.run(&["push"]));
            let payload: Value =
                serde_json::from_slice(&fs::read(fixture.dir.path().join("payload.json")).unwrap())
                    .unwrap();
            assert_eq!(
                payload[if gitlab { "source_branch" } else { "head" }],
                branch
            );
            assert_eq!(
                fixture
                    .git(&fixture.repo, &["branch", "--show-current"])
                    .trim(),
                branch
            );
            let remote_ref = format!("refs/heads/{branch}");
            assert_eq!(
                fixture
                    .git(
                        &fixture.repo,
                        &["config", &format!("branch.{branch}.merge")]
                    )
                    .trim(),
                remote_ref
            );
            let remote = fixture.dir.path().join("remote.git");
            assert_eq!(
                fixture
                    .git(
                        &remote,
                        &["for-each-ref", "--format=%(refname)", "refs/heads"]
                    )
                    .trim(),
                remote_ref
            );
            assert_eq!(
                fixture.git(&fixture.repo, &["rev-parse", "HEAD"]),
                fixture.git(&remote, &["rev-parse", &remote_ref])
            );
            assert!(
                !fixture
                    .calls()
                    .iter()
                    .any(|call| call[0] != "git" && call[6] == "user")
            );
        }
    }
}

#[test]
fn workspace_discovery_matches_exact_local_branch_names() {
    for gitlab in [false, true] {
        for branch in ["feature/topic", "erebe/feature/topic"] {
            let fixture = Fixture::new(gitlab);
            let checkout = fixture.workspace();
            fixture.configure(json!({"username": "erebe", "existing": true, "branch": branch}));
            let records = fixture.records(&["list", "--json", "--workspace", "feature"]);
            if branch == "feature/topic" {
                assert_eq!(records.as_array().unwrap().len(), 1);
                assert_eq!(records[0]["request"]["branch"], branch);
                assert_eq!(
                    records[0]["origins"][0]["checkout"],
                    checkout.to_str().unwrap()
                );
                assert_eq!(records[0]["origins"][0]["workspace"]["name"], "feature");
            } else {
                assert_eq!(records, json!([]));
            }
        }
    }
}

#[test]
fn push_reuses_existing_request_for_the_current_branch() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        fixture.configure(json!({"existing": true, "base": "develop", "fail_user": true}));
        assert!(success(fixture.run(&["push"])).contains("Tracking existing request:"));
        assert!(!fixture.dir.path().join("payload.json").exists());
        let remote = fixture.dir.path().join("remote.git");
        assert_eq!(
            fixture
                .git(
                    &remote,
                    &["for-each-ref", "--format=%(refname)", "refs/heads"]
                )
                .trim(),
            "refs/heads/feature/topic"
        );
        assert_eq!(
            fixture.git(&fixture.repo, &["rev-parse", "HEAD"]),
            fixture.git(&remote, &["rev-parse", "refs/heads/feature/topic"])
        );
        let records = fixture.records(&["list", "--offline", "--json"]);
        assert_eq!(records[0]["request"]["branch"], "feature/topic");
        assert_eq!(records[0]["request"]["base"], "develop");
    }
}

#[test]
fn github_push_publishes_branch_and_reuses_request() {
    let fixture = Fixture::new(false);
    let output = success(fixture.run(&[
        "push",
        "--title",
        "Literal $(echo nope)",
        "--body",
        "Body\nwith newlines",
        "--draft",
    ]));
    assert!(output.contains("Created request:"));
    let local = fixture.git(&fixture.repo, &["rev-parse", "HEAD"]);
    let remote = fixture.git(
        &fixture.dir.path().join("remote.git"),
        &["rev-parse", "refs/heads/feature/topic"],
    );
    assert_eq!(local, remote);
    assert_eq!(
        fixture
            .git(&fixture.repo, &["branch", "--show-current"])
            .trim(),
        "feature/topic"
    );
    assert_eq!(
        fixture
            .git(&fixture.repo, &["config", "branch.feature/topic.merge"])
            .trim(),
        "refs/heads/feature/topic"
    );
    let payload: Value =
        serde_json::from_slice(&fs::read(fixture.dir.path().join("payload.json")).unwrap())
            .unwrap();
    assert_eq!(payload["title"], "Literal $(echo nope)");
    assert_eq!(payload["body"], "Body\nwith newlines");
    assert_eq!(payload["draft"], true);
    assert_eq!(payload["base"], "main");
    assert_eq!(payload["head"], "feature/topic");
    fixture.git(
        &fixture.repo,
        &["commit", "--allow-empty", "-m", "Update feature"],
    );
    assert!(success(fixture.run(&["push"])).contains("Tracking existing request:"));
    assert_eq!(
        fixture.git(&fixture.repo, &["rev-parse", "HEAD"]),
        fixture.git(
            &fixture.dir.path().join("remote.git"),
            &["rev-parse", "refs/heads/feature/topic"]
        )
    );
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|c| c.as_array().unwrap().contains(&json!("POST")) && c[6] != "graphql")
            .count(),
        1
    );
    let records = fixture.records(&["list", "--offline", "--json"]);
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["request"]["status"], "draft");
    assert_eq!(
        records[0]["origins"][0]["repository"],
        fixture.repo.to_str().unwrap()
    );
}

#[test]
fn gitlab_self_hosted_nested_namespace_and_grove_worktree() {
    let fixture = Fixture::new(true);
    let checkout = fixture.workspace();
    let subdir = checkout.join("subdir");
    fs::create_dir(&subdir).unwrap();
    assert!(success(fixture.run_at(&subdir, &["push", "--draft"])).contains("/-/merge_requests/7"));
    let payload: Value =
        serde_json::from_slice(&fs::read(fixture.dir.path().join("payload.json")).unwrap())
            .unwrap();
    assert_eq!(payload["title"], "Draft: Add feature");
    assert_eq!(payload["source_branch"], "feature/topic");
    assert_eq!(payload["description"], "Commit description");
    let records = fixture.records(&[
        "list",
        "--offline",
        "--json",
        "--workspace",
        "feature",
        "--repo",
        "group/sub/repo",
    ]);
    assert_eq!(records[0]["origins"][0]["workspace"]["name"], "feature");
    assert_eq!(
        records[0]["origins"][0]["repository"],
        fixture.repo.to_str().unwrap()
    );
    assert_eq!(
        records[0]["origins"][0]["checkout"],
        checkout.to_str().unwrap()
    );
    fixture.configure(json!({"status":"merged"}));
    let refreshed = fixture.records(&["list", "--json"]);
    assert_eq!(refreshed[0]["request"]["status"], "merged");
}

#[test]
fn discovers_existing_grove_requests_and_follows_closed_status() {
    let fixture = Fixture::new(false);
    let checkout = fixture.workspace();
    fixture.configure(json!({"existing":true,"extra_page":true}));
    let records = fixture.records(&["list", "--json"]);
    assert_eq!(records[0]["origins"][0]["workspace"]["name"], "feature");
    assert_eq!(
        records[0]["origins"][0]["checkout"],
        checkout.to_str().unwrap()
    );
    assert!(!fixture.dir.path().join("payload.json").exists());
    fixture.configure(json!({"status":"closed"}));
    assert_eq!(
        fixture.records(&["list", "--json"])[0]["request"]["status"],
        "closed"
    );
    assert_eq!(
        fixture.records(&["list", "--offline", "--json", "--workspace", "unrelated"]),
        json!([])
    );
}

#[test]
fn discovery_filters_authors_on_the_server_and_keeps_all_matching_pages() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let project = if gitlab {
            "group/sub/repo"
        } else {
            "owner/repo"
        };
        let host = if gitlab {
            "code.example.test"
        } else {
            "github.com"
        };
        let request = |number, author: &str, fork, draft, open| {
            let url = format!(
                "https://{host}/{project}/{}{number}",
                if gitlab { "-/merge_requests/" } else { "pull/" }
            );
            json!({
                "number": number, "iid": number, "title": "Another request",
                "html_url": url, "web_url": url,
                "head": {"ref": "feature/topic", "repo": {"full_name": if fork { "someone/fork" } else { project }}},
                "base": {"ref": "develop"}, "source_branch": "feature/topic", "target_branch": "develop",
                "source_project_id": if fork { 43 } else { 42 }, "target_project_id": 42,
                "state": if open { if gitlab { "opened" } else { "open" } } else { "closed" },
                "draft": draft, "user": {"login": author}, "author": {"username": author}
            })
        };
        fixture.configure(json!({
            "existing": true, "extra_page": true, "include_issue": true,
            "extra_requests": [
                request(8, "alice", false, true, true),
                request(9, "someone-else", false, false, true),
                request(10, "alice", true, false, true),
                request(11, "alice", false, false, true),
                request(12, "alice", false, false, false)
            ]
        }));
        let records = fixture.records(&["list", "--json"]);
        let records = records.as_array().unwrap();
        assert_eq!(
            records
                .iter()
                .map(|r| r["request"]["number"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![7, 8, 11]
        );
        assert_eq!(records[1]["request"]["status"], "draft");
        assert_eq!(records[1]["request"]["base"], "develop");
        assert_eq!(records[1]["request"]["branch"], "feature/topic");

        let calls = fixture.calls();
        if gitlab {
            let listing = calls
                .iter()
                .find(|call| {
                    call[6]
                        .as_str()
                        .is_some_and(|e| e.contains("author_username="))
                })
                .unwrap();
            assert!(listing.as_array().unwrap().contains(&json!("--paginate")));
            assert_eq!(
                listing[6],
                "projects/group%2Fsub%2Frepo/merge_requests?state=opened&per_page=100&scope=all&author_username=alice"
            );
        } else {
            let payloads: Vec<Value> = fs::read_to_string(fixture.dir.path().join("graphql.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            // The fixture returns one result per page, including the excluded fork.
            assert_eq!(payloads.len(), 6);
            let authored: Vec<_> = payloads
                .iter()
                .filter(|payload| {
                    payload["variables"]["search"] == "repo:owner/repo is:pr is:open author:alice"
                })
                .collect();
            assert_eq!(authored.len(), 4);
            for (page, payload) in authored.iter().enumerate() {
                assert_eq!(
                    payload["variables"]["cursor"],
                    if page == 0 {
                        Value::Null
                    } else {
                        json!(format!("cursor:{page}"))
                    }
                );
            }
            let assigned = payloads
                .iter()
                .find(|payload| {
                    payload["variables"]["search"] == "repo:owner/repo is:pr is:open assignee:alice"
                })
                .unwrap();
            assert!(assigned["variables"]["cursor"].is_null());
            assert_eq!(calls.iter().filter(|call| call[6] == "graphql").count(), 6);
            assert!(!calls.iter().any(|call| {
                call[6].as_str().is_some_and(|endpoint| {
                    endpoint.starts_with("repos/owner/repo/pulls")
                        || endpoint.starts_with("repos/owner/repo/issues")
                })
            }));
        }
    }
}

#[test]
fn refresh_skips_closed_and_merged_requests_but_retries_failed_fetches() {
    for (gitlab, status) in [
        (false, "merged"),
        (true, "merged"),
        (false, "closed"),
        (true, "closed"),
    ] {
        let fixture = Fixture::new(gitlab);
        let detail_suffix = if gitlab {
            "/merge_requests/7"
        } else {
            "/pulls/7"
        };
        let detail_calls = || {
            fixture
                .calls()
                .iter()
                .filter(|call| {
                    call[6]
                        .as_str()
                        .is_some_and(|endpoint| endpoint.ends_with(detail_suffix))
                })
                .count()
        };
        fixture.configure(json!({"existing": true}));
        fixture.records(&["list", "--json"]);
        fixture.configure(json!({"status": status}));
        let cached = fixture.records(&["list", "--json"]);
        assert_eq!(cached[0]["request"]["status"], status);
        let calls = detail_calls();

        fixture.configure(json!({"fail_detail": true}));
        for _ in 0..2 {
            assert_eq!(fixture.records(&["list", "--json"]), cached);
            assert_eq!(detail_calls(), calls);
        }

        // A provider failure marks the cached request stale.
        fixture.configure(json!({"fail_api": true}));
        assert!(fixture.records(&["list", "--json"])[0]["refresh_error"].is_string());
        fixture.configure(json!({"fail_api": false}));
        let failed = fixture.records(&["list", "--json"]);
        assert_eq!(failed[0]["request"]["status"], status);
        assert!(
            failed[0]["refresh_error"]
                .as_str()
                .unwrap()
                .contains("Detail lookup unavailable")
        );
        assert_eq!(detail_calls(), calls + 1);

        fixture.configure(json!({"fail_detail": false}));
        let recovered = fixture.records(&["list", "--json"]);
        assert_eq!(recovered[0]["request"]["status"], status);
        assert!(recovered[0]["refresh_error"].is_null());
        assert_eq!(detail_calls(), calls + 2);
        assert_eq!(fixture.records(&["list", "--json"]), recovered);
        assert_eq!(detail_calls(), calls + 2);
    }
}

#[test]
fn closed_requests_skip_detail_lookups_and_can_be_rediscovered() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        fixture.configure(json!({"existing": true}));
        fixture.records(&["list", "--json"]);
        fixture.configure(json!({"status": "closed"}));
        assert_eq!(
            fixture.records(&["list", "--json"])[0]["request"]["status"],
            "closed"
        );
        let calls = fixture.calls().len();
        fixture.configure(json!({"fail_discovery": true}));
        assert_eq!(
            fixture.records(&["list", "--json"])[0]["request"]["status"],
            "closed"
        );
        let suffix = if gitlab {
            "/merge_requests/7"
        } else {
            "/pulls/7"
        };
        assert_eq!(
            fixture.calls()[calls..]
                .iter()
                .filter(|call| {
                    call[6]
                        .as_str()
                        .is_some_and(|endpoint| endpoint.ends_with(suffix))
                })
                .count(),
            0
        );
        // Reopened requests are found by normal discovery, without polling closed ones.
        fixture.configure(json!({"status": "open", "fail_discovery": false}));
        assert_eq!(
            fixture.records(&["list", "--json"])[0]["request"]["status"],
            "open"
        );
    }
}

#[test]
fn github_discovery_batches_request_details() {
    let fixture = Fixture::new(false);
    let extra: Vec<_> = (8..=107).map(|number| json!({
        "number": number, "title": "Another request", "html_url": format!("https://github.com/owner/repo/pull/{number}"),
        "head": {"ref": "feature/topic", "repo": {"full_name": "owner/repo"}},
        "base": {"ref": "main"}, "state": "open", "user": {"login": "alice"}
    })).collect();
    fixture.configure(json!({"existing": true, "extra_requests": extra, "fail_detail": true}));
    let records = fixture.records(&["list", "--json"]);
    assert_eq!(records.as_array().unwrap().len(), 101);
    assert_eq!(records[100]["request"]["number"], 107);
    let calls = fixture.calls();
    assert_eq!(calls.iter().filter(|call| call[6] == "graphql").count(), 6);
    assert!(!calls.iter().any(|call| {
        call[6]
            .as_str()
            .is_some_and(|endpoint| endpoint.contains("/pulls/"))
    }));
}

#[test]
fn github_discovery_rejects_partial_graphql_errors_and_repeated_cursors() {
    for failure in [
        "graphql_errors",
        "graphql_error_page",
        "graphql_repeated_cursor",
    ] {
        let fixture = Fixture::new(false);
        fixture.configure(json!({"existing": true, "extra_page": true, failure: true, "extra_requests": [{
            "number": 8, "title": "Another request", "html_url": "https://github.com/owner/repo/pull/8",
            "head": {"ref": "feature/topic", "repo": {"full_name": "owner/repo"}},
            "base": {"ref": "main"}, "state": "open", "user": {"login": "alice"}
        }]}));
        let output = fixture.run(&["list", "--json"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Search unavailable") || stderr.contains("repeated or empty cursor"),
            "{stderr}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&success(output)).unwrap(),
            json!([])
        );
        fixture.configure(json!({failure: false}));
        assert_eq!(
            fixture
                .records(&["list", "--json"])
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn github_discovery_falls_back_when_search_exceeds_its_limit() {
    let fixture = Fixture::new(false);
    fixture.configure(
        json!({"existing": true, "graphql_count": 1001, "include_issue": true, "extra_page": true}),
    );
    assert_eq!(
        fixture
            .records(&["list", "--json"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let calls = fixture.calls();
    // Two searches, one approval batch, and one initial pipeline batch.
    assert_eq!(calls.iter().filter(|call| call[6] == "graphql").count(), 4);
    assert!(
        calls
            .iter()
            .any(|call| call[6] == "repos/owner/repo/issues?state=open&per_page=100&creator=alice")
    );
    assert!(
        calls
            .iter()
            .any(|call| call[6] == "repos/owner/repo/pulls/7")
    );
}

#[test]
fn failed_refresh_keeps_cached_request_and_marks_stale() {
    let fixture = Fixture::new(false);
    success(fixture.run(&["push"]));
    fixture.configure(json!({"fail_api":true}));
    let output = fixture.run(&["list", "--json"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("Warning:"));
    let records: Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(records[0]["request"]["status"], "open");
    assert!(
        records[0]["refresh_error"]
            .as_str()
            .unwrap()
            .contains("Authentication unavailable")
    );
    let calls = fixture.calls().len();
    fixture.records(&["list", "--offline", "--json"]);
    assert_eq!(fixture.calls().len(), calls);
}

#[test]
fn push_and_api_failures_do_not_create_requests() {
    let fixture = Fixture::new(false);
    fixture.configure(json!({"fail_api":true}));
    assert!(!fixture.run(&["push"]).status.success());
    assert!(!fixture.calls().iter().any(|c| c[0] == "git"));
    fixture.configure(json!({"fail_api":false,"fail_discovery":true}));
    assert!(!fixture.run(&["push"]).status.success());
    assert!(!fixture.calls().iter().any(|c| c[0] == "git"));
    fixture.configure(json!({"fail_discovery":false,"fail_push":true}));
    assert!(!fixture.run(&["push"]).status.success());
    assert!(!fixture.dir.path().join("payload.json").exists());
    assert!(!fixture.dir.path().join("data/requests.json").exists());
}

#[test]
fn create_failure_is_recoverable_after_successful_push() {
    let fixture = Fixture::new(false);
    fixture.configure(json!({"fail_create":true}));
    let output = fixture.run(&["push"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Branch was pushed"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("auth login"));
    fixture.configure(json!({"fail_create":false}));
    success(fixture.run(&["push"]));
    assert_eq!(
        fixture
            .records(&["list", "--offline", "--json"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rejects_detached_head_and_target_branch_without_pushing() {
    let fixture = Fixture::new(false);
    fixture.git(&fixture.repo, &["checkout", "--detach"]);
    assert!(String::from_utf8_lossy(&fixture.run(&["push"]).stderr).contains("detached HEAD"));
    fixture.git(&fixture.repo, &["checkout", "main"]);
    assert!(
        String::from_utf8_lossy(&fixture.run(&["push"]).stderr)
            .contains("Source and target branches must differ")
    );
    assert!(!fixture.calls().iter().any(|c| c[0] == "git"));
}

#[test]
fn list_from_outside_git_uses_stored_remotes_after_workspace_removal() {
    let fixture = Fixture::new(false);
    let checkout = fixture.workspace();
    success(fixture.run_at(&checkout, &["push"]));
    fixture.git(
        &fixture.repo,
        &["worktree", "remove", checkout.to_str().unwrap()],
    );
    fs::write(fixture.dir.path().join("grove/state.json"), "[]").unwrap();
    fixture.configure(json!({"status":"merged"}));
    let records: Value = serde_json::from_str(&success(
        fixture.run_at(fixture.dir.path(), &["list", "--json"]),
    ))
    .unwrap();
    assert_eq!(records[0]["request"]["status"], "merged");
    assert_eq!(records[0]["origins"][0]["workspace"]["name"], "feature");
}

#[test]
fn reuses_nondefault_target_and_honors_push_remote_configuration() {
    let fixture = Fixture::new(false);
    fixture.git(
        &fixture.repo,
        &[
            "remote",
            "add",
            "publish",
            "git@github.com:owner/destination.git",
        ],
    );
    fixture.git(
        &fixture.repo,
        &["config", "branch.feature/topic.pushRemote", "publish"],
    );
    fixture.configure(json!({"project":"owner/destination","existing":true,"base":"develop","branch":"feature/topic"}));
    assert!(success(fixture.run(&["push"])).contains("Tracking existing request:"));
    assert!(!fixture.dir.path().join("payload.json").exists());
    let records = fixture.records(&["list", "--offline", "--json"]);
    assert_eq!(records[0]["request"]["base"], "develop");
    assert_eq!(records[0]["remote"]["project"], "owner/destination");
    let push = fixture.calls().into_iter().find(|c| c[0] == "git").unwrap();
    assert!(push.as_array().unwrap().contains(&json!("publish")));

    fixture.configure(json!({"project":"owner/repo","existing":false}));
    success(fixture.run(&["push", "--remote", "origin", "--base", "release"]));
    let records = fixture.records(&["list", "--offline", "--json", "--repo", "owner/repo"]);
    assert_eq!(records[0]["request"]["base"], "release");
}

#[test]
fn fallback_workspace_discovery_does_not_assign_other_branches() {
    let fixture = Fixture::new(false);
    fixture.git(&fixture.repo, &["checkout", "main"]);
    let checkout = fixture.dir.path().join("grove/workspaces/fallback/repo");
    fixture.git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            checkout.to_str().unwrap(),
            "feature/topic",
        ],
    );
    fixture.configure(json!({"existing":true}));
    let records = fixture.records(&["list", "--json", "--workspace", "fallback"]);
    assert_eq!(records[0]["origins"][0]["workspace"]["name"], "fallback");
    fixture.git(&checkout, &["checkout", "-b", "another-feature"]);
    fs::remove_file(fixture.dir.path().join("data/requests.json")).unwrap();
    assert_eq!(
        fixture.records(&["list", "--json", "--workspace", "fallback"]),
        json!([])
    );
}

#[test]
fn fresh_list_and_invalid_cli_arguments() {
    let fixture = Fixture::new(false);
    assert_eq!(fixture.records(&["list", "--offline", "--json"]), json!([]));
    assert!(fixture.calls().is_empty());
    assert!(
        !fixture
            .run(&["push", "--provider", "unknown"])
            .status
            .success()
    );
    assert!(!fixture.run(&["not-a-command"]).status.success());
    assert!(success(fixture.run(&["--help"])).contains("push"));
    fixture.git(&fixture.repo, &["checkout", "--orphan", "new-project"]);
    let output = fixture.run(&["push"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Create a commit"));
}

#[test]
fn file_paths_replace_environment_and_follow_config_changes() {
    let fixture = Fixture::new(true);
    fixture.git(&fixture.repo, &["config", "--unset", "pr.provider"]);
    fixture.git(&fixture.repo, &["config", "ptrack.provider", "gitlab"]);
    let checkout = fixture.workspace();
    success(
        fixture
            .command(&checkout, &["push"])
            .env("PR_DATA_DIR", fixture.dir.path().join("unused"))
            .env("PR_GROVE_DIR", fixture.dir.path().join("unused"))
            .env("PTRACK_DATA_DIR", fixture.dir.path().join("unused-legacy"))
            .env("PTRACK_GROVE_DIR", fixture.dir.path().join("unused-legacy"))
            .output()
            .unwrap(),
    );
    let records = fixture.records(&["list", "--offline", "--json"]);
    assert_eq!(records[0]["origins"][0]["workspace"]["name"], "feature");
    assert!(!fixture.dir.path().join("unused").exists());
    assert!(!fixture.dir.path().join("unused-legacy").exists());

    // The new provider key takes precedence over the legacy key.
    fixture.git(&fixture.repo, &["config", "pr.provider", "gitlab"]);
    fixture.git(&fixture.repo, &["config", "ptrack.provider", "github"]);
    success(fixture.run_at(&checkout, &["push"]));

    fs::write(
        fixture.dir.path().join("config.toml"),
        "grove_dir = 'grove'\ndata_dir = 'empty'\n",
    )
    .unwrap();
    let output = fixture
        .command(&fixture.repo, &["list", "--offline", "--json"])
        .output()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&success(output)).unwrap(),
        json!([])
    );

    fs::write(
        fixture.dir.path().join("config.toml"),
        "grove_dir = 'grove'\ndata_dir = 'data'\n",
    )
    .unwrap();
    let output = fixture.run(&["list", "--offline", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&success(output))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[cfg(target_os = "linux")]
#[test]
fn explicit_cache_path_reuses_legacy_tracking_without_implicit_fallback() {
    let fixture = Fixture::new(false);
    success(fixture.run(&["push"]));
    let expected = fixture.records(&["list", "--offline", "--json"]);
    let data_home = fixture.dir.path().join("xdg-data");
    let legacy = data_home.join("ptrack");
    fs::create_dir_all(&legacy).unwrap();
    fs::copy(
        fixture.dir.path().join("data/requests.json"),
        legacy.join("requests.json"),
    )
    .unwrap();
    fs::write(
        fixture.dir.path().join("config.toml"),
        "grove_dir = 'grove'\ndata_dir = 'xdg-data/ptrack'\n",
    )
    .unwrap();
    let run = || {
        let output = fixture
            .command(&fixture.repo, &["list", "--offline", "--json"])
            .env("XDG_DATA_HOME", &data_home)
            .output()
            .unwrap();
        serde_json::from_str::<Value>(&success(output)).unwrap()
    };
    assert_eq!(run(), expected);
    fs::create_dir(data_home.join("pr")).unwrap();
    assert_eq!(run(), expected);
    fs::write(
        fixture.dir.path().join("config.toml"),
        "grove_dir = 'grove'\ndata_dir = 'xdg-data/pr'\n",
    )
    .unwrap();
    assert_eq!(run(), json!([]));
    assert!(legacy.join("requests.json").exists());
}

/// File-level provider selection overrides repository settings, and explicit flags override the file.
#[test]
fn file_provider_override_and_cli_provider_precedence() {
    let fixture = Fixture::new(true);
    fixture.git(&fixture.repo, &["config", "pr.provider", "github"]);
    fs::write(
        fixture.dir.path().join("config.toml"),
        "data_dir = 'data'\ngrove_dir = 'grove'\nprovider = 'gitlab'\n",
    )
    .unwrap();
    success(fixture.run(&["push"]));
    assert_eq!(
        fixture.records(&["list", "--offline", "--json"])[0]["remote"]["provider"],
        "gitlab"
    );

    fs::write(
        fixture.dir.path().join("config.toml"),
        "data_dir = 'data'\ngrove_dir = 'grove'\nprovider = 'github'\n",
    )
    .unwrap();
    success(fixture.run(&["--provider", "gitlab", "push"]));
    assert_eq!(
        fixture.records(&["list", "--offline", "--json"])[0]["remote"]["provider"],
        "gitlab"
    );
}

#[test]
fn gix_resolves_included_config_and_push_url_rewrites() {
    let fixture = Fixture::new(false);
    fixture.git(
        &fixture.repo,
        &["remote", "set-url", "origin", "gh-alias:owner/repo.git"],
    );
    let config = fixture.dir.path().join("included.config");
    fs::write(&config, "[url \"git@github.com:\"]\n\tinsteadOf = gh-alias:\n[url \"git@github.com:team/\"]\n\tpushInsteadOf = gh-alias:owner/\n").unwrap();
    fixture.git(
        &fixture.repo,
        &["config", "include.path", config.to_str().unwrap()],
    );
    fixture.configure(json!({"project":"team/repo"}));
    success(fixture.run(&["push"]));
    let records = fixture.records(&["list", "--offline", "--json"]);
    assert_eq!(records[0]["remote"]["project"], "team/repo");
    assert_eq!(records[0]["request"]["branch"], "feature/topic");
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call[0] == "git")
            .count(),
        1
    );
}

#[test]
fn gix_rejects_multiple_push_urls_and_unborn_branches_without_publishing() {
    let fixture = Fixture::new(false);
    fixture.git(
        &fixture.repo,
        &[
            "config",
            "--add",
            "remote.origin.pushurl",
            "git@github.com:owner/repo.git",
        ],
    );
    fixture.git(
        &fixture.repo,
        &[
            "config",
            "--add",
            "remote.origin.pushurl",
            "git@github.com:another/repo.git",
        ],
    );
    let output = fixture.run(&["push"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("multiple push URLs"));
    assert!(fixture.calls().is_empty());
    fixture.git(
        &fixture.repo,
        &["config", "--unset-all", "remote.origin.pushurl"],
    );
    fixture.git(&fixture.repo, &["checkout", "--orphan", "unborn"]);
    let output = fixture.run(&["push"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Create a commit"));
    assert!(fixture.calls().is_empty());
}

#[test]
fn gix_reads_multiline_commit_subject_and_body_from_a_subdirectory() {
    let fixture = Fixture::new(false);
    fixture.git(
        &fixture.repo,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "A multiline\nsubject",
            "-m",
            "First paragraph.\n\nSecond paragraph.",
        ],
    );
    let nested = fixture.repo.join("nested/path");
    fs::create_dir_all(&nested).unwrap();
    success(fixture.run_at(&nested, &["push"]));
    let payload: Value =
        serde_json::from_slice(&fs::read(fixture.dir.path().join("payload.json")).unwrap())
            .unwrap();
    assert_eq!(payload["title"], "A multiline subject");
    assert_eq!(payload["body"], "First paragraph.\n\nSecond paragraph.");
    let records = fixture.records(&["list", "--offline", "--json"]);
    assert_eq!(records[0]["origins"][0]["checkout"], json!(fixture.repo));
}

#[test]
fn approval_state_refreshes_and_is_retained_offline() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        fixture.configure(json!({"existing": true, "approved": true}));
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(records[0]["request"]["status"], "open");
        assert_eq!(records[0]["request"]["approved"], true);
        assert!(success(fixture.run(&["list", "--offline"])).contains("APPROVED\t"));
        fixture.configure(json!({"fail_approvals": true, "graphql_errors": true}));
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(records[0]["request"]["approved"], true);
        assert!(records[0]["refresh_error"].is_string());
        fixture.configure(
            json!({"fail_approvals": false, "graphql_errors": false, "approved": false}),
        );
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(records[0]["request"]["approved"], false);
        assert!(records[0]["refresh_error"].is_null());
        assert!(success(fixture.run(&["list", "--offline"])).contains("OPEN\t"));
    }
}

#[test]
fn gitlab_approval_lookup_is_batched_for_large_lists() {
    let fixture = Fixture::new(true);
    let extra: Vec<_> = (8..=507).map(|number| json!({
        "iid": number, "title": "Another request",
        "web_url": format!("https://code.example.test/group/sub/repo/-/merge_requests/{number}"),
        "source_branch": "feature/topic", "target_branch": "main", "state": "opened",
        "source_project_id": 42, "target_project_id": 42, "author": {"username": "alice"},
        "approved": number % 2 == 0
    })).collect();
    fixture.configure(json!({
        "existing": true, "extra_requests": extra, "fail_detail": true,
        "measure_concurrency": true, "concurrency_barrier": "^graphql$"
    }));
    let records = fixture.records(&["list", "--json"]);
    assert_eq!(records.as_array().unwrap().len(), 501);
    for record in records.as_array().unwrap().iter().skip(1) {
        assert_eq!(
            record["request"]["approved"],
            record["request"]["number"].as_u64().unwrap() % 2 == 0
        );
    }
    let calls = fixture.calls();
    assert_eq!(calls.iter().filter(|call| call[6] == "graphql").count(), 22);
    let probe: Value =
        serde_json::from_slice(&fs::read(fixture.dir.path().join("concurrency.json")).unwrap())
            .unwrap();
    assert_eq!(probe["peak"], 10);
    assert_eq!(probe["active"], 0);
    assert!(probe["barrier_broken"].is_null());
    assert!(!calls.iter().any(|call| {
        call[6]
            .as_str()
            .is_some_and(|endpoint| endpoint.contains("/merge_requests/"))
    }));
}

#[test]
fn discovers_assigned_requests_and_deduplicates_authored_requests() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        fixture.workspace();
        let (host, project, path) = if gitlab {
            ("code.example.test", "group/sub/repo", "-/merge_requests")
        } else {
            ("github.com", "owner/repo", "pull")
        };
        let request = |number, assignees: &[&str], branch: &str, draft, open| {
            let url = format!("https://{host}/{project}/{path}/{number}");
            json!({
                "number": number, "iid": number, "title": "Assigned request",
                "html_url": url, "web_url": url,
                "head": {"ref": branch, "repo": {"full_name": project}},
                "base": {"ref": "main"}, "source_branch": branch, "target_branch": "main",
                "source_project_id": 42, "target_project_id": 42,
                "state": if open { if gitlab { "opened" } else { "open" } } else { "closed" },
                "draft": draft, "approved": true,
                "user": {"login": "bob"}, "author": {"username": "bob"},
                "assignees": assignees.iter().map(|name| json!({"login": name, "username": name})).collect::<Vec<_>>(),
                "reviewers": [{"username": "alice"}], "requested_reviewers": [{"login": "alice"}]
            })
        };
        fixture.configure(json!({
            "existing": true, "extra_page": true,
            "assignees": [{"login": "alice", "username": "alice"}],
            "extra_requests": [
                request(8, &["bob", "ALICE"], "bob/feature", false, true),
                request(9, &["alice"], "feature/topic", true, true),
                request(10, &["alice"], "feature/topic", false, false),
                request(11, &["bob"], "feature/topic", false, true),
                request(12, &[], "feature/topic", false, true),
                request(13, &["alice"], "last-page", false, true)
            ]
        }));
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(
            records
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["request"]["number"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [7, 9, 8, 13]
        );
        assert_eq!(records[2]["request"]["approved"], true);
        assert!(records[2]["origins"][0]["workspace"].is_null());
        assert_eq!(records[1]["request"]["status"], "draft");
        assert_eq!(records[1]["origins"][0]["workspace"]["name"], "feature");
        let calls = fixture.calls();
        if gitlab {
            let mut listings: Vec<_> = calls
                .iter()
                .filter_map(|call| call[6].as_str())
                .filter(|endpoint| endpoint.contains("?state=opened"))
                .collect();
            listings.sort_unstable();
            assert_eq!(
                listings,
                [
                    "projects/group%2Fsub%2Frepo/merge_requests?state=opened&per_page=100&scope=all&assignee_username[]=alice",
                    "projects/group%2Fsub%2Frepo/merge_requests?state=opened&per_page=100&scope=all&author_username=alice"
                ]
            );
            // Approval and pipeline batching happen after deduplication.
            assert_eq!(calls.iter().filter(|call| call[6] == "graphql").count(), 2);
        } else {
            let payloads: Vec<Value> = fs::read_to_string(fixture.dir.path().join("graphql.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(payloads.len(), 6);
            assert_eq!(
                payloads
                    .iter()
                    .filter(|payload| {
                        payload["variables"]["search"]
                            == "repo:owner/repo is:pr is:open author:alice"
                    })
                    .count(),
                1
            );
            assert_eq!(
                payloads
                    .iter()
                    .filter(|payload| {
                        payload["variables"]["search"]
                            == "repo:owner/repo is:pr is:open assignee:alice"
                    })
                    .count(),
                4
            );
        }
        assert_eq!(fixture.records(&["list", "--offline", "--json"]), records);
        assert_eq!(fixture.calls().len(), calls.len());

        fixture.configure(json!({"fail_assignee_discovery": true}));
        let output = fixture.run(&["list", "--json"]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("Warning:"));
        let cached: Value = serde_json::from_str(&success(output)).unwrap();
        assert_eq!(cached.as_array().unwrap().len(), 4);
        assert_eq!(cached[2]["request"]["number"], 8);
    }
}

#[test]
fn github_assignee_search_limit_falls_back_to_assignee_filter() {
    let fixture = Fixture::new(false);
    fixture.configure(json!({
        "graphql_counts": {"assignee": 1001}, "include_issue": true,
        "extra_requests": [{
            "number": 8, "title": "Assigned request", "html_url": "https://github.com/owner/repo/pull/8",
            "head": {"ref": "bob/feature", "repo": {"full_name": "owner/repo"}}, "base": {"ref": "main"},
            "state": "open", "user": {"login": "bob"}, "assignees": [{"login": "alice"}], "approved": true
        }]
    }));
    let records = fixture.records(&["list", "--json"]);
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["request"]["number"], 8);
    assert_eq!(records[0]["request"]["approved"], true);
    let calls = fixture.calls();
    assert!(calls.iter().any(|call| call[6] == "repos/owner/repo/issues?state=open&per_page=100&assignee=alice"));
    assert!(!calls.iter().any(|call| {
        call[6]
            .as_str()
            .is_some_and(|endpoint| endpoint.contains("creator="))
    }));
}

/// Exercise multiple status batches, mixed states, and partial failures on both providers.
/// Request counts guard against regressing to one detail/approval call per tracked PR.
#[test]
fn refresh_batches_known_requests_and_preserves_individual_failures() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let (host, project, path) = if gitlab {
            ("code.example.test", "group/sub/repo", "-/merge_requests")
        } else {
            ("github.com", "owner/repo", "pull")
        };
        let requests = |updated: bool| {
            (8..=108).map(|number| {
            let closed = updated && number % 4 < 2;
            let merged = closed && number % 4 == 1;
            let url = format!("https://{host}/{project}/{path}/{number}");
            json!({
                "number": number, "iid": number,
                "title": if updated { "Updated request" } else { "Cached request" },
                "html_url": url, "web_url": url,
                "head": {"ref": "feature/topic", "repo": {"full_name": project}},
                "base": {"ref": "main"}, "source_branch": "feature/topic", "target_branch": "main",
                "source_project_id": 42, "target_project_id": 42,
                "state": if merged && gitlab { "merged" } else if closed { "closed" }
                    else if gitlab { "opened" } else { "open" },
                "merged_at": if merged { Some("2026-10-10") } else { None },
                "draft": updated && number % 4 == 2, "approved": updated,
                "user": {"login": if updated { "bob" } else { "alice" }},
                "author": {"username": if updated { "bob" } else { "alice" }},
                "issue_comments": [{"created_at": if updated { "2026-10-02T12:00:00Z" } else { "2026-10-01T12:00:00Z" }}],
                "discussions": [{"notes": [{"created_at": if updated { "2026-10-02T12:00:00Z" } else { "2026-10-01T12:00:00Z" }}]}]
            })
        }).collect::<Vec<_>>()
        };
        fixture.configure(json!({"existing": true, "extra_requests": requests(false)}));
        let cached = fixture.records(&["list", "--json"]);
        assert_eq!(cached.as_array().unwrap().len(), 102);

        fixture.configure(json!({"status": "closed", "extra_requests": requests(true), "fail_detail_numbers": [8], "fail_comment_numbers": [9]}));
        let before_calls = fixture.calls().len();
        let output = fixture.run(&["list", "--json"]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("#8:"));
        let updated: Value = serde_json::from_str(&success(output)).unwrap();
        assert_eq!(updated.as_array().unwrap().len(), 102);
        for (index, record) in updated.as_array().unwrap().iter().enumerate() {
            let number = record["request"]["number"].as_u64().unwrap();
            assert_eq!(record["origins"], cached[index]["origins"]);
            if number == 8 || number == 9 {
                assert_eq!(record["request"], cached[index]["request"]);
                assert!(record["refresh_error"].is_string());
                continue;
            }
            assert!(record["refresh_error"].is_null(), "{record}");
            let state = if number == 7 || number % 4 == 0 {
                "closed"
            } else if number % 4 == 1 {
                "merged"
            } else if number % 4 == 2 {
                "draft"
            } else {
                "open"
            };
            assert_eq!(record["request"]["status"], state);
            if number != 7 {
                assert_eq!(record["request"]["last_comment_at"], "2026-10-02T12:00:00Z");
            }
            if matches!(state, "open" | "draft") {
                assert_eq!(record["request"]["approved"], true);
            }
        }
        let calls = fixture.calls();
        let calls = &calls[before_calls..];
        assert!(!calls.iter().any(|call| {
            call[6]
                .as_str()
                .is_some_and(|e| e.contains("/pulls/") || e.contains("/merge_requests/"))
        }));
        if gitlab {
            assert_eq!(
                calls
                    .iter()
                    .filter(|call| call[6].as_str().is_some_and(|e| e.contains("iids[]=")))
                    .count(),
                3
            );
            assert_eq!(calls.iter().filter(|call| call[6] == "graphql").count(), 3);
        } else {
            // Two discovery searches and three batches, with approvals included.
            assert_eq!(calls.iter().filter(|call| call[6] == "graphql").count(), 5);
        }
        assert_eq!(fixture.records(&["list", "--offline", "--json"]), updated);
        fixture.configure(json!({"fail_detail_numbers": [], "fail_comment_numbers": []}));
        let recovered = fixture.records(&["list", "--json"]);
        assert_eq!(recovered[1]["request"]["status"], "closed");
        assert!(recovered[1]["refresh_error"].is_null());
        assert_eq!(recovered[2]["request"]["status"], "merged");
        assert_eq!(
            recovered[2]["request"]["last_comment_at"],
            "2026-10-02T12:00:00Z"
        );
        assert!(recovered[2]["refresh_error"].is_null());
    }
}

/// Measure overlapping provider processes across repositories and both hosts.
/// This catches nested pools exceeding the cap and duplicated identity lookups.
#[test]
fn refresh_shares_ten_calls_and_identity_lookups_across_repositories() {
    let fixture = Fixture::new(false);
    fixture.configure(json!({"existing": true}));
    let template = fixture.records(&["list", "--json"])[0].clone();
    let mut repositories = serde_json::Map::new();
    let records: Vec<_> = (0..24).map(|index| {
        let gitlab = index % 2 == 1;
        let host = if gitlab { "gitlab.com" } else { "github.com" };
        let project = format!("owner/repo-{index}");
        repositories.insert(format!("{host}/{project}"), json!({
            "existing": true, "approved": true, "fail_api": index == 0,
            "fail_discovery": index == 1, "assignees": [{"login": "alice", "username": "alice"}]
        }));
        let mut record = template.clone();
        record["remote"] = json!({"provider": if gitlab {"gitlab"} else {"github"}, "host": host, "project": project});
        record["origins"] = json!([]);
        record
    }).collect();
    fs::write(
        fixture.dir.path().join("data/requests.json"),
        serde_json::to_vec(&json!({
            "version": 1, "requests": records
        }))
        .unwrap(),
    )
    .unwrap();
    fixture.configure(json!({
        "repositories": repositories, "measure_concurrency": true,
        "concurrency_barrier": "^repos/[^/]+/[^/]+$|^projects/[^/]+$"
    }));
    let before_calls = fixture.calls().len();
    let output = fixture.run_at(fixture.dir.path(), &["list", "--json"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("Warning:"));
    let records: Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 24);
    for record in records.as_array().unwrap() {
        if record["remote"]["project"] == "owner/repo-0" {
            assert!(record["refresh_error"].is_string());
        } else {
            assert!(record["refresh_error"].is_null(), "{record}");
            assert_eq!(record["request"]["approved"], true);
        }
    }
    let probe: Value =
        serde_json::from_slice(&fs::read(fixture.dir.path().join("concurrency.json")).unwrap())
            .unwrap();
    assert_eq!(probe["active"], 0);
    assert_eq!(probe["peak"], 10);
    assert!(probe["barrier_broken"].is_null());
    let calls = fixture.calls();
    assert_eq!(
        calls[before_calls..]
            .iter()
            .filter(|call| call[6] == "user")
            .count(),
        2
    );
    let offline = fixture.run_at(fixture.dir.path(), &["list", "--offline", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&success(offline)).unwrap(),
        records
    );
    assert_eq!(fixture.calls().len(), calls.len());

    // Authentication failures are also cached once per host, without blocking workers.
    fixture.configure(json!({"fail_user": true}));
    success(fixture.run_at(fixture.dir.path(), &["list", "--json"]));
    assert_eq!(
        fixture.calls()[calls.len()..]
            .iter()
            .filter(|call| call[6] == "user")
            .count(),
        2
    );
}

/// Distinguish batch-wide GraphQL failures from approval-only failures.
/// Cached data must survive errors while confirmed closed states still advance.
#[test]
fn bulk_refresh_rejects_global_errors_and_keeps_successful_closed_states() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let project = if gitlab {
            "group/sub/repo"
        } else {
            "owner/repo"
        };
        fixture.configure(json!({"existing": true, "extra_requests": [{
            "number": 8, "iid": 8, "title": "Another request", "html_url": "https://github.com/owner/repo/pull/8",
            "web_url": "https://code.example.test/group/sub/repo/-/merge_requests/8",
            "head": {"ref": "feature/topic", "repo": {"full_name": project}}, "base": {"ref": "main"},
            "source_branch": "feature/topic", "target_branch": "main",
            "source_project_id": 42, "target_project_id": 42,
            "state": if gitlab {"opened"} else {"open"}, "user": {"login": "alice"}, "author": {"username": "alice"}
        }]}));
        let cached = fixture.records(&["list", "--json"]);
        fixture.configure(json!({
            "fail_discovery": true, "status": "closed", "fail_approvals": true,
            "batch_graphql_errors": [{"message": "Rate limit exceeded"}]
        }));
        let output = fixture.run(&["list", "--json"]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("Warning:"));
        let records: Value = serde_json::from_str(&success(output)).unwrap();
        assert_eq!(records.as_array().unwrap().len(), 2);
        assert_eq!(records[1]["request"], cached[1]["request"]);
        assert!(records[1]["refresh_error"].is_string());
        if gitlab {
            assert_eq!(records[0]["request"]["status"], "closed");
            assert!(records[0]["refresh_error"].is_null());
        } else {
            assert_eq!(records[0]["request"], cached[0]["request"]);
            assert!(records[0]["refresh_error"].is_string());
        }
    }
}

/// Track written conversation/review activity without mistaking approvals or system notes
/// for comments, and retain the precise value when refresh is offline or fails.
#[test]
fn last_comment_includes_conversation_reviews_and_inline_replies() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        fixture.configure(json!({"existing": true}));
        let empty = fixture.records(&["list", "--json"]);
        assert!(empty[0]["request"]["last_comment_at"].is_null());
        let conversation = "2026-10-01T10:00:00Z";
        let summary = "2026-10-01T11:00:00Z";
        let reply = "2026-10-01T12:00:00Z";
        for (step, expected) in [conversation, summary, reply].into_iter().enumerate() {
            fixture.configure(json!({
                "issue_comments": [{"created_at": conversation}],
                "reviews": [
                    {"id": 1, "state": "COMMENTED", "body": if step >= 1 { "Review summary" } else { "" }, "submitted_at": summary},
                    {"id": 2, "state": "APPROVED", "body": "  ", "submitted_at": "2026-10-01T14:00:00Z"},
                    {"id": 3, "state": "PENDING", "body": "Unpublished", "submitted_at": "2026-10-01T15:00:00Z"}
                ],
                "review_comments": if step == 2 { json!([
                    {"pull_request_review_id": 1, "body": "Reply on an old thread", "created_at": "2026-10-01T13:00:00+02:00", "published_at": reply},
                    {"pull_request_review_id": 3, "body": "Pending", "created_at": "2026-10-01T15:00:00Z"}
                ]) } else { json!([]) },
                "discussions": [
                    {"notes": [{"body": "Conversation", "created_at": conversation}]},
                    {"notes": [
                        {"body": "Review", "created_at": summary, "system": step < 1},
                        {"body": "Inline reply", "created_at": "2026-10-01T14:00:00+02:00", "system": step < 2},
                        {"body": "Approved", "created_at": "2026-10-01T15:00:00Z", "system": true}
                    ]}
                ]
            }));
            let before = fixture.calls().len();
            let records = fixture.records(&["list", "--json"]);
            assert!(records[0]["refresh_error"].is_null(), "{records}");
            assert_eq!(records[0]["request"]["last_comment_at"], expected);
            // Comment metadata rides along with existing discovery/approval calls.
            let calls = fixture.calls();
            assert_eq!(
                calls[before..].iter().filter(|c| c[6] == "graphql").count(),
                if gitlab { 1 } else { 2 }
            );
            assert_eq!(fixture.records(&["list", "--offline", "--json"]), records);
            assert_eq!(fixture.calls().len(), calls.len());
        }
        let table = success(fixture.run(&["list", "--offline"]));
        let lines: Vec<_> = table.lines().collect();
        assert_eq!(
            lines[0],
            "STATUS\tPIPELINE\tREPOSITORY\tTITLE\tUNRESOLVED\tLAST COMMENT\tWORKSPACE\tBRANCH\tURL"
        );
        let columns: Vec<_> = lines[1].split('\t').collect();
        assert_eq!(columns.len(), 9);
        assert_eq!(columns[1], "SUCCEED");
        assert_eq!(columns[3], "Add feature");
        assert_eq!(columns[4], "0");
        assert!(columns[5].ends_with(" ago") || columns[5] == "just now");

        fixture.configure(json!({"fail_api": true}));
        let failed = fixture.records(&["list", "--json"]);
        assert_eq!(failed[0]["request"]["last_comment_at"], reply);
        assert!(failed[0]["refresh_error"].is_string());
        fixture.configure(json!({"fail_api": false, "issue_comments": [], "reviews": [], "review_comments": [], "discussions": []}));
        let cleared = fixture.records(&["list", "--json"]);
        assert!(cleared[0]["refresh_error"].is_null());
        assert!(cleared[0]["request"]["last_comment_at"].is_null());
    }
}

/// Count discussions once, exclude ordinary comments, and preserve unknown/stale counts.
#[test]
fn unresolved_threads_count_discussions_and_keep_offline_and_stale_values() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let note = |resolvable, resolved| {
            json!({
                "body": "Review note", "created_at": "2026-10-01T12:00:00Z",
                "resolvable": resolvable, "resolved": resolved
            })
        };
        fixture.configure(json!({
            "existing": true,
            "review_threads": [
                {"isResolved": false}, {"isResolved": false, "isOutdated": true}, {"isResolved": true}
            ],
            "issue_comments": [note(false, false)],
            "discussions": [
                {"notes": [note(true, false), note(true, false)]},
                {"notes": [note(true, false)]},
                {"notes": [note(true, true)]},
                {"notes": [note(false, false)]}
            ]
        }));
        let mut records = fixture.records(&["list", "--json"]);
        assert_eq!(records[0]["request"]["unresolved_threads"], 2);
        let calls = fixture.calls().len();
        assert_eq!(fixture.records(&["list", "--offline", "--json"]), records);
        let table = success(fixture.run(&["list", "--offline"]));
        assert_eq!(
            table.lines().next().unwrap().split('\t').nth(4),
            Some("UNRESOLVED")
        );
        assert_eq!(table.lines().nth(1).unwrap().split('\t').nth(4), Some("2"));
        assert_eq!(fixture.calls().len(), calls);

        // Old caches distinguish an unknown value from a successful zero.
        records[0]["request"]
            .as_object_mut()
            .unwrap()
            .remove("unresolved_threads");
        let cache = fixture.dir.path().join("data/requests.json");
        fs::write(
            &cache,
            serde_json::to_vec(&json!({"version": 1, "requests": records})).unwrap(),
        )
        .unwrap();
        assert!(
            fixture.records(&["list", "--offline", "--json"])[0]["request"]["unresolved_threads"]
                .is_null()
        );
        let table = success(fixture.run(&["list", "--offline"]));
        assert_eq!(table.lines().nth(1).unwrap().split('\t').nth(4), Some("—"));
        assert_eq!(
            fixture.records(&["list", "--json"])[0]["request"]["unresolved_threads"],
            2
        );

        fixture.configure(json!({"fail_thread_numbers": [7]}));
        let failed = fixture.records(&["list", "--json"]);
        assert_eq!(failed[0]["request"]["unresolved_threads"], 2);
        assert!(failed[0]["refresh_error"].is_string());
        let table = success(fixture.run(&["list", "--offline"]));
        assert_eq!(table.lines().nth(1).unwrap().split('\t').nth(4), Some("2*"));

        fixture.configure(
            json!({"fail_thread_numbers": [], "review_threads": [{"isResolved": true}],
            "discussions": [{"notes": [note(true, true), note(false, false)]}]}),
        );
        let resolved = fixture.records(&["list", "--json"]);
        assert_eq!(resolved[0]["request"]["unresolved_threads"], 0);
        assert!(resolved[0]["refresh_error"].is_null());
        assert_eq!(fixture.records(&["list", "--offline", "--json"]), resolved);

        // Direct URL tracking fetches the same metadata, including closed requests.
        fixture.configure(
            json!({"status": "closed", "review_threads": [{"isResolved": false}],
            "discussions": [{"notes": [note(true, false)]}]}),
        );
        success(fixture.run(&["add", resolved[0]["request"]["url"].as_str().unwrap()]));
        let closed = fixture.records(&["list", "--offline", "--json"]);
        assert_eq!(closed[0]["request"]["unresolved_threads"], 1);
        assert_eq!(closed[0]["request"]["status"], "closed");
    }
}

/// Later pages and malformed metadata must never silently undercount pending threads.
#[test]
fn github_unresolved_threads_follow_all_pages_and_preserve_cache_on_errors() {
    let fixture = Fixture::new(false);
    let threads: Vec<_> = (0..205).map(|i| json!({"isResolved": i < 100})).collect();
    fixture.configure(json!({"existing": true, "review_threads": threads}));
    let records = fixture.records(&["list", "--json"]);
    assert_eq!(records[0]["request"]["unresolved_threads"], 105);
    let queries: Vec<Value> = fs::read_to_string(fixture.dir.path().join("graphql.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for cursor in ["threads:100", "threads:200"] {
        assert_eq!(
            queries
                .iter()
                .filter(|q| q["variables"]["cursor"] == cursor)
                .count(),
            1
        );
    }
    for failure in [
        "fail_thread_page",
        "thread_repeated_cursor",
        "thread_empty_cursor",
    ] {
        fixture.configure(json!({failure: true}));
        let failed = fixture.records(&["list", "--json"]);
        assert_eq!(failed[0]["request"]["unresolved_threads"], 105);
        assert!(failed[0]["refresh_error"].is_string());
        fixture.configure(json!({failure: false}));
    }
    fixture.configure(json!({"thread_page_errors": [{"message": "Thread metadata unavailable"}]}));
    let failed = fixture.records(&["list", "--json"]);
    assert_eq!(failed[0]["request"]["unresolved_threads"], 105);
    assert!(failed[0]["refresh_error"].is_string());
    fixture.configure(json!({"thread_page_errors": [], "review_threads": [{}]}));
    let failed = fixture.records(&["list", "--json"]);
    assert_eq!(failed[0]["request"]["unresolved_threads"], 105);
    assert!(
        failed[0]["refresh_error"]
            .as_str()
            .unwrap()
            .contains("resolution state")
    );
    fixture.configure(json!({"review_threads": []}));
    let empty = fixture.records(&["list", "--json"]);
    assert_eq!(empty[0]["request"]["unresolved_threads"], 0);
    assert!(empty[0]["refresh_error"].is_null());
}

/// Refresh counts in detail batches and isolate each request's missing metadata.
#[test]
fn unresolved_thread_batch_errors_keep_other_request_counts() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let project = if gitlab {
            "group/sub/repo"
        } else {
            "owner/repo"
        };
        let mut other = json!({
            "number": 8, "iid": 8, "title": "Another request",
            "html_url": "https://github.com/owner/repo/pull/8",
            "web_url": "https://code.example.test/group/sub/repo/-/merge_requests/8",
            "head": {"ref": "feature", "repo": {"full_name": project}}, "base": {"ref": "main"},
            "source_branch": "feature", "target_branch": "main", "source_project_id": 42, "target_project_id": 42,
            "state": if gitlab { "opened" } else { "open" },
            "user": {"login": "alice"}, "author": {"username": "alice"},
            "review_threads": [{"isResolved": false}],
            "discussions": [{"notes": [{"resolvable": true, "resolved": false, "created_at": "2026-10-01T12:00:00Z"}]}]
        });
        fixture.configure(
            json!({"existing": true, "review_threads": other["review_threads"],
            "discussions": other["discussions"], "extra_requests": [other.clone()]}),
        );
        let initial = fixture.records(&["list", "--json"]);
        assert_eq!(initial.as_array().unwrap().len(), 2);
        assert!(
            initial
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["request"]["unresolved_threads"] == 1)
        );
        other["review_threads"] = json!([]);
        other["discussions"] = json!([]);
        fixture.configure(
            json!({"fail_discovery": true, "fail_thread_numbers": [7], "extra_requests": [other]}),
        );
        let refreshed = fixture.records(&["list", "--json"]);
        let first = refreshed
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["request"]["number"] == 7)
            .unwrap();
        let second = refreshed
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["request"]["number"] == 8)
            .unwrap();
        assert_eq!(first["request"]["unresolved_threads"], 1);
        assert!(first["refresh_error"].is_string());
        assert_eq!(second["request"]["unresolved_threads"], 0);
        assert!(second["refresh_error"].is_null());
    }
}

/// Reviews span multiple pages; both old-thread replies and later review bodies count.
/// Broken pagination must preserve the cached timestamp rather than overwrite it.
#[test]
fn github_last_comment_follows_review_pages_and_preserves_cache_on_errors() {
    let fixture = Fixture::new(false);
    let reviews: Vec<_> = (1..=101).map(|id| json!({
        "id": id, "state": "COMMENTED", "body": "Review",
        "submitted_at": if id == 101 { "2026-10-02T12:00:00Z" } else { "2026-10-01T12:00:00Z" }
    })).collect();
    fixture.configure(json!({"existing": true, "reviews": reviews}));
    let records = fixture.records(&["list", "--json"]);
    assert_eq!(
        records[0]["request"]["last_comment_at"],
        "2026-10-02T12:00:00Z"
    );
    let calls: Vec<Value> = fs::read_to_string(fixture.dir.path().join("graphql.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c["variables"]["cursor"] == "reviews:100")
            .count(),
        1
    );
    fixture.configure(json!({"review_comments": [{
        "pull_request_review_id": 1, "body": "Reply on the oldest thread",
        "created_at": "2026-10-03T12:00:00Z"
    }]}));
    let records = fixture.records(&["list", "--json"]);
    assert_eq!(
        records[0]["request"]["last_comment_at"],
        "2026-10-03T12:00:00Z"
    );
    for failure in ["fail_review_page", "review_repeated_cursor"] {
        fixture.configure(json!({failure: true}));
        let failed = fixture.records(&["list", "--json"]);
        assert!(failed[0]["refresh_error"].is_string(), "{failed}");
        assert_eq!(
            failed[0]["request"]["last_comment_at"],
            "2026-10-03T12:00:00Z"
        );
        fixture.configure(json!({failure: false}));
    }
}

/// CI polling is independent of discovery, with successful results cached permanently.
#[test]
fn pipeline_refresh_retries_active_states_and_keeps_success_offline_and_online() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        fixture.configure(json!({"existing": true, "pipeline_status": "PENDING"}));
        let running = fixture.records(&["list", "--json"]);
        assert_eq!(running[0]["request"]["pipeline_status"], "running");
        assert_eq!(fixture.pipeline_queries().len(), 1);
        assert_eq!(fixture.records(&["list", "--offline", "--json"]), running);
        assert_eq!(fixture.pipeline_queries().len(), 1);

        fixture.configure(json!({"pipeline_status": if gitlab { "FAILED" } else { "FAILURE" }}));
        let failed = fixture.records(&["list", "--json"]);
        assert_eq!(failed[0]["request"]["pipeline_status"], "failed");
        assert_eq!(fixture.pipeline_queries().len(), 2);

        fixture.configure(json!({"fail_pipelines": true}));
        let output = fixture.run(&["list", "--json"]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("pipeline lookup failed"));
        let stale: Value = serde_json::from_str(&success(output)).unwrap();
        assert_eq!(stale[0]["request"]["pipeline_status"], "failed");
        assert!(stale[0]["refresh_error"].is_null());
        assert!(stale[0]["pipeline_error"].is_string());
        assert!(success(fixture.run(&["list", "--offline"])).contains("FAILED*"));

        fixture.configure(json!({"fail_pipelines": false, "pipeline_status": "SUCCESS"}));
        let passed = fixture.records(&["list", "--json"]);
        assert_eq!(passed[0]["request"]["pipeline_status"], "succeed");
        assert!(passed[0]["pipeline_error"].is_null());
        let queries = fixture.pipeline_queries().len();
        // Discovery and missing-from-discovery detail refreshes must preserve successful CI.
        for closed in [false, true] {
            fixture.configure(json!({"pipeline_status": "PENDING", "fail_pipelines": true,
                "status": if closed { "closed" } else { "open" }}));
            let cached = fixture.records(&["list", "--json"]);
            assert_eq!(cached[0]["request"]["pipeline_status"], "succeed");
            assert_eq!(
                cached[0]["request"]["status"],
                if closed { "closed" } else { "open" }
            );
            assert_eq!(fixture.pipeline_queries().len(), queries);
            assert!(cached[0]["pipeline_error"].is_null());
        }
    }
}

/// Workspace grouping, date normalization, legacy caches, and filters share one display order.
#[test]
fn list_groups_workspaces_then_orders_updates_without_rewriting_offline_cache() {
    let fixture = Fixture::new(false);
    fixture.configure(json!({"existing": true}));
    let template = fixture.records(&["list", "--json"])[0].clone();
    let make = |number, workspaces: &[&str], updated: Option<&str>, comment: Option<&str>| {
        let mut record = template.clone();
        record["request"]["number"] = json!(number);
        record["request"]["title"] = json!(format!("Request {number}"));
        // Omit the new field to exercise loading existing caches.
        record["request"]
            .as_object_mut()
            .unwrap()
            .remove("updated_at");
        if let Some(updated) = updated {
            record["request"]["updated_at"] = json!(updated);
        }
        record["request"]["last_comment_at"] = json!(comment);
        record["origins"] = json!(
            workspaces
                .iter()
                .map(|name| json!({
                    "repository": "/source/repo", "checkout": format!("/grove/{name}/repo"),
                    "workspace": {"name": name, "path": format!("/grove/{name}")}
                }))
                .collect::<Vec<_>>()
        );
        record
    };
    let records = vec![
        make(1, &["beta"], Some("2026-10-09T00:00:00Z"), None),
        make(2, &["alpha"], Some("2026-10-01T00:00:00Z"), None),
        make(3, &["alpha"], Some("2026-10-03T02:00:00+02:00"), None),
        make(4, &[], Some("2026-10-10T00:00:00Z"), None),
        make(5, &["alpha"], None, Some("2026-10-02T00:00:00Z")),
        make(6, &["alpha"], None, None),
        make(
            7,
            &["beta", "alpha", "alpha"],
            Some("2026-10-04T00:00:00Z"),
            None,
        ),
        make(8, &["alpha", "beta"], Some("2026-10-04T00:00:00Z"), None),
        make(9, &["alpha"], Some("2026-10-03T00:00:00Z"), None),
    ];
    let cache = fixture.dir.path().join("data/requests.json");
    let original = serde_json::to_vec(&json!({"version": 1, "requests": records})).unwrap();
    fs::write(&cache, &original).unwrap();
    let calls = fixture.calls().len();
    let numbers = |records: Value| {
        records
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["request"]["number"].as_u64().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        numbers(fixture.records(&["list", "--offline", "--json"])),
        [3, 9, 5, 2, 6, 7, 8, 1, 4]
    );
    assert_eq!(
        numbers(fixture.records(&["list", "--offline", "--json", "--workspace", "alpha"])),
        [7, 8, 3, 9, 5, 2, 6]
    );
    assert!(
        numbers(fixture.records(&["list", "--offline", "--json", "--repo", "unknown/repo"]))
            .is_empty()
    );
    let text = success(fixture.run(&["list", "--offline"]));
    let titles: Vec<_> = text
        .lines()
        .skip(1)
        .map(|line| line.split('\t').nth(3).unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "Request 3",
            "Request 9",
            "Request 5",
            "Request 2",
            "Request 6",
            "Request 7",
            "Request 8",
            "Request 1",
            "Request 4"
        ]
    );
    assert_eq!(fs::read(cache).unwrap(), original);
    assert_eq!(fixture.calls().len(), calls);
}

/// Both discovery and known-request batches must retain the provider's update timestamp.
#[test]
fn list_orders_provider_updates_and_keeps_cached_dates_after_errors() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let project = if gitlab {
            "group/sub/repo"
        } else {
            "owner/repo"
        };
        fixture.configure(json!({"existing": true, "updated_at": "2026-10-01T00:00:00Z", "extra_requests": [{
            "number": 8, "iid": 8, "title": "Newer request",
            "html_url": "https://github.com/owner/repo/pull/8", "web_url": "https://code.example.test/group/sub/repo/-/merge_requests/8",
            "head": {"ref": "feature/topic", "repo": {"full_name": project}}, "base": {"ref": "main"},
            "source_branch": "feature/topic", "target_branch": "main", "source_project_id": 42, "target_project_id": 42,
            "state": if gitlab { "opened" } else { "open" }, "user": {"login": "alice"}, "author": {"username": "alice"},
            "updated_at": "2026-10-02T00:00:00Z"
        }]}));
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(records[0]["request"]["number"], 8);
        assert_eq!(records[0]["request"]["updated_at"], "2026-10-02T00:00:00Z");
        assert_eq!(fixture.records(&["list", "--offline", "--json"]), records);
        fixture.configure(json!({"updated_at": "2026-10-03T02:00:00+02:00"}));
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(records[0]["request"]["number"], 7);
        assert_eq!(records[0]["request"]["updated_at"], "2026-10-03T00:00:00Z");

        fixture.configure(json!({"fail_discovery": true, "updated_at": "2026-10-04T00:00:00Z"}));
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(records[0]["request"]["updated_at"], "2026-10-04T00:00:00Z");
        // Invalid dates are fetch errors, rather than values that scramble list ordering.
        fixture.configure(json!({"updated_at": "invalid"}));
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(records[0]["request"]["updated_at"], "2026-10-04T00:00:00Z");
        assert!(records[0]["refresh_error"].is_string());
        assert_eq!(records[1]["request"]["updated_at"], "2026-10-02T00:00:00Z");
        fixture.configure(json!({"updated_at": "2026-10-05T00:00:00Z"}));
        let url = records[0]["request"]["url"].as_str().unwrap();
        success(fixture.run(&["add", url]));
        assert_eq!(
            fixture.records(&["list", "--offline", "--json"])[0]["request"]["updated_at"],
            "2026-10-05T00:00:00Z"
        );
    }
}

/// Old caches get an initial CI lookup without restarting closed/merged PR status polling.
#[test]
fn pipeline_initializes_old_closed_caches_and_defaults_absent_pipelines_to_success() {
    for (gitlab, state) in [
        (false, "closed"),
        (false, "merged"),
        (true, "closed"),
        (true, "merged"),
    ] {
        let fixture = Fixture::new(gitlab);
        fixture.configure(json!({"existing": true}));
        let mut records = fixture.records(&["list", "--json"]);
        let record = &mut records[0];
        record["request"]
            .as_object_mut()
            .unwrap()
            .remove("pipeline_status");
        record.as_object_mut().unwrap().remove("pipeline_error");
        record["request"]["status"] = json!(state);
        fs::write(
            fixture.dir.path().join("data/requests.json"),
            serde_json::to_vec(&json!({"version": 1, "requests": records})).unwrap(),
        )
        .unwrap();
        fixture
            .configure(json!({"status": state, "fail_detail": true, "pipeline_status": "PENDING"}));
        let calls = fixture.calls().len();
        let active = fixture.records(&["list", "--json"]);
        assert_eq!(active[0]["request"]["pipeline_status"], "running");
        assert_eq!(active[0]["request"]["status"], state);
        fixture.configure(json!({"pipeline_status": null}));
        let passed = fixture.records(&["list", "--json"]);
        assert_eq!(passed[0]["request"]["pipeline_status"], "succeed");
        assert!(passed[0]["refresh_error"].is_null());
        assert!(passed[0]["pipeline_error"].is_null());
        assert!(
            !fixture.calls()[calls..].iter().any(|call| call[6]
                .as_str()
                .is_some_and(|endpoint| endpoint.contains("/pulls/7")
                    || endpoint.contains("/merge_requests/7")))
        );
        let queries = fixture.pipeline_queries().len();
        assert_eq!(fixture.records(&["list", "--json"]), passed);
        assert_eq!(fixture.pipeline_queries().len(), queries);
    }
}

/// Batch and partial GraphQL failures must not hide successful neighboring CI results.
#[test]
fn pipeline_batches_isolate_errors_and_query_only_unfinished_requests() {
    for gitlab in [false, true] {
        let fixture = Fixture::new(gitlab);
        let project = if gitlab {
            "group/sub/repo"
        } else {
            "owner/repo"
        };
        let extra = |running| {
            (8..=58).map(|number| json!({
            "number": number, "iid": number, "title": "Another request",
            "html_url": format!("https://github.com/owner/repo/pull/{number}"),
            "web_url": format!("https://code.example.test/group/sub/repo/-/merge_requests/{number}"),
            "head": {"ref": "feature/topic", "repo": {"full_name": project}}, "base": {"ref": "main"},
            "source_branch": "feature/topic", "target_branch": "main",
            "source_project_id": 42, "target_project_id": 42,
            "state": if gitlab { "opened" } else { "open" },
            "user": {"login": "alice"}, "author": {"username": "alice"},
            "pipeline_status": if running { Some("PENDING") } else { None }
        })).collect::<Vec<_>>()
        };
        fixture.configure(json!({"existing": true, "extra_requests": extra(true), "pipeline_status": "PENDING", "fail_pipeline_numbers": [8]}));
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(records.as_array().unwrap().len(), 52);
        assert!(records[1]["request"]["pipeline_status"].is_null());
        assert!(records[1]["pipeline_error"].is_string());
        assert_eq!(records[2]["request"]["pipeline_status"], "running");
        assert_eq!(fixture.pipeline_queries().len(), 2);

        fixture.configure(json!({"extra_requests": extra(false), "pipeline_status": "SUCCESS", "fail_pipeline_numbers": [7]}));
        let records = fixture.records(&["list", "--json"]);
        assert_eq!(records[0]["request"]["pipeline_status"], "running");
        assert!(records[0]["pipeline_error"].is_string());
        for record in records.as_array().unwrap().iter().skip(1) {
            assert_eq!(record["request"]["pipeline_status"], "succeed");
            assert!(record["pipeline_error"].is_null());
        }
        assert_eq!(fixture.pipeline_queries().len(), 4);

        fixture.configure(json!({"fail_pipeline_numbers": [], "pipeline_graphql_errors": [{"message": "Rate limited"}]}));
        let stale = fixture.records(&["list", "--json"]);
        assert_eq!(stale[0]["request"]["pipeline_status"], "running");
        assert!(
            stale[0]["pipeline_error"]
                .as_str()
                .unwrap()
                .contains("Rate limited")
        );
        assert_eq!(fixture.pipeline_queries().len(), 5);
        let query = fixture.pipeline_queries().last().unwrap()["query"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            query
                .matches(if gitlab {
                    ": mergeRequest("
                } else {
                    ": pullRequest("
                })
                .count(),
            1
        );
        fixture.configure(json!({"pipeline_graphql_errors": []}));
        let recovered = fixture.records(&["list", "--json"]);
        assert_eq!(recovered[0]["request"]["pipeline_status"], "succeed");
        assert!(recovered[0]["pipeline_error"].is_null());
        assert_eq!(fixture.pipeline_queries().len(), 6);
    }
}
