#![cfg_attr(not(feature = "rt"), allow(dead_code))]

//! Source of time abstraction.
//!
//! By default, `std::time::Instant::now()` is used. However, when the
//! `test-util` feature flag is enabled, the values returned for `now()` are
//! configurable.

cfg_not_test_util! {
    use crate::time::{Duration, Instant};
    use std::sync::{Arc, atomic::Ordering};
    use pausable_clock::PausableClock;

    #[derive(Debug, Clone)]
    pub(crate) struct Clock {
        pausable: bool,
        pausing_clock: Arc<PausableClock>
    }

    pub(crate) fn now() -> Instant {
        Instant::from_std(std::time::Instant::now())
    }

    impl Clock {

        pub(crate) fn is_test() -> bool {
            false
        }

        pub(crate) fn new() -> Clock {
            Clock {
                pausable: false,
                pausing_clock: Arc::new(PausableClock::default())
            }
        }

        pub(crate) fn new_pausable(paused: bool, elapsed_time: std::time::Duration) -> Clock {
            Clock {
                pausable: true,
                pausing_clock: Arc::new(PausableClock::new(elapsed_time, paused))
            }
        }

        pub(crate) fn pausable(&self) -> bool {
            self.pausable
        }

        pub(crate) fn now(&self) -> Instant {
            if self.pausable {
                Instant::from_std(self.pausing_clock.now_std())
            }
            else {
                now()
            }
        }

        pub(crate) fn elapsed_millis(&self) -> u64 {
            if self.pausable {
                self.pausing_clock.now().elapsed_millis()
            }
            else {
                panic!("elapsed time is not supported for non-pausable clocks")
            }
        }

        pub(crate) fn is_paused(&self) -> bool {
            if self.pausable {
                self.pausing_clock.is_paused()
            }
            else {
                false
            }
        }

        pub(crate) fn is_paused_ordered(&self, ordering: Ordering) -> bool {
            if self.pausable {
                self.pausing_clock.is_paused_ordered(ordering)
            }
            else {
                false
            }
        }

        pub(crate) fn advance(&self, _dur: Duration) {
            unreachable!();
        }

        pub(crate) fn pause(&self) -> bool {
            if self.pausable {
                self.pausing_clock.pause()
            }
            else {
                panic!("Not pausable");
            }
        }

        pub(crate) fn resume(&self) -> bool {
            if self.pausable {
                self.pausing_clock.resume()
            }
            else {
                panic!("Not pausable");
            }
        }

        pub(crate) fn run_unpausable<T,F>(&self, action: F) -> T
            where F : FnOnce() -> T
        {
            if self.pausable {
                self.pausing_clock.run_unpausable(action)
            }
            else {
                action()
            }
        }

        pub(crate) fn run_unresumable<T,F>(&self, action: F) -> T
            where F : FnOnce() -> T
        {
            if self.pausable {
                self.pausing_clock.run_unresumable(action)
            }
            else {
                unreachable!("I think this is better than blocking forever");
            }
        }

        pub(crate) fn run_if_resumed<T,F>(&self, action: F) -> Option<T>
            where F : FnOnce() -> T
        {
            if self.pausable {
                self.pausing_clock.run_if_resumed(action)
            }
            else {
                Some(action())
            }
        }

        pub(crate) fn run_if_paused<T,F>(&self, action: F) -> Option<T>
            where F : FnOnce() -> T
        {
            if self.pausable {
                self.pausing_clock.run_if_paused(action)
            }
            else {
                None
            }
        }

        pub(crate) fn wait_for_resume(&self) {
            if self.pausable {
                self.pausing_clock.wait_for_resume();
            }
        }

        pub(crate) fn wait_for_pause(&self) {
            if self.pausable {
                self.pausing_clock.wait_for_pause();
            }
        }
    }
}

cfg_test_util! {
    use crate::time::{Duration, Instant};
    use std::sync::{ Arc, Mutex, atomic::Ordering };
    use crate::runtime::context;

    /// A handle to a source of time.
    #[derive(Debug, Clone)]
    pub(crate) struct Clock {
        inner: Arc<Mutex<Inner>>,
    }

    cfg_rt! {
        fn clock() -> Option<Clock> {
            crate::runtime::context::clock()
        }
    }

    cfg_not_rt! {
        fn clock() -> Option<Clock> {
            None
        }
    }

    #[derive(Debug)]
    struct Inner {
        /// Instant to use as the clock's base instant.
        base: std::time::Instant,

        /// Instant at which the clock was last unfrozen
        unfrozen: Option<std::time::Instant>,
    }

    /// Pause time
    ///
    /// The current value of `Instant::now()` is saved and all subsequent calls
    /// to `Instant::now()` until the timer wheel is checked again will return the saved value.
    /// Once the timer wheel is checked, time will immediately advance to the next registered
    /// `Sleep`. This is useful for running tests that depend on time.
    ///
    /// # Panics
    ///
    /// Panics if time is already frozen or if called from outside of the Tokio
    /// runtime.
    pub fn pause() {
        let clock = clock().expect("time cannot be frozen from outside the Tokio runtime");
        clock.pause();
    }

    /// Resume time
    ///
    /// Clears the saved `Instant::now()` value. Subsequent calls to
    /// `Instant::now()` will return the value returned by the system call.
    ///
    /// # Panics
    ///
    /// Panics if time is not frozen or if called from outside of the Tokio
    /// runtime.
    pub fn resume() {
        let clock = clock().expect("time cannot be frozen from outside the Tokio runtime");
        let mut inner = clock.inner.lock().unwrap();

        if inner.unfrozen.is_some() {
            panic!("time is not frozen");
        }

        inner.unfrozen = Some(std::time::Instant::now());
    }


    /// Advance time
    ///
    /// Increments the saved `Instant::now()` value by `duration`. Subsequent
    /// calls to `Instant::now()` will return the result of the increment.
    ///
    /// # Panics
    ///
    /// Panics if time is not frozen or if called from outside of the Tokio
    /// runtime.
    pub async fn advance(duration: Duration) {
        use crate::future::poll_fn;
        use std::task::Poll;

        let clock = clock().expect("time cannot be frozen from outside the Tokio runtime");
        clock.advance(duration);

        let mut yielded = false;
        poll_fn(|cx| {
            if yielded {
                Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }).await;
    }

    /// Return the current instant, factoring in frozen time.
    pub(crate) fn now() -> Instant {
        if let Some(clock) = clock() {
            clock.now()
        } else {
            Instant::from_std(std::time::Instant::now())
        }
    }

    impl Clock {


        pub(crate) fn is_test() -> bool {
            true
        }

        /// Return a new `Clock` instance that uses the current execution context's
        /// source of time.
        pub(crate) fn new() -> Clock {
            let now = std::time::Instant::now();

            Clock {
                inner: Arc::new(Mutex::new(Inner {
                    base: now,
                    unfrozen: Some(now),
                })),
            }
        }

        #[allow(dead_code)]
        pub(crate) fn new_pausable(_pausable: bool, _elapsed_time: std::time::Duration) -> Clock {
            Self::new()
        }

        pub(crate) fn pause(&self) -> bool {
            let mut inner = self.inner.lock().unwrap();

            let elapsed = inner.unfrozen.as_ref().expect("time is already frozen").elapsed();
            inner.base += elapsed;
            inner.unfrozen = None;

            true
        }

        pub(crate) fn is_paused(&self) -> bool {
            let inner = self.inner.lock().unwrap();
            inner.unfrozen.is_none()
        }

        pub(crate) fn is_paused_ordered(&self, _: Ordering) -> bool {
            self.is_paused()
        }

        pub(crate) fn resume(&self) -> bool {
            self.advance(Default::default());
            true
        }

        pub(crate) fn advance(&self, duration: Duration) {
            let mut inner = self.inner.lock().unwrap();

            if inner.unfrozen.is_some() {
                panic!("time is not frozen");
            }

            inner.base += duration;
        }

        pub(crate) fn now(&self) -> Instant {
            let inner = self.inner.lock().unwrap();

            let mut ret = inner.base;

            if let Some(unfrozen) = inner.unfrozen {
                ret += unfrozen.elapsed();
            }

            Instant::from_std(ret)
        }

        pub(crate) fn elapsed_millis(&self) -> u64 {
            unreachable!("Not implemented for tests");
        }

        #[allow(dead_code)]
        pub(crate) fn run_unpausable<T,F>(&self, action: F) -> T
            where F : FnOnce() -> T
        {
            action()
        }

        pub(crate) fn run_unresumable<T,F>(&self, action: F) -> T
            where F : FnOnce() -> T
        {
            unreachable!("Not implemented for tests");
        }

        pub(crate) fn run_if_resumed<T,F>(&self, action: F) -> Option<T>
            where F : FnOnce() -> T
        {
            Some(action())
        }

        pub(crate) fn run_if_paused<T,F>(&self, action: F) -> Option<T>
            where F : FnOnce() -> T
        {
            None
        }

        pub(crate) fn wait_for_resume(&self) {
            unreachable!("Not implemented for tests");
        }

        pub(crate) fn wait_for_pause(&self) {
            unreachable!("Not implemented for tests");
        }
    }
}
