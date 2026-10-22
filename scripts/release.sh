#!/bin/sh
# Prepares the release X.Y.Z on main: bumps the version in Cargo.toml and
# Cargo.lock, adds a section to CHANGELOG.md and lets you edit it, runs the
# tests, then commits and tags. Pushing the tag, which makes the CI publish
# the release, is left to you. See RELEASING.md.
#
# Usage: scripts/release.sh X.Y.Z

set -eu

die() {
    echo "$0: $*" >&2
    exit 1
}

[ $# -eq 1 ] || die "usage: $0 X.Y.Z"
version=$1
tag=v$version
echo "$version" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+' || die "not a version: $version"

cd "$(git rev-parse --show-toplevel)"

[ "$(git branch --show-current)" = main ] || die "not on main"
git diff --quiet HEAD -- || die "uncommitted changes"
! git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "$tag exists already"
git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "main differs from origin/main"

sed -i "/^\[workspace\.package\]/,/^\[/ s/^version = \".*\"/version = \"$version\"/" Cargo.toml
grep -qx "version = \"$version\"" Cargo.toml || die "could not set the version in Cargo.toml"
cargo update -q --workspace

if git describe --tags --abbrev=0 --match 'v[0-9]*' >/dev/null 2>&1; then
    git cliff --unreleased --tag "$tag" --prepend CHANGELOG.md
else
    git cliff --tag "$tag" -o CHANGELOG.md
fi

echo "Edit the changelog section of $tag; it becomes the release notes."
${VISUAL:-${EDITOR:-vi}} CHANGELOG.md

cargo test -q --workspace --locked

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -q -m "Release $tag"
git tag -a "$tag" -m "Release $tag"

echo "Tagged $tag. Look it over (git show $tag), then publish it with"
echo "  git push --atomic origin main $tag"
