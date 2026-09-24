use thiserror::Error;

pub type Result<T> = std::result::Result<T, CloudError>;

/// The server's refusal as one shape: the status, the `error` code its body
/// carried, and the seconds it asked us to wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(status: u16, code: impl Into<String>) -> ApiError {
        ApiError {
            status,
            code: code.into(),
            retry_after: None,
        }
    }

    pub fn after(mut self, seconds: Option<u64>) -> ApiError {
        self.retry_after = seconds;
        self
    }

    pub fn is(&self, code: &str) -> bool {
        self.code == code
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.code, self.status)
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CloudError {
    #[error("the server answered {0}")]
    Api(#[from] ApiError),

    #[error("{url} could not be reached: {reason}")]
    Offline { url: String, reason: String },

    #[error("{url} redirected, and penv follows no redirect a credential could leak through")]
    Redirected { url: String },

    #[error("{url} answered something that is not the JSON this route promises: {reason}")]
    Unreadable { url: String, reason: String },

    #[error(
        "{url} answered more bytes than penv will hold, or not the file it asked for: {reason}"
    )]
    Body { url: String, reason: String },

    #[error("{0}")]
    Url(String),

    #[error(
        "the {0} name is made only of dots, which a proxy reads as a step up the path rather than a name"
    )]
    DotSegment(&'static str),

    #[error("the keychain could not be used: {0}")]
    Keychain(String),

    #[error("the cache could not be {0}")]
    Cache(String),

    #[error("no credential")]
    NoCredential,

    #[error("{0}")]
    Credential(String),
}

impl CloudError {
    /// True when nothing reached the server, which is the only case the cache is
    /// allowed to answer on its own.
    pub fn is_offline(&self) -> bool {
        matches!(self, CloudError::Offline { .. })
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            CloudError::Api(e) => Some(e.status),
            _ => None,
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            CloudError::Api(e) => Some(&e.code),
            _ => None,
        }
    }

    pub fn is(&self, code: &str) -> bool {
        self.code() == Some(code)
    }
}
