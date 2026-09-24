use serde_json::{Value, json};

/// The exit code table the design publishes. `help --json` emits it.
pub const EXIT_CODES: [(i32, &str, &str); 7] = [
    (0, "ok", "the command did what it says"),
    (1, "error", "something went wrong"),
    (2, "auth", "not signed in, or the credential was rejected"),
    (3, "validation", "the schema or the values did not pass"),
    (
        4,
        "confirmation",
        "a human has to confirm; the JSON carries the replay command",
    ),
    (
        5,
        "no_credential",
        "no credential is available and none can be obtained",
    ),
    (
        6,
        "environment_refused",
        "this identity may not read that environment, or its values are write-only",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Exit {
    Ok = 0,
    Error = 1,
    Auth = 2,
    Validation = 3,
    Confirmation = 4,
    NoCredential = 5,
    EnvironmentRefused = 6,
}

/// A refusal the user can act on: what failed, and the one thing to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub code: &'static str,
    pub message: String,
    pub fix: String,
    pub exit: Exit,
    /// What the caller needs beyond the words, such as the approval id an exit 4
    /// is replayed with.
    pub details: Vec<(&'static str, Value)>,
}

impl CliError {
    pub fn new(code: &'static str, message: impl Into<String>, fix: impl Into<String>) -> CliError {
        CliError {
            code,
            message: message.into(),
            fix: fix.into(),
            exit: Exit::Error,
            details: Vec::new(),
        }
    }

    pub fn with_exit(mut self, exit: Exit) -> CliError {
        self.exit = exit;
        self
    }

    /// One more member on the JSON. A null is left out, since the contract asks
    /// for absent fields to be absent.
    pub fn with(mut self, name: &'static str, value: Value) -> CliError {
        if !value.is_null() {
            self.details.push((name, value));
        }
        self
    }

    pub fn to_json(&self) -> Value {
        let mut json = json!({ "error": self.code, "message": self.message, "fix": self.fix });
        if let Some(object) = json.as_object_mut() {
            for (name, value) in &self.details {
                object.insert((*name).to_string(), value.clone());
            }
        }
        json
    }
}
