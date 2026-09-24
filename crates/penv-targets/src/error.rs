use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No folder for this name in the repo, the home directory or the binary.
    NotFound {
        name: String,
        looked: Vec<String>,
    },
    /// A name that is not a word, so never a folder or a section.
    Name {
        name: String,
    },
    /// `dir` is where the broken file lives: a folder, or a config section.
    Malformed {
        dir: String,
        message: String,
    },
    /// The `[types]` map has no entry for a base type the schema uses.
    NoTypeFor {
        target: String,
        base: String,
    },
    Render {
        target: String,
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound { name, looked } => {
                write!(f, "no target named {name}; looked in {}", looked.join(", "))
            }
            Error::Name { name } => write!(
                f,
                "{name} is not a target name; a name is a-z, 0-9, - and _"
            ),
            Error::Malformed { dir, message } => write!(f, "{dir} is not a target: {message}"),
            Error::NoTypeFor { target, base } => {
                write!(f, "target {target} has no [types] entry for {base}")
            }
            Error::Render { target, message } => {
                write!(f, "target {target} failed to render: {message}")
            }
        }
    }
}

impl std::error::Error for Error {}
