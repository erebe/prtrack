"""Exercise the real event loop in a PTY, with local provider fixtures only."""
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time


class Screen:
    """Small terminal buffer for the cursor/erase sequences emitted by Ratatui."""
    def __init__(self):
        self.rows = [[" "] * 160 for _ in range(35)]
        self.row = self.col = 0
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")()

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        i = 0
        while i < len(self.pending):
            if self.pending[i] == "\x1b":
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", self.pending[i:])
                if not match:
                    break
                params, _, command = match.groups()
                if not params.startswith("?"):
                    nums = [int(n or "0") for n in params.split(";")]
                    n = nums[0]
                    if command in ("H", "f"):
                        self.row = max(0, n - 1)
                        self.col = max(0, (nums[1] if len(nums) > 1 else 1) - 1)
                    elif command == "G":
                        self.col = max(0, n - 1)
                    elif command == "A":
                        self.row = max(0, self.row - (n or 1))
                    elif command == "B":
                        self.row += n or 1
                    elif command == "C":
                        self.col += n or 1
                    elif command == "D":
                        self.col = max(0, self.col - (n or 1))
                    elif command == "J" and n in (2, 3):
                        self.rows = [[" "] * 160 for _ in range(35)]
                    elif command == "K" and self.row < 35:
                        start = 0 if n in (1, 2) else self.col
                        end = self.col + 1 if n == 1 else 160
                        self.rows[self.row][start:end] = [" "] * max(0, end - start)
                i += len(match[0])
                continue
            char = self.pending[i]
            if char == "\r":
                self.col = 0
            elif char == "\n":
                self.row += 1
            elif char >= " " and self.row < 35 and self.col < 160:
                self.rows[self.row][self.col] = char
                self.col += 1
            i += 1
        self.pending = self.pending[i:]

    def text(self):
        return "\n".join("".join(row) for row in self.rows)


class Session:
    def __init__(self, root, env, offline=False, args=(), config_path=None):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 160, 0, 0))
        self.screen = Screen()
        self.output = bytearray()
        self.process = subprocess.Popen(
            [sys.argv[1], "--config", str(config_path or root / "config.toml"),
             "list", *(["--offline"] if offline else []), *args],
            cwd=root, env=env, stdin=slave, stdout=slave, stderr=slave,
        )
        os.close(slave)

    def send(self, keys):
        os.write(self.master, keys)

    def wait_for(self, text, row=None):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            visible = self.screen.text() if row is None else "".join(self.screen.rows[row])
            if text in visible:
                return
            if not self.read():
                break
        raise AssertionError(f"Missing {text!r} on screen:\n{self.screen.text()}")

    def read(self):
        if select.select([self.master], [], [], 0.05)[0]:
            try:
                data = os.read(self.master, 65536)
            except OSError:
                return False
            if not data:
                return False
            self.output.extend(data)
            self.screen.feed(data)
        return True

    def wait_for_output(self, sequence):
        deadline = time.monotonic() + 8
        while sequence not in self.output:
            assert time.monotonic() < deadline, f"Missing terminal sequence {sequence!r}"
            assert self.read(), f"Terminal closed before {sequence!r}"

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=3)
        os.close(self.master)


def exercise(root, provider):
    gitlab = provider == "gitlab"
    host = "code.example.test" if gitlab else "github.com"
    project = "old/sub/repo" if gitlab else "old/repo"
    canonical = "team/sub/repo" if gitlab else "team/repo"
    url = f"https://{host}/{canonical}/" + ("-/merge_requests/7" if gitlab else "pull/7")
    config = {"host": host, "project": project, "canonical_project": canonical,
              "existing": True, "branch": "alice/feature", "title": "Request navigation"}

    note = {"body": "Latest note", "created_at": "2026-10-01T12:00:05Z"}
    config.update(issue_comments=[note], discussions=[{"notes": [note]}])
    config_path = root / "provider.json"
    config_path.write_text(json.dumps(config))
    (root / "data").mkdir()
    record = {"remote": {"provider": provider, "host": host, "project": project},
              "request": {"number": 7, "title": "Request navigation", "url": url,
                          "branch": "alice/feature", "base": "main", "status": "open"},
              "origins": [], "checked_at": 0, "refresh_error": None}
    (root / "data/requests.json").write_text(json.dumps({"version": 1, "requests": [record]}))
    (root / "bin").mkdir()
    for program in ("gh", "glab"):
        path = root / "bin" / program
        shutil.copyfile(Path(__file__).with_name("host.py"), path)
        path.chmod(0o755)
    browser = root / "bin/browser"
    browser.write_text("#!/usr/bin/env python3\nimport os,sys\nfrom pathlib import Path\nPath(os.environ['PR_TEST_ROOT'], 'browser-url').write_text(sys.argv[1])\n")
    browser.chmod(0o755)
    (root / "config.toml").write_text("data_dir = 'data'\ngrove_dir = 'grove'\n")
    env = dict(os.environ, PATH=str(root / "bin") + os.pathsep + os.environ["PATH"],
               PR_TEST_ROOT=str(root), TERM="xterm-256color", BROWSER=str(browser))

    if len(sys.argv) > 3 and sys.argv[3] == "review":
        from review import exercise_review
        exercise_review(root, env, config, record)
        return

    if len(sys.argv) > 3 and sys.argv[3] == "review_hooks":
        from review import exercise_hooks
        exercise_hooks(root, env, config, record)
        return

    if len(sys.argv) > 3 and sys.argv[3] == "merge":
        exercise_merge(root, env, config, record)
        return
    if len(sys.argv) > 3 and sys.argv[3] == "close":
        exercise_close(root, env, config, record)
        return
    if len(sys.argv) > 3 and sys.argv[3] == "browser":
        exercise_browser(root, env, record)
        return
    if len(sys.argv) > 3 and sys.argv[3] == "notify":
        exercise_notify(root, env, config)
        return
    if len(sys.argv) > 3 and sys.argv[3] == "ordering":
        exercise_ordering(root, env, config, record)
        return
    if len(sys.argv) > 3 and sys.argv[3] == "merge_recovery":
        exercise_merge_recovery(root, env, config, record)
        return

    if len(sys.argv) > 3 and sys.argv[3] == "focus":
        exercise_focus(root, env, config)
        return
    raise AssertionError(f"Unknown scenario: {sys.argv[3:]}")


def exercise_focus(root, env, config):
    """Exercise terminal focus reports, refresh deduplication, retries, and cleanup."""
    config_path = root / "provider.json"
    cache = root / "data/requests.json"

    def user_calls():
        calls = map(json.loads, (root / "calls.jsonl").read_text().splitlines())
        return sum(call[call.index("--method") + 2] == "user" for call in calls)

    def save_config(**changes):
        config.update(changes)
        config_path.write_text(json.dumps(config))

    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        assert b"\x1b[?1004h" in session.output
        assert user_calls() == 1

        # Losing focus and Enter do not fetch anything or leave the request list.
        # The notification key provides a barrier after processing those events.
        session.send(b"\x1b[O\rn")
        session.wait_for("Set slack.token")
        assert "Pull / Merge Requests" in session.screen.text()
        assert "Enter: comments" not in session.screen.text()
        assert user_calls() == 1

        save_config(title="Updated after focus", discovery_delay=0.5)
        session.send(b"\x1b[I")
        session.wait_for("Refreshing requests")
        session.send(b"\x1b[O\x1b[I\x1b[I\x1b[O\x1b[Ir")
        session.wait_for("Updated after focus", row=4)
        session.wait_for("Statuses refreshed")
        assert user_calls() == 2
        assert json.loads(cache.read_text())["requests"][0]["request"]["title"] == "Updated after focus"

        save_config(fail_discovery=True, discovery_delay=0)
        session.send(b"\x1b[O\x1b[I")
        session.wait_for("warning(s)")
        assert user_calls() == 3
        save_config(fail_discovery=False, title="Recovered on focus")
        session.send(b"\x1b[O\x1b[I")
        session.wait_for("Recovered on focus", row=4)
        session.wait_for("Statuses refreshed")
        assert user_calls() == 4

        save_config(title="Manually refreshed")
        session.send(b"r")
        session.wait_for("Manually refreshed", row=4)
        session.wait_for("Statuses refreshed")
        assert user_calls() == 5
        session.send(b"q")
        session.wait_for_output(b"\x1b[?1004l")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()

    calls = (root / "calls.jsonl").read_bytes()
    cached = cache.read_bytes()
    session = Session(root, env, offline=True)
    try:
        session.wait_for("Offline: showing cached statuses")
        session.send(b"\x1b[O\x1b[I\rrn")
        session.wait_for("Cannot notify offline")
        assert "Pull / Merge Requests" in session.screen.text()
        session.send(b"q")
        session.wait_for_output(b"\x1b[?1004l")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()
    assert (root / "calls.jsonl").read_bytes() == calls
    assert cache.read_bytes() == cached


def exercise_ordering(root, env, config, record):
    """Group cached rows and retain the selected PR when provider updates move its row."""
    cache = root / "data/requests.json"
    config_path = root / "provider.json"
    project = config["canonical_project"]
    records, extra = [], []
    for number, workspace, day in [(7, "alpha", 1), (8, "alpha", 3), (9, "beta", 6), (10, None, 9)]:
        tracked = json.loads(json.dumps(record))
        request = tracked["request"]
        request.update(number=number, title=f"Request {number}", url=request["url"].rsplit("/", 1)[0] + f"/{number}",
                       updated_at=f"2026-10-{day:02}T00:00:00Z")
        if workspace:
            tracked["origins"] = [{"repository": "/source/repo", "checkout": f"/missing/{workspace}/repo",
                                   "workspace": {"name": workspace, "path": f"/missing/{workspace}"}}]
        records.append(tracked)
        if number != 7:
            extra.append({"number": number, "iid": number, "title": request["title"],
                          "html_url": request["url"], "web_url": request["url"],
                          "head": {"ref": "alice/feature", "repo": {"full_name": project}}, "base": {"ref": "main"},
                          "source_branch": "alice/feature", "target_branch": "main",
                          "source_project_id": 42, "target_project_id": 42,
                          "state": "opened" if record["remote"]["provider"] == "gitlab" else "open",
                          "user": {"login": "alice"}, "author": {"username": "alice"},
                          "updated_at": request["updated_at"]})
    config.update(title="Request 7", updated_at=records[0]["request"]["updated_at"], extra_requests=extra)
    config_path.write_text(json.dumps(config))
    cache.write_text(json.dumps({"version": 1, "requests": records}))
    original = cache.read_bytes()
    for offline in (True, False):
        session = Session(root, env, offline=offline)
        try:
            session.wait_for("Offline: showing cached statuses" if offline else "Statuses refreshed")
            for row, number in enumerate([8, 7, 9, 10], start=4):
                session.wait_for(f"Request {number}", row=row)
            session.send(b"j")
            session.wait_for("Link: " + records[0]["request"]["url"])
            if not offline:
                config["updated_at"] = "2026-10-04T00:00:00Z"
                config_path.write_text(json.dumps(config))
                session.send(b"\x1b[O\x1b[I")
                session.wait_for("Request 7", row=4)
                session.wait_for("Request 8", row=5)
                session.wait_for("Link: " + records[0]["request"]["url"])
                assert "›" in "".join(session.screen.rows[4])
                session.send(b"o")
                session.wait_for("Opened request in browser")
                assert (root / "browser-url").read_text() == records[0]["request"]["url"]
            session.send(b"q")
            assert session.process.wait(timeout=3) == 0
        finally:
            session.close()
        if offline:
            assert cache.read_bytes() == original
            assert not (root / "calls.jsonl").exists()


def exercise_notify(root, env, config):
    """Exercise notification key routing and configuration without any Slack connection."""
    # Old Slack environment settings must not bypass file configuration validation.
    env = dict(env, PR_SLACK_TOKEN="ignored-env-token", PR_SLACK_CHANNEL_ID="CIGNORED")
    base = "data_dir = 'data'\ngrove_dir = 'grove'\n"
    for key, settings, expected in [
        (b"n", {}, "Set slack.token"),
        (b"n", {"token": "test-secret-token"}, "Set slack.channel_id"),
        (b"n", {"token": "test-secret-token", "channel_id": "#engineering"},
         "slack.channel_id must be a Slack channel ID"),
        (b"n", {"token": "test-secret-token\n", "channel_id": "C0123456789"},
         "slack.token must be nonempty"),
        (b"n", {"token": "test-secret-token", "channel_id": "C0123456789"},
             "Set slack.review_message"),
            (b"n", {"token": "test-secret-token", "channel_id": "C0123456789", "review_message": "review"},
             "slack.review_message must contain {pr_url}"),
        (b"d", {}, "Set slack.token"),
        (b"d", {"token": "test-secret-token", "channel_id": "C0123456789"},
         "Set slack.deploy_message"),
        (b"d", {"token": "test-secret-token", "channel_id": "C0123456789", "deploy_message": "release"},
         "slack.deploy_message must contain {pr_url}"),
    ]:
        slack = "[slack]\n" + "\n".join(f"{key} = {json.dumps(value)}" for key, value in settings.items())
        (root / "config.toml").write_text(base + slack)
        session = Session(root, env)
        try:
            session.wait_for("Statuses refreshed")
            before = (root / "data/requests.json").read_bytes()
            session.send(key)
            session.wait_for(expected)
            assert "test-secret-token" not in session.screen.text()
            assert (root / "data/requests.json").read_bytes() == before
            session.send(b"q")
            assert session.process.wait(timeout=3) == 0
        finally:
            session.close()

    (root / "config.toml").write_text(base)
    calls = (root / "calls.jsonl").read_bytes()
    session = Session(root, env, offline=True)
    try:
        session.wait_for("Offline: showing cached statuses")
        session.send(b"n")
        session.wait_for("Cannot notify offline")
        session.send(b"d")
        session.wait_for("Cannot notify offline")
        assert json.loads((root / "data/requests.json").read_text())["requests"]
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()
    assert (root / "calls.jsonl").read_bytes() == calls

    (root / "data/requests.json").write_text(json.dumps({"version": 1, "requests": []}))
    config["existing"] = False
    (root / "provider.json").write_text(json.dumps(config))
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        session.send(b"n")
        session.wait_for("No request selected")
        session.send(b"d")
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()


def exercise_browser(root, env, record):
    cache = root / "data/requests.json"
    log = root / "browser-urls"
    # A text-browser name makes webbrowser wait for this fake launcher, so we
    # can deterministically exercise an in-flight batch without a real browser.
    browser = root / "bin/lynx"
    browser.write_text("""#!/usr/bin/env python3
import os, sys, time
from pathlib import Path
time.sleep(0.05)
with Path(os.environ['PR_TEST_ROOT'], 'browser-urls').open('a') as log:
    log.write(sys.argv[1] + '\\n')
""")
    browser.chmod(0o755)
    env = dict(env, BROWSER=str(browser))

    def request(number, status, project=None, workspace="feature"):
        item = json.loads(json.dumps(record))
        item["request"].update(number=number, status=status,
                               url=record["request"]["url"].rsplit("/", 1)[0] + f"/{number}")
        if project:
            item["remote"]["project"] = project
        item["origins"] = [{"repository": "/source/repo", "checkout": f"/grove/{workspace}/repo",
                            "workspace": {"name": workspace, "path": f"/grove/{workspace}"}}]
        return item

    entries = [request(7, "open"), request(8, "open"), request(9, "draft"),
               request(10, "merged"), request(11, "closed"),
               request(12, "open", project="another/repo"),
               request(13, "open", workspace="another-workspace"), request(7, "open")]
    cache.write_text(json.dumps({"version": 1, "requests": entries}))
    before = cache.read_bytes()
    session = Session(root, env, offline=True,
                      args=["--repo", record["remote"]["project"], "--workspace", "feature"])
    try:
        session.wait_for("Offline: showing cached statuses")
        # Start from a draft row: Ctrl+O must use the whole filtered list.
        # Equal-date rows use request number, so both cached #7 copies come first.
        session.send(b"jjj")
        session.wait_for("Link: " + entries[2]["request"]["url"])
        session.send(b"\x0f\x0f")
        session.wait_for("Opened 2 requests in browser")
        expected = [item["request"]["url"] for item in entries[:2]]
        assert log.read_text().splitlines() == expected
        # Plain o still opens just the selected request, including drafts.
        session.send(b"o")
        session.wait_for("Opened request in browser")
        assert log.read_text().splitlines() == expected + [entries[2]["request"]["url"]]
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()
    assert cache.read_bytes() == before

    for entries in ([request(9, "draft"), request(10, "merged"), request(11, "closed")], []):
        cache.write_text(json.dumps({"version": 1, "requests": entries}))
        log.write_text("")
        session = Session(root, env, offline=True)
        try:
            session.wait_for("Offline: showing cached statuses")
            session.send(b"\x0f")
            session.wait_for("No OPEN requests in the current list")
            assert log.read_text() == ""
            session.send(b"q")
            assert session.process.wait(timeout=3) == 0
        finally:
            session.close()
    assert not (root / "calls.jsonl").exists()


def exercise_close(root, env, config, record):
    cache = root / "data/requests.json"
    config_path = root / "provider.json"

    def save_config(**changes):
        config.update(changes)
        config_path.write_text(json.dumps(config))

    def reset_cache():
        cache.write_text(json.dumps({"version": 1, "requests": [record]}))

    def records():
        return json.loads(cache.read_text())["requests"]

    def close_calls():
        return [json.loads(line) for line in (root / "calls.jsonl").read_text().splitlines()
                if '"PATCH"' in line or '"PUT"' in line]

    save_config(fail_close=True, draft=True)
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        # The former close key must no longer start a provider action.
        session.send(b"Dn")
        session.wait_for("Set slack.token")
        assert len(close_calls()) == 0
        assert len(records()) == 1
        session.send(b"U")
        session.wait_for("Close/remove failed")
        assert len(records()) == 1
        assert json.loads(config_path.read_text()).get("status", "open") == "open"
        assert len(close_calls()) == 1

        # An HTTP success that still reports open is not a confirmed closure.
        save_config(fail_close=False, close_noop=True)
        session.send(b"U")
        session.wait_for("Provider did not confirm closure")
        assert len(records()) == 1

        # Repeated keypresses must not duplicate writes.
        save_config(close_noop=False, close_delay=0.3)
        session.send(b"UU")
        session.wait_for("Closed and stopped tracking")
        assert "Pull / Merge Requests" in session.screen.text()
        assert records() == []
        assert json.loads(config_path.read_text())["status"] == "closed"
        assert len(close_calls()) == 3
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()

    # Offline uppercase U cannot mutate either the provider or the tracking file;
    # lowercase u remains local removal only.
    reset_cache()
    before = (root / "calls.jsonl").read_text()
    session = Session(root, env, offline=True)
    try:
        session.wait_for("Pull / Merge Requests")
        session.send(b"U")
        session.wait_for("Cannot close requests offline")
        assert len(records()) == 1
        session.send(b"u")
        session.wait_for("Stopped tracking")
        assert records() == []
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()
    assert (root / "calls.jsonl").read_text() == before

    # Cache failure after remote success is recoverable without closing twice.
    reset_cache()
    save_config(status="open", close_delay=0)
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        cache.write_text("invalid cache")
        count = len(close_calls())
        session.send(b"U")
        session.wait_for("Request is closed or merged")
        assert json.loads(config_path.read_text())["status"] == "closed"
        assert cache.read_text() == "invalid cache"
        reset_cache()
        session.send(b"U")
        session.wait_for("Closed and stopped tracking")
        assert records() == []
        assert len(close_calls()) == count + 1
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()

    # A merged request only needs its tracking entry removed after a remote read.
    reset_cache()
    save_config(status="merged")
    count = len(close_calls())
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        session.send(b"U")
        session.wait_for("Closed and stopped tracking")
        assert records() == []
        assert len(close_calls()) == count
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()

    # A refresh that fetched the old open state cannot reinsert the closed row.
    reset_cache()
    save_config(status="open", discovery_delay=1)
    session = Session(root, env)
    try:
        session.wait_for("Pull / Merge Requests")
        deadline = time.monotonic() + 5
        while not (root / "discovery-started").exists():
            assert time.monotonic() < deadline
            time.sleep(0.01)
        session.send(b"U")
        session.wait_for("Closed and stopped tracking")
        session.wait_for("Statuses refreshed")
        assert "No matching requests" in session.screen.text()
        assert records() == []
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()

    # Quitting immediately after Shift+U still finishes the close and deletion.
    reset_cache()
    save_config(status="open", discovery_delay=0, close_delay=0.3)
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        session.send(b"Uq")
        assert session.process.wait(timeout=3) == 0
        assert records() == []
        assert json.loads(config_path.read_text())["status"] == "closed"
    finally:
        session.close()


def exercise_merge_recovery(root, env, config, record):
    """Exercise GitLab recovery writes and failure boundaries using the real merge keybind."""
    assert record["remote"]["provider"] == "gitlab"
    cache = root / "data/requests.json"
    config_path = root / "provider.json"
    audit = root / "mutations.jsonl"
    prefix = "projects/" + config["canonical_project"].replace("/", "%2F") + "/merge_requests/7"
    initial = dict(config, merge_requires_recovery=True, rebase_running_polls=2)
    initial["discussions"] = [
        {"id": "ordinary", "notes": [{"body": "Keep this comment", "resolvable": False}]},
        {"id": "open/thread", "notes": [{"body": "Review", "resolvable": True, "resolved": False},
                                         {"body": "Reply", "resolvable": True, "resolved": False}]},
        {"id": "resolved", "notes": [{"body": "Already done", "resolvable": True, "resolved": True}]},
        {"id": "last-page", "notes": [{"body": "Last page review", "resolvable": True, "resolved": False}]},
    ]

    # Completion, asynchronous errors, rejected/no-op resolution, and ambiguous merge results.
    cases = [
        ({}, "Merged ", ["merge", "rebase", "discussions/open%2Fthread", "discussions/last-page", "merge"], "merged"),
        ({"fail_rebase": True}, "Could not start GitLab rebase", ["merge", "rebase"], "open"),
        ({"rebase_error": "Rebase conflict"}, "Could not complete GitLab rebase", ["merge", "rebase"], "open"),
        ({"fail_rebase_poll": True}, "Could not complete GitLab rebase", ["merge", "rebase"], "open"),
        ({"fail_discussions": True}, "Could not resolve GitLab discussions", ["merge", "rebase"], "open"),
        ({"fail_resolution": True}, "Could not resolve GitLab discussions", ["merge", "rebase", "discussions/open%2Fthread"], "open"),
        ({"resolve_noop": True}, "Could not resolve GitLab discussions", ["merge", "rebase", "discussions/open%2Fthread"], "open"),
        ({"fail_merge": True}, "GitLab merge retry failed", ["merge", "rebase", "discussions/open%2Fthread", "discussions/last-page", "merge"], "open"),
        ({"merge_requires_recovery": False, "merge_response_lost": True}, "Merged ", ["merge"], "merged"),
        ({"reject_merge_once": True, "status_after_rejection": "closed"}, "Could not confirm request state", ["merge"], "open"),
        ({"status_after_rebase": "closed"}, "Only open, non-draft requests", ["merge", "rebase"], "open"),
        ({"status_after_rebase": "merged"}, "Merged ", ["merge", "rebase"], "merged"),
    ]
    for changes, expected, actions, cached_state in cases:
        settings = json.loads(json.dumps(initial))
        settings.update(changes)
        # Only the success case needs delayed polling; failure cases finish immediately.
        if changes:
            settings["rebase_running_polls"] = 0
        # Keep the discussion metadata valid for the normal refresh before merging.
        settings["discussions"] = [dict(d, notes=[dict(n, created_at="2026-10-01T12:00:00Z") for n in d["notes"]])
                                   for d in settings["discussions"]]
        config_path.write_text(json.dumps(settings))
        cache.write_text(json.dumps({"version": 1, "requests": [record]}))
        audit.unlink(missing_ok=True)
        (root / "rebase-polls.json").unlink(missing_ok=True)
        session = Session(root, env)
        try:
            session.wait_for("Statuses refreshed")
            before = cache.read_bytes()
            # Repeated presses while the recovery worker runs must not start another merge.
            session.send(b"mm")
            session.wait_for(expected)
            writes = [json.loads(line) for line in audit.read_text().splitlines()]
            assert [w["endpoint"].removeprefix(prefix + "/") for w in writes] == actions, writes
            assert all(w["method"] == "PUT" for w in writes)
            for write in writes:
                if write["endpoint"].endswith("/rebase"):
                    assert write["payload"] == {"skip_ci": True}
                elif "/discussions/" in write["endpoint"]:
                    assert write["payload"] == {"resolved": True}
            cached = json.loads(cache.read_text())["requests"]
            assert len(cached) == 1 and cached[0]["request"]["status"] == cached_state
            if cached_state == "open":
                assert cache.read_bytes() == before
            if not changes:
                assert cached[0]["request"]["unresolved_threads"] == 0
                assert writes[0]["payload"] == {"sha": "abc123"}
                assert writes[-1]["payload"] == {"sha": "rebased456"}
                assert json.loads((root / "rebase-polls.json").read_text()) == 3
                remote = json.loads(config_path.read_text())
                assert all(n.get("resolved") for d in remote["discussions"] for n in d["notes"] if n.get("resolvable"))
                assert [[n["body"] for n in d["notes"]] for d in remote["discussions"]] == [[n["body"] for n in d["notes"]] for d in initial["discussions"]]
            session.send(b"q")
            assert session.process.wait(timeout=3) == 0
        finally:
            session.close()


def exercise_merge(root, env, config, record):
    cache = root / "data/requests.json"
    config_path = root / "provider.json"
    gitlab = record["remote"]["provider"] == "gitlab"

    def save_config(**changes):
        config.update(changes)
        config_path.write_text(json.dumps(config))

    def reset_cache():
        cache.write_text(json.dumps({"version": 1, "requests": [record]}))

    def records():
        return json.loads(cache.read_text())["requests"]

    def merge_calls():
        return [json.loads(line) for line in (root / "calls.jsonl").read_text().splitlines()
                if '/7/merge"' in line]

    save_config(approved=True, fail_merge=True, allow_merge_commit=False)
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        session.wait_for("APPROVED")
        assert records()[0]["request"]["approved"] is True
        assert records()[0]["request"]["status"] == "open"
        session.send(b"m")
        session.wait_for("Merge failed")
        assert len(merge_calls()) == (2 if gitlab else 1)
        assert records()[0]["request"]["status"] == "open"

        save_config(fail_merge=False, merge_noop=True)
        session.send(b"m")
        session.wait_for("Provider did not confirm merge")
        assert records()[0]["request"]["status"] == "open"

        # Repeated presses must issue one mutation.
        save_config(merge_noop=False, merge_delay=0.3)
        session.send(b"mm")
        session.wait_for("Merged ")
        assert len(merge_calls()) == (5 if gitlab else 3)
        assert len(records()) == 1
        assert records()[0]["request"]["status"] == "merged"
        assert records()[0]["request"]["last_comment_at"] == "2026-10-01T12:00:05Z"
        session.wait_for("MERGED")
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()

    # Offline m must leave both the remote and cached state untouched.
    before = (root / "calls.jsonl").read_text()
    cached = cache.read_bytes()
    session = Session(root, env, offline=True)
    try:
        session.wait_for("Offline: showing cached statuses")
        session.send(b"m")
        session.wait_for("Cannot merge requests offline")
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()
    assert cache.read_bytes() == cached
    assert (root / "calls.jsonl").read_text() == before

    # Read current remote state: drafts and closed requests cannot be merged.
    for status, draft in [("open", True), ("closed", False)]:
        reset_cache()
        save_config(status=status, draft=draft, merge_delay=0)
        count = len(merge_calls())
        session = Session(root, env)
        try:
            session.wait_for("Statuses refreshed")
            assert "APPROVED" not in session.screen.text()
            session.send(b"m")
            session.wait_for("Only open, non-draft requests can be merged")
            assert len(merge_calls()) == count
            assert len(records()) == 1
            session.send(b"q")
            assert session.process.wait(timeout=3) == 0
        finally:
            session.close()

    # Saving can fail after the remote merge; retry without merging twice.
    reset_cache()
    save_config(status="open", draft=False)
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        cache.write_text("invalid cache")
        count = len(merge_calls())
        session.send(b"m")
        session.wait_for("Request was merged, but updating requests.json failed")
        assert cache.read_text() == "invalid cache"
        assert json.loads(config_path.read_text())["status"] == "merged"
        reset_cache()
        session.send(b"m")
        session.wait_for("Merged ")
        assert records()[0]["request"]["status"] == "merged"
        assert len(merge_calls()) == count + 1
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()

    # An in-flight refresh must not replace MERGED with its old open snapshot.
    reset_cache()
    save_config(status="open", discovery_delay=1)
    session = Session(root, env)
    try:
        session.wait_for("Pull / Merge Requests")
        deadline = time.monotonic() + 5
        while not (root / "discovery-started").exists():
            assert time.monotonic() < deadline
            time.sleep(0.01)
        session.send(b"m")
        session.wait_for("Merged ")
        session.wait_for("Statuses refreshed")
        session.wait_for("MERGED")
        assert records()[0]["request"]["status"] == "merged"
        session.send(b"q")
        assert session.process.wait(timeout=3) == 0
    finally:
        session.close()

    reset_cache()
    save_config(status="open", discovery_delay=0, merge_delay=0.3,
                allow_squash_merge=False)
    session = Session(root, env)
    try:
        session.wait_for("Statuses refreshed")
        session.send(b"mq")
        assert session.process.wait(timeout=3) == 0
        assert records()[0]["request"]["status"] == "merged"
    finally:
        session.close()


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as directory:
        exercise(Path(directory), sys.argv[2])
