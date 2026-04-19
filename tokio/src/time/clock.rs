#![cfg_attr(not(feature = "rt"), allow(dead_code))]

//! Source of time abstraction.
//!
//! By default, `std::time::Instant::now()` is used. However, when the
//! `test-util` feature flag is enabled, the values returned for `now()` are
//! configurable.

cfg_not_test_util! {
    use crate::time::Instant;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use pausable_clock::PausableClock;

    /// A source of `Instant::now()` that can optionally be paused.
    ///
    /// When constructed via `Clock::new`, the clock is a no-op wrapper around
    /// [`std::time::Instant::now`]. When constructed via
    /// [`Clock::new_pausable`] (indirectly through the
    /// [`Builder::pausable_time`](crate::runtime::Builder::pausable_time)
    /// runtime builder method) the clock is backed by a
    /// [`pausable_clock::PausableClock`] and the runtime's notion of time can
    /// be paused and resumed by calling [`Runtime::pause`](crate::runtime::Runtime::pause)
    /// and [`Runtime::resume`](crate::runtime::Runtime::resume).
    #[derive(Debug, Clone)]
    pub(crate) struct Clock {
        pausable: bool,
        pausing_clock: Arc<PausableClock>,
    }

    pub(crate) fn now() -> Instant {
        Instant::from_std(std::time::Instant::now())
    }

    impl Clock {
        /// Returns `true` when the `test-util` feature is enabled. This is used
        /// by the time driver to know whether it should call `advance` (which
        /// is a test-only concept) or `wait_for_resume`.
        #[allow(dead_code)]
        pub(crate) fn is_test() -> bool {
            false
        }

        pub(crate) fn new(_enable_pausing: bool, _start_paused: bool) -> Clock {
            Clock {
                pausable: false,
                pausing_clock: Arc::new(PausableClock::default()),
            }
        }

        pub(crate) fn new_pausable(start_paused: bool, elapsed_time: Duration) -> Clock {
            Clock {
                pausable: true,
                pausing_clock: Arc::new(PausableClock::new(elapsed_time, start_paused)),
            }
        }

        /// Returns true when this clock can be paused/resumed.
        pub(crate) fn pausable(&self) -> bool {
            self.pausable
        }

        pub(crate) fn now(&self) -> Instant {
            if self.pausable {
                // `PausableInstant` converts into `std::time::Instant` via
                // `From`. The conversion preserves the pausable clock's view
                // of time so that the runtime sees the paused instant.
                Instant::from_std(std::time::Instant::from(self.pausing_clock.now()))
            } else {
                now()
            }
        }

        /// Returns the number of milliseconds elapsed since the pausable clock
        /// was created. Panics if the clock is not pausable.
        pub(crate) fn elapsed_millis(&self) -> u64 {
            if self.pausable {
                self.pausing_clock.now().elapsed_millis()
            } else {
                panic!("elapsed_millis is only supported on pausable clocks")
            }
        }

        pub(crate) fn is_paused(&self) -> bool {
            if self.pausable {
                self.pausing_clock.is_paused()
            } else {
                false
            }
        }

        pub(crate) fn is_paused_ordered(&self, ordering: Ordering) -> bool {
            if self.pausable {
                self.pausing_clock.is_paused_ordered(ordering)
            } else {
                false
            }
        }

        pub(crate) fn pause(&self) -> bool {
            if self.pausable {
                self.pausing_clock.pause()
            } else {
                panic!("this runtime was not configured to be pausable. \
                    Use `Builder::pausable_time` to enable pausable time.");
            }
        }

        pub(crate) fn resume(&self) -> bool {
            if self.pausable {
                self.pausing_clock.resume()
            } else {
                panic!("this runtime was not configured to be pausable. \
                    Use `Builder::pausable_time` to enable pausable time.");
            }
        }

        /// Runs `action` while preventing the clock from being paused. If the
        /// clock is paused when this is called, the call blocks until the
        /// clock is resumed.
        pub(crate) fn run_unpausable<T, F>(&self, action: F) -> T
        where
            F: FnOnce() -> T,
        {
            if self.pausable {
                self.pausing_clock.run_unpausable(action)
            } else {
                action()
            }
        }

        /// Runs `action` while preventing the clock from being resumed. If the
        /// clock is running when this is called, the call blocks until the
        /// clock is paused.
        pub(crate) fn run_unresumable<T, F>(&self, action: F) -> T
        where
            F: FnOnce() -> T,
        {
            if self.pausable {
                self.pausing_clock.run_unresumable(action)
            } else {
                // Running an unresumable action on a non-pausable clock would
                // otherwise block forever since the clock can never be paused.
                unreachable!(
                    "run_unresumable called on non-pausable clock; this would block forever"
                );
            }
        }

        /// Runs `action` atomically if the clock is currently resumed,
        /// returning `None` if the clock is paused.
        pub(crate) fn run_if_resumed<T, F>(&self, action: F) -> Option<T>
        where
            F: FnOnce() -> T,
        {
            if self.pausable {
                self.pausing_clock.run_if_resumed(action)
            } else {
                Some(action())
            }
        }

        /// Runs `action` atomically if the clock is currently paused,
        /// returning `None` if the clock is resumed.
        pub(crate) fn run_if_paused<T, F>(&self, action: F) -> Option<T>
        where
            F: FnOnce() -> T,
        {
            if self.pausable {
                self.pausing_clock.run_if_paused(action)
            } else {
                None
            }
        }

        /// Blocks the current thread until the clock is resumed. If the clock
        /// is not currently paused, or not pausable, this returns immediately.
        pub(crate) fn wait_for_resume(&self) {
            if self.pausable {
                self.pausing_clock.wait_for_resume();
            }
        }

        /// Blocks the current thread until the clock is paused. If the clock is
        /// not pausable, this does nothing.
        pub(crate) fn wait_for_pause(&self) {
            if self.pausable {
                self.pausing_clock.wait_for_pause();
            }
        }
    }
}

cfg_test_util! {
    use crate::time::{Duration, Instant};
    use crate::loom::sync::Mutex;
    use crate::loom::sync::atomic::Ordering;
    use std::sync::atomic::AtomicBool as StdAtomicBool;

    cfg_rt! {
        #[track_caller]
        fn with_clock<R>(f: impl FnOnce(Option<&Clock>) -> Result<R, &'static str>) -> R {
            use crate::runtime::Handle;

            let res = match Handle::try_current() {
                Ok(handle) => f(Some(handle.inner.driver().clock())),
                Err(ref e) if e.is_missing_context() => f(None),
                Err(_) => panic!("{}", crate::util::error::THREAD_LOCAL_DESTROYED_ERROR),
            };

            match res {
                Ok(ret) => ret,
                Err(msg) => panic!("{}", msg),
            }
        }
    }

    cfg_not_rt! {
        #[track_caller]
        fn with_clock<R>(f: impl FnOnce(Option<&Clock>) -> Result<R, &'static str>) -> R {
            match f(None) {
                Ok(ret) => ret,
                Err(msg) => panic!("{}", msg),
            }
        }
    }

    /// A handle to a source of time.
    #[derive(Debug)]
    pub(crate) struct Clock {
        inner: Mutex<Inner>,
    }

    // Used to track if the clock was ever paused. This is an optimization to
    // avoid touching the mutex if `test-util` was accidentally enabled in
    // release mode.
    //
    // A static is used so we can avoid accessing the thread-local as well. The
    // `std` AtomicBool is used directly because loom does not support static
    // atomics.
    static DID_PAUSE_CLOCK: StdAtomicBool = StdAtomicBool::new(false);

    #[derive(Debug)]
    struct Inner {
        /// True if the ability to pause time is enabled.
        enable_pausing: bool,

        /// Instant to use as the clock's base instant.
        base: std::time::Instant,

        /// Instant at which the clock was last unfrozen.
        unfrozen: Option<std::time::Instant>,

        /// Number of `inhibit_auto_advance` calls still in effect.
        auto_advance_inhibit_count: usize,
    }

    /// Pauses time.
    ///
    /// The current value of `Instant::now()` is saved and all subsequent calls
    /// to `Instant::now()` will return the saved value. The saved value can be
    /// changed by [`advance`] or by the time auto-advancing once the runtime
    /// has no work to do. This only affects the `Instant` type in Tokio, and
    /// the `Instant` in std continues to work as normal.
    ///
    /// Pausing time requires the `current_thread` Tokio runtime. This is the
    /// default runtime used by `#[tokio::test]`. The runtime can be initialized
    /// with time in a paused state using the `Builder::start_paused` method.
    ///
    /// For cases where time is immediately paused, it is better to pause
    /// the time using the `main` or `test` macro:
    /// ```
    /// #[tokio::main(flavor = "current_thread", start_paused = true)]
    /// async fn main() {
    ///    println!("Hello world");
    /// }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if time is already frozen or if called from outside of a
    /// `current_thread` Tokio runtime.
    ///
    /// # Auto-advance
    ///
    /// If time is paused and the runtime has no work to do, the clock is
    /// auto-advanced to the next pending timer. This means that [`Sleep`] or
    /// other timer-backed primitives can cause the runtime to advance the
    /// current time when awaited.
    ///
    /// # Preventing auto-advance
    ///
    /// In some testing scenarios, you may want to keep the clock paused without
    /// auto-advancing, even while waiting for I/O or other asynchronous operations.
    /// This can be achieved by using [`spawn_blocking`] to wrap your I/O operations.
    ///
    /// When a blocking task is running, the clock's auto-advance is temporarily
    /// inhibited. This allows you to wait for I/O to complete while keeping the
    /// paused clock stationary:
    ///
    /// ```ignore
    /// use tokio::time::{Duration, Instant};
    /// use tokio::task;
    ///
    /// #[tokio::test(start_paused = true)]
    /// async fn test_with_io() {
    ///     let start = Instant::now();
    ///
    ///     // The clock will NOT auto-advance while this blocking task runs
    ///     let result = task::spawn_blocking(|| {
    ///         // Perform I/O operations here
    ///         std::thread::sleep(std::time::Duration::from_millis(10));
    ///         42
    ///     }).await.unwrap();
    ///
    ///     // Time has not advanced
    ///     assert_eq!(start.elapsed(), Duration::ZERO);
    /// }
    /// ```
    ///
    /// [`Sleep`]: crate::time::Sleep
    /// [`advance`]: crate::time::advance
    /// [`spawn_blocking`]: crate::task::spawn_blocking
    #[track_caller]
    pub fn pause() {
        with_clock(|maybe_clock| {
            match maybe_clock {
                Some(clock) => clock.pause(),
                None => Err("time cannot be frozen from outside the Tokio runtime"),
            }
        });
    }

    /// Resumes time.
    ///
    /// Clears the saved `Instant::now()` value. Subsequent calls to
    /// `Instant::now()` will return the value returned by the system call.
    ///
    /// # Panics
    ///
    /// Panics if time is not frozen or if called from outside of the Tokio
    /// runtime.
    #[track_caller]
    pub fn resume() {
        with_clock(|maybe_clock| {
            let clock = match maybe_clock {
                Some(clock) => clock,
                None => return Err("time cannot be frozen from outside the Tokio runtime"),
            };

            let mut inner = clock.inner.lock();

            if inner.unfrozen.is_some() {
                return Err("time is not frozen");
            }

            inner.unfrozen = Some(std::time::Instant::now());
            Ok(())
        });
    }

    /// Advances time.
    ///
    /// Increments the saved `Instant::now()` value by `duration`. Subsequent
    /// calls to `Instant::now()` will return the result of the increment.
    ///
    /// This function will make the current time jump forward by the given
    /// duration in one jump. This means that all `sleep` calls with a deadline
    /// before the new time will immediately complete "at the same time", and
    /// the runtime is free to poll them in any order.  Additionally, this
    /// method will not wait for the `sleep` calls it advanced past to complete.
    /// If you want to do that, you should instead call [`sleep`] and rely on
    /// the runtime's auto-advance feature.
    ///
    /// Note that calls to `sleep` are not guaranteed to complete the first time
    /// they are polled after a call to `advance`. For example, this can happen
    /// if the runtime has not yet touched the timer driver after the call to
    /// `advance`. However if they don't, the runtime will poll the task again
    /// shortly.
    ///
    /// # When to use `sleep` instead
    ///
    /// **Important:** `advance` is designed for testing scenarios where you want to
    /// instantly jump forward in time. However, it has limitations that make it
    /// unsuitable for certain use cases:
    ///
    /// - **Forcing timeouts:** If you want to reliably trigger a timeout, prefer
    ///   using [`sleep`] with auto-advance rather than `advance`. The `advance`
    ///   function jumps time forward but doesn't guarantee that all timers will be
    ///   processed before your code continues.
    ///
    /// - **Simulating freezes:** If you're trying to simulate a scenario where the
    ///   program freezes and then resumes, the batch behavior of `advance` may not
    ///   produce the expected results. All timers that expire during the advance
    ///   complete simultaneously.
    ///
    /// For most testing scenarios where you want to wait for a duration to pass
    /// and have all timers fire in order, use [`sleep`] instead:
    ///
    /// ```ignore
    /// use tokio::time::{self, Duration};
    ///
    /// #[tokio::test(start_paused = true)]
    /// async fn test_timeout_reliable() {
    ///     // Use sleep with auto-advance for reliable timeout testing
    ///     time::sleep(Duration::from_secs(5)).await;
    ///     // All timers that were scheduled to fire within 5 seconds
    ///     // have now been processed in order
    /// }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if any of the following conditions are met:
    ///
    /// - The clock is not frozen, which means that you must
    ///   call [`pause`] before calling this method.
    /// - If called outside of the Tokio runtime.
    /// - If the input `duration` is too large (such as [`Duration::MAX`])
    ///   to be safely added to the current time without causing an overflow.
    ///
    /// # Caveats
    ///
    /// Using a very large `duration` is not recommended,
    /// as it may cause panicking due to overflow.
    ///
    /// # Auto-advance
    ///
    /// If the time is paused and there is no work to do, the runtime advances
    /// time to the next timer. See [`pause`](pause#auto-advance) for more
    /// details.
    ///
    /// [`sleep`]: fn@crate::time::sleep
    pub async fn advance(duration: Duration) {
        with_clock(|maybe_clock| {
            let clock = match maybe_clock {
                Some(clock) => clock,
                None => return Err("time cannot be frozen from outside the Tokio runtime"),
            };

            clock.advance(duration)
        });

        crate::task::yield_now().await;
    }

    /// Returns the current instant, factoring in frozen time.
    pub(crate) fn now() -> Instant {
        if !DID_PAUSE_CLOCK.load(Ordering::Acquire) {
            return Instant::from_std(std::time::Instant::now());
        }

        with_clock(|maybe_clock| {
            Ok(if let Some(clock) = maybe_clock {
                clock.now()
            } else {
                Instant::from_std(std::time::Instant::now())
            })
        })
    }

    impl Clock {
        /// Returns a new `Clock` instance that uses the current execution context's
        /// source of time.
        pub(crate) fn new(enable_pausing: bool, start_paused: bool) -> Clock {
            let now = std::time::Instant::now();

            let clock = Clock {
                inner: Mutex::new(Inner {
                    enable_pausing,
                    base: now,
                    unfrozen: Some(now),
                    auto_advance_inhibit_count: 0,
                }),
            };

            if start_paused {
                if let Err(msg) = clock.pause() {
                    panic!("{}", msg);
                }
            }

            clock
        }

        pub(crate) fn pause(&self) -> Result<(), &'static str> {
            let mut inner = self.inner.lock();

            if !inner.enable_pausing {
                return Err("`time::pause()` requires the `current_thread` Tokio runtime. \
                        This is the default Runtime used by `#[tokio::test].");
            }

            // Track that we paused the clock
            DID_PAUSE_CLOCK.store(true, Ordering::Release);

            let elapsed = match inner.unfrozen.as_ref() {
                Some(v) => v.elapsed(),
                None => return Err("time is already frozen")
            };
            inner.base += elapsed;
            inner.unfrozen = None;

            Ok(())
        }

        /// Temporarily stop auto-advancing the clock (see `tokio::time::pause`).
        pub(crate) fn inhibit_auto_advance(&self) {
            let mut inner = self.inner.lock();
            inner.auto_advance_inhibit_count += 1;
        }

        pub(crate) fn allow_auto_advance(&self) {
            let mut inner = self.inner.lock();
            inner.auto_advance_inhibit_count -= 1;
        }

        pub(crate) fn can_auto_advance(&self) -> bool {
            let inner = self.inner.lock();
            inner.unfrozen.is_none() && inner.auto_advance_inhibit_count == 0
        }

        pub(crate) fn advance(&self, duration: Duration) -> Result<(), &'static str> {
            let mut inner = self.inner.lock();

            if inner.unfrozen.is_some() {
                return Err("time is not frozen");
            }

            inner.base += duration;
            Ok(())
        }

        pub(crate) fn now(&self) -> Instant {
            let inner = self.inner.lock();

            let mut ret = inner.base;

            if let Some(unfrozen) = inner.unfrozen {
                ret += unfrozen.elapsed();
            }

            Instant::from_std(ret)
        }

        // Extra methods that mirror the `cfg_not_test_util` variant so that
        // `Runtime` can expose a uniform pausability API regardless of the
        // `test-util` feature. These generally defer to the existing
        // test-util primitives or are no-ops where no equivalent exists.

        #[allow(dead_code)]
        pub(crate) fn is_test() -> bool {
            true
        }

        #[allow(dead_code)]
        pub(crate) fn new_pausable(start_paused: bool, _elapsed_time: Duration) -> Clock {
            // When `test-util` is enabled we always run the test clock with
            // pausability enabled so that `Runtime::pause` / `Runtime::resume`
            // are available in tests.
            Self::new(true, start_paused)
        }

        #[allow(dead_code)]
        pub(crate) fn pausable(&self) -> bool {
            let inner = self.inner.lock();
            inner.enable_pausing
        }

        #[allow(dead_code)]
        pub(crate) fn elapsed_millis(&self) -> u64 {
            let inner = self.inner.lock();
            let mut ret = inner.base;
            if let Some(unfrozen) = inner.unfrozen {
                ret += unfrozen.elapsed();
            }
            ret.duration_since(inner.base - inner.base.elapsed())
                .as_millis() as u64
        }

        pub(crate) fn is_paused_ordered(&self, _ordering: crate::loom::sync::atomic::Ordering) -> bool {
            self.is_paused()
        }

        // The trait's public `pause()` returns `Result<(), &'static str>` to
        // match historical behavior; the runtime-level `pause` simply returns
        // whether the call succeeded.
        #[allow(dead_code)]
        pub(crate) fn try_pause(&self) -> bool {
            match self.pause() {
                Ok(()) => true,
                Err(_) => false,
            }
        }

        #[allow(dead_code)]
        pub(crate) fn try_resume(&self) -> bool {
            let mut inner = self.inner.lock();
            if inner.unfrozen.is_some() {
                return false;
            }
            inner.unfrozen = Some(std::time::Instant::now());
            true
        }

        pub(crate) fn is_paused(&self) -> bool {
            let inner = self.inner.lock();
            inner.unfrozen.is_none()
        }

        /// No-op stub for compatibility with the pausable_clock-backed
        /// implementation. In test mode, no separate `run_unpausable` mechanism
        /// is needed because `time::pause` is controlled by the user directly.
        pub(crate) fn run_unpausable<T, F>(&self, action: F) -> T
        where
            F: FnOnce() -> T,
        {
            action()
        }

        #[allow(dead_code)]
        pub(crate) fn run_unresumable<T, F>(&self, _action: F) -> T
        where
            F: FnOnce() -> T,
        {
            unreachable!("run_unresumable is not supported when `test-util` is enabled")
        }

        #[allow(dead_code)]
        pub(crate) fn run_if_resumed<T, F>(&self, action: F) -> Option<T>
        where
            F: FnOnce() -> T,
        {
            if self.is_paused() {
                None
            } else {
                Some(action())
            }
        }

        #[allow(dead_code)]
        pub(crate) fn run_if_paused<T, F>(&self, action: F) -> Option<T>
        where
            F: FnOnce() -> T,
        {
            if self.is_paused() {
                Some(action())
            } else {
                None
            }
        }

        #[allow(dead_code)]
        pub(crate) fn wait_for_resume(&self) {
            // The test-util clock has no blocking notification for resume.
            // The time driver uses `can_auto_advance` + `advance` instead of
            // calling `wait_for_resume` when the `test-util` feature is
            // enabled, so this should never be reached.
            unreachable!("wait_for_resume is not supported when `test-util` is enabled")
        }

        #[allow(dead_code)]
        pub(crate) fn wait_for_pause(&self) {
            unreachable!("wait_for_pause is not supported when `test-util` is enabled")
        }
    }
}
