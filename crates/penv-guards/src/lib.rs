//! Harness guard loading and additive, non-weakening config merging.
//!
//! A guard is a folder: `guard.toml` and the templates it names, found through
//! the shared `penv_targets::folder` lookup. Merging only ever adds; nothing an
//! existing config says is removed or overwritten, so a second run leaves the
//! same bytes as the first.

mod error;
mod guard;
mod load;
mod merge;
mod render;

pub use error::Error;
pub use guard::{Deny, Format, Guard, Hook, Merge, Payload, Probe, Scope, Write, is_installed};
pub use load::{BUILT_IN, Roots, Tree, available, hook, load};
pub use merge::{Outcome, apply};
pub use render::{deny, render};
