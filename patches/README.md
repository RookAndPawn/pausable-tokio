# pausable-tokio patches

A small, ordered set of patches that turn upstream
[`tokio-rs/tokio`](https://github.com/tokio-rs/tokio) into the
`pausable-tokio` fork. The patches are kept in this directory so the
fork can be re-synced with new upstream tokio releases without doing a
full rebase.

## What each patch does

| # | File | Touches | Purpose |
|---|------|---------|---------|
| 0001 | `0001-pausable-time-runtime.patch` | `tokio/src/**` | All Rust source changes: pausable-clock-backed `Clock`, `Builder::pausable_time`, `Runtime::pause` / `resume` / `wait_for_resume` / `run_unpausable` / etc., wrapping `task.run()` in both schedulers, and the `wait_for_resume` hook in the time driver's `park_thread_timeout`. |
| 0002 | `0002-pausable-time-deps.patch` | `tokio/Cargo.toml` | Adds the optional `pausable_clock = "1.0.2"` dependency and pulls it into the existing `time` feature. |
| 0003 | `0003-pausable-time-tests.patch` | `tests-integration/**` | Adds the `rt-time-pausable` cargo feature on `tests-integration` and the `tests/rt_pausable_time.rs` integration-test file (correctness tests + 6 stress tests). |
| 0004 | `0004-rename-for-crates-io.patch` | `tokio/Cargo.toml`, `Cargo.toml` (workspace) | Renames the crate from `tokio` to `pausable-tokio` and detaches it from the parent workspace (adds an empty `[workspace]` to `tokio/Cargo.toml`, drops the `tokio` member entry and the dangling `[patch.crates-io]` line, and removes the `[lints] workspace = true` block that's no longer applicable). Apply this **only** at publish time. |

Each patch is independent of the next at the *file* level, but they
have a logical dependency order:

```
0001 ──► 0002 ──► 0003 ──► 0004
                                    (each step depends on the previous)
```

`0001` references the `pausable_clock` crate, so the runtime won't
compile without `0002` also applied. `0003` consumes the public
`Runtime::pause` etc. API from `0001`. `0004` is independent of
everything but should be applied last (see why below).

## Workflows

### Syncing with a new upstream tokio release

```sh
# 1. Start with a clean checkout of the new upstream tokio tag.
git clone https://github.com/tokio-rs/tokio.git tokio-1.53.0
cd tokio-1.53.0
git checkout tokio-1.53.0

# 2. Apply the runtime + deps + tests patches.
git apply /path/to/patches/0001-pausable-time-runtime.patch
git apply /path/to/patches/0002-pausable-time-deps.patch
git apply /path/to/patches/0003-pausable-time-tests.patch

# 3. Build and run the existing tokio test suite to confirm the
#    upstream changes haven't broken our pausable code paths.
cargo build -p tokio --features=full

# 4. Run the pausable-specific integration tests.
cargo test -p tests-integration --release \
    --features=rt-time-pausable --test rt_pausable_time \
    -- --test-threads=1 --nocapture
```

If a patch fails to apply because upstream has rewritten one of the
hunks, the patch can usually be updated by hand against the new
upstream and committed back to this directory.

### Publishing to crates.io

`crates.io` doesn't care about git history — it packages the **current
file state** at publish time. So a patch-based publish flow works
fine; you just need to apply patches first.

```sh
# (from a fresh checkout already on the desired tokio tag)
./apply.sh --with-rename

# Publish from inside the renamed crate. The rename patch detaches
# `tokio/` from the parent workspace, so cargo packages and verifies
# it standalone.
cd tokio
cargo publish --dry-run --allow-dirty   # sanity-check
cargo publish --allow-dirty             # for real
```

`--allow-dirty` is required because the patches are not committed; if
you'd rather have a tidy git record, `git add -A && git commit -m
'pausable-tokio v1.x.y'` between the apply step and `cargo publish` and
drop `--allow-dirty`. Either way the published artifact is identical.

If you've already been doing development on the patched tree (i.e.,
patches 0001-0003 are applied and you've run `cargo build`/`cargo test`
in the workspace), you can apply just the rename on top:

```sh
./apply.sh --rename-only
cd tokio && cargo publish --allow-dirty
```

This works because the rename patch simultaneously detaches `tokio/`
from the parent workspace, so prior workspace state doesn't trip up
`cargo publish`.

### Local development on the fork itself

For day-to-day development on the pausable code (running tokio's own
tests under the `tokio::` namespace, hacking on the implementation),
apply only patches `0001`, `0002`, and `0003`. Skip `0004`.

The rename patch is technically safe to apply during dev — it
preserves a coherent workspace by detaching `tokio/` and dropping the
parent workspace's `tokio` references — but tokio's own internal tests
(in `tokio/tests/`) reference items as `tokio::...` paths. Renaming
the crate to `pausable-tokio` would require either rewriting all those
test paths or running them with a `tokio = "pausable-tokio"` rename
in the dev-deps. Easier to just keep the original name during dev.

## Verifying the patches

The patches in this directory were generated with `git diff` against
the `tokio-1.52.1` upstream tag. To verify they apply cleanly to that
tag at any time:

```sh
mkdir -p /tmp/verify && cd /tmp/verify
git clone https://github.com/tokio-rs/tokio.git . 2>/dev/null || \
    (cd /tmp/verify && git fetch https://github.com/tokio-rs/tokio.git tokio-1.52.1)
git checkout tokio-1.52.1

for p in /path/to/patches/0001-*.patch \
         /path/to/patches/0002-*.patch \
         /path/to/patches/0003-*.patch \
         /path/to/patches/0004-*.patch; do
    git apply --check "$p" || { echo "patch failed: $p"; exit 1; }
    git apply "$p"
done
```

`apply.sh` in this directory automates the apply step.

## Regenerating the patches

If you make local changes to the `pausable-tokio` fork's master branch
and want the patches to reflect the new state, regenerate them with:

```sh
# From the fork's repo root, with master pointing at the new state and
# the upstream tokio-1.52.1 tag still present:
git diff tokio-1.52.1..HEAD -- 'tokio/src/'           > patches/0001-pausable-time-runtime.patch
git diff tokio-1.52.1..HEAD -- 'tokio/Cargo.toml'     > patches/0002-pausable-time-deps.patch
git diff tokio-1.52.1..HEAD -- 'tests-integration/'   > patches/0003-pausable-time-tests.patch
# 0004 is hand-maintained; only edit it if the rename target name changes.
```

Bumping the upstream base after a sync is the same: re-tag `master`'s
upstream-base point and run the four `git diff` commands against that
new tag.
