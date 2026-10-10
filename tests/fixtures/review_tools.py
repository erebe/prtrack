#!/usr/bin/env python3
"""Grove/StGit/Herdr doubles backed by real local Git worktrees; no network access."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

root = Path(os.environ["PR_TEST_ROOT"])
config = json.loads((root / "review-config.json").read_text())
program = Path(sys.argv[0]).name
args = sys.argv[1:]
with (root / "review-calls.jsonl").open("a") as log:
    log.write(json.dumps({"program": program, "args": args, "cwd": os.getcwd()}) + "\n")

if program == "git":
    if "fetch" in args:
        if config.get("fail_fetch"):
            sys.exit("simulated fetch failure")
        args = ["-c", f"url.{root / 'remote.git'}.insteadOf={config['remote']}", *args]
    os.execv(config["git"], [config["git"], *args])

if program == "gw":
    if args == ["repos", "--json"]:
        if config.get("fail_repos"):
            sys.exit("simulated Grove lookup failure")
        print(json.dumps(config["repos"]))
    elif args[:2] == ["ws", "show"]:
        workspace = root / "custom workspaces" / args[2]
        if not workspace.is_dir():
            sys.exit("workspace does not exist")
        print(workspace)
    elif args[0] == "create":
        time.sleep(config.get("delay", 0))
        if config.get("fail_create"):
            sys.exit("simulated Grove creation failure")
        assert "--track" in args, args
        assert "--no-hooks" in args, args
        repo = args[args.index("--repos") + 1]
        branch = args[args.index("--branch") + 1]
        assert args[args.index("--source-url") + 1] == config["url"], args
        workspace = root / "custom workspaces" / args[1]
        workspace.mkdir(parents=True)
        subprocess.run(["git", "worktree", "add", "--track", "-b", branch,
                        str(workspace / repo), "origin/" + branch], check=True)
    else:
        raise AssertionError(args)
elif program == "herdr":
    if config.get("fail_herdr"):
        sys.exit("simulated Herdr failure")
    assert (root / "review-patches").exists(), "Herdr opened before stg uncommit"
    state = root / "herdr-workspaces.json"
    workspaces = json.loads(state.read_text()) if state.exists() else []
    if args == ["workspace", "list"]:
        print(json.dumps({"result": {"workspaces": workspaces}}))
    elif args[:2] == ["workspace", "create"]:
        assert args == ["workspace", "create", "--cwd",
                        str(root / "custom workspaces/review-local-repo-7"),
                        "--label", "review-local-repo-7", "--focus"], args
        workspaces.append({"workspace_id": "review-herdr", "label": "review-local-repo-7"})
        state.write_text(json.dumps(workspaces))
        print(json.dumps({"result": {"workspace": workspaces[-1]}}))
    elif args[:2] == ["workspace", "focus"]:
        assert args == ["workspace", "focus", "review-herdr"], args
        assert any(ws["workspace_id"] == args[2] for ws in workspaces)
    else:
        raise AssertionError(args)
elif program == "stg":
    def git(*argv):
        return subprocess.check_output([config["git"], *argv], text=True).strip()
    state = root / "stg-base"
    if args == ["id", "{base}"]:
        print(state.read_text() if state.exists() else git("rev-parse", "HEAD"))
    elif args[0] == "uncommit":
        if config.get("fail_stg"):
            sys.exit("simulated StGit failure")
        assert args == ["uncommit", "--to", config["merge_base"], "--exclusive"], args
        assert git("rev-parse", "HEAD") == config["head"]
        state.write_text(args[2])
        (root / "review-patches").write_text(git("rev-list", "--reverse", args[2] + "..HEAD"))
    else:
        raise AssertionError(args)
