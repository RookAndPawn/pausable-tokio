#!/usr/bin/env bash
# Apply the pausable-tokio patches to a checkout of upstream tokio.
#
# Usage:
#   ./apply.sh                  # apply 0001 + 0002 + 0003 (development state)
#   ./apply.sh --with-rename    # apply all 4 (one-shot publish from fresh checkout)
#   ./apply.sh --no-tests       # apply 0001 + 0002 only
#   ./apply.sh --rename-only    # apply 0004 only (use when 0001..0003 are
#                                 already applied; e.g. preparing to publish
#                                 from your existing development state)
#   ./apply.sh --check          # `git apply --check` mode: don't apply, verify only
#
# The default set is 0001 + 0002 + 0003. Add `--with-rename` to also rename
# the crate, or use `--rename-only` to apply just the rename on top of an
# already-patched tree.
#
# Run this from the root of the tokio checkout you want to patch (the
# directory that contains `tokio/` and `tests-integration/`).

set -euo pipefail

PATCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

include_runtime=1
include_deps=1
include_tests=1
include_rename=0
check_only=0

for arg in "$@"; do
    case "$arg" in
        --with-rename)  include_rename=1 ;;
        --no-tests)     include_tests=0 ;;
        --rename-only)
            include_runtime=0; include_deps=0; include_tests=0
            include_rename=1
            ;;
        --check)        check_only=1 ;;
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

if [[ ! -f "tokio/Cargo.toml" ]]; then
    echo "error: tokio/Cargo.toml not found." >&2
    echo "Run this script from the root of an upstream tokio checkout." >&2
    exit 1
fi

# Pick the apply tool.
if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    use_git=1
else
    use_git=0
fi

mode=apply
[[ "$check_only" -eq 1 ]] && mode=check

for p in "${patches[@]}"; do
    if [[ ! -f "$p" ]]; then
        echo "error: patch file not found: $p" >&2
        exit 1
    fi
    echo "[$mode] $(basename "$p")"
    if [[ "$use_git" -eq 1 ]]; then
        if [[ "$check_only" -eq 1 ]]; then
            git apply --check "$p"
        else
            git apply "$p"
        fi
    else
        if [[ "$check_only" -eq 1 ]]; then
            patch -p1 --dry-run -i "$p" >/dev/null
        else
            patch -p1 -i "$p" >/dev/null
        fi
    fi
done

if [[ "$check_only" -eq 1 ]]; then
    echo "all selected patches apply cleanly."
else
    echo "all selected patches applied."
    if [[ "$include_rename" -eq 1 ]]; then
        cat <<'EOF'

Rename applied. `cd tokio && cargo publish` is now the next step.
The renamed crate is detached from the parent workspace so cargo will
build it in isolation; tokio's other workspace members (benches,
tokio-util, etc.) will fall back to the published `tokio` from
crates.io if you build them after this point.
EOF
    fi
fi
