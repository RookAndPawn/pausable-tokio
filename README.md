# pausable-tokio

A fork of [`tokio`](https://github.com/tokio-rs/tokio) that exposes the
runtime's clock as a real, runtime-controllable pause/resume primitive.
Useful when you want to suspend an entire async runtime (timers, sleeps,
intervals, etc.) from the outside without rebuilding the world around
`#[cfg(test)]`.

This repository contains **only** the patches that turn upstream tokio
into pausable-tokio, plus a vendored copy of upstream as a git
submodule. There is no in-tree fork of the tokio source. To build, you
apply the patches; to update the upstream base, you bump the submodule
and (if needed) refresh the patches. See `patches/README.md` for the
full workflow.

## Layout

```
.
├── README.md                # this file
├── patches/                 # the patches + driver script
│   ├── 0001-pausable-time-runtime.patch
│   ├── 0002-pausable-time-deps.patch
│   ├── 0003-pausable-time-tests.patch
│   ├── 0004-rename-for-crates-io.patch
│   ├── apply.sh
│   └── README.md            # patch-specific docs
└── tokio-upstream/          # git submodule -> tokio-rs/tokio @ tokio-1.52.1
```

## Quick start

```sh
# Clone the fork (with the submodule in one go).
git clone --recurse-submodules <fork-url> pausable-tokio
cd pausable-tokio

# Apply runtime + deps + integration tests.
./patches/apply.sh

# Build/test the patched tokio.
cd tokio-upstream
cargo build -p tokio --features=full
cargo test -p tests-integration --release \
    --features=rt-time-pausable --test rt_pausable_time \
    -- --test-threads=1 --nocapture
```

## Publishing to crates.io

The crate is renamed to `pausable-tokio` only at publish time. From a
clean submodule:

```sh
./patches/apply.sh --with-rename
cd tokio-upstream/tokio
cargo publish --dry-run --allow-dirty   # sanity-check
cargo publish --allow-dirty             # for real
```

If you've already been doing development with patches 0001-0003 applied
and a populated `target/` dir, just add the rename:

```sh
./patches/apply.sh --rename-only
cd tokio-upstream/tokio
cargo publish --allow-dirty
```

## Updating to a new upstream tokio

```sh
# Move the submodule to the new tag and apply patches in one step.
# Accepts "1.53.0", "tokio-1.53.0", a branch name, or a commit sha.
./patches/apply.sh --tokio-version 1.53.0 --check   # dry-run probe
./patches/apply.sh --tokio-version 1.53.0           # actually apply

# Verify nothing regressed.
cd tokio-upstream
cargo build -p tokio --features=full
cargo test -p tests-integration --release \
    --features=rt-time-pausable --test rt_pausable_time \
    -- --test-threads=1
cd ..

# Commit the new submodule pointer.
git add tokio-upstream
git commit -m "sync to tokio-1.53.0"
```

If `--check` reports a patch that no longer applies (typically 0001
if upstream rewrote runtime internals), fix the offending hunks by
hand, regenerate the patches, and commit both the submodule bump and
the updated patches together. See `patches/README.md` for the
patch-by-patch breakdown and how to regenerate.

## What the patches do

| # | Patch | Purpose |
|---|-------|---------|
| 0001 | runtime | Adds `Builder::pausable_time`, `Runtime::pause` / `resume` / `wait_for_resume` / `run_unpausable` / etc., wraps `task.run()` in both schedulers, and gates the time driver's `park_thread_timeout` on the pausable clock's resume state. |
| 0002 | deps | Adds the optional `pausable_clock = "1.0.2"` dependency and pulls it into the existing `time` feature. |
| 0003 | tests | Adds `tests-integration/tests/rt_pausable_time.rs` (correctness + stress tests) and the `rt-time-pausable` feature on `tests-integration`. |
| 0004 | rename | Renames the crate to `pausable-tokio` and detaches it from the parent workspace for publishing. Apply only at publish time. |

## License

Inherits MIT license from upstream tokio.
