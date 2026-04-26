#!/usr/bin/env bash
#
# release.sh - End-to-end pausable-tokio release flow.
#
# Usage:
#   ./release.sh <tokio-version>
#   ./release.sh <tokio-version> --yes        # skip confirmation prompt
#   ./release.sh <tokio-version> --no-publish # do everything except the
#                                               final `cargo publish`
#                                               (and skip tag/push too)
#   ./release.sh <tokio-version> --no-push    # publish to crates.io but
#                                               don't push the commit/tag
#                                               to GitHub
#
# Examples:
#   ./release.sh 1.52.1
#   ./release.sh 1.53.0 --yes
#   ./release.sh tokio-1.53.0 --no-publish    # full dry-run
#
# `<tokio-version>` is forwarded to `patches/apply.sh --tokio-version`.
# It accepts the same forms ("1.53.0", "tokio-1.53.0", a branch, a sha).
#
# The published crate's version is whatever upstream sets in
# tokio/Cargo.toml at the chosen ref (the rename patch only changes the
# name, not the version), i.e. publishing for tokio-1.53.0 yields
# `pausable-tokio v1.53.0`.
#
# Workflow (8 phases):
#   1. Dry-run-check that all six patches apply cleanly to the requested
#      upstream tokio ref.
#   2. Move the tokio-upstream submodule to that ref and apply the patches
#      (including the rename + publish-metadata patches).
#   3. Commit the new submodule pointer in this parent repo.
#   4. Run `cargo publish --dry-run` from inside the submodule's tokio/
#      crate to verify packaging.
#   5. Prompt for confirmation (skipped with --yes; suppressed entirely
#      with --no-publish).
#   6. Run `cargo publish` for real.
#   7. Reset the submodule's working tree, discarding the now-published
#      patch artifacts so the local checkout returns to a clean upstream
#      state.
#   8. Tag the release commit `pausable-tokio-v<version>` and push the
#      commit + tag to origin (skipped with --no-push or --no-publish).

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
    sed -n '2,42p' "$0"
}

auto_yes=0
no_publish=0
no_push=0
tokio_version=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        -y|--yes)       auto_yes=1; shift ;;
        --no-publish)   no_publish=1; shift ;;
        --no-push)      no_push=1; shift ;;
        -h|--help)      usage; exit 0 ;;
        --)             shift; break ;;
        -*)
            echo "unknown flag: $1" >&2
            usage >&2
            exit 2
            ;;
        *)
            if [[ -n "$tokio_version" ]]; then
                echo "error: multiple positional arguments given" >&2
                exit 2
            fi
            tokio_version="$1"
            shift
            ;;
    esac
done

if [[ -z "$tokio_version" ]]; then
    echo "error: tokio version is required as the first positional argument" >&2
    usage >&2
    exit 2
fi

cd "$REPO_DIR"

# ------------------------------------------------------------------
# Pre-flight: warn (but don't block) on a dirty parent-repo work tree.
# ------------------------------------------------------------------

git update-index --refresh -q >/dev/null || true
if ! git diff --quiet -- ':(exclude)tokio-upstream' \
   || ! git diff --cached --quiet -- ':(exclude)tokio-upstream'; then
    echo "warning: parent repo has uncommitted changes outside tokio-upstream/."
    echo "         The submodule-bump commit will not include them, but you"
    echo "         may want to commit/stash them before continuing."
    echo
fi

# ------------------------------------------------------------------
# Phase 1: dry-run check that every patch applies cleanly. This *does*
# move the submodule to the requested ref as a side effect (--tokio-
# version always resets and checks out). If patches don't apply, we
# abort here, leaving the submodule at the new ref so the user can
# inspect.
# ------------------------------------------------------------------

echo "==> 1/8  Verifying patches apply cleanly to tokio-$tokio_version"
./patches/apply.sh --tokio-version "$tokio_version" --with-rename --check

# ------------------------------------------------------------------
# Phase 2: actually apply.
# ------------------------------------------------------------------

echo
echo "==> 2/8  Applying patches"
./patches/apply.sh --tokio-version "$tokio_version" --with-rename

upstream_ref="$(cd tokio-upstream && git describe --tags HEAD 2>/dev/null \
    || git rev-parse --short HEAD)"

# The rename patch detaches the crate from the parent workspace, so
# `tokio-upstream/tokio/Cargo.toml`'s [package] table is in a state
# where the version field is the only `version = "..."` near the top.
package_version="$(grep -m1 -E '^version = "[^"]+"' tokio-upstream/tokio/Cargo.toml \
    | sed -E 's/version = "(.+)"/\1/')"

if [[ -z "$package_version" ]]; then
    echo "error: could not parse package version from tokio-upstream/tokio/Cargo.toml" >&2
    exit 1
fi

release_tag="pausable-tokio-v$package_version"

echo
echo "    upstream ref:    $upstream_ref"
echo "    crate name:      pausable-tokio"
echo "    crate version:   $package_version"
echo "    release tag:     $release_tag"

# ------------------------------------------------------------------
# Phase 3: commit the new submodule pointer in the parent repo.
# Skip if the pointer hasn't actually changed (idempotent re-runs).
# ------------------------------------------------------------------

echo
echo "==> 3/8  Recording submodule bump"

sm_marker="$(git submodule status -- tokio-upstream | cut -c1)"
case "$sm_marker" in
    '+')
        git add tokio-upstream
        git commit -m "release: pausable-tokio v$package_version (from $upstream_ref)"
        echo "    committed: $(git log -1 --oneline)"
        ;;
    ' ')
        echo "    submodule pointer unchanged; no commit created"
        ;;
    *)
        echo "error: unexpected submodule status '$sm_marker' for tokio-upstream" >&2
        exit 1
        ;;
esac

# ------------------------------------------------------------------
# Phase 4: cargo publish --dry-run.
# ------------------------------------------------------------------

echo
echo "==> 4/8  cargo publish --dry-run"
# We pass the same RUSTFLAGS / RUSTDOCFLAGS that tokio's docs.rs build
# uses (see tokio/docs/contributing/pull-requests.md). The
# `tokio_unstable` cfg ensures the cargo-publish verification build
# exercises unstable code paths, and `docsrs` keeps any
# `#[cfg(docsrs)]`-gated items in scope. Both flags are harmless when
# the relevant code paths aren't present, but matching upstream's
# documented setup means we catch issues that docs.rs itself would
# otherwise surface only after the crate is already on crates.io.
(
    cd tokio-upstream/tokio
    RUSTFLAGS="--cfg docsrs --cfg tokio_unstable" \
    RUSTDOCFLAGS="--cfg docsrs --cfg tokio_unstable" \
        cargo publish --dry-run --allow-dirty --all-features
)

# ------------------------------------------------------------------
# Phase 5: confirmation.
# ------------------------------------------------------------------

echo

if [[ "$no_publish" -eq 1 ]]; then
    echo "==> 5/8  Skipping real publish (--no-publish)"
    echo
    echo "Phases 5/8 (publish), 6/8 (clean up), 7/8 (tag), and 8/8 (push)"
    echo "all skipped. The submodule bump commit (if any) is still in"
    echo "this repo's HEAD; revert with 'git reset --hard HEAD~1' if you"
    echo "don't want it."
    exit 0
fi

if [[ "$auto_yes" -eq 0 ]]; then
    if [[ ! -t 0 ]]; then
        echo "error: --yes not given and stdin is not a terminal (cannot prompt)" >&2
        echo "       re-run with --yes for non-interactive use, or --no-publish" >&2
        echo "       to stop before the real publish step" >&2
        exit 1
    fi
    printf 'Publish pausable-tokio v%s to crates.io and push tag %s? [y/N] ' \
        "$package_version" "$release_tag"
    read -r confirm
    case "$confirm" in
        y|Y|yes|YES|Yes) ;;
        *)
            echo
            echo "aborted by user. The submodule bump commit (if any) is still"
            echo "in place locally; revert with 'git reset --hard HEAD~1' if"
            echo "you don't want it. The crate has NOT been published and the"
            echo "tag has NOT been created."
            exit 0
            ;;
    esac
fi

echo
echo "==> 5/8  cargo publish (real)"
(
    cd tokio-upstream/tokio
    RUSTFLAGS="--cfg docsrs --cfg tokio_unstable" \
    RUSTDOCFLAGS="--cfg docsrs --cfg tokio_unstable" \
        cargo publish --allow-dirty --all-features
)

# ------------------------------------------------------------------
# Phase 6: reset the submodule's working tree.
# After cargo publish, the submodule's tokio/ has all the patches
# splattered across its working tree. Now that we've packaged it,
# get back to a clean upstream-tag state so subsequent operations
# (rebuilds, future runs, normal `cd tokio-upstream && git status`)
# don't show stale changes.
# ------------------------------------------------------------------

echo
echo "==> 6/8  Resetting tokio-upstream working tree"
(
    cd tokio-upstream
    git reset --hard --quiet HEAD
    git clean -fdq
)
echo "    submodule clean at $(cd tokio-upstream && git describe --tags HEAD)"

# ------------------------------------------------------------------
# Phase 7: tag the release commit.
# We always tag the current HEAD, which is either the brand-new
# release commit from phase 3 or the previously-committed bump.
# Re-running for the same version errors out the second time because
# the tag already exists; we treat that as user-fixable rather than
# silently overwriting.
# ------------------------------------------------------------------

echo
echo "==> 7/8  Tagging $release_tag"
if git rev-parse --quiet --verify "refs/tags/$release_tag" >/dev/null; then
    echo "    tag $release_tag already exists; leaving it alone"
else
    git tag -a "$release_tag" \
        -m "pausable-tokio v$package_version (from $upstream_ref)"
    echo "    created tag: $release_tag -> $(git rev-parse --short HEAD)"
fi

# ------------------------------------------------------------------
# Phase 8: push commit + tag to origin.
# ------------------------------------------------------------------

echo
if [[ "$no_push" -eq 1 ]]; then
    echo "==> 8/8  Skipping push (--no-push)"
    echo
    echo "To push manually:"
    echo "    git push && git push origin $release_tag"
    exit 0
fi

echo "==> 8/8  Pushing commit + tag to origin"
current_branch="$(git symbolic-ref --short HEAD)"
git push origin "$current_branch"
git push origin "$release_tag"

echo
echo "released pausable-tokio v$package_version (from $upstream_ref)."
echo "    crates.io:  https://crates.io/crates/pausable-tokio/$package_version"
echo "    docs.rs:    https://docs.rs/pausable-tokio/$package_version"
echo "    git tag:    $release_tag (pushed to origin)"
