//! `.env.schema` as a value: parse it, project it to JSON, validate values against
//! it, and render it back. No I/O lives here.

pub mod editor;
mod ir;
mod parse;
pub mod placeholder;
mod render;
pub mod resolve;
pub mod rotate;
mod validate;

pub use ir::{
    Assert, BaseType, Diagnostic, Import, Key, PUBLIC_PREFIXES, RequiredDefault, SCHEMA_VERSION,
    Schema, Type, is_public_prefixed, is_valid_key_name,
};
pub use parse::parse;
pub use render::{render, render_key, set_header};
pub use validate::{
    Values, Violation, extras, is_absolute_url, is_email, parse_boolean, validate, validate_key,
};
