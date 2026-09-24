//! Language target loading and rendering over the schema JSON.
//!
//! A target is a `[targets.<name>]` section in `.penv/config.toml` plus an
//! optional `.penv/<name>.tmpl` ([`Settled`]), or a folder holding `target.toml`
//! and `env.tmpl`. The same layout is read
//! from the repository, from the home directory and from inside the binary, so
//! adding a language touches no Rust. [`folder`] is that lookup, shared with the
//! harness guards.

mod detect;
mod error;
pub mod folder;
mod import;
mod load;
mod render;
mod settled;
mod target;
mod view;

pub use detect::{Scan, layout_output, layout_root, output_path, package_of, suggested};
pub use error::Error;
pub use folder::{BuiltIn, Roots, Source, Tree};
pub use import::{Config, extends_of, import_line, join};
pub use load::{BUILT_IN, available, load, merge};
pub use render::render;
pub use settled::{CONFIG, Settled};
pub use target::{
    BASE_TYPES, Check, INT_TYPE, Knob, Layout, OptionValue, Rule, Suggest, Target, word,
};
pub use view::view;
