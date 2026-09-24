use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotFound {
        name: String,
        looked: Vec<String>,
    },
    Malformed {
        dir: String,
        message: String,
    },
    /// A repository folder named after a built-in guard.
    Shadows {
        name: String,
        dir: String,
    },
    Render {
        guard: String,
        message: String,
    },
    /// The file on disk is not the format the guard says it is.
    Unreadable {
        path: String,
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound { name, looked } => {
                write!(f, "no guard named {name}; looked in {}", looked.join(", "))
            }
            Error::Malformed { dir, message } => write!(f, "{dir} is not a guard: {message}"),
            Error::Shadows { name, dir } => write!(
                f,
                "{dir} would replace penv's built-in {name} guard, and a folder committed to the repository may only add a harness. Rename the folder, or move it to ~/.penv/guards/{name} to change {name} on this machine"
            ),
            Error::Render { guard, message } => {
                write!(f, "guard {guard} failed to render: {message}")
            }
            Error::Unreadable { path, message } => {
                write!(f, "{path} could not be merged: {message}")
            }
        }
    }
}

impl std::error::Error for Error {}
