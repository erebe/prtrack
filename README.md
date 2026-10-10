# pr

A CLI for tracking GitHub pull requests and GitLab merge requests across repositories and Grove workspaces. Built with Clap and Ratatui.

## Install and authenticate

Install a current stable Rust toolchain, Git, and the CLI for each provider you use:

```sh
cargo install --path . --locked
gh auth login       # GitHub
glab auth login     # GitLab
```

`pr` delegates authenticated API requests to [GitHub CLI](https://cli.github.com/manual/gh_api) and [GitLab CLI](https://docs.gitlab.com/cli/api/). Local Git inspection uses `gix`. `git push` is the sole Git subprocess because gix currently has no push implementation; it retains your usual SSH keys, credential helpers, hooks, and upstream configuration. Git provider credentials remain managed by `gh` and `glab`.

To use an HTTP proxy for provider API calls and HTTP(S) Git pushes:

```sh
HTTP_PROXY=http://127.0.0.1:3128 pr list
HTTP_PROXY=http://127.0.0.1:3128 pr push
```

`pr` forwards proxy settings to its subprocesses. It uses `HTTP_PROXY` (or `http_proxy`) as the HTTPS fallback when neither `HTTPS_PROXY` nor `https_proxy` is set. Explicit HTTPS settings, including empty values, take precedence over this fallback. It also supplies `http_proxy` from `HTTP_PROXY` when the lowercase variable is absent, for Git's HTTP transport. `NO_PROXY` / `no_proxy` bypass lists are preserved. [Git proxy configuration](https://git-scm.com/docs/git-config#Documentation/git-config.txt-httpproxy) can override environment settings; SSH remotes use SSH's own proxy configuration.

## Configuration

Create the config directory and an example file with:

```sh
pr init config
# Or choose another location:
pr --config /path/to/config.toml init config
```

The command prints the file's full path and preserves any existing file. New files are private on Unix (mode `0600`). `pr` reads TOML from the platform config directory: `~/.config/pr/config.toml` on Linux, respecting `XDG_CONFIG_HOME`. The generated file follows [config.example.toml](config.example.toml):

```toml
grove_dir = "~/.grove"
data_dir = "~/.local/share/pr"
# provider = "gitlab" # Optional override for all repositories: github or gitlab.

[slack]
token = "your-slack-oauth-token"
channel_id = "C0123456789"
review_message = "<@U07F73SKT0V> review {pr_url} in <#C02NQ0LC8M9>"
deploy_message = "<@U07F73SKT0V> release {pr_url}"
```

`grove_dir` and `data_dir` are required, nonempty paths. Other settings are optional; Slack notifications require a token, channel, and the selected action's message template. A missing config file reports an error with instructions to run `pr init config`. Protect a manually created file containing a Slack token with `chmod 600 ~/.config/pr/config.toml`.

Select another file with `pr --config /path/to/config.toml list`. An explicit file replaces the default file; missing explicit files, unreadable files, invalid TOML, unknown keys, and incorrect value types produce an error. Parse errors report their location without printing file contents or token values. `~/` paths expand to your home directory; other relative paths in the file are relative to the file's directory.

Directory paths are configured only through `data_dir` and `grove_dir` in the file; there are no implicit directory defaults or cache fallbacks. The `--provider` flag overrides the file's provider setting, which takes precedence over repository provider selection. Configuration is read at startup; restart `pr list` after editing it. Push options and list filters remain command-line options.

The former `PR_DATA_DIR`, `PR_GROVE_DIR`, `PTRACK_DATA_DIR`, `PTRACK_GROVE_DIR`, `PR_SLACK_TOKEN`, and `PR_SLACK_CHANNEL_ID` environment settings are no longer read; move their values into the file. Standard proxy, browser, XDG, and Git/provider authentication environment variables continue to work normally.

## Push a branch and open a request

From a repository or a Grove worktree:

```sh
pr push
pr push --title "Add request tracking" --body "Track requests by workspace" --draft
pr push --remote origin --base develop
pr push --target toto
```

By default, the command publishes the current branch using its exact local name. For example, local branch `eks-log-ingestion` is pushed to remote branch `eks-log-ingestion`. `--target toto` overrides the destination branch name. The local branch keeps its name and tracks the destination branch. Combine `--remote origin --target toto --base develop` to publish to `origin/toto` and open a request into `develop`.

It then creates a PR/MR and prints its URL, or reuses an open request for the chosen remote source branch; `--base` selects a specific PR/MR base when needed. New requests default to the repository's default branch, with the latest commit's subject and body as their title and description. Reusing a request does not edit its title, description, or draft status. Repeat `--target` on subsequent pushes to use the same override.

The remote is selected from `branch.<name>.pushRemote`, `remote.pushDefault`, the branch's upstream remote, `origin`, or the sole configured remote, in that order. Source and target belong to that selected remote repository; cross-fork requests are not supported yet. Pushes never force-update branches. Detached HEADs, branches without commits, and identical source/target branches are rejected. A creation failure after a successful push can be retried with `pr push` and the same options.

For a self-hosted instance whose hostname does not start with `github.` or `gitlab.`, configure the provider in the repository:

```sh
git config pr.provider gitlab
glab auth login --hostname code.example.com
pr push
# Or override for one invocation:
pr --provider gitlab push
```

HTTPS and SSH remotes, GitLab subgroups, and explicit push URLs are supported. SSH aliases must resolve to an API hostname understood by `gh`/`glab`.

Renamed or transferred repositories are resolved through provider metadata before PR/MR API calls. You can keep an older remote URL as long as the provider redirects it to the current repository.

## Track an existing request

```sh
pr add https://gitlab.com/qovery/backend/q-core/-/merge_requests/4096
pr add https://github.com/owner/repo/pull/123
```

`add` fetches the request and saves it to `requests.json` using your `gh` or `glab` authentication. It works outside a Git checkout, for requests by any author and in any status. The URL path identifies GitHub or GitLab, including self-hosted instances and nested GitLab namespaces. Query strings, fragments, and a trailing slash are accepted.

Adding the same URL again updates its details and preserves existing repository/workspace origins. New records initially have no local checkout association; normal online listing can discover matching workspaces later. The command prints the confirmed request URL and does not push branches or modify the request on the provider.

## List requests

```sh
pr list
pr list --workspace my-feature
pr list --repo owner/repository
pr list --offline
pr list --json
```

The Ratatui table shows status (`OPEN`, `DRAFT`, `MERGED`, `CLOSED`), pipeline, repository with request number (e.g. `gh/Qovery/documentation#416` or `gl/team/repo!7`), title, unresolved threads, last comment age, workspace, and branch, in that order. The TUI shortens `github.com` to `gh` and `gitlab.com` to `gl`; self-hosted domains retain their full hostname. The plain-text table uses the same column order with full hostnames and adds the URL. The header counts each status among the visible requests. Statuses, repository names, workspaces, branches, and keyboard hints use a consistent palette. Select a row to see its URL, original repository path, workspace path, and checkout path.

Requests are grouped alphabetically by workspace, then sorted by the provider’s last update time, newest first. Requests without a workspace appear last. TUI, plain text, and JSON share this order, including offline mode. A request linked to multiple workspaces is listed once under its sorted workspace names; `--workspace` groups all matches together. Older caches fall back to the last comment time, with unknown dates last; JSON exposes the nullable update timestamp as `request.updated_at`.

The **Pipeline** column shows `RUNNING` (yellow), `FAILED` (red), or `SUCCEED` (green). It uses GitHub's [combined checks and commit status](https://docs.github.com/en/graphql/reference/commits#statuscheckrollup) for the latest PR commit, or GitLab's [head pipeline](https://docs.gitlab.com/api/merge_requests/#get-single-mr) aggregate. Pending/manual work counts as running; failed/canceled work counts as failed; skipped or absent pipelines count as successful. Historical runs are excluded.

Pipeline lookups run in batches of up to 50 within the shared 10-call limit. New and older cached entries get one initial lookup; subsequent refreshes poll only `RUNNING` and `FAILED` entries. `SUCCEED` stays cached, including after new commits or pipeline reruns. This also applies to closed/merged requests, without refetching their PR details. API failures retain the previous result and show `*`; `—` means no successful lookup yet and is retried online. Offline mode uses the cache. JSON includes `request.pipeline_status` (`running`, `failed`, `succeed`, or `null`) and a separate `pipeline_error`.

The **Unresolved** column counts review threads still awaiting resolution, once per thread regardless of replies. GitHub uses each [review thread’s resolution state](https://docs.github.com/en/graphql/reference/pulls#pullrequestreviewthread), including outdated threads; GitLab subtracts resolved discussions from resolvable discussions. Ordinary comments without a resolution state do not count. Zero is green, pending threads are yellow, and `—` means the count has not been fetched yet. Refresh failures retain the cached count with `*`; offline mode uses the cache. JSON exposes `request.unresolved_threads` as a number or `null`.

Counts are fetched with the existing discovery/metadata queries; GitHub follows additional pages when a PR has more than 100 review threads. Existing closed/merged requests keep their cached count; use `pr add <url>` to fetch a missing or newer count for one of these entries.

The **Last comment** column shows elapsed time (`just now`, `5m ago`, `3h ago`, `2d ago`, then weeks, months, or years). The selected request shows the exact date and time in UTC. It includes conversation comments, written review summaries, and inline review replies, including replies on older threads. Empty reviews, pending GitHub reviews, and GitLab system notes do not count. JSON exposes the nullable RFC 3339 timestamp as `request.last_comment_at`; `—` means no comment or no cached timestamp. Offline mode computes the age from the cached timestamp. Existing closed/merged entries keep their cached data; use `pr add <url>` to fetch a missing timestamp for one of these entries.

- `↑`/`↓` or `j`/`k`: select a request; `g`/`G`: first/last.
- `n`: send `slack.review_message` to request a review of the selected request. Requires online mode and [Slack configuration](#slack-review-and-deploy-notifications).
- `d`: send `slack.deploy_message` to request a deployment of the selected request, using the same Slack channel. Requires online mode; the request stays tracked.
- `o`: open the selected PR/MR link in your default browser.
- `Ctrl+O`: open all `OPEN` PRs/MRs in the current filtered list in your browser, once per URL. Draft, merged, and closed requests are skipped. Offline mode uses cached statuses.
- `m`: merge the selected PR/MR on GitHub/GitLab and retain it as `MERGED`. Requires online mode.
- `u`: stop tracking the selected request and remove its entry from `requests.json`.
- `Shift+U`: close the selected PR/MR on GitHub/GitLab, then remove it from `requests.json`. Requires online mode; a failed close leaves the entry tracked.
- `r`: refresh from the providers in the background.
- `Shift+R`: run all configured review hooks for the selected PR/MR.
- `q`, `Esc`, or `Ctrl-C`: quit and restore the terminal.

The list refreshes automatically when the terminal window, tab, or pane regains focus, provided the terminal reports focus changes. Focus events reuse the background refresh, keep the selected request, and do not start another refresh while one is running. `--offline` disables automatic and manual refreshes. Focus reporting is disabled again when the TUI exits.

`Shift+R` discovers executable files named `review-*` in `hook/` beside the active config file: normally `~/.config/pr/hook/` on Linux, respecting `XDG_CONFIG_HOME`. With `--config /path/to/config.toml`, it uses `/path/to/hook/`. Hooks run sequentially in filename order, in the background. Each press discovers the files again, so adding or editing a hook needs no rebuild or restart. Other names, directories, and files without execute permission are ignored. An empty or missing hook directory produces a message with its path.

Hooks run directly using their shebang, with no arguments, closed stdin, and the working directory from which `pr list` was launched. They inherit the environment, including `PATH` and authentication/proxy settings. The selected request is provided through these environment variables; quote them when using them in shell commands:

| Variable | Value |
| --- | --- |
| `PR_PROVIDER` | `github` or `gitlab` |
| `PR_HOST`, `PR_PROJECT` | Repository host and namespace/repository path |
| `PR_NUMBER`, `PR_URL`, `PR_TITLE` | Selected PR/MR number, URL, and title |
| `PR_BRANCH`, `PR_BASE` | Source and target branch names |
| `PR_CONFIG_DIR` | Absolute directory containing the active config file |

All hooks run even if an earlier one fails. Failures report the hook name, exit status or launch error, and the last nonempty stderr line. Success shows the hook count and the last nonempty stdout line. Keep dependent steps inside one hook so it can stop on failure. The TUI remains responsive, prevents overlapping request actions, and waits for all review hooks before quitting. Offline mode disables this action.

For the Grove/StGit/Herdr workflow, install the example hook from this repository (adjust the destination for a custom config):

```sh
install -Dm755 config/hook/review-grove.sh "${XDG_CONFIG_HOME:-$HOME/.config}/pr/hook/review-grove.sh"
```

The [`config/hook/review-grove.sh`](config/hook/review-grove.sh) hook requires Bash, Git, Grove (`gw` with `create --track` and `ws show --output path`), StGit (`stg`), Herdr (`herdr`), and `jq` on `PATH`, with a running Herdr session. The repository must be registered in Grove with an `origin` matching the selected request. Grove uses its own configuration; the script obtains the actual workspace location from `gw` and disables hooks during creation. Custom hooks do not require these tools. To extend the review action, add another executable script, such as `hook/review-notify.sh`, alongside it.

The script fetches the PR source and base branches, then runs `gw create review-<repo>-<number> --no-hooks --repos <repo> --branch <source> --track --source-url <url>`. In that worktree it sets the upstream to `origin/<source>` and runs `stg uncommit --to <merge-base> --exclusive`, converting every commit above the common ancestor with the PR base into an applied patch. This preserves the commits and working tree. A base branch that advanced after the PR branched off is supported; merge commits in the PR range are rejected because StGit cannot uncommit them. After successful preparation, it opens and focuses a Herdr workspace at the Grove workspace directory, labeled `review-<repo>-<number>`. If that Herdr label already exists, it focuses that workspace. A Herdr failure is reported and leaves the prepared checkout available for retry.

Repeating `Shift+R` with this hook reuses a matching review workspace without duplicating patches. Diverged local branches, dirty/mismatched worktrees, ambiguous Grove repositories, and stacks extending below the PR base produce an error; existing branches are never reset or deleted. If the branch is already checked out elsewhere, Grove reports that conflict.

Online listing discovers open requests you authored or are assigned to in the current repository, Grove workspaces, and previously tracked repositories, including requests created outside `pr`. Both filters run on the provider using your authenticated username: GitHub uses separate GraphQL searches with `author` and `assignee`, returning up to 100 complete PR records per page; GitLab uses `author_username` and `assignee_username[]`. The results are combined and deduplicated, so requests you both authored and are assigned to appear once. GitHub falls back to the paginated repository issues endpoint with the matching `creator` or `assignee` filter and individual PR lookups when a search exceeds its [1,000-result search limit](https://docs.github.com/en/graphql/reference/search#search). Only requests whose source and target are in the same repository are discovered. Previously tracked requests remain visible after merging, closing, reassignment, or deleting a workspace. Historical closed requests are not imported automatically.

Removing a request from tracking leaves it unchanged on GitHub/GitLab. Merged and closed requests stay removed; open requests can be discovered again on a later online refresh.

Open requests display `APPROVED` in bright green when the provider reports approval. GitHub uses [`reviewDecision`](https://docs.github.com/en/graphql/reference/pulls#pullrequestreviewdecision); GitLab uses [`approved`](https://docs.gitlab.com/api/graphql/reference/#mergerequest-approved), reflecting its configured approval requirements. Approval does not guarantee that CI or other merge checks have passed. Draft, closed, and merged states take precedence. JSON retains the lifecycle `status` and includes a separate nullable `approved` field; older cache files remain supported. Approval is fetched with GitHub search results or in batches of up to 50 requests, including GitLab, and cached for offline use.

Merging with `m` runs in the background from the list. It checks the current remote state and head commit, respects provider merge restrictions, and reports failures without removing tracking. GitHub uses a merge commit when enabled, otherwise squash or rebase according to the repository's allowed methods; GitLab uses its configured merge behavior. A successful merge updates `requests.json` and stays visible as `MERGED`. If saving fails afterward, press `m` again to retry the cache update. Quitting waits for an active merge to finish.

After a GitLab merge attempt fails, `pr` checks whether the MR already merged, then performs one recovery pass: [rebase the source branch with `skip_ci=true`](https://docs.gitlab.com/api/merge_requests/#rebase-a-merge-request), wait for completion, [resolve all open resolvable discussion threads](https://docs.gitlab.com/api/discussions/#resolve-a-merge-request-thread), and retry merging with the rebased commit SHA. General comments and thread contents are preserved. Rebase polling has a 60-second deadline. A failed rebase, thread resolution, or merge retry leaves the request tracked and reports the error. GitLab permissions and merge requirements still apply; skipping CI prevents the rebase from creating a pipeline.

Closing with `Shift+U` runs in the background. Already closed or merged requests are confirmed remotely and removed from tracking. If the local file cannot be updated after closing, the error explains that closure succeeded; press `Shift+U` again to retry removal.

Refresh runs up to 10 provider calls at once across repositories, author/assignee searches, and status/approval batches. Comment metadata is included in the existing discovery/metadata queries; GitHub follows additional review pages only for requests with more than 100 submitted reviews. Tracked requests missing from discovery are refreshed in batches of up to 50: GitHub returns details and approvals together via GraphQL; GitLab filters by request IDs and batches approvals separately. Independent batches run concurrently, including approval batches for large discovered lists. The limit is shared across the entire refresh. Each provider/host gets one authenticated-user lookup, pagination stays sequential within each search, and results are saved once after all workers finish. A failed or missing request keeps its cached data and does not discard successful updates for other requests.

Successfully fetched closed and merged requests remain cached without further detail lookups, unless a refresh error needs retrying. Reopened requests can be rediscovered by the normal open-request listing. Refresh failures preserve cached data, display a warning, and mark affected statuses with `*`. `--offline` makes no provider calls. `--json` emits records with origin paths and refresh errors; without a terminal, the default output is a tab-separated table.

## Slack review and deploy notifications

Configure the `[slack]` section of your TOML file for the Qovery workspace:

- `token`: a user OAuth token or a separate notifier bot's token with [`chat:write`](https://docs.slack.dev/reference/methods/chat.postMessage/) and access to the channel. Homer ignores messages sent as itself, so do not use Homer's own bot token.
- `channel_id`: the command channel's ID (for example, `C0123456789`). Invite both Homer and the sending bot, if used, to the channel, including private channels.
- `review_message`: the review template used by `n`. The example uses `<@U07F73SKT0V> review {pr_url} in <#C02NQ0LC8M9>`; customize the user and destination channel IDs in the file.
- `deploy_message`: the deployment template used by `d`, for example `<@U07F73SKT0V> release {pr_url}`. It uses the same `token` and `channel_id` as reviews. Homer sends release updates to its own configured release channel, so do not append `in #channel` to a Homer release command.

Each template must contain `{pr_url}`. Every occurrence is replaced with the selected request's URL; other text and Slack formatting are preserved. Templates are validated independently: configuring only review notifications or only deployment notifications is supported.

Homer must also belong to the review destination channel named in the message.

Missing or empty Slack settings are reported only when pressing `n` or `d`, so other commands work without Slack credentials. Tokens are never stored in `requests.json` or included in debug output.

Select a row and press `n` or `d` to send the selected message with Slack mention parsing enabled. Slack displays the user and channel names; plain `@Homer` text is not a [real mention](https://docs.slack.dev/messaging/formatting-message-text/#mentioning-users). Homer must have the corresponding review or release mention handler deployed, the `app_mentions:read` bot scope granted, and the `app_mention` event subscribed at `https://<homer-host>/api/v1/homer/event`. Reinstall Homer after adding the scope. This invokes the mention handler; posting a message does not execute a slash command. Homer's release handler requires a GitLab MR merged into the project's default branch and applies its normal release readiness checks.

Delivery runs in the background with a 30-second timeout, honors proxy environment variables, and reports Slack errors in the TUI. Delivery confirmation means Slack accepted the message; Homer processes it separately and may reject the release. Both actions share one pending delivery slot; repeated presses do not enqueue another message, and quitting waits for delivery to finish. No automatic delivery retries are made.

## Workspace and cache locations

Grove associations come from `<grove_dir>/state.json`, including custom workspace paths. Unregistered checkouts under `<grove_dir>/workspaces/<workspace>/<repo>` are also discovered. The example config uses `~/.grove`. Branches are read from Git, so a stale branch name in Grove state does not mislabel a request. Request source branches are matched to exact local branch names; `pr push` also records the workspace when using `--target`. Outside Grove, requests still retain their source repository and checkout paths.

Tracking data is stored in `<data_dir>/requests.json`; the example config uses `~/.local/share/pr`. Writes use a lock and atomic replacement. Grove's own state is read only.

Set locations with `grove_dir` and `data_dir` in the config file. Repository filters accept `owner/repository` or `host/owner/repository`; workspace filters match the exact workspace name.

The legacy Git configuration key `ptrack.provider` remains a fallback for `pr.provider`. To reuse an older cache, set `data_dir` to its directory explicitly, for example `~/.local/share/ptrack`.

## Development

```sh
cargo build --locked
cargo test --locked
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Integration tests require Git and Python 3 on Unix. They use temporary local repositories and fake `gh`/`glab` executables; they do not publish branches or requests to a real host.

Modules own both their behavior and their data types. Small shared structs and enums are grouped in each module's `types.rs`; callers import public types through that module's re-exports. Concrete implementations stay alongside their behavior.

| Module | Responsibility |
| --- | --- |
| `src/cli/` | Clap parsing, with command selection and `Init` / `Push` / `Add` / `List` arguments in `types.rs`. |
| `src/config/` | Config initialization, TOML loading, required file-relative paths, and Slack settings with redacted credential diagnostics. |
| `src/app/` | Push, add, list, refresh, close, and merge workflows; runtime `Settings`, refresh `Snapshot`, and internal `Checkout` data. |
| `src/provider/` | `GitProvider` interface and normalized `Request`, `Status`, `Remote`, and `Provider` types. GitHub/GitLab implementations and API transport stay private. |
| `src/git/` | `GitRepository` interface for local Git operations, with a private gix implementation and push fallback. |
| `src/grove/` | `Grove` interface, `Workspace`, and repository `Origin`; filesystem discovery and Grove state representations stay private. |
| `src/notify/` | `Notify` interface for review notifications; Slack validation and HTTP transport remain private. Create it with `<dyn Notify>::from_config(&settings.slack)`. |
| `src/store/` | Thread-safe `Store` interface and `Tracked` records, including merging and sorting. The JSON envelope and persistence implementation stay private. |
| `src/output.rs` | Plain-text table formatting, workspace labels, and terminal text sanitization. |
| `src/tui/` | Terminal lifecycle, focus reporting, event loop, and request list. Feedback and notification types live in `types.rs`; the shared palette stays in `theme.rs`. |

Create adapters with `provider::from_remote(&remote)`, `<dyn GitRepository>::discover(path)`, `<dyn Grove>::new(path)`, and `<dyn Store>::open(path)`. Application workflows compose these interfaces. Providers return normalized data without depending on storage; tracking records add local origins and refresh metadata. Storage retains file locking, atomic writes, and protection against refreshes restoring removed entries.
