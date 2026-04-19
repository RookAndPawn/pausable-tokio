//! Tests for the `Builder::pausable_time` runtime option and the
//! corresponding `Runtime::pause` / `Runtime::resume` / helpers.
//!
//! These tests exercise the production-path pausable clock (backed by the
//! `pausable_clock` crate), which is independent of the `test-util`-only
//! `time::pause` mechanism. The `test-util` feature is intentionally NOT
//! required for these tests.

//! Integration tests for the pausable runtime clock.
//!
//! These live in `tests-integration` (rather than `tokio/tests`) so that we
//! can run tokio's production path without the `test-util` feature being
//! pulled in via `tokio-test`. The tests only run when the
//! `rt-time-pausable` feature is enabled.

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
