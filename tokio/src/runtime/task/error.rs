use std::any::Any;
use std::fmt;
use std::io;
use std::sync::Mutex;

doc_rt_core! {
    /// Task failed to execute to completion.
    pub struct JoinError {
        repr: Repr,
    }
}

enum Repr {
    Cancelled,
    Panic(Mutex<Box<dyn Any + Send + 'static>>),
}

impl JoinError {
    pub(crate) fn cancelled() -> JoinError {
        JoinError {
            repr: Repr::Cancelled,
        }
    }

    pub(crate) fn panic(err: Box<dyn Any + Send + 'static>) -> JoinError {
        JoinError {
            repr: Repr::Panic(Mutex::new(err)),
        }
    }

    /// Returns true if the error was caused by the task being cancelled
    pub fn is_cancelled(&self) -> bool {
        match &self.repr {
            Repr::Cancelled => true,
            _ => false,
        }
    }
}

impl fmt::Display for JoinError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            Repr::Cancelled => write!(fmt, "cancelled"),
            Repr::Panic(_) => write!(fmt, "panic"),
        }
    }
}

impl fmt::Debug for JoinError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            Repr::Cancelled => write!(fmt, "JoinError::Cancelled"),
            Repr::Panic(_) => write!(fmt, "JoinError::Panic(...)"),
        }
    }
}

impl std::error::Error for JoinError {}

impl From<JoinError> for io::Error {
    fn from(src: JoinError) -> io::Error {
        io::Error::new(
            io::ErrorKind::Other,
            match src.repr {
                Repr::Cancelled => "task was cancelled",
                Repr::Panic(_) => "task panicked",
            },
        )
    }
}
