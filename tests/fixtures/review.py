"""Review hooks through the real TUI, including the example Grove workflow."""
import json
from pathlib import Path
import shutil
import subprocess

from tui import Session


def exercise_review(root, env, provider, record):
    source = root / "clones with spaces" / "local-repo"
    source.mkdir(parents=True)
    real_git = shutil.which("git")

    def git(*args, cwd=source):
        result = subprocess.run([real_git, *args], cwd=cwd, capture_output=True, text=True)
        assert result.returncode == 0, result.stderr
        return result.stdout.strip()

    git("init", "-b", "main")
    git("config", "user.name", "Test")
    git("config", "user.email", "test@example.test")
    git("commit", "--allow-empty", "-m", "Base")
    base = git("rev-parse", "HEAD")
    # Quoting must preserve shell metacharacters in a valid Git branch name.
    branch = "review/topic;$(touch${IFS}INJECTED)"
    git("checkout", "-b", branch)
    for title in ("First change", "Second change"):
        git("commit", "--allow-empty", "-m", title)
    head = git("rev-parse", "HEAD")
    expected = git("rev-list", "--reverse", base + "..HEAD")
    git("checkout", "main")
    git("commit", "--allow-empty", "-m", "Base advanced independently")
    git("init", "--bare", str(root / "remote.git"))
    git("remote", "add", "origin", str(root / "remote.git"))
    git("push", "origin", "main", branch)
    git("branch", "-D", branch)
    remote = f"https://{provider['host']}/{provider['project']}.git"
    git("remote", "set-url", "origin", remote)
    provider.update(branch=branch)
    (root / "provider.json").write_text(json.dumps(provider))
    record["request"]["branch"] = branch
    cache = root / "data/requests.json"
    cache.write_text(json.dumps({"version": 1, "requests": [record]}))
    repos = [{"name": source.name, "path": str(source), "remote": remote}]
    config = {"git": real_git, "repos": repos, "remote": remote, "url": record["request"]["url"],
              "head": head, "merge_base": base, "delay": 0.7}

    def configure(**changes):
        config.update(changes)
        (root / "review-config.json").write_text(json.dumps(config))

    def calls(program, first=None):
        path = root / "review-calls.jsonl"
        rows = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        return [r for r in rows if r["program"] == program and (first is None or r["args"][0] == first)]

    configure()
    hook_dir = root / "hook"
    hook_dir.mkdir()
    script = hook_dir / "review-grove.sh"
    shutil.copyfile(Path(__file__).resolve().parents[2] / "config/hook/review-grove.sh", script)
    script.chmod(0o755)
    for program in ("git", "gw", "stg", "herdr"):
        path = root / "bin" / program
        shutil.copyfile(Path(__file__).with_name("review_tools.py"), path)
        path.chmod(0o755)

    checkout = root / "custom workspaces/review-local-repo-7/local-repo"
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        session.wait_for("Shift+R")
        # Lowercase r must still refresh, without invoking the review script.
        session.send(b"r")
        session.wait_for("Refreshing requests")
        session.wait_for("Statuses refreshed")
        assert calls("gw") == []
        session.send(b"R")
        session.wait_for("Preparing review")
        session.send(b"Rm")
        session.wait_for("A request action is already in progress")
        session.wait_for("Review ready:")
        assert len(calls("gw", "create")) == 1
        assert [call["args"][1] for call in calls("herdr")] == ["list", "create"]
        assert (root / "review-patches").read_text() == expected
        assert git("rev-parse", "HEAD", cwd=checkout) == head
        assert git("rev-parse", "HEAD") != head
        assert git("rev-parse", "--symbolic-full-name", "@{upstream}", cwd=checkout) == "refs/remotes/origin/" + branch
        assert not list(root.rglob("INJECTED"))

        session.send(b"R")
        session.wait_for("Preparing review")
        session.wait_for("Review ready:")
        assert len(calls("gw", "create")) == 1
        assert [call["args"][1] for call in calls("herdr")] == ["list", "create", "list", "focus"]
        assert (root / "review-patches").read_text() == expected

        (checkout / "local-change").write_text("do not discard")
        before = len(calls("stg", "uncommit"))
        session.send(b"R")
        session.wait_for("Review checkout has local changes")
        assert len(calls("stg", "uncommit")) == before
        (checkout / "local-change").unlink()

        for key, message in [("fail_fetch", "simulated fetch failure"),
                             ("fail_stg", "simulated StGit failure"),
                             ("fail_herdr", "simulated Herdr failure")]:
            before_herdr = len(calls("herdr"))
            configure(**{key: True})
            session.send(b"R")
            session.wait_for(message)
            if key != "fail_herdr":
                assert len(calls("herdr")) == before_herdr
            configure(**{key: False})

        configure(repos=[repos[0], dict(repos[0], remote="https://github.com/other/repo.git")])
        before = len(calls("git", "fetch"))
        session.send(b"R")
        session.wait_for("Multiple Grove repositories are named")
        assert len(calls("git", "fetch")) == before
        configure(repos=[])
        session.send(b"R")
        session.wait_for("No Grove repository matches")
        configure(repos=repos)
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()

    before = len(calls("gw"))
    session = Session(root, env, offline=True)
    try:
        session.wait_for("Offline: showing cached statuses")
        session.send(b"R")
        session.wait_for("Cannot prepare review offline")
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
        assert len(calls("gw")) == before
    finally:
        session.close()

    def prepare(source_branch):
        context = dict(env, PR_BRANCH=source_branch, PR_BASE="main", PR_URL=config["url"],
                       PR_HOST=record["remote"]["host"], PR_PROJECT=record["remote"]["project"],
                       PR_NUMBER=str(record["request"]["number"]))
        return subprocess.run(
            [str(script)], cwd=root, env=context, text=True, capture_output=True,
        )

    # Repository lookup lives entirely in the example hook and supports SSH
    # spellings, default HTTP ports and upper-case hostnames just like HTTPS.
    for alternate in [f"git@{provider['host'].upper()}:{provider['project']}.git",
                      f"ssh://git@{provider['host']}:2222/{provider['project']}.git",
                      f"https://{provider['host']}:443/{provider['project']}.git/"]:
        configure(repos=[dict(repos[0], remote=alternate)])
        prepared = prepare(branch)
        assert prepared.returncode == 0, prepared.stderr
    configure(repos=repos)

    before_fetch = len(calls("git", "fetch"))
    configure(repos=[dict(repos[0], remote=f"https://{provider['host']}:8443/{provider['project']}.git")])
    unmatched = prepare(branch)
    assert unmatched.returncode != 0 and "No Grove repository matches" in unmatched.stderr
    configure(repos=repos)
    git("remote", "set-url", "origin", "https://elsewhere.test/other/repo.git")
    mismatched = prepare(branch)
    assert mismatched.returncode != 0 and "origin no longer matches" in mismatched.stderr
    assert len(calls("git", "fetch")) == before_fetch
    git("remote", "set-url", "origin", remote)

    before = len(calls("gw", "create"))
    uncommits = len(calls("stg", "uncommit"))
    missing = prepare("missing-remote-branch")
    assert missing.returncode != 0 and "remote ref" in missing.stderr, missing.stderr
    assert len(calls("gw", "create")) == before
    assert len(calls("stg", "uncommit")) == uncommits

    # Reject merge commits before creating a workspace or changing any stack.
    git("checkout", "-b", "merge-side", base)
    git("commit", "--allow-empty", "-m", "Side change")
    git("checkout", "-b", "merged-review", "main")
    git("merge", "--no-ff", "merge-side", "-m", "Merge side")
    git("push", str(root / "remote.git"), "merged-review")
    git("checkout", "main")
    merged = prepare("merged-review")
    assert merged.returncode != 0 and "requires linear history" in merged.stderr, merged.stderr
    assert len(calls("gw", "create")) == before
    assert len(calls("stg", "uncommit")) == uncommits

    # Local commits in a previously prepared workspace must never be reset.
    git("commit", "--allow-empty", "-m", "Local review work", cwd=checkout)
    local_head = git("rev-parse", "HEAD", cwd=checkout)
    diverged = prepare(branch)
    assert diverged.returncode != 0 and "leaving it unchanged" in diverged.stderr, diverged.stderr
    assert git("rev-parse", "HEAD", cwd=checkout) == local_head
    assert len(calls("stg", "uncommit")) == uncommits

    # A fresh PR/workspace verifies that quitting waits for the worker to finish.
    git("worktree", "remove", str(checkout))
    git("branch", "-D", branch)
    shutil.rmtree(checkout.parent)
    configure(delay=0.7, fail_create=True)
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        session.send(b"R")
        session.wait_for("simulated Grove creation failure")
        assert not checkout.exists()
        configure(fail_create=False)
        session.send(b"Rq")
        session.wait_for("Waiting for request action")
        assert session.process.wait(timeout=5) == 0
        assert checkout.is_dir()
        assert len(calls("gw", "create")) == 3
        assert len(json.loads((root / "herdr-workspaces.json").read_text())) == 1
    finally:
        session.close()


def exercise_hooks(root, env, provider, record):
    """User hooks need no Grove, are rediscovered, and receive literal PR metadata."""
    config_path = Path("config with spaces") / "custom.toml"
    (root / config_path).parent.mkdir()
    (root / config_path).write_text("data_dir = '../data'\ngrove_dir = '../grove'\n")
    hooks = (root / config_path).parent / "hook"
    branch = "feature/$(touch${IFS}INJECTED)"
    provider.update(branch=branch)
    (root / "provider.json").write_text(json.dumps(provider))
    record["request"]["branch"] = branch
    (root / "data/requests.json").write_text(json.dumps({"version": 1, "requests": [record]}))
    grove = root / "bin/gw"
    grove.write_text("#!/bin/sh\ntouch \"$PR_TEST_ROOT/unexpected-grove\"\nexit 77\n")
    grove.chmod(0o755)
    log = root / "hook-runs.jsonl"

    def install(name, tag, error=None):
        path = hooks / name
        path.write_text('''#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
with Path(os.environ["PR_TEST_ROOT"], "hook-runs.jsonl").open("a") as log:
    log.write(json.dumps({"tag": TAG, "cwd": os.getcwd(), "args": sys.argv[1:],
        "context": {k: v for k, v in os.environ.items() if k.startswith("PR_")},
        "stdin": sys.stdin.read(), "cd_file": os.environ.get("GROVE_CD_FILE")}) + "\\n")
time.sleep(0.3)
if ERROR is not None:
    sys.exit(ERROR)
'''.replace("TAG", repr(tag)).replace("ERROR", repr(error)))
        path.chmod(0o755)
        return path

    def rows():
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    session = Session(root, dict(env, GROVE_CD_FILE=str(root / "cd-file")), config_path=config_path)
    try:
        session.wait_for("Statuses refreshed")
        session.send(b"R")
        session.wait_for("No executable review-* hooks")
        hooks.mkdir()
        (hooks / "review-directory").mkdir()
        install("not-a-review", "ignored")
        install("review-", "ignored")
        install("review-disabled", "ignored").chmod(0o644)
        # Deliberately create in reverse order; scripts can use any interpreter.
        second = install("review-20.py", "second")
        first = install("review-10.py", "first")
        session.send(b"R")
        session.wait_for("Preparing review")
        session.wait_for("Review hooks completed (2)")
        assert [row["tag"] for row in rows()] == ["first", "second"]
        for row in rows():
            expected = dict(PR_PROVIDER=record["remote"]["provider"], PR_HOST=record["remote"]["host"],
                            PR_PROJECT=record["remote"]["project"], PR_NUMBER="7", PR_BRANCH=branch,
                            PR_BASE="main", PR_URL=record["request"]["url"],
                            PR_TITLE=record["request"]["title"], PR_CONFIG_DIR=str(hooks.parent))
            assert {key: row["context"].get(key) for key in expected} == expected, row
            assert row["cwd"] == str(root) and row["args"] == [] and row["stdin"] == "", row
            assert row["cd_file"] is None
        assert not list(root.rglob("INJECTED"))

        # Edits, additions and executable bits take effect without restarting pr.
        install(first.name, "edited")
        second.chmod(0o644)
        last = install("review-30.py", "new")
        session.send(b"R")
        session.wait_for("Preparing review")
        session.wait_for("Review hooks completed (2)")
        assert [row["tag"] for row in rows()][-2:] == ["edited", "new"]

        install(first.name, "failed", error="deliberate failure")
        session.send(b"R")
        session.wait_for("Preparing review")
        session.wait_for("review-10.py (exit status: 1): deliberate failure")
        assert [row["tag"] for row in rows()][-2:] == ["failed", "new"]

        # Failure to spawn one hook must also leave later hooks runnable.
        first.write_text("#!/missing-review-interpreter\n")
        before = len(rows())
        session.send(b"R")
        session.wait_for("Preparing review")
        session.wait_for("review-10.py: No such file")
        assert [row["tag"] for row in rows()][before:] == ["new"]
        assert not (root / "unexpected-grove").exists()
        first.unlink()
        last.unlink()
        session.send(b"R")
        session.wait_for("No executable review-* hooks")
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()
