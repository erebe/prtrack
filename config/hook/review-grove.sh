#!/usr/bin/env bash
# Install in the active config's hook/ directory. pr supplies PR_* environment variables.
set -euo pipefail

branch=${PR_BRANCH:?PR_BRANCH required}
base=${PR_BASE:?PR_BASE required}
url=${PR_URL:?PR_URL required}
host=${PR_HOST:?PR_HOST required}
project=${PR_PROJECT:?PR_PROJECT required}
number=${PR_NUMBER:?PR_NUMBER required}

fail() { printf '%s\n' "$*" >&2; exit 1; }
for program in git gw stg herdr jq; do
    command -v "$program" >/dev/null || fail "Install $program and ensure it is on PATH"
done

# Match HTTPS, SSH URL and scp-style remotes, including self-hosted GitLab
# subgroups. SSH ports are transport details; HTTP ports identify the API host.
remote_id='
def remote_id:
    (if test("^(https?|ssh|git)://") then
        capture("^(?<scheme>https?|ssh|git)://(?:[^/@]*@)?(?<host>\\[[^]]+\\]|[^/:]+)(?::(?<port>[0-9]+))?/(?<project>[^?#]*)")
        | if (.scheme == "http" or .scheme == "https") and .port != null
             and ([.scheme, .port] != ["http", "80"])
             and ([.scheme, .port] != ["https", "443"])
          then .host += ":" + .port else . end
     else
        capture("^(?:[^/@:]+@)?(?<host>[^/:]+):(?<project>.+)$")
     end)
    | {host: (.host | ascii_downcase),
       project: (.project | gsub("^/+|/+$"; "") | sub("\\.git$"; ""))};
'
repositories=$(gw repos --json)
matches=$(jq --arg host "$host" --arg project "$project" "$remote_id"'
    [.[] | select((try (.remote | remote_id) catch null) == {host: $host, project: $project})]
' <<< "$repositories")
case $(jq 'length' <<< "$matches") in
    0) fail "No Grove repository matches $host/$project; register its clone with gw add-dir" ;;
    1) ;;
    *) fail 'Multiple Grove clones match this request' ;;
esac
repo=$(jq -er '.[0].name' <<< "$matches")
source=$(jq -er '.[0].path' <<< "$matches")
[[ $(jq --arg name "$repo" '[.[] | select(.name == $name)] | length' <<< "$repositories") == 1 ]] ||
    fail "Multiple Grove repositories are named $repo; use distinct clone directory names"
[[ -n "$repo" && "$repo" != '.' && "$repo" != '..' && "$repo" != *[,$'\n\r/']*
    && "$repo" != [[:space:]]* && "$repo" != *[[:space:]] ]] || fail 'Unsupported Grove repository name'
cd -- "$source"
origin=$(git remote get-url origin)
jq -en --arg origin "$origin" --arg host "$host" --arg project "$project" "$remote_id"'
    ($origin | remote_id) == {host: $host, project: $project}
' >/dev/null || fail "Grove repository origin no longer matches this request; refresh Grove's repository list"
workspace="review-$repo-$number"

git check-ref-format "refs/heads/$branch"
git check-ref-format "refs/heads/$base"
[[ "$branch" != "$base" ]] || fail 'PR source and base branches must differ'

# A failed fetch must stop here: gw --track otherwise falls back to creating a
# branch from the default base when the remote branch is unavailable.
git fetch --no-tags -- origin \
    "+refs/heads/$branch:refs/remotes/origin/$branch" \
    "+refs/heads/$base:refs/remotes/origin/$base" >&2
head=$(git rev-parse --verify "refs/remotes/origin/$branch^{commit}")
merge_base=$(git merge-base --all "$head" "refs/remotes/origin/$base")
[[ -n "$merge_base" && "$merge_base" != *$'\n'* ]] || fail 'PR has no unique merge-base'
[[ $(git rev-list --count --min-parents=2 "$merge_base..$head") == 0 ]] ||
    fail 'PR contains merge commits; stg uncommit requires linear history'
count=$(git rev-list --count "$merge_base..$head")

# Never reset an existing local branch to make room for a review checkout.
if local_head=$(git rev-parse --verify "refs/heads/$branch" 2>/dev/null); then
    [[ "$local_head" == "$head" ]] || fail "Local branch $branch differs from origin/$branch; leaving it unchanged"
fi
source_common=$(git rev-parse --path-format=absolute --git-common-dir)
source_common=$(cd "$source_common" && pwd -P)

if ! workspace_path=$(gw ws show "$workspace" --output path 2>/dev/null); then
    gw create "$workspace" --no-hooks --repos "$repo" --branch "$branch" --track --source-url "$url" >&2
    workspace_path=$(gw ws show "$workspace" --output path)
fi
checkout="$workspace_path/$repo"
[[ -d "$checkout" ]] || fail "Review checkout does not exist: $checkout"
cd "$checkout"
checkout=$(pwd -P)
[[ $(git rev-parse --show-toplevel) == "$checkout" ]] || fail 'Review path is not a worktree root'
checkout_common=$(git rev-parse --path-format=absolute --git-common-dir)
checkout_common=$(cd "$checkout_common" && pwd -P)
[[ "$checkout_common" == "$source_common" ]] || fail 'Review workspace belongs to another repository'
[[ $(git symbolic-ref HEAD) == "refs/heads/$branch" ]] || fail 'Review workspace is on another branch'
[[ $(git rev-parse HEAD) == "$head" ]] || fail 'Review checkout differs from the fetched PR head; leaving it unchanged'
[[ -z $(git status --porcelain) ]] || fail "Review checkout has local changes: $checkout"

# Existing stacks are safe to retry, but must not already include base commits.
stack_base=$(stg id '{base}')
git merge-base --is-ancestor "$merge_base" "$stack_base" ||
    fail 'Existing StGit stack extends below the PR merge-base; leaving it unchanged'
git branch --set-upstream-to="refs/remotes/origin/$branch" -- "$branch" >&2
stg uncommit --to "$merge_base" --exclusive >&2

# Open Herdr only after the stack is ready. Repeated reviews focus the workspace
# created under this Grove review name instead of adding duplicate workspaces.
herdr_workspaces=$(herdr workspace list)
herdr_workspace=$(jq -r --arg name "$workspace" \
    '[.result.workspaces[] | select(.label == $name) | .workspace_id][0] // empty' \
    <<< "$herdr_workspaces")
if [[ -n "$herdr_workspace" ]]; then
    herdr workspace focus "$herdr_workspace" >/dev/null
else
    herdr workspace create --cwd "$workspace_path" --label "$workspace" --focus >/dev/null
fi
printf 'Review ready: %s (%s PR commits)\n' "$checkout" "$count"
