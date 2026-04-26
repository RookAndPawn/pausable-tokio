#!/usr/bin/env bash
# Apply the pausable-tokio patches to the upstream tokio submodule.
#
# Usage:
#   ./apply.sh                  # apply 0001 + 0002 + 0003 (development state)
#   ./apply.sh --with-rename    # apply all 4 (one-shot publish from a clean submodule)
#   ./apply.sh --no-tests       # apply 0001 + 0002 only
#   ./apply.sh --rename-only    # apply 0004 only (use when 0001..0003 are
#                                 already applied; e.g. preparing to publish
#                                 from your existing development state)
#   ./apply.sh --check          # `git apply --check` mode: don't apply, verify only
#   ./apply.sh --reset          # reset the submodule to its pinned tag, then
#                                 apply the requested patches on top
#
# The default set is 0001 + 0002 + 0003. Add `--with-rename` to also rename
# the crate, or `--rename-only` to apply just the rename on top of an
# already-patched submodule.
#
# All patches are applied inside `tokio-upstream/`, the upstream tokio
# submodule. Run this script from the repo root (or anywhere; it auto-
# detects its own location).

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PATCH_DIR="$REPO_DIR/patches"
SUBMODULE_DIR="$REPO_DIR/tokio-upstream"

include_runtime=1
include_deps=1
include_tests=1
include_rename=0
check_only=0
reset_first=0

for arg in "$@"; do
    case "$arg" in
        --with-rename)  include_rename=1 ;;
        --no-tests)     include_tests=0 ;;
        --rename-only)
            include_runtime=0; include_deps=0; include_tests=0
            include_rename=1
            ;;
        --check)        check_only=1 ;;
        --reset)        reset_first=1 ;;
        -h|--help)
            sed -n '2,18p' "$0"
            exit 0
            ;;
        *)
            echo "unknown argument: $arg" >&2
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

if [[ ! -f "$SUBMODULE_DIR/tokio/Cargo.toml" ]]; then
    echo "error: $SUBMODULE_DIR/tokio/Cargo.toml not found." >&2
    echo "Submodule appears uninitialized or the upstream layout has changed." >&2
    exit 1
fi

# Optionally reset the submodule to its pinned commit before applying.
# This discards any uncommitted changes inside the submodule.
if [[ "$reset_first" -eq 1 ]]; then
    echo "[reset] resetting tokio-upstream to its pinned commit"
    (cd "$SUBMODULE_DIR" && git reset --hard --quiet HEAD && git clean -fdq)
fi

# Build the ordered list of patches to apply.
patches=()
[[ "$include_runtime" -eq 1 ]] && patches+=("$PATCH_DIR/0001-pausable-time-runtime.patch")
[[ "$include_deps"    -eq 1 ]] && patches+=("$PATCH_DIR/0002-pausable-time-deps.patch")
[[ "$include_tests"   -eq 1 ]] && patches+=("$PATCH_DIR/0003-pausable-time-tests.patch")
[[ "$include_rename"  -eq 1 ]] && patches+=("$PATCH_DIR/0004-rename-for-crates-io.patch")

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
