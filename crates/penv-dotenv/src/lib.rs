//! The plain `.env` format: a reader tolerant of what real files contain, a writer
//! that emits only the safe subset, and inference of a schema draft from a file.

mod cascade;
mod gitignore;
mod infer;
mod read;
mod write;

pub use cascade::{
    NOT_VALUES, cascade, is_environment_name, is_local, is_value_file, overlays, shared,
};
pub use gitignore::{GitignoreUpdate, IGNORE_LINES, ensure_ignored};
pub use infer::{infer, infer_type};
pub use read::{Dotenv, Entry, REDACTED_MARKER, Warning, read, redacted_marker};
pub use write::{WriteError, remove, upsert, write, write_redacted};
