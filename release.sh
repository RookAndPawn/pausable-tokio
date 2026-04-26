#!/usr/bin/env bash
#
# release.sh - End-to-end pausable-tokio release flow.
#
# Usage:
#   ./release.sh <tokio-version>
#   ./release.sh <tokio-version> --yes        # skip confirmation prompt
#   ./release.sh <tokio-version> --no-publish # do everything except the
#                                               final `cargo publish`
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
# Workflow:
#   1. Dry-run-check that all four patches apply cleanly to the requested
#      upstream tokio ref.
#   2. Move the tokio-upstream submodule to that ref and apply the patches
#      (including the rename patch).
#   3. Commit the new submodule pointer in this parent repo.
#   4. Run `cargo publish --dry-run` from inside the submodule's tokio/
#      crate to verify packaging.
#   5. Prompt for confirmation (skipped with --yes; suppressed entirely
#      with --no-publish).
#   6. Run `cargo publish` for real.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
    sed -n '2,32p' "$0"
}

auto_yes=0
no_publish=0
tokio_version=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        -y|--yes)       auto_yes=1; shift ;;
        --no-publish)   no_publish=1; shift ;;
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
# Anything we add at this level should already be committed; warn the
# user so they don't accidentally commingle their own edits with the
# release commit.
# ------------------------------------------------------------------

dirty=0
git update-index --refresh -q >/dev/null || true
if ! git diff --quiet -- ':(exclude)tokio-upstream' \
   || ! git diff --cached --quiet -- ':(exclude)tokio-upstream'; then
    dirty=1
fi

if [[ "$dirty" -eq 1 ]]; then
    echo "warning: parent repo has uncommitted changes outside tokio-upstream/."
    echo "         The submodule-bump commit will not include them, but you"
    echo "         may want to commit/stash them before continuing."
    echo
fi

# ------------------------------------------------------------------
# Phase 1: dry-run check that every patch (including the rename) applies
# cleanly to the requested upstream ref. This *does* move the submodule
# to the new ref as a side effect (--tokio-version always resets and
# checks out). If patches don't apply, we abort here with a non-zero
# exit, leaving the submodule at the new ref so the user can inspect.
# ------------------------------------------------------------------

echo "==> 1/5  Verifying patches apply cleanly to tokio-$tokio_version"
./patches/apply.sh --tokio-version "$tokio_version" --with-rename --check

# ------------------------------------------------------------------
# Phase 2: actually apply (the --check above only verified hunks).
# We pass --tokio-version again so the submodule is reset to the
# pinned ref (the check left it patched? no, --check doesn't apply,
# but the previous --check call did move the ref). Either way, the
# resulting state is "submodule at <ref>, patches applied, rename
# applied".
# ------------------------------------------------------------------

echo
echo "==> 2/5  Applying patches"
./patches/apply.sh --tokio-version "$tokio_version" --with-rename

# Resolve the full tag/ref name the apply.sh just checked out, so the
# commit message matches what's actually pinned.
upstream_ref="$(cd tokio-upstream && git describe --tags HEAD 2>/dev/null \
    || git rev-parse --short HEAD)"

# Resolve the cargo package version that's about to be published. This
# is whatever upstream's tokio/Cargo.toml says, since the rename patch
# doesn't touch the version field.
package_version="$(grep -m1 -E '^version = "[^"]+"' tokio-upstream/tokio/Cargo.toml \
    | sed -E 's/version = "(.+)"/\1/')"

if [[ -z "$package_version" ]]; then
    echo "error: could not parse package version from tokio-upstream/tokio/Cargo.toml" >&2
    exit 1
fi

echo
echo "    upstream ref:    $upstream_ref"
echo "    crate name:      pausable-tokio"
echo "    crate version:   $package_version"

# ------------------------------------------------------------------
# Phase 3: commit the new submodule pointer in the parent repo.
# Skip if the pointer hasn't actually changed (idempotent re-runs).
# ------------------------------------------------------------------

echo
echo "==> 3/5  Recording submodule bump"

# Detect whether the submodule POINTER (not its working-tree contents)
# has moved relative to what HEAD records. `git submodule status`
# prefixes the line with '+' when the working-tree commit differs from
# the recorded commit, and ' ' when they match. We deliberately ignore
# uncommitted edits inside the submodule -- those are the patches we
# just applied and they live only in the published-crate output.
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
echo "==> 4/5  cargo publish --dry-run"
(cd tokio-upstream/tokio && cargo publish --dry-run --allow-dirty)

# ------------------------------------------------------------------
# Phase 5: confirmation + real publish.
# ------------------------------------------------------------------

echo

if [[ "$no_publish" -eq 1 ]]; then
    echo "==> 5/5  Skipping real publish (--no-publish)"
    echo
    echo "Everything ready. To publish for real, run from inside the submodule:"
    echo
    echo "    cd tokio-upstream/tokio && cargo publish --allow-dirty"
    exit 0
fi

if [[ "$auto_yes" -eq 0 ]]; then
    if [[ ! -t 0 ]]; then
        echo "error: --yes not given and stdin is not a terminal (cannot prompt)" >&2
        echo "       re-run with --yes for non-interactive use, or --no-publish" >&2
        echo "       to stop before the real publish step" >&2
        exit 1
    fi
    printf 'Publish pausable-tokio v%s to crates.io? [y/N] ' "$package_version"
    read -r confirm
    case "$confirm" in
        y|Y|yes|YES|Yes) ;;
        *)
            echo
            echo "aborted by user. The submodule bump commit (if any) is still in"
            echo "place; revert with 'git reset --hard HEAD~1' if you don't want it."
            exit 0
            ;;
    esac
fi

echo "==> 5/5  cargo publish (real)"
(cd tokio-upstream/tokio && cargo publish --allow-dirty)

echo
echo "released pausable-tokio v$package_version (from $upstream_ref)."
echo
echo "next steps:"
echo "  * tag this release in the parent repo, e.g."
echo "        git tag pausable-tokio-v$package_version"
echo "  * push the tag and the release commit:"
echo "        git push && git push --tags"
