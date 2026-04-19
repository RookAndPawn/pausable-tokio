use std::time::Duration;

/// Configuration for a runtime that uses a pausable clock.
///
/// This is populated by [`Builder::pausable_time`](crate::runtime::Builder::pausable_time)
/// and, when set, causes the runtime's clock to be backed by
/// [`pausable_clock::PausableClock`], allowing it to be paused and resumed at
/// runtime via [`Runtime::pause`](crate::runtime::Runtime::pause) /
/// [`Runtime::resume`](crate::runtime::Runtime::resume).
#[derive(Debug, Copy, Clone, Default)]
pub(crate) struct PausableTimeConfig {
    /// If `true`, the runtime starts with the clock already paused.
    pub(crate) start_paused: bool,

    /// Initial value of elapsed time reported by the clock. Using a non-zero
    /// value is useful if the runtime is resuming from a previously persisted
    /// state.
    pub(crate) elapsed_time: Duration,
}
