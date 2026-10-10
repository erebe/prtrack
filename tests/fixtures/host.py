#!/usr/bin/env python3
"""Local provider/transport fixture. Never contacts a network service."""
import atexit
import fcntl
import json
import os
import re
from pathlib import Path
import sys
import time
from datetime import datetime
from urllib.parse import parse_qs, unquote

root = Path(os.environ["PR_TEST_ROOT"])
config_path = root / "provider.json"
config = json.loads(config_path.read_text())
program = Path(sys.argv[0]).name
args = sys.argv[1:]

for name, expected in config.get("expected_proxy", {}).items():
    assert os.environ.get(name) == expected, f"Unexpected proxy environment: {name}"


def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)


if program == "git":
    if not args or args[0] != "push":
        fail("Only git push may use a subprocess; repository reads must use gix")
    with (root / "calls.jsonl").open("a") as output:
        output.write(json.dumps(["git", *args]) + "\n")
    if config.get("fail_push"):
        fail("Push rejected by fixture")
    remote = args[-2]
    args = ["-c", f"remote.{remote}.pushurl={root / 'remote.git'}", *args]
    os.execv(os.environ["PR_TEST_GIT"], ["git", *args])

with (root / "calls.jsonl").open("a") as output:
    output.write(json.dumps([program, *args]) + "\n")
assert args[0] == "api"
method = args[args.index("--method") + 1]
endpoint = args[args.index("--method") + 2]
payload = None
if "--input" in args:
    # Like glab, a raw body needs an explicit type for GitLab to parse JSON.
    assert "--header" in args
    assert args[args.index("--header") + 1] == "Content-Type: application/json"
    payload = json.load(sys.stdin)
if "repositories" in config:
    host = args[args.index("--hostname") + 1]
    if endpoint == "graphql":
        variables = payload["variables"]
        if "search" in variables:
            project = next(term.removeprefix("repo:") for term in variables["search"].split()
                           if term.startswith("repo:"))
        else:
            project = variables.get("project") or f'{variables["owner"]}/{variables["name"]}'
    elif endpoint.startswith("repos/"):
        project = "/".join(endpoint.split("/")[1:3])
    elif endpoint.startswith("projects/"):
        project = unquote(endpoint.split("/")[1])
    else:
        assert endpoint == "user"
        project = config["project"]
    config.update(config["repositories"].get(f"{host}/{project}", {}))
    config.update(host=host, project=project)
assert args[args.index("--hostname") + 1] == config["host"]


def measure_concurrency(delta, barrier_broken=False):
    # Track active provider processes, including failures, under a process lock.
    with (root / "concurrency.json").open("a+") as probe:
        fcntl.flock(probe, fcntl.LOCK_EX)
        probe.seek(0)
        stats = json.loads(probe.read() or '{"active": 0, "peak": 0}')
        stats["active"] += delta
        stats["peak"] = max(stats["peak"], stats["active"])
        if barrier_broken:
            stats["barrier_broken"] = True
        probe.seek(0)
        probe.truncate()
        probe.write(json.dumps(stats))
        return stats


if config.get("measure_concurrency"):
    measure_concurrency(1)
    atexit.register(measure_concurrency, -1)
    if re.search(config.get("concurrency_barrier", r"(?!)"), endpoint):
        deadline = time.monotonic() + 5
        while True:
            stats = measure_concurrency(0)
            if stats["peak"] >= 10 or stats.get("barrier_broken"):
                break
            if time.monotonic() >= deadline:
                measure_concurrency(0, barrier_broken=True)
                break
            time.sleep(0.005)
    time.sleep(config.get("call_delay", 0.05))

if config.get("fail_api"):
    fail("Authentication unavailable in fixture")
is_gitlab = program == "glab"
project = config.get("canonical_project", config["project"])
username = config.get("username", "alice")
prefix = "projects/" + project.replace("/", "%2F") if is_gitlab else "repos/" + project
original_prefix = "projects/" + config["project"].replace("/", "%2F") if is_gitlab else "repos/" + config["project"]
collection = prefix + ("/merge_requests" if is_gitlab else "/pulls")

if endpoint == "user":
    if config.get("fail_user"):
        fail("Cannot determine authenticated username")
    print(json.dumps({"username": username, "login": username}))
    sys.exit(0)
if method == "GET" and endpoint in (prefix, original_prefix):
    print(json.dumps({"default_branch": "main", "full_name": project, "path_with_namespace": project,
                      "allow_merge_commit": config.get("allow_merge_commit", True),
                      "allow_squash_merge": config.get("allow_squash_merge", True)}))
    sys.exit(0)
if original_prefix != prefix and endpoint.startswith(original_prefix + "/"):
    if method in ("POST", "PATCH", "PUT"):
        fail("gh: HTTP 307")
    # GETs follow the rename redirect, but gh cannot replay this streamed POST body.
    endpoint = prefix + endpoint[len(original_prefix):]

if method in ("PUT", "PATCH"):
    with (root / "mutations.jsonl").open("a") as output:
        output.write(json.dumps({"method": method, "endpoint": endpoint, "payload": payload}) + "\n")

if is_gitlab and method == "PUT" and endpoint == collection + "/7/rebase":
    assert payload == {"skip_ci": True}, payload
    if config.get("fail_rebase"):
        fail("Rebase rejected by fixture")
    (root / "rebase-polls.json").write_text("0")
    print(json.dumps({"rebase_in_progress": True}))
    sys.exit(0)

if is_gitlab and method == "PUT" and endpoint.startswith(collection + "/7/discussions/"):
    assert payload == {"resolved": True}, payload
    discussion_id = unquote(endpoint.rsplit("/", 1)[1])
    if config.get("fail_resolution"):
        fail("Resolving discussion rejected by fixture")
    discussion = next(d for d in config["discussions"] if d["id"] == discussion_id)
    if not config.get("resolve_noop"):
        for note in discussion["notes"]:
            if note.get("resolvable"):
                note["resolved"] = True
        config_path.write_text(json.dumps(config))
    print(json.dumps(discussion))
    sys.exit(0)

# GitLab merge recovery still resolves native discussions, including later pages.
if is_gitlab and endpoint == collection + "/7/discussions?per_page=100":
    assert method == "GET"
    assert "--paginate" in args
    if config.get("fail_discussions"):
        fail("Discussions unavailable in fixture")
    discussions = config.get("discussions", [])
    print(json.dumps(discussions[:1]))
    print(json.dumps(discussions[1:]))
    sys.exit(0)

if method == "POST" and endpoint != "graphql":
    assert endpoint == collection
    assert args[-2:] == ["--input", "-"]
    (root / "payload.json").write_text(json.dumps(payload))
    if config.get("fail_create"):
        fail("Request creation unavailable in fixture")
    config.update(existing=True, title=payload["title"], draft=payload.get("draft", payload["title"].startswith("Draft:")),
                  base=payload.get("base", payload.get("target_branch")), branch=payload.get("head", payload.get("source_branch")))
    config_path.write_text(json.dumps(config))

if method == "PUT" and endpoint == collection + "/7/merge":
    assert payload["sha"] == config.get("sha", "abc123"), payload
    if not is_gitlab:
        expected = "merge" if config.get("allow_merge_commit", True) else (
            "squash" if config.get("allow_squash_merge", True) else "rebase")
        assert payload["merge_method"] == expected, payload
    (root / "merge-payload.json").write_text(json.dumps(payload))
    if config.get("merge_delay"):
        time.sleep(config["merge_delay"])
    if config.get("fail_merge"):
        fail("Merge rejected by fixture")
    if config.pop("reject_merge_once", False):
        config["status"] = config.get("status_after_rejection", "open")
        config_path.write_text(json.dumps(config))
        fail("Merge initially rejected by fixture")
    if config.get("merge_requires_recovery"):
        assert is_gitlab
        if not config.get("rebased") or any(n.get("resolvable") and not n.get("resolved")
                                             for d in config.get("discussions", []) for n in d["notes"]):
            fail("Rebase and resolved discussions required by fixture")
    if not config.get("merge_noop"):
        config["status"] = "merged"
        config_path.write_text(json.dumps(config))
        if config.get("merge_response_lost"):
            fail("Merge response lost in fixture")
    if not is_gitlab:
        print(json.dumps({"merged": not config.get("merge_noop", False), "message": "Merge pending"}))
        sys.exit(0)
    endpoint = collection + "/7"
elif method in ("PATCH", "PUT"):
    assert method == ("PUT" if is_gitlab else "PATCH")
    assert endpoint == collection + "/7"
    assert args[-2:] == ["--input", "-"]
    assert payload == ({"state_event": "close"} if is_gitlab else {"state": "closed"})
    if config.get("close_delay"):
        time.sleep(config["close_delay"])
    if config.get("fail_close"):
        fail("Closing unavailable in fixture")
    if not config.get("close_noop"):
        config["status"] = "closed"
        config_path.write_text(json.dumps(config))

state = config.get("status", "open")
branch = config.get("branch", "feature/topic")
base = config.get("base", "main")
title = config.get("title", "Add feature")
draft = config.get("draft", False)
host = config["host"]
if is_gitlab:
    request = {"iid": 7, "title": title, "web_url": f"https://{host}/{project}/-/merge_requests/7",
               "source_branch": branch, "target_branch": base, "sha": config.get("sha", "abc123"), "state": "opened" if state == "open" else state,
               "draft": draft, "source_project_id": 42, "target_project_id": 42, "author": {"username": username}}
else:
    request = {"number": 7, "title": title, "html_url": f"https://{host}/{project}/pull/7",
               "head": {"ref": branch, "sha": "abc123", "repo": {"full_name": project}}, "base": {"ref": base},
               "state": "closed" if state == "merged" else state, "merged_at": "2026-10-10" if state == "merged" else None,
               "draft": draft, "user": {"login": username}}

request["updated_at"] = config.get("updated_at", "2026-10-01T00:00:00Z")
if is_gitlab and endpoint == collection + "/7?include_rebase_in_progress=true":
    assert method == "GET"
    if config.get("fail_rebase_poll"):
        fail("Rebase status unavailable in fixture")
    probe = root / "rebase-polls.json"
    polls = json.loads(probe.read_text()) + 1
    probe.write_text(json.dumps(polls))
    running = polls <= config.get("rebase_running_polls", 0)
    request.update(rebase_in_progress=running, merge_error=config.get("rebase_error"))
    if not running and not config.get("rebase_error"):
        config.update(sha="rebased456", rebased=True, status=config.get("status_after_rebase", state))
        config_path.write_text(json.dumps(config))
        request.update(sha=config["sha"], state="opened" if config["status"] == "open" else config["status"])
    print(json.dumps(request))
    sys.exit(0)
request["approved"] = config.get("approved", False)
request["pipeline_status"] = config.get("pipeline_status")
request["assignees"] = config.get("assignees", [])
for field in ("issue_comments", "reviews", "review_comments", "review_threads", "discussions"):
    request[field] = config.get(field, [])
all_requests = ([request] if config.get("existing") else []) + config.get("extra_requests", [])
for item in all_requests:
    item.setdefault("updated_at", "2026-10-01T00:00:00Z")


def assigned_to(item, username):
    return any((user.get("username") or user.get("login", "")).lower() == username.lower()
               for user in item.get("assignees", []))


def graphql_request(item):
    return {"number": item["number"], "title": item["title"], "url": item["html_url"],
            "headRefName": item["head"]["ref"], "baseRefName": item["base"]["ref"],
            "state": "MERGED" if item.get("merged_at") else item["state"].upper(),
            "isDraft": item.get("draft", False), "updatedAt": item["updated_at"],
            "reviewDecision": "APPROVED" if item.get("approved") else "REVIEW_REQUIRED",
            "headRepository": {"nameWithOwner": item["head"]["repo"]["full_name"]}
            if item["head"].get("repo") else None, **github_comment_metadata(item)}


def newest_comments(comments):
    """Model the API's chronological connection, including timestamp offsets."""
    comments = sorted(comments, key=lambda c: datetime.fromisoformat(c["created_at"]))
    return [{"createdAt": c["created_at"], "publishedAt": c.get("published_at")}
            for c in comments[-1:]]


def github_review_page(item, offset=0):
    """Model submitted review pagination and each review's inline-comment connection."""
    reviews = [r for r in item.get("reviews", []) if r.get("state") != "PENDING"]
    nodes = []
    for review in reviews[offset:offset + 100]:
        comments = [c for c in item.get("review_comments", [])
                    if c.get("pull_request_review_id") == review.get("id")]
        nodes.append({"body": review.get("body", ""), "submittedAt": review.get("submitted_at"),
                      "comments": {"nodes": newest_comments(comments)}})
    more = offset + 100 < len(reviews)
    cursor = f"reviews:{offset + 100}" if more else None
    if config.get("review_repeated_cursor"):
        more, cursor = True, "reviews:0"
    return {"nodes": nodes, "pageInfo": {"hasNextPage": more, "endCursor": cursor}}


def github_thread_page(item, offset=0):
    threads = item.get("review_threads", [])
    more = offset + 100 < len(threads)
    cursor = f"threads:{offset + 100}" if more else None
    if config.get("thread_repeated_cursor"):
        more, cursor = True, "threads:0"
    if config.get("thread_empty_cursor"):
        more, cursor = True, ""
    return {"nodes": [{"isResolved": thread.get("isResolved")} for thread in threads[offset:offset + 100]],
            "pageInfo": {"hasNextPage": more, "endCursor": cursor}}


def github_comment_metadata(item):
    return {"comments": {"nodes": newest_comments(item.get("issue_comments", []))},
            "reviews": github_review_page(item),
            "reviewThreads": None if item["number"] in config.get("fail_thread_numbers", [])
            else github_thread_page(item)}


def gitlab_comment_metadata(item):
    discussions = item.get("discussions", [])
    notes = [n for d in discussions for n in d["notes"] if not n.get("system")]
    resolvable = [d for d in discussions if any(n.get("resolvable") for n in d["notes"])]
    resolved = [d for d in resolvable if all(n.get("resolved") for n in d["notes"] if n.get("resolvable"))]
    return {"notes": {"nodes": newest_comments(notes)},
            "resolvableDiscussionsCount": None if item["iid"] in config.get("fail_thread_numbers", [])
            else config.get("resolvable_discussions_count", len(resolvable)),
            "resolvedDiscussionsCount": config.get("resolved_discussions_count", len(resolved))}


if endpoint == "graphql":
    assert method == "POST"
    assert args[-2:] == ["--input", "-"]
    with (root / "graphql.jsonl").open("a") as output:
        output.write(json.dumps(payload) + "\n")
    query = payload["query"]
    if "headRefName" in query:
        assert "updatedAt" in query
    if "headPipeline" in query or "statusCheckRollup" in query:
        if config.get("fail_pipelines"):
            fail("Pipelines unavailable in fixture")
        parent, alias, key = ("project", "mr", "iid") if is_gitlab else ("repository", "pr", "number")
        if is_gitlab:
            assert payload["variables"] == {"project": project}
        else:
            assert payload["variables"] == dict(zip(("owner", "name"), project.split("/", 1)))
            assert "commits(last: 1)" in query
        numbers = re.findall(alias + r'([0-9]+): ', query)
        assert 0 < len(numbers) <= 50
        result, errors = {}, list(config.get("pipeline_graphql_errors", []))
        for number in numbers:
            item = next((i for i in [request, *all_requests] if i[key] == int(number)), None)
            if item is None or int(number) in config.get("fail_pipeline_numbers", []):
                result[f"{alias}{number}"] = None
                errors.append({"message": "Pipeline unavailable", "path": [parent, f"{alias}{number}"]})
                continue
            state = item.get("pipeline_status")
            if is_gitlab:
                value = {"headPipeline": {"status": state} if state else None}
            else:
                value = {"commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": state} if state else None}}]}}
            result[f"{alias}{number}"] = value
        print(json.dumps({"data": {parent: result}, "errors": errors}))
        sys.exit(0)
    if not is_gitlab and "$number: Int!" in query and "reviewThreads" in query:
        variables = payload["variables"]
        assert "reviewThreads(first: 100, after: $cursor)" in query
        assert "nodes { isResolved }" in query
        if config.get("fail_thread_page"):
            fail("Thread page unavailable in fixture")
        item = next(i for i in [request, *all_requests] if i["number"] == variables["number"])
        offset = int(variables["cursor"].removeprefix("threads:"))
        print(json.dumps({"data": {"repository": {"pullRequest": {"reviewThreads": github_thread_page(item, offset)}}},
                          "errors": config.get("thread_page_errors", [])}))
        sys.exit(0)
    if not is_gitlab:
        assert "states: [APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED]" in query
        assert "comments(last: 1)" in query and "publishedAt" in query
        if "$number: Int!" not in query:
            assert "reviewThreads(first: 100) { nodes { isResolved }" in query
    else:
        assert "notes(last: 1, filter: ONLY_COMMENTS)" in query
        assert "resolvableDiscussionsCount resolvedDiscussionsCount" in query
    if not is_gitlab and "$number: Int!" in query:
        variables = payload["variables"]
        assert "reviews(first: 100, after: $cursor" in query
        if config.get("fail_review_page"):
            fail("Review page unavailable in fixture")
        item = next(i for i in [request, *all_requests] if i["number"] == variables["number"])
        offset = int(variables["cursor"].removeprefix("reviews:"))
        print(json.dumps({"data": {"repository": {"pullRequest": {"reviews": github_review_page(item, offset)}}}}))
        sys.exit(0)
    if not is_gitlab and "headRefName" in query and "type: ISSUE" not in query:
        if config.get("fail_detail"):
            fail("Detail lookup unavailable in fixture")
        assert payload["variables"] == dict(zip(("owner", "name"), project.split("/", 1)))
        numbers = re.findall(r'pr([0-9]+): pullRequest', query)
        assert 1 < len(numbers) <= 50
        results, errors = {}, list(config.get("batch_graphql_errors", []))
        for number in numbers:
            item = next((item for item in [request, *all_requests] if item["number"] == int(number)), None)
            if not item or int(number) in config.get("fail_detail_numbers", []):
                results[f"pr{number}"] = None
                errors.append({"message": "Detail lookup unavailable in fixture",
                               "path": ["repository", f"pr{number}"]})
            else:
                results[f"pr{number}"] = graphql_request(item)
                if int(number) in config.get("fail_comment_numbers", []):
                    results[f"pr{number}"]["comments"] = None
                    errors.append({"message": "Comments unavailable", "path": ["repository", f"pr{number}", "comments"]})
        print(json.dumps({"data": {"repository": results}, "errors": errors}))
        sys.exit(0)
    if "type: ISSUE" not in query:
        if config.get("fail_approvals") and not is_gitlab:
            print(json.dumps({"errors": [{"message": "Approvals unavailable"}]}))
            sys.exit(0)
        if is_gitlab:
            assert payload["variables"]["project"] == project
            numbers = re.findall(r'mr([0-9]+): mergeRequest', query)
        else:
            assert payload["variables"] == dict(zip(("owner", "name"), project.split("/", 1)))
            numbers = re.findall(r'pr([0-9]+): pullRequest', query)
        assert 0 < len(numbers) <= 50
        result, errors = {}, []
        for number in numbers:
            item = next((item for item in [request, *all_requests]
                         if item["iid" if is_gitlab else "number"] == int(number)), None)
            approved = item and item.get("approved", False)
            result[("mr" if is_gitlab else "pr") + number] = (
                {"approved": approved, **gitlab_comment_metadata(item)} if is_gitlab else
                {"reviewDecision": "APPROVED" if approved else "REVIEW_REQUIRED", **github_comment_metadata(item)}) if item else None
            if is_gitlab and item:
                if config.get("fail_approvals") and f'mr{number}: mergeRequest(iid: "{number}") {{ approved ' in query:
                    result[f"mr{number}"]["approved"] = None
                    errors.append({"message": "Approvals unavailable", "path": ["project", f"mr{number}", "approved"]})
                if int(number) in config.get("fail_comment_numbers", []):
                    result[f"mr{number}"]["notes"] = None
                    errors.append({"message": "Comments unavailable", "path": ["project", f"mr{number}", "notes"]})
        print(json.dumps({"data": {"project" if is_gitlab else "repository": result}, "errors": errors}))
        sys.exit(0)
    assert not is_gitlab
    assert query.lstrip().startswith("query(")
    assert "type: ISSUE" in query and "first: 100" in query and "after: $cursor" in query
    for field in ("issueCount", "pageInfo", "hasNextPage", "endCursor", "number", "title", "url",
                  "headRefName", "baseRefName", "state", "isDraft", "reviewDecision", "headRepository", "nameWithOwner"):
        assert field in query, field
    variables = payload["variables"]
    terms = variables["search"].split()
    assert f"repo:{project}" in terms and "is:pr" in terms and "is:open" in terms
    filters = [(role, term.removeprefix(role + ":")) for role in ("author", "assignee")
               for term in terms if term.startswith(role + ":")]
    assert len(filters) == 1, "Author and assignee must be queried separately (OR, not AND)"
    role, user = filters[0]
    if config.get("fail_discovery") or (role == "assignee" and config.get("fail_assignee_discovery")):
        fail("Discovery unavailable in fixture")
    matches = [item for item in all_requests if item["state"] == "open"
               and (item["user"]["login"].lower() == user.lower() if role == "author"
                    else assigned_to(item, user))]
    offset = int(variables["cursor"].removeprefix("cursor:")) if variables.get("cursor") else 0
    size = 1 if config.get("extra_page") else 100
    nodes = [graphql_request(item) for item in matches[offset:offset + size]]
    more = offset + size < len(matches)
    cursor = f"cursor:{offset + size}" if more else None
    if config.get("graphql_repeated_cursor"):
        more, cursor = True, "cursor:0"
    response = {"data": {"search": {"issueCount": config.get("graphql_counts", {}).get(role, config.get("graphql_count", len(matches))),
                                    "nodes": nodes,
                                    "pageInfo": {"hasNextPage": more, "endCursor": cursor}}}}
    if config.get("graphql_errors") or (config.get("graphql_error_page") and offset > 0):
        response["errors"] = [{"message": "Search unavailable in fixture"}]
    if config.get("discovery_delay"):
        (root / "discovery-started").touch()
        time.sleep(config["discovery_delay"])
    print(json.dumps(response))
    sys.exit(0)

issues_endpoint = not is_gitlab and endpoint.startswith(prefix + "/issues?")
if endpoint.startswith(collection + "?") or issues_endpoint:
    assert "--paginate" in args
    query = parse_qs(endpoint.split("?", 1)[1])
    if "iids[]" in query:
        assert is_gitlab and query["state"] == ["all"] and query["scope"] == ["all"]
        assert 1 < len(query["iids[]"]) <= 50
        assert "author_username" not in query and "assignee_username[]" not in query
        if config.get("fail_detail"):
            fail("Detail lookup unavailable in fixture")
        numbers = {int(number) for number in query["iids[]"]}
        items = {item["iid"]: item for item in [request, *all_requests]}
        requests = [item for number, item in items.items() if number in numbers
                    and number not in config.get("fail_detail_numbers", [])]
        # The caller must keep results from every page, regardless of return order.
        requests.reverse()
        print(json.dumps(requests[:1]))
        print(json.dumps(requests[1:]))
        sys.exit(0)
    if config.get("fail_discovery"):
        fail("Discovery unavailable in fixture")
    author = query.get("creator" if issues_endpoint else "author_username", [None])[0]
    assignee = query.get("assignee" if issues_endpoint else "assignee_username[]", [None])[0]
    assert author is None or assignee is None
    if assignee is not None and config.get("fail_assignee_discovery"):
        fail("Discovery unavailable in fixture")
    requests = [item for item in all_requests if item["state"] == ("opened" if is_gitlab else "open")]
    if author is not None:
        requests = [item for item in requests if
                    (item["author"]["username"] if is_gitlab else item["user"]["login"]).lower() == author.lower()]
    if assignee is not None:
        requests = [item for item in requests if assigned_to(item, assignee)]
    if issues_endpoint:
        requests = [{"number": item["number"], "pull_request": {"url": item["html_url"]}}
                    for item in requests]
        if config.get("include_issue"):
            requests.insert(0, {"number": 99, "title": "An ordinary issue"})
    if config.get("discovery_delay"):
        (root / "discovery-started").touch()
        time.sleep(config["discovery_delay"])
    if config.get("extra_page"):
        print(json.dumps(requests[:1]))
        print(json.dumps(requests[1:]))
    else:
        print(json.dumps(requests))
elif endpoint == collection or endpoint.startswith(collection + "/"):
    if method == "GET" and config.get("fail_detail"):
        fail("Detail lookup unavailable in fixture")
    if method == "GET" and endpoint != collection and int(endpoint.rsplit("/", 1)[1]) in config.get("fail_detail_numbers", []):
        fail("Detail lookup unavailable in fixture")
    if endpoint in (collection, collection + "/7"):
        print(json.dumps(request))
    else:
        number = int(endpoint.rsplit("/", 1)[1])
        match = next((item for item in all_requests if item["iid" if is_gitlab else "number"] == number), None)
        if match is None:
            fail(f"Unexpected request detail lookup: {endpoint}")
        print(json.dumps(match))
else:
    fail(f"Unexpected API endpoint: {endpoint}")
