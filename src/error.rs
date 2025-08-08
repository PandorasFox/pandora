#[derive(Debug)]
pub struct CommandError {
    pub response: String,
}

impl CommandError {
    pub fn new(s: &str) -> DaemonError {
        return DaemonError::CommandError(CommandError{
            response: s.to_string()
        })
    }
}

#[derive(Debug)]
pub enum DaemonError {
    IoError(std::io::Error),
    ImageError(image::ImageError),
    CommandError(CommandError),
    PoisonError, // typedef annoying and doesn't really add much. "a mutex got fucked" is all that really matters
}

impl From<std::io::Error> for DaemonError {
    fn from(err: std::io::Error) -> DaemonError {
        DaemonError::IoError(err)
    }
}

impl From<image::ImageError> for DaemonError {
    fn from(err: image::ImageError) -> DaemonError {
        DaemonError::ImageError(err)
    }
}

impl<T> From<std::sync::PoisonError<T>> for DaemonError {
    fn from(_: std::sync::PoisonError<T>) -> DaemonError {
        DaemonError::PoisonError
    }
}

/*
impl From<std::sync::mpsc::SendError<ThreadCommand>> for DaemonError {
    fn from(_: std::sync::mpsc::SendError<ThreadCommand>) -> DaemonError {
        DaemonError::ThreadProbablyDead
    }
}
*/