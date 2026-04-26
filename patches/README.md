# pausable-tokio patches

A small, ordered set of patches that turn the
[`tokio-rs/tokio`](https://github.com/tokio-rs/tokio) submodule
checked out at `../tokio-upstream/` into the `pausable-tokio` fork.

The fork is maintained as patches (rather than as a long-lived rebase
or in-tree copy) so that re-syncing with new upstream tokio releases is
just "bump the submodule, see if patches still apply, fix what doesn't."

## What each patch does

| # | File | Touches | Purpose |
|---|------|---------|---------|
| 0001 | `0001-pausable-time-runtime.patch` | `tokio/src/**` | All Rust source changes: pausable-clock-backed `Clock`, `Builder::pausable_time`, `Runtime::pause` / `resume` / `wait_for_resume` / `run_unpausable` / etc., wrapping `task.run()` in both schedulers, and the `wait_for_resume` hook in the time driver's `park_thread_timeout`. |
| 0002 | `0002-pausable-time-deps.patch` | `tokio/Cargo.toml` | Adds the optional `pausable_clock = "1.0.2"` dependency and pulls it into the existing `time` feature. |
| 0003 | `0003-pausable-time-tests.patch` | `tests-integration/**` | Adds the `rt-time-pausable` cargo feature on `tests-integration` and the `tests/rt_pausable_time.rs` integration-test file (correctness tests + 6 stress tests). |
| 0004 | `0004-rename-for-crates-io.patch` | `tokio/Cargo.toml`, `Cargo.toml` (workspace) | Renames the crate from `tokio` to `pausable-tokio` and detaches it from the parent workspace so `cargo publish` works. Apply only at publish time. |

The patches have a logical dependency order:

```
0001 ──► 0002 ──► 0003 ──► 0004
                                    (each step depends on the previous)
```

`0001` references the `pausable_clock` crate, so the runtime won't
compile without `0002` also applied. `0003` consumes the public
`Runtime::pause` etc. API from `0001`. `0004` is independent of
everything but should be applied last (and only at publish time).

## Workflows

### One-time setup

```sh
# Already cloned without --recurse-submodules? Catch up:
git submodule update --init --recursive
```

### Syncing to a new upstream tokio release

The `--tokio-version` flag wraps the bump-and-apply round-trip into a
single command. It:

1. resets the submodule's working tree;
2. fetches new tags from the submodule's `origin` remote if needed;
3. checks out the requested tag (`1.53.0` is rewritten to
   `tokio-1.53.0`; full tags, branches, and shas pass through verbatim);
4. applies the requested patches on top.

```sh
# Move to a new upstream and dry-run-check the patches.
./patches/apply.sh --tokio-version 1.53.0 --check

# If they apply: actually apply them and verify the build/tests.
./patches/apply.sh --tokio-version 1.53.0
cd tokio-upstream
cargo build -p tokio --features=full
cargo test -p tests-integration --release \
    --features=rt-time-pausable --test rt_pausable_time \
    -- --test-threads=1 --nocapture
cd ..

# Commit the submodule pointer bump in this repo.
git add tokio-upstream
git commit -m "sync to tokio-1.53.0"
```

If you'd rather drive the submodule by hand, the equivalent of the
single command above is:

```sh
cd tokio-upstream
git fetch --tags
git checkout tokio-1.53.0
cd ..
./patches/apply.sh --check
```

If `--check` reports a patch that no longer applies, the fix workflow
is the standard one for unified-diff conflicts: edit the patch by hand
to match the new upstream context, or apply with `--reject` and merge
the rejected hunks manually. Once the patches are clean, regenerate
them as described under "Regenerating the patches" below and commit
the updated patches alongside the submodule bump.

### Publishing to crates.io

`crates.io` doesn't care about git history -- it packages the **current
file state** at publish time. So a patch-driven publish flow works
fine; you just need to apply patches first.

```sh
# Make sure the submodule is at its pinned tag with no leftover edits.
./patches/apply.sh --reset --with-rename

# Publish from inside the renamed crate. The rename patch detaches
# `tokio/` from the parent workspace, so cargo packages and verifies
# it standalone.
cd tokio-upstream/tokio
cargo publish --dry-run --allow-dirty   # sanity-check
cargo publish --allow-dirty             # for real
```

`--allow-dirty` is required because the patches are applied to the
submodule's working tree and not committed inside the submodule. (The
parent repo only tracks the submodule's pinned commit, not its
in-flight edits.) If you want a tidy git record you can `git commit`
inside the submodule before publishing and drop `--allow-dirty`;
either way the published artifact is byte-for-byte identical.

If you've already been doing development on the patched submodule
(0001-0003 applied, `cargo build`/`cargo test` runs in `target/`), just
add the rename on top:

```sh
./patches/apply.sh --rename-only
cd tokio-upstream/tokio
cargo publish --allow-dirty
```

This works because the rename patch simultaneously detaches `tokio/`
from the parent workspace, so prior workspace state doesn't trip up
`cargo publish`.

### Local development on the fork itself

For day-to-day development on the pausable code, apply only patches
`0001`, `0002`, and `0003`. Skip `0004`.

The rename patch is technically safe to apply during dev -- it
preserves a coherent workspace by detaching `tokio/` and dropping the
parent workspace's `tokio` references -- but tokio's own internal
tests (in `tokio/tests/`) reference items as `tokio::...` paths.
Renaming the crate to `pausable-tokio` would require either rewriting
all those test paths or running them with a `tokio = "pausable-tokio"`
rename in the dev-deps. Easier to just keep the original name during
dev.

## `apply.sh` reference

```text
./apply.sh                              # 0001 + 0002 + 0003 (dev state)
./apply.sh --with-rename                # 0001..0004 (one-shot publish)
./apply.sh --no-tests                   # 0001 + 0002 only
./apply.sh --rename-only                # 0004 only (when 0001..0003
                                          already applied)
./apply.sh --check                      # `git apply --check` mode
./apply.sh --reset                      # `git reset --hard` the
                                          submodule first
./apply.sh --tokio-version <ref>        # bump the submodule to <ref>
                                          (e.g. `1.53.0`, full tag,
                                          branch, or sha) and apply
                                          patches on top. Implies
                                          --reset.
```

Combine flags freely:

* `./apply.sh --reset --with-rename` reapplies the entire fork from
  the currently-pinned upstream, ready to publish.
* `./apply.sh --tokio-version 1.53.0 --check` does a "would the
  patches still apply if we bumped to 1.53.0?" probe. The submodule
  is moved to the new tag whether the check succeeds or fails, so
  inspect `git status` from the parent repo afterwards if you want
  to commit (or roll back) the bump.

## Regenerating the patches

If you've made local changes inside the `tokio-upstream` submodule and
want the patches to reflect the new state, regenerate them with:

```sh
cd tokio-upstream
# Patches are split by directory; regenerate them in matching order.
git diff HEAD -- 'tokio/src/'         > ../patches/0001-pausable-time-runtime.patch
git diff HEAD -- 'tokio/Cargo.toml'   > ../patches/0002-pausable-time-deps.patch
git diff HEAD -- 'tests-integration/' > ../patches/0003-pausable-time-tests.patch
# 0004 is hand-maintained; only edit it if the rename target name changes.
cd ..
git add patches
git commit -m "regenerate patches"
```

The submodule's pinned commit is the diff base -- so as long as
`tokio-upstream/.git/HEAD` is your latest commit on top of that pinned
commit, the `git diff HEAD` invocations above produce a clean
"diff against the upstream tag with all my fork changes" patch set.

## Verifying the patches apply to a clean upstream

```sh
# Reset the submodule to its pinned tag, then dry-run-apply.
./patches/apply.sh --reset --check
```

If you've changed the pinned tag (e.g., during a sync) and want to be
sure all four patches still apply, also run:

```sh
./patches/apply.sh --reset --with-rename --check
```
