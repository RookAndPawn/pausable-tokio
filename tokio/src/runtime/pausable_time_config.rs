use std::time::Duration;

#[derive(Debug, Copy, Clone, Default)]
pub(crate) struct PausableTimeConfig {
    pub(crate) start_paused: bool,
    pub(crate) elapsed_time: Duration,
}
