//! Integration tests for the `Builder::pausable_time` runtime option and the
//! corresponding `Runtime::pause` / `Runtime::resume` / helpers.
//!
//! These tests exercise the production-path pausable clock (backed by the
//! `pausable_clock` crate), which is independent of the `test-util`-only
//! `time::pause` mechanism. They live in `tests-integration` (rather than
//! `tokio/tests`) so tokio can be compiled without `test-util` being pulled
//! in via `tokio-test`. They only run when the `rt-time-pausable` feature is
//! enabled.

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
