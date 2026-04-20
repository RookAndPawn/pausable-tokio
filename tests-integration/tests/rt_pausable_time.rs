//! Integration tests for the `Builder::pausable_time` runtime option and the
//! corresponding `Runtime::pause` / `Runtime::resume` / helpers.
//!
//! These tests exercise the production-path pausable clock (backed by the
//! `pausable_clock` crate), which is independent of the `test-util`-only
//! `time::pause` mechanism. They live in `tests-integration` (rather than
//! `tokio/tests`) so tokio can be compiled without `test-util` being pulled
//! in via `tokio-test`. They only run when the `rt-time-pausable` feature is
//! enabled.
//!
//! # Running
//!
//! The stress tests (`stress_*`) each spin up their own pausable runtime and
//! an aggressive pauser OS thread. Running several of them in parallel on
//! the same machine can starve each other's timing budgets, so we
//! recommend running them serially:
//!
//! ```text
//! cargo test -p tests-integration --release \
//!     --features=rt-time-pausable --test rt_pausable_time \
//!     -- --test-threads=1 --nocapture
//! ```
//!
//! The quick correctness tests above the stress tests are safe to run in
//! parallel.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tokio::runtime::{Builder, Runtime};

fn pausable_current_thread() -> Runtime {
    Builder::new_current_thread()
        .enable_all()
        .pausable_time(false, Duration::from_secs(0))
        .build()
        .unwrap()
}

fn pausable_multi_thread() -> Runtime {
    Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .pausable_time(false, Duration::from_secs(0))
        .build()
        .unwrap()
}

#[test]
fn starts_unpaused_by_default_when_pausable() {
    let rt = pausable_current_thread();
    assert!(!rt.is_paused());
}

#[test]
fn start_paused_flag_is_respected() {
    let rt = Builder::new_current_thread()
        .enable_all()
        .pausable_time(true, Duration::from_secs(0))
        .build()
        .unwrap();

    assert!(rt.is_paused());
}

#[test]
fn pause_and_resume_return_values() {
    let rt = pausable_current_thread();

    // First pause should succeed (returns true).
    assert!(rt.pause());
    // Second pause should be a no-op (returns false).
    assert!(!rt.pause());
    assert!(rt.is_paused());

    // First resume should succeed.
    assert!(rt.resume());
    // Second resume is a no-op.
    assert!(!rt.resume());
    assert!(!rt.is_paused());
}

#[test]
fn elapsed_millis_does_not_advance_while_paused() {
    let rt = pausable_current_thread();

    // Let real time advance a bit.
    thread::sleep(Duration::from_millis(20));
    rt.pause();

    let before = rt.elapsed_millis();
    thread::sleep(Duration::from_millis(30));
    let after = rt.elapsed_millis();

    // While paused, `elapsed_millis` should not change (modulo a tiny
    // tolerance for pausing/pausing races).
    assert!(
        after.saturating_sub(before) <= 1,
        "elapsed_millis should not advance while paused: before={before} after={after}"
    );
}

#[test]
fn elapsed_millis_advances_after_resume() {
    let rt = pausable_current_thread();

    rt.pause();
    let paused = rt.elapsed_millis();
    thread::sleep(Duration::from_millis(20));
    rt.resume();

    // After resume, `elapsed_millis` should eventually reflect real elapsed
    // time again.
    thread::sleep(Duration::from_millis(30));
    let after = rt.elapsed_millis();
    assert!(
        after > paused,
        "elapsed_millis should advance after resume: paused={paused} after={after}"
    );
}

#[test]
fn sleep_waits_for_resume_before_firing() {
    let rt = pausable_current_thread();

    // Mark the runtime paused before any async sleep starts.
    rt.pause();

    let fired = Arc::new(AtomicBool::new(false));
    let fired_clone = fired.clone();

    // Spawn a blocking task that waits a bit then resumes the clock. We run
    // this on a separate OS thread so it can make progress while the runtime
    // is paused.
    let handle = rt.handle().clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(80));
        handle.block_on(async {});
        // Resume on the thread that created this closure.
        // We can't go through `handle` because the runtime is paused; instead
        // we dispatch through `Handle::block_on` which executes synchronously
        // without requiring a tick.
    });

    // Do the actual resume manually on another thread.
    let resumer_rt = Arc::new(rt);
    let resumer = resumer_rt.clone();
    let resumer_handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(80));
        resumer.resume();
    });

    resumer_rt.block_on(async move {
        let start = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let real_elapsed = start.elapsed();
        // Because the clock was paused for ~80ms before resuming, the sleep
        // should only complete after at least ~80ms of real time (and usually
        // quite a bit more because the 50ms pausable sleep still needs to
        // elapse once resumed).
        assert!(
            real_elapsed >= Duration::from_millis(75),
            "sleep completed too quickly: {:?}",
            real_elapsed
        );
        fired_clone.store(true, Ordering::SeqCst);
    });

    resumer_handle.join().unwrap();
    let _ = handle;
    assert!(fired.load(Ordering::SeqCst));
}

#[test]
fn run_unpausable_blocks_pause_until_action_finishes() {
    let rt = pausable_current_thread();

    let rt = Arc::new(rt);

    let rt_clone = rt.clone();
    let action_running = Arc::new(AtomicBool::new(false));
    let action_running_clone = action_running.clone();

    // Kick off a thread that will try to pause after we enter the
    // unpausable section.
    let pause_thread = thread::spawn(move || {
        // Wait for the action to start.
        while !action_running_clone.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        // Now request a pause. This call should block until the action
        // completes.
        let before = std::time::Instant::now();
        rt_clone.pause();
        before.elapsed()
    });

    // The action sleeps, blocking pauses for its entire duration.
    let elapsed_before_pause = rt.run_unpausable(|| {
        action_running.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(50));
        std::time::Instant::now()
    });

    // Join the pause thread; the pause should have taken approximately
    // however long our sleep blocked it for.
    let pause_elapsed = pause_thread.join().unwrap();
    assert!(
        pause_elapsed >= Duration::from_millis(30),
        "pause should have been blocked by run_unpausable: {pause_elapsed:?}"
    );

    // After pause returns, the runtime should be paused.
    assert!(rt.is_paused());
    let _ = elapsed_before_pause;
}

#[test]
fn run_if_paused_and_run_if_resumed_are_atomic() {
    let rt = pausable_current_thread();

    let counter = AtomicUsize::new(0);

    // Not paused yet: run_if_resumed fires, run_if_paused does not.
    assert_eq!(
        rt.run_if_resumed(|| counter.fetch_add(1, Ordering::SeqCst)),
        Some(0)
    );
    assert_eq!(rt.run_if_paused(|| counter.fetch_add(1, Ordering::SeqCst)), None);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // After pausing, run_if_paused fires and run_if_resumed does not.
    rt.pause();
    assert_eq!(rt.run_if_resumed(|| counter.fetch_add(1, Ordering::SeqCst)), None);
    assert_eq!(
        rt.run_if_paused(|| counter.fetch_add(1, Ordering::SeqCst)),
        Some(1)
    );
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn wait_for_resume_returns_immediately_when_running() {
    let rt = pausable_current_thread();

    // Not paused: should return immediately.
    let start = std::time::Instant::now();
    rt.wait_for_resume();
    assert!(start.elapsed() < Duration::from_millis(5));
}

#[test]
fn wait_for_resume_blocks_until_resume() {
    let rt = Arc::new(pausable_current_thread());
    rt.pause();

    let rt_clone = rt.clone();
    let resumer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        rt_clone.resume();
    });

    let start = std::time::Instant::now();
    rt.wait_for_resume();
    let elapsed = start.elapsed();

    resumer.join().unwrap();
    assert!(
        elapsed >= Duration::from_millis(20),
        "wait_for_resume returned too early: {elapsed:?}"
    );
    assert!(!rt.is_paused());
}

#[test]
fn elapsed_time_initial_offset_is_honored() {
    let rt = Builder::new_current_thread()
        .enable_all()
        .pausable_time(true, Duration::from_secs(100))
        .build()
        .unwrap();

    // We started paused, with 100s pre-seeded elapsed time. `elapsed_millis`
    // should report >= 100s initially.
    assert!(rt.elapsed_millis() >= 100_000);
}

#[test]
fn multi_thread_pause_then_resume_sleep() {
    let rt = Arc::new(pausable_multi_thread());
    rt.pause();

    let rt_clone = rt.clone();
    let resumer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(60));
        rt_clone.resume();
    });

    rt.block_on(async move {
        let start = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let real_elapsed = start.elapsed();

        // The 20ms Tokio sleep should have been gated on the resume happening
        // ~60ms later, so we expect at least 55ms of real time to have passed.
        assert!(
            real_elapsed >= Duration::from_millis(55),
            "sleep completed too quickly on multi-thread runtime: {:?}",
            real_elapsed
        );
    });

    resumer.join().unwrap();
}

#[test]
fn is_paused_ordered_matches_is_paused() {
    use std::sync::atomic::Ordering as AtomicOrdering;

    let rt = pausable_current_thread();
    assert_eq!(rt.is_paused(), rt.is_paused_ordered(AtomicOrdering::Relaxed));
    assert_eq!(rt.is_paused(), rt.is_paused_ordered(AtomicOrdering::Acquire));

    rt.pause();
    assert_eq!(rt.is_paused(), rt.is_paused_ordered(AtomicOrdering::Relaxed));
    assert_eq!(rt.is_paused(), rt.is_paused_ordered(AtomicOrdering::Acquire));
}

/// Long-running stress test that hammers the pausable clock from multiple
/// directions concurrently for several seconds of wall-clock time. This is
/// intended to shake out races, deadlocks, and invariant violations that
/// would not show up in the single-shot pause/resume tests above.
///
/// Concurrency:
///  * A `pauser` OS thread rapidly toggles the clock between paused and
///    resumed.
///  * Many async tasks loop, each alternating between `tokio::time::sleep`
///    and `rt.run_unpausable` critical sections.
///  * Each iteration records the pausable-clock `now()` before and after and
///    checks pausable-clock invariants.
///  * A dedicated `now-watcher` task samples `rt.now()` continuously and
///    asserts monotonicity across pause/resume boundaries.
///
/// Invariants verified at the end:
///  * The test completes in a bounded amount of real time (no deadlock).
///  * Every spawned worker runs to completion.
///  * Pausable elapsed time is strictly less than real elapsed time because
///    the pauser kept the clock paused for a non-trivial fraction.
///  * Every `sleep(D)` observed at least `D` of pausable-clock advance
///    between its start and end (the core timing guarantee).
///  * `rt.now()` is non-decreasing across all samples.
///  * A reasonable number of pause/resume cycles actually occurred (so we
///    know the test stressed the mechanism and did not trivially finish).
#[test]
fn stress_pause_resume_multi_thread() {
    // `TEST_DURATION` is measured in *real* time so the test cost is
    // predictable even though pausing the clock can stretch pausable-time
    // operations significantly.
    const TEST_DURATION: Duration = Duration::from_secs(4);
    const WORKER_COUNT: usize = 16;
    const MAX_TEST_RUNTIME: Duration = Duration::from_secs(60);

    let rt = Arc::new(
        Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .pausable_time(false, Duration::from_secs(0))
            .build()
            .unwrap(),
    );

    // Drives pause/resume from outside the runtime. It toggles the state as
    // fast as it can, with small varying sleeps, so that pause/resume races
    // against every operation the worker tasks perform.
    let stop = Arc::new(AtomicBool::new(false));
    let pauses = Arc::new(AtomicUsize::new(0));
    let resumes = Arc::new(AtomicUsize::new(0));

    let pauser_rt = rt.clone();
    let pauser_stop = stop.clone();
    let pauser_pauses = pauses.clone();
    let pauser_resumes = resumes.clone();
    let pauser = thread::spawn(move || {
        // Use a simple LCG so the test remains deterministic relative to
        // its own iteration count; we just need some jitter.
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        while !pauser_stop.load(Ordering::Relaxed) {
            // `advance` the LCG.
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let jitter_us = ((state >> 32) as u32 % 2000) as u64; // 0..2ms

            if pauser_rt.pause() {
                pauser_pauses.fetch_add(1, Ordering::Relaxed);
            }
            thread::sleep(Duration::from_micros(500 + jitter_us));
            if pauser_rt.resume() {
                pauser_resumes.fetch_add(1, Ordering::Relaxed);
            }
            thread::sleep(Duration::from_micros(500 + jitter_us));
        }
        // Make sure we leave the clock resumed so any remaining work can
        // finish cleanly.
        pauser_rt.resume();
    });

    // Watches `rt.now()` and asserts monotonicity. Any regression would
    // indicate a serious pausable-clock correctness bug.
    let watcher_rt = rt.clone();
    let watcher_stop = stop.clone();
    let now_samples = Arc::new(AtomicUsize::new(0));
    let watcher_samples = now_samples.clone();
    let monotonicity_violations = Arc::new(AtomicUsize::new(0));
    let watcher_violations = monotonicity_violations.clone();

    // Completion tracking.
    let sleep_observations = Arc::new(AtomicUsize::new(0));
    let timing_violations = Arc::new(AtomicUsize::new(0));
    let unpausable_critical_sections = Arc::new(AtomicUsize::new(0));
    let workers_done = Arc::new(AtomicUsize::new(0));

    let start_real = std::time::Instant::now();
    let start_pausable = rt.elapsed_millis();

    rt.block_on({
        let rt = rt.clone();
        let stop = stop.clone();
        let watcher_samples = watcher_samples.clone();
        let watcher_violations = watcher_violations.clone();
        let workers_done = workers_done.clone();
        let sleep_observations = sleep_observations.clone();
        let timing_violations = timing_violations.clone();
        let unpausable_critical_sections = unpausable_critical_sections.clone();
        async move {
            // `rt.now()` monotonicity watcher (runs inside the runtime).
            // We use `yield_now` rather than `tokio::time::sleep` because
            // tokio's sleep has 1ms resolution and would be heavily gated
            // by pause cycles; `yield_now` lets this task sample tightly
            // without being blocked waiting for the pausable clock.
            let watcher = tokio::spawn({
                let watcher_rt = watcher_rt.clone();
                let watcher_stop = watcher_stop.clone();
                async move {
                    let mut last = watcher_rt.now();
                    while !watcher_stop.load(Ordering::Relaxed) {
                        let cur = watcher_rt.now();
                        if cur < last {
                            watcher_violations.fetch_add(1, Ordering::Relaxed);
                        }
                        last = cur;
                        watcher_samples.fetch_add(1, Ordering::Relaxed);
                        tokio::task::yield_now().await;
                    }
                }
            });

            let mut handles = Vec::with_capacity(WORKER_COUNT);
            for worker_idx in 0..WORKER_COUNT {
                let rt_for_worker = rt.clone();
                let stop = stop.clone();
                let sleep_observations = sleep_observations.clone();
                let timing_violations = timing_violations.clone();
                let unpausable_critical_sections = unpausable_critical_sections.clone();
                let workers_done = workers_done.clone();

                handles.push(tokio::spawn(async move {
                    // Each worker uses a distinct sleep cadence.
                    let sleep_ms = 2 + (worker_idx as u64 % 5);

                    while !stop.load(Ordering::Relaxed) {
                        // Sleep with pausable-clock bookkeeping.
                        let before = rt_for_worker.now();
                        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                        let after = rt_for_worker.now();

                        sleep_observations.fetch_add(1, Ordering::Relaxed);

                        // Core guarantee: `sleep(D)` must advance the
                        // pausable clock by at least `D`. If this ever
                        // fails we have a correctness bug in the driver.
                        if after.duration_since(before) < Duration::from_millis(sleep_ms) {
                            timing_violations.fetch_add(1, Ordering::Relaxed);
                        }

                        // Enter an unpausable critical section. The pauser
                        // thread may be banging on `pause()` right now, so
                        // this also exercises `pause` waiting on
                        // `run_unpausable` to finish.
                        rt_for_worker.run_unpausable(|| {
                            unpausable_critical_sections.fetch_add(1, Ordering::Relaxed);
                            // Busy-ish work so a pause attempt is forced
                            // to wait noticeably.
                            let mut acc: u64 = 0;
                            for i in 0..10_000 {
                                acc = acc.wrapping_add(i);
                            }
                            // Prevent the optimizer from removing the loop.
                            std::hint::black_box(acc);
                        });

                        // A second, yield-based pause point, mostly to
                        // keep the scheduler honest.
                        tokio::task::yield_now().await;
                    }

                    workers_done.fetch_add(1, Ordering::Relaxed);
                }));
            }

            // Wait the configured test duration. Use tokio sleep so the
            // pause/resume toggling also applies to this waiter.
            tokio::time::sleep(TEST_DURATION).await;
            stop.store(true, Ordering::Relaxed);

            for h in handles {
                // Each worker should finish promptly once `stop` is set.
                // Give a generous timeout, but panic if they don't because
                // that would indicate a deadlock.
                tokio::time::timeout(Duration::from_secs(10), h)
                    .await
                    .expect("worker did not finish in time (possible deadlock)")
                    .expect("worker task panicked");
            }

            // Let the watcher task finish.
            tokio::time::timeout(Duration::from_secs(2), watcher)
                .await
                .expect("watcher did not finish in time")
                .expect("watcher task panicked");
        }
    });

    pauser.join().expect("pauser thread panicked");

    let real_elapsed = start_real.elapsed();
    let pausable_elapsed_ms = rt.elapsed_millis() - start_pausable;
    let pausable_elapsed = Duration::from_millis(pausable_elapsed_ms);

    let pauses_observed = pauses.load(Ordering::Relaxed);
    let resumes_observed = resumes.load(Ordering::Relaxed);
    let sleep_obs = sleep_observations.load(Ordering::Relaxed);
    let timing_viol = timing_violations.load(Ordering::Relaxed);
    let unpausable_obs = unpausable_critical_sections.load(Ordering::Relaxed);
    let now_samples_obs = now_samples.load(Ordering::Relaxed);
    let monotonicity_viol = monotonicity_violations.load(Ordering::Relaxed);
    let workers_finished = workers_done.load(Ordering::Relaxed);

    // Bail out early if we took absurdly long (something froze).
    assert!(
        real_elapsed <= MAX_TEST_RUNTIME,
        "stress test took too long: {real_elapsed:?} (likely a deadlock)"
    );

    // Every worker should have finished cleanly.
    assert_eq!(
        workers_finished, WORKER_COUNT,
        "some workers did not finish (got {workers_finished} / {WORKER_COUNT})"
    );

    // The pauser must have actually exercised the pause mechanism a lot.
    assert!(
        pauses_observed >= 100,
        "pauser did not pause often enough: {pauses_observed} pauses"
    );
    assert!(
        resumes_observed >= 100,
        "pauser did not resume often enough: {resumes_observed} resumes"
    );

    // The pausable clock should have been paused for a non-trivial
    // fraction of the test, so its elapsed time should be meaningfully
    // smaller than wall-clock elapsed time.
    assert!(
        pausable_elapsed < real_elapsed,
        "pausable elapsed ({pausable_elapsed:?}) should be < real elapsed ({real_elapsed:?})"
    );
    // But it should still have advanced - otherwise we were always paused
    // which would also be a bug.
    assert!(
        pausable_elapsed >= Duration::from_millis(100),
        "pausable clock did not advance enough: {pausable_elapsed:?}"
    );

    // Sleeps must have observed the pausable-clock duration guarantee.
    assert!(
        sleep_obs > 0,
        "no sleep observations recorded; workers may not have run"
    );
    assert_eq!(
        timing_viol, 0,
        "saw {timing_viol} sleep(D) completions that advanced the pausable \
         clock by less than D (out of {sleep_obs} observations)"
    );

    // `run_unpausable` should have fired at least once per worker.
    assert!(
        unpausable_obs >= WORKER_COUNT,
        "run_unpausable fired too few times: {unpausable_obs} (expected at least {WORKER_COUNT})"
    );

    // Monotonicity must never have been violated.
    assert!(
        now_samples_obs > 0,
        "watcher recorded no `now()` samples; it may not have run"
    );
    assert_eq!(
        monotonicity_viol, 0,
        "rt.now() was non-monotonic on {monotonicity_viol} / {now_samples_obs} samples"
    );

    eprintln!(
        "stress summary: real={:?} pausable={:?} pauses={} resumes={} \
         sleeps={} unpausable_sections={} now_samples={} workers={}",
        real_elapsed,
        pausable_elapsed,
        pauses_observed,
        resumes_observed,
        sleep_obs,
        unpausable_obs,
        now_samples_obs,
        workers_finished,
    );
}

// ==================================================================
// Additional stress tests targeting features added to tokio after
// the original pausable fork was written on tokio 0.3.4.
// ==================================================================

/// Spawns a thread that toggles the clock between paused and resumed with
/// jittered intervals. `base_cycle_us` controls the baseline half-cycle
/// length in microseconds; the actual value has up to 2ms of jitter added
/// on top. The returned `JoinHandle` expects `stop` to be set; joining it
/// returns `(pauses, resumes)` counts.
fn spawn_pauser_with_rate(
    rt: Arc<Runtime>,
    stop: Arc<AtomicBool>,
    base_cycle_us: u64,
) -> thread::JoinHandle<(usize, usize)> {
    thread::spawn(move || {
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        let mut pauses = 0usize;
        let mut resumes = 0usize;
        while !stop.load(Ordering::Relaxed) {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let jitter_us = ((state >> 32) as u32 % 2000) as u64;

            if rt.pause() {
                pauses += 1;
            }
            thread::sleep(Duration::from_micros(base_cycle_us + jitter_us));
            if rt.resume() {
                resumes += 1;
            }
            thread::sleep(Duration::from_micros(base_cycle_us + jitter_us));
        }
        // Leave the clock resumed so outstanding work can finish.
        rt.resume();
        (pauses, resumes)
    })
}

/// Convenience: aggressive ~500us pauser used by most stress tests.
fn spawn_pauser(
    rt: Arc<Runtime>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<(usize, usize)> {
    spawn_pauser_with_rate(rt, stop, 500)
}

/// Stress test for `JoinSet` (tokio 1.21+) combined with pausable time on
/// the current-thread scheduler.
///
/// Dynamically spawns tasks into a `JoinSet`, each of which performs a
/// pausable `sleep` + some `run_unpausable` work. The pauser thread
/// hammers pause/resume. New tasks are continually spawned as old ones
/// complete. We assert that every task that was spawned actually completes
/// with the expected result (the task's own id), no task is lost, and
/// the pausable clock advances non-trivially.
#[test]
fn stress_joinset_current_thread() {
    use tokio::task::JoinSet;

    const TASK_BATCHES: usize = 8;
    const TASKS_PER_BATCH: usize = 8;
    const EXPECTED_TOTAL: usize = TASK_BATCHES * TASKS_PER_BATCH;
    const MAX_TEST_RUNTIME: Duration = Duration::from_secs(60);

    let rt = Arc::new(pausable_current_thread());

    let stop = Arc::new(AtomicBool::new(false));
    let pauser = spawn_pauser(rt.clone(), stop.clone());

    let start_real = std::time::Instant::now();
    let start_pausable = rt.elapsed_millis();

    let completed: usize = rt.block_on(async {
        let mut joins: JoinSet<usize> = JoinSet::new();
        let mut observed = vec![false; EXPECTED_TOTAL];
        let mut next_id = 0usize;
        let mut completed = 0usize;

        // Seed the initial batch.
        for _ in 0..TASKS_PER_BATCH {
            let id = next_id;
            next_id += 1;
            joins.spawn(async move {
                // Variable sleep so tasks complete out-of-order.
                let sleep_ms = 1 + (id as u64 % 7);
                tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                id
            });
        }

        while let Some(res) = joins.join_next().await {
            let id = res.expect("task panicked");
            assert!(id < EXPECTED_TOTAL, "task id out of bounds: {id}");
            assert!(!observed[id], "task {id} completed twice");
            observed[id] = true;
            completed += 1;

            // Keep feeding the set until we've spawned all tasks.
            if next_id < EXPECTED_TOTAL {
                let id = next_id;
                next_id += 1;
                joins.spawn(async move {
                    let sleep_ms = 1 + (id as u64 % 7);
                    tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                    id
                });
            }
        }

        // Verify every task was observed exactly once.
        assert!(observed.iter().all(|seen| *seen));
        completed
    });

    stop.store(true, Ordering::Relaxed);
    let (pauses, resumes) = pauser.join().expect("pauser thread panicked");
    let real_elapsed = start_real.elapsed();
    let pausable_elapsed = Duration::from_millis(rt.elapsed_millis() - start_pausable);

    assert!(
        real_elapsed <= MAX_TEST_RUNTIME,
        "test took too long: {real_elapsed:?} (possible deadlock)"
    );
    assert_eq!(completed, EXPECTED_TOTAL);
    assert!(
        pauses >= 10 && resumes >= 10,
        "pauser did not churn enough: {pauses} pauses, {resumes} resumes"
    );
    assert!(
        pausable_elapsed < real_elapsed,
        "pausable ({pausable_elapsed:?}) should be < real ({real_elapsed:?})"
    );

    eprintln!(
        "joinset stress: real={:?} pausable={:?} pauses={} resumes={} \
         completed={}/{}",
        real_elapsed, pausable_elapsed, pauses, resumes, completed, EXPECTED_TOTAL,
    );
}

/// Stress test for `Interval` + `MissedTickBehavior` (added in tokio 1.9)
/// under pause pressure.
///
/// Because the pausable clock does not advance while paused, interval
/// deadlines do not roll over during a pause; they roll over only as
/// pausable time catches up. This test drives three intervals with the
/// three different `MissedTickBehavior` policies and verifies that each
/// still produces a monotonically non-decreasing sequence of `rt.now()`
/// values at the ticks, and each fires with a pausable-time gap no
/// smaller than its configured period.
#[test]
fn stress_interval_missed_tick_behaviors() {
    use tokio::time::MissedTickBehavior;

    const RUN_DURATION: Duration = Duration::from_secs(3);
    const PERIOD: Duration = Duration::from_millis(10);
    const MAX_TEST_RUNTIME: Duration = Duration::from_secs(60);

    let rt = Arc::new(pausable_multi_thread());

    let stop = Arc::new(AtomicBool::new(false));
    let pauser = spawn_pauser(rt.clone(), stop.clone());

    let start_real = std::time::Instant::now();
    let start_pausable_ms = rt.elapsed_millis();

    let (burst_ticks, delay_ticks, skip_ticks) = rt.block_on({
        let rt = rt.clone();
        let stop = stop.clone();
        async move {
            async fn drive_interval(
                rt: Arc<Runtime>,
                stop: Arc<AtomicBool>,
                behavior: MissedTickBehavior,
                period: Duration,
            ) -> Vec<tokio::time::Instant> {
                let mut interval = tokio::time::interval(period);
                interval.set_missed_tick_behavior(behavior);
                let mut ticks = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    interval.tick().await;
                    ticks.push(rt.now());
                }
                ticks
            }

            let burst = tokio::spawn(drive_interval(
                rt.clone(),
                stop.clone(),
                MissedTickBehavior::Burst,
                PERIOD,
            ));
            let delay = tokio::spawn(drive_interval(
                rt.clone(),
                stop.clone(),
                MissedTickBehavior::Delay,
                PERIOD,
            ));
            let skip = tokio::spawn(drive_interval(
                rt.clone(),
                stop.clone(),
                MissedTickBehavior::Skip,
                PERIOD,
            ));

            tokio::time::sleep(RUN_DURATION).await;
            stop.store(true, Ordering::Relaxed);

            (
                burst.await.unwrap(),
                delay.await.unwrap(),
                skip.await.unwrap(),
            )
        }
    });

    let (pauses, resumes) = pauser.join().expect("pauser thread panicked");
    let real_elapsed = start_real.elapsed();
    let pausable_elapsed = Duration::from_millis(rt.elapsed_millis() - start_pausable_ms);

    assert!(real_elapsed <= MAX_TEST_RUNTIME, "took too long: {real_elapsed:?}");
    assert!(pauses >= 100 && resumes >= 100);

    // Shared invariants across all three policies:
    //   * each interval must produce some ticks
    //   * tick timestamps are monotonic non-decreasing
    //   * adjacent ticks must be at least `PERIOD` apart in pausable time,
    //     except for Burst which can produce back-to-back catch-up ticks.
    for (name, ticks, allow_zero_gap) in [
        ("Burst", &burst_ticks, true),
        ("Delay", &delay_ticks, false),
        ("Skip", &skip_ticks, false),
    ] {
        assert!(
            ticks.len() >= 2,
            "{name}: expected at least 2 ticks, got {}",
            ticks.len()
        );
        for pair in ticks.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(
                pair[1] >= pair[0],
                "{name}: tick timestamps must be monotonic: {:?} -> {:?}",
                pair[0],
                pair[1]
            );
            if !allow_zero_gap {
                assert!(
                    gap >= PERIOD,
                    "{name}: gap between ticks ({gap:?}) must be >= period ({PERIOD:?})"
                );
            }
        }
    }

    // Burst should produce the most (or at least tied for most) ticks
    // because it catches up after a pause by firing repeatedly.
    assert!(
        burst_ticks.len() >= delay_ticks.len(),
        "Burst ({}) should produce >= Delay ticks ({})",
        burst_ticks.len(),
        delay_ticks.len()
    );

    eprintln!(
        "interval stress: real={:?} pausable={:?} pauses={} resumes={} \
         burst_ticks={} delay_ticks={} skip_ticks={}",
        real_elapsed,
        pausable_elapsed,
        pauses,
        resumes,
        burst_ticks.len(),
        delay_ticks.len(),
        skip_ticks.len(),
    );
}

/// Stress test for `tokio::time::timeout` (the combinator was in 0.3 but
/// its current form and the surrounding timer driver have been heavily
/// rewritten) under pause/resume churn.
///
/// Spawns many tasks that each race a `tokio::time::sleep` against a
/// `tokio::time::timeout`. We pick the timeout to be reliably longer than
/// the sleep in pausable-clock time. The key invariant: every task must
/// observe the sleep completing (`Ok`), never the timeout elapsing
/// first, even though the real-time elapsed may be much larger than the
/// timeout because of pauses. This guarantees that pausing the clock
/// does NOT cause spurious timeout firings.
#[test]
fn stress_timeout_under_pause() {
    use tokio::time::{sleep, timeout};

    const TASKS: usize = 8;
    const ITERATIONS_PER_TASK: usize = 8;
    const SLEEP_MS: u64 = 5;
    // `TIMEOUT_MS` is the pausable-clock deadline for the timeout. With
    // pausable time enforced correctly, the inner `sleep(SLEEP_MS)` should
    // always complete well before `TIMEOUT_MS` of pausable time elapses.
    const TIMEOUT_MS: u64 = 50;
    const MAX_TEST_RUNTIME: Duration = Duration::from_secs(60);

    let rt = Arc::new(pausable_multi_thread());
    let stop = Arc::new(AtomicBool::new(false));
    let pauser = spawn_pauser(rt.clone(), stop.clone());

    let start_real = std::time::Instant::now();
    let start_pausable_ms = rt.elapsed_millis();

    let (successes, failures) = rt.block_on({
        let rt = rt.clone();
        async move {
            let mut handles = Vec::with_capacity(TASKS);
            for _ in 0..TASKS {
                let rt = rt.clone();
                handles.push(tokio::spawn(async move {
                    let mut ok = 0usize;
                    let mut err = 0usize;
                    for _ in 0..ITERATIONS_PER_TASK {
                        let before = rt.now();
                        match timeout(
                            Duration::from_millis(TIMEOUT_MS),
                            sleep(Duration::from_millis(SLEEP_MS)),
                        )
                        .await
                        {
                            Ok(()) => {
                                let after = rt.now();
                                // Core timing guarantee: once the inner
                                // sleep completes, at least `SLEEP_MS` of
                                // pausable time must have passed. The
                                // task may be polled later than that
                                // under pause pressure, but not earlier.
                                let gap = after.duration_since(before);
                                assert!(
                                    gap >= Duration::from_millis(SLEEP_MS),
                                    "sleep completed too early in pausable time: {gap:?}"
                                );
                                ok += 1;
                            }
                            Err(_) => err += 1,
                        }
                    }
                    (ok, err)
                }));
            }

            let mut successes = 0usize;
            let mut failures = 0usize;
            for h in handles {
                let (ok, err) = h.await.expect("task panicked");
                successes += ok;
                failures += err;
            }
            (successes, failures)
        }
    });

    stop.store(true, Ordering::Relaxed);
    let (pauses, resumes) = pauser.join().expect("pauser thread panicked");
    let real_elapsed = start_real.elapsed();
    let pausable_elapsed = Duration::from_millis(rt.elapsed_millis() - start_pausable_ms);

    assert!(
        real_elapsed <= MAX_TEST_RUNTIME,
        "test took too long: {real_elapsed:?}"
    );
    let expected = TASKS * ITERATIONS_PER_TASK;
    assert_eq!(
        failures, 0,
        "{failures} of {expected} iterations saw their timeout fire; pausing \
         must not cause timeouts to spuriously elapse (successes={successes})"
    );
    assert_eq!(successes, expected);
    assert!(pauses >= 10 && resumes >= 10);

    eprintln!(
        "timeout stress: real={:?} pausable={:?} pauses={} resumes={} \
         successes={}/{} (all sleeps completed before timeout)",
        real_elapsed, pausable_elapsed, pauses, resumes, successes, expected,
    );
}

/// Verifies that `spawn_blocking` tasks are **not** gated by the pausable
/// clock. This is the intended design: blocking threads live in the
/// blocking pool and only async timing work is affected by pause/resume.
///
/// We start the runtime paused, then spawn many blocking tasks while the
/// clock is still paused. Each task does real work (`std::thread::sleep`)
/// and returns its input. We assert every task completes within a real-
/// time bound that's much shorter than would be possible if blocking
/// tasks were blocked on the pausable clock.
#[test]
fn stress_spawn_blocking_ignores_pausable_clock() {
    const TASKS: usize = 64;
    const PER_TASK_SLEEP: Duration = Duration::from_millis(15);
    const MAX_REAL_TIME: Duration = Duration::from_secs(5);

    let rt = Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .max_blocking_threads(32)
        .pausable_time(true, Duration::from_secs(0)) // start paused!
        .build()
        .unwrap();

    assert!(rt.is_paused(), "runtime should start paused");

    let start = std::time::Instant::now();
    let results: Vec<usize> = rt.block_on(async {
        let mut handles = Vec::with_capacity(TASKS);
        for i in 0..TASKS {
            // The runtime is still paused here; these should still run.
            handles.push(tokio::task::spawn_blocking(move || {
                std::thread::sleep(PER_TASK_SLEEP);
                i
            }));
        }
        let mut out = Vec::with_capacity(TASKS);
        for h in handles {
            out.push(h.await.expect("blocking task panicked"));
        }
        out
    });
    let elapsed = start.elapsed();

    // Every task should have returned its input.
    let mut seen = vec![false; TASKS];
    for v in results {
        assert!(v < TASKS);
        assert!(!seen[v], "duplicate result: {v}");
        seen[v] = true;
    }
    assert!(seen.iter().all(|s| *s), "not all blocking tasks completed");

    // The runtime was paused the whole time, yet all blocking tasks ran
    // to completion within a short real-time bound. This is the key
    // assertion: the blocking pool is not gated by the pausable clock.
    assert!(
        elapsed <= MAX_REAL_TIME,
        "blocking tasks took too long while clock was paused: {elapsed:?}"
    );

    // Clock must still be paused, since we never resumed it.
    assert!(rt.is_paused(), "clock should still be paused at end");

    eprintln!(
        "spawn_blocking while paused: real={elapsed:?} tasks={TASKS} \
         per_task_sleep={PER_TASK_SLEEP:?} clock_still_paused=true",
    );
}

/// Stress test for `Notify::notify_waiters` (tokio 1.x) under pause/resume
/// churn. Verifies broadcast wakeups are delivered to parked waiters
/// despite the pausable clock being aggressively toggled.
///
/// Structure: several rounds. In each round we spawn `WAITERS` fresh
/// tasks, wait for them all to park on a shared `Notify`, then call
/// `notify_waiters()` once and confirm every one of them wakes. This
/// avoids the "waiter in transit" race that makes a looped
/// `notify_waiters` usage non-deterministic.
#[test]
fn stress_notify_waiters_under_pause() {
    use tokio::sync::Notify;

    const ROUNDS: usize = 4;
    const WAITERS: usize = 6;
    const MAX_TEST_RUNTIME: Duration = Duration::from_secs(60);

    let rt = Arc::new(pausable_multi_thread());
    let stop = Arc::new(AtomicBool::new(false));
    let pauser = spawn_pauser(rt.clone(), stop.clone());

    let start_real = std::time::Instant::now();

    let (total_wakeups, rounds_run) = rt.block_on({
        let rt = rt.clone();
        async move {
            let mut total_wakeups = 0usize;

            for round in 0..ROUNDS {
                let notify = Arc::new(Notify::new());
                let parked = Arc::new(AtomicUsize::new(0));
                let wakeups = Arc::new(AtomicUsize::new(0));

                let mut handles = Vec::with_capacity(WAITERS);
                for _ in 0..WAITERS {
                    let notify = notify.clone();
                    let parked = parked.clone();
                    let wakeups = wakeups.clone();
                    handles.push(tokio::spawn(async move {
                        // Register intent to wait, then await.
                        //
                        // `notified()` returns a `Notified` future that
                        // is "registered" only after its first poll.
                        let fut = notify.notified();
                        tokio::pin!(fut);
                        // Touch fut once to enable registration on next
                        // poll; we can't easily observe registration, so
                        // instead we bump the counter before yielding so
                        // the notifier can use it as a live-ness signal.
                        parked.fetch_add(1, Ordering::Release);
                        fut.as_mut().await;
                        wakeups.fetch_add(1, Ordering::Relaxed);
                    }));
                }

                // Wait until every waiter has parked. We use a generous
                // timeout and yield repeatedly, occasionally sleeping on
                // the pausable clock so we cross pause/resume boundaries.
                let wait_for_parked = async {
                    while parked.load(Ordering::Acquire) < WAITERS {
                        tokio::task::yield_now().await;
                    }
                    // After every task has bumped `parked`, spin-yield a
                    // few more times to give each task's `notified()`
                    // future a chance to actually register with `Notify`.
                    for _ in 0..2000 {
                        tokio::task::yield_now().await;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                };
                tokio::time::timeout(Duration::from_secs(10), wait_for_parked)
                    .await
                    .expect("timeout waiting for waiters to park");

                // Broadcast once. Every registered waiter should wake.
                notify.notify_waiters();

                // Wait for all waiters to finish. They have nothing to
                // do except complete after being notified.
                for h in handles {
                    tokio::time::timeout(Duration::from_secs(15), h)
                        .await
                        .unwrap_or_else(|_| {
                            panic!(
                                "waiter did not finish in round {round} \
                                 (wakeups so far: {})",
                                wakeups.load(Ordering::Relaxed)
                            )
                        })
                        .expect("waiter panicked");
                }

                let round_wakeups = wakeups.load(Ordering::Relaxed);
                assert_eq!(
                    round_wakeups, WAITERS,
                    "round {round}: expected {WAITERS} wakeups, got {round_wakeups}"
                );
                total_wakeups += round_wakeups;

                // Small pausable-clock sleep between rounds so the
                // pauser's timing drifts across round boundaries.
                tokio::time::sleep(Duration::from_millis(5)).await;

                // Exercise the clock to make sure it's still healthy.
                let _ = rt.now();
            }

            (total_wakeups, ROUNDS)
        }
    });

    stop.store(true, Ordering::Relaxed);
    let (pauses, resumes) = pauser.join().expect("pauser thread panicked");
    let real_elapsed = start_real.elapsed();

    assert!(
        real_elapsed <= MAX_TEST_RUNTIME,
        "test took too long: {real_elapsed:?}"
    );

    let expected = ROUNDS * WAITERS;
    assert_eq!(
        total_wakeups, expected,
        "expected {expected} wakeups across {rounds_run} rounds, got {total_wakeups}"
    );
    assert!(pauses >= 10 && resumes >= 10);

    eprintln!(
        "notify stress: real={:?} pauses={} resumes={} rounds={} \
         total_wakeups={}/{}",
        real_elapsed, pauses, resumes, rounds_run, total_wakeups, expected,
    );
}
