use thiserror::Error;

pub type Result<T> = std::result::Result<T, CloudError>;

/// The server's refusal as one shape: the status, the `error` code its body
/// carried, the seconds it asked us to wait, and the `message` it wrote for a
/// person, where the route promises one that is safe to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub retry_after: Option<u64>,
    pub message: Option<String>,
}

impl ApiError {
    pub fn new(status: u16, code: impl Into<String>) -> ApiError {
        ApiError {
            status,
            code: code.into(),
            retry_after: None,
            message: None,
        }
    }

    pub fn after(mut self, seconds: Option<u64>) -> ApiError {
        self.retry_after = seconds;
        self
    }

    pub fn saying(mut self, message: Option<&str>) -> ApiError {
        self.message = message.map(printable).filter(|m| !m.is_empty());
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

/// The longest server text penv repeats.
const MAX_SHOWN: usize = 500;

/// Server text as a terminal may show it: no control or bidi characters, so it
/// cannot move the cursor or reorder what surrounds it, and no longer than
/// [`MAX_SHOWN`] characters.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control() && !is_format(*c))
        .take(MAX_SHOWN)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Bidi controls and zero-width characters.
pub fn is_format(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

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

    #[error("{0} is a workspace slug, and a CI or AWS login names its workspace by id")]
    NotWorkspaceId(String),

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_text_cannot_steer_the_terminal() {
        assert_eq!(
            printable("Use the \x1b[31mid\x1b[0m\ninstead\u{202E}.\u{200B}"),
            "Use the [31mid[0m instead."
        );
        assert_eq!(printable(&"a".repeat(900)).len(), MAX_SHOWN);
        assert_eq!(ApiError::new(400, "x").saying(Some(" \n ")).message, None);
    }
}
