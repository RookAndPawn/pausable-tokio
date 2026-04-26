#!/usr/bin/env bash
# Apply the pausable-tokio patches to the upstream tokio submodule.
#
# Usage:
#   ./apply.sh                              # apply 0001 + 0002 + 0003
#   ./apply.sh --with-rename                # apply all six (one-shot publish flow:
#                                             0001 + 0002 + 0003 + 0004 + 0005 + 0006)
#   ./apply.sh --no-tests                   # apply 0001 + 0002 only
#   ./apply.sh --rename-only                # apply 0004 + 0005 + 0006 (publish-time
#                                             metadata) on top of an already-patched
#                                             submodule
#   ./apply.sh --check                      # dry-run: verify hunks would land
#   ./apply.sh --reset                      # reset the submodule to its
#                                             current HEAD before applying
#   ./apply.sh --tokio-version 1.53.0       # bump the submodule to a different
#                                             upstream tokio release tag, then
#                                             apply patches on top. Implies
#                                             --reset.
#
# Flags compose: e.g. `./apply.sh --tokio-version 1.53.0 --check` will move
# the submodule to tokio-1.53.0 and then dry-run-check the patches against
# that new base, leaving the submodule at the new commit so you can `git
# add tokio-upstream` if you want to record the bump.
#
# `--tokio-version` accepts:
#   * a semver-looking string ("1.53.0", "1.52.0-rc.1") which is rewritten
#     to the corresponding upstream tag ("tokio-1.53.0", "tokio-1.52.0-rc.1").
#   * a fully-qualified tag ("tokio-1.53.0").
#   * any other git ref the submodule's remote has: a branch ("master"),
#     short sha ("abc1234"), or full commit hash. We never normalize these.
#
# The patches assume upstream's directory layout (`tokio/`, `tests-integration/`,
# top-level `Cargo.toml`). If a future upstream rearranges those, you'll
# get hunk-rejection errors that need to be fixed in the patch files.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PATCH_DIR="$REPO_DIR/patches"
SUBMODULE_DIR="$REPO_DIR/tokio-upstream"

include_runtime=1
include_deps=1
include_tests=1
include_rename=0
include_publish_meta=0  # patches 0005 + 0006: README + Cargo.toml + lib.rs
check_only=0
reset_first=0
tokio_version=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --with-rename)
            include_rename=1
            include_publish_meta=1
            shift
            ;;
        --no-tests)     include_tests=0; shift ;;
        --rename-only)
            include_runtime=0; include_deps=0; include_tests=0
            include_rename=1
            include_publish_meta=1
            shift
            ;;
        --check)        check_only=1; shift ;;
        --reset)        reset_first=1; shift ;;
        --tokio-version)
            if [[ -z "${2:-}" ]]; then
                echo "error: --tokio-version requires an argument" >&2
                exit 2
            fi
            tokio_version="$2"
            reset_first=1
            shift 2
            ;;
        --tokio-version=*)
            tokio_version="${1#*=}"
            if [[ -z "$tokio_version" ]]; then
                echo "error: --tokio-version requires an argument" >&2
                exit 2
            fi
            reset_first=1
            shift
            ;;
        -h|--help)
            sed -n '2,30p' "$0"
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

if [[ ! -d "$SUBMODULE_DIR/.git" && ! -f "$SUBMODULE_DIR/.git" ]]; then
    cat <<EOF >&2
error: tokio-upstream/ submodule is not initialized.

Run from the repo root:

    git submodule update --init --recursive

EOF
    exit 1
fi

# Resolve `--tokio-version` (if given) into a concrete git ref to check out
# inside the submodule.
checkout_ref=""
if [[ -n "$tokio_version" ]]; then
    case "$tokio_version" in
        # "1.53.0" / "1.52.0-rc.1" / "1.0" ... -> rewrite to the upstream
        # tag naming convention.
        [0-9]*) checkout_ref="tokio-$tokio_version" ;;
        # Anything else (already a tag, a branch, a sha) is passed through.
        *)      checkout_ref="$tokio_version" ;;
    esac

    echo "[version] switching tokio-upstream to '$checkout_ref'"
    (
        cd "$SUBMODULE_DIR"
        # Discard any in-flight patches so the checkout doesn't fight them.
        git reset --hard --quiet HEAD
        git clean -fdq
        # Make sure the requested ref is in our local object store. Tags
        # are not always pulled by default; fetch explicitly.
        if ! git rev-parse --verify --quiet "$checkout_ref^{commit}" >/dev/null; then
            echo "[version] '$checkout_ref' not in local refs; fetching" >&2
            git fetch --tags --quiet origin
        fi
        if ! git rev-parse --verify --quiet "$checkout_ref^{commit}" >/dev/null; then
            echo "error: '$checkout_ref' is not a valid ref in tokio-upstream's remote" >&2
            echo "       (input was --tokio-version '$tokio_version')" >&2
            exit 1
        fi
        git checkout --quiet --detach "$checkout_ref"
    )
fi

if [[ ! -f "$SUBMODULE_DIR/tokio/Cargo.toml" ]]; then
    echo "error: $SUBMODULE_DIR/tokio/Cargo.toml not found." >&2
    echo "Submodule appears uninitialized or the upstream layout has changed." >&2
    exit 1
fi

# Optionally reset the submodule to its current HEAD before applying. This
# discards any uncommitted changes inside the submodule. If `--tokio-version`
# was specified, that path already reset the working tree, so this becomes
# a no-op in practice.
if [[ "$reset_first" -eq 1 && -z "$tokio_version" ]]; then
    echo "[reset] resetting tokio-upstream working tree"
    (cd "$SUBMODULE_DIR" && git reset --hard --quiet HEAD && git clean -fdq)
fi

# Build the ordered list of patches to apply.
patches=()
[[ "$include_runtime" -eq 1 ]] && patches+=("$PATCH_DIR/0001-pausable-time-runtime.patch")
[[ "$include_deps"    -eq 1 ]] && patches+=("$PATCH_DIR/0002-pausable-time-deps.patch")
[[ "$include_tests"   -eq 1 ]] && patches+=("$PATCH_DIR/0003-pausable-time-tests.patch")
[[ "$include_rename"  -eq 1 ]] && patches+=("$PATCH_DIR/0004-rename-for-crates-io.patch")
if [[ "$include_publish_meta" -eq 1 ]]; then
    patches+=("$PATCH_DIR/0005-publish-readme.patch")
    patches+=("$PATCH_DIR/0006-publish-cargo-metadata-and-lib-rs-note.patch")
fi

if [[ ${#patches[@]} -eq 0 ]]; then
    echo "error: nothing to apply (all patch groups are disabled)" >&2
    exit 2
fi

mode=apply
[[ "$check_only" -eq 1 ]] && mode=check

cd "$SUBMODULE_DIR"
for p in "${patches[@]}"; do
    if [[ ! -f "$p" ]]; then
        echo "error: patch file not found: $p" >&2
        exit 1
    fi
    echo "[$mode] $(basename "$p")"
    if [[ "$check_only" -eq 1 ]]; then
        git apply --check "$p"
    else
        git apply "$p"
    fi
done

if [[ "$check_only" -eq 1 ]]; then
    echo "all selected patches apply cleanly to $(basename "$SUBMODULE_DIR")."
else
    echo "all selected patches applied to $(basename "$SUBMODULE_DIR")."
    if [[ "$include_rename" -eq 1 ]]; then
        cat <<'EOF'

Rename applied. `cd tokio-upstream/tokio && cargo publish` is now the
next step. The renamed crate is detached from the parent workspace so
cargo will build and verify it in isolation; tokio's other workspace
members (benches, tokio-util, etc.) will fall back to the published
`tokio` from crates.io if you build them after this point.
EOF
    fi
fi

if [[ -n "$tokio_version" ]]; then
    cat <<EOF

Tokio submodule is now at '$checkout_ref'. To make the new pinned
version part of this repo, commit the submodule pointer from the parent:

    git add tokio-upstream
    git commit -m "sync to $checkout_ref"
EOF
fi
