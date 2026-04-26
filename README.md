# pausable-tokio

A fork of [tokio](https://github.com/tokio-rs/tokio) that adds a
runtime-controllable pause/resume primitive on top of its clock.

When the runtime is built with `Builder::pausable_time`, its notion of
time is backed by [`pausable_clock`] and can be paused or resumed at
any moment by calling methods on the `Runtime`. Sleeps, intervals,
timeouts, and anything else that uses tokio's clock will not advance
while the runtime is paused.

The rest of the tokio API is unchanged, so this crate is a drop-in
replacement: most code only needs a one-line `Cargo.toml` change.

[![Crates.io](https://img.shields.io/crates/v/pausable-tokio.svg)](https://crates.io/crates/pausable-tokio)
[![Docs.rs](https://img.shields.io/docsrs/pausable-tokio)](https://docs.rs/pausable-tokio)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Using the crate

Add to your `Cargo.toml`. Renaming the dependency to `tokio` lets you
keep using `tokio::...` paths everywhere; the rest of your code does
not need to change.

```toml
[dependencies]
tokio = { package = "pausable-tokio", version = "1.52.1", features = ["full"] }
```

Then enable pausable time on the runtime builder, and pause/resume
whenever you like:

```rust
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::runtime::Builder;

fn main() {
    let rt = Arc::new(
        Builder::new_multi_thread()
            .enable_all()
            // start unpaused, no preset elapsed time
            .pausable_time(false, Duration::from_secs(0))
            .build()
            .unwrap(),
    );

    // Pause the runtime from outside after 100ms; resume 200ms later.
    {
        let rt = rt.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            rt.pause();
            thread::sleep(Duration::from_millis(200));
            rt.resume();
        });
    }

    rt.block_on(async {
        // Sleeps don't advance while the runtime is paused, so this
        // ~50ms sleep takes ~250ms of wall-clock time.
        let start = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(50)).await;
        println!("real elapsed: {:?}", start.elapsed());
    });
}
```

### What's added beyond stock tokio

| Method | What it does |
|---|---|
| `Builder::pausable_time(start_paused, elapsed_time)` | Enable the pausable clock when constructing the runtime. `elapsed_time` is a starting offset for `Runtime::elapsed_millis`. |
| `Runtime::pause()` / `resume()` | Pause/resume the clock. Returns `true` if it actually flipped the state. |
| `Runtime::is_paused()` / `is_paused_ordered(Ordering)` | Query state. |
| `Runtime::wait_for_pause()` / `wait_for_resume()` | Block the calling thread until the clock changes state. |
| `Runtime::run_unpausable(f)` | Run a closure while preventing the clock from being paused. Used internally to wrap every task poll. |
| `Runtime::run_unresumable(f)` | Mirror of the above for the resumed-state side. |
| `Runtime::run_if_paused(f)` / `run_if_resumed(f)` | Atomic conditional run. |
| `Runtime::now()` | The runtime's notion of the current `Instant`. |
| `Runtime::elapsed_millis()` | Elapsed time on the pausable clock, in ms. |

Everything else is identical to upstream tokio. Full reference docs:
<https://docs.rs/pausable-tokio>.

### Differences from `tokio::time::pause`

|                          | `pausable-tokio`                                   | upstream `tokio::time::pause`                          |
|--------------------------|----------------------------------------------------|--------------------------------------------------------|
| Feature gate             | `time` (default in `full`)                         | `test-util`                                            |
| Intended use             | production                                         | tests                                                  |
| Schedulers supported     | both current-thread and multi-thread               | current-thread only                                    |
| Behavior while paused    | clock literally stops                              | "auto-advance" jumps time forward to next pending sleep|
| Driven from              | any thread, any time                               | only via `tokio::time::*` API                          |

## Building / contributing to this fork

This repo doesn't contain a checked-in copy of tokio. It contains a
small set of patches and a git submodule pointing at upstream tokio:

```
.
├── README.md
├── release.sh                    # end-to-end release driver
├── patches/
│   ├── 0001-pausable-time-runtime.patch
│   ├── 0002-pausable-time-deps.patch
│   ├── 0003-pausable-time-tests.patch
│   ├── 0004-rename-for-crates-io.patch
│   ├── 0005-publish-readme.patch
│   ├── 0006-publish-cargo-metadata-and-lib-rs-note.patch
│   ├── apply.sh                  # patch driver
│   └── README.md                 # per-patch docs + sync workflow
└── tokio-upstream/               # submodule -> tokio-rs/tokio @ tokio-1.52.1
```

To build / test the fork locally:

```sh
git clone --recurse-submodules https://github.com/RookAndPawn/pausable-tokio.git
cd pausable-tokio

# Apply runtime + deps + integration tests (everything except the
# "rename for publish" patches).
./patches/apply.sh

cd tokio-upstream
cargo build -p tokio --features=full
cargo test -p tests-integration --release \
    --features=rt-time-pausable --test rt_pausable_time \
    -- --test-threads=1 --nocapture
```

To release a new version of `pausable-tokio` based on a given upstream
tokio tag, use the top-level `release.sh`:

```sh
./release.sh 1.53.0                # interactive
./release.sh 1.53.0 --yes          # skip the y/N confirmation
./release.sh 1.53.0 --no-publish   # full dry-run, never publish
```

It walks through five phases (check, apply, commit, dry-run publish,
real publish) and aborts on the first failure. See `patches/README.md`
for the manual equivalent and the patch-by-patch breakdown.

## License

MIT, same as upstream tokio.

[`pausable_clock`]: https://crates.io/crates/pausable_clock
