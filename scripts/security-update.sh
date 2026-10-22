#!/bin/sh
# Checks Cargo.lock for crates with known vulnerabilities (the RustSec
# advisory database, through cargo-audit) and updates them to fixed versions
# where Cargo.toml allows it, rust-version included. The updates are pushed
# to the branch $BRANCH and proposed in a merge request to the default
# branch, which is created or, when it is open already, updated.
#
# Runs in a scheduled CI pipeline (see .gitlab-ci.yml) and needs
# SECURITY_UPDATE_TOKEN, a token with the api and write_repository scopes.
# Fails when vulnerabilities remain that no update fixes, so that the
# pipeline shows it; ignore an advisory that does not apply in
# .cargo/audit.toml.
#
# Usage: scripts/security-update.sh

set -eu

BRANCH=security-updates

die() {
    echo "$0: $*" >&2
    exit 1
}

# The vulnerable crates as "name@version advisory: title", one per line.
vulnerabilities() {
    # cargo audit exits with 1 when it finds something, which is no error here.
    cargo audit --json >"$tmp/audit.json" || true
    jq -r '.vulnerabilities.list[]
        | "\(.package.name)@\(.package.version) \(.advisory.id): \(.advisory.title)"' "$tmp/audit.json" \
        || die "cannot read the output of cargo audit"
}

cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

before=$(vulnerabilities)
if [ -z "$before" ]; then
    echo "No known vulnerabilities."
    exit 0
fi
echo "Vulnerable crates:"
echo "$before"

for package in $(echo "$before" | cut -d' ' -f1 | sort -u); do
    cargo update -p "$package"
done

after=$(vulnerabilities)
if [ -n "$after" ]; then
    fixed=$(echo "$before" | grep -vxF "$after" || true)
else
    fixed=$before
fi

if [ -n "$fixed" ]; then
    [ -n "${SECURITY_UPDATE_TOKEN:-}" ] || die "SECURITY_UPDATE_TOKEN is not set"

    title="Fix security vulnerabilities in dependencies"
    {
        echo "Updates the crates with these advisories:"
        echo
        echo "$fixed" | sed 's/^/- /'
        if [ -n "$after" ]; then
            echo
            echo "These remain, no version allowed by Cargo.toml (including its"
            echo "rust-version) fixes them:"
            echo
            echo "$after" | sed 's/^/- /'
        fi
    } >"$tmp/description.md"

    git config user.name "Security updates"
    git config user.email "noreply@$CI_SERVER_HOST"
    git checkout -q -B "$BRANCH"
    git commit -q -F - Cargo.lock <<EOF
$title

$(cat "$tmp/description.md")
EOF

    remote="https://security-update:$SECURITY_UPDATE_TOKEN@$CI_SERVER_HOST/$CI_PROJECT_PATH.git"
    # Pushing the same updates again would only run another pipeline.
    if git fetch -q "$remote" "$BRANCH" 2>/dev/null && git diff --quiet FETCH_HEAD HEAD; then
        echo "$BRANCH has these updates already."
    else
        git push -q -f "$remote" "HEAD:refs/heads/$BRANCH"
    fi

    api="$CI_API_V4_URL/projects/$CI_PROJECT_ID/merge_requests"
    iid=$(curl -sS --fail-with-body --header "PRIVATE-TOKEN: $SECURITY_UPDATE_TOKEN" \
        "$api?state=opened&source_branch=$BRANCH" | jq -r '.[0].iid // empty')
    if [ -n "$iid" ]; then
        curl -sS --fail-with-body -o /dev/null --header "PRIVATE-TOKEN: $SECURITY_UPDATE_TOKEN" \
            -X PUT --data-urlencode "description@$tmp/description.md" "$api/$iid"
        echo "Updated merge request !$iid."
    else
        curl -sS --fail-with-body -o /dev/null --header "PRIVATE-TOKEN: $SECURITY_UPDATE_TOKEN" \
            --data-urlencode "source_branch=$BRANCH" \
            --data-urlencode "target_branch=$CI_DEFAULT_BRANCH" \
            --data-urlencode "title=$title" \
            --data-urlencode "description@$tmp/description.md" \
            --data-urlencode "remove_source_branch=true" \
            "$api"
        echo "Created a merge request from $BRANCH."
    fi
fi

if [ -n "$after" ]; then
    echo "No version allowed by Cargo.toml (including its rust-version) fixes these:" >&2
    echo "$after" >&2
    exit 1
fi
