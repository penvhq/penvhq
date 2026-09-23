//! The penv binary: the command tree, the manifest it publishes, the output
//! contract and the exit codes. I/O lives here; the parsing and validation it
//! calls are pure.

pub mod agent;
pub mod ancestry;
pub mod claim;
pub mod cli;
pub mod commands;
pub mod completions;
pub mod config;
pub mod env;
pub mod error;
pub mod files;
pub mod gitexposure;
pub mod manifest;
pub mod output;
pub mod preload;
pub mod prompt;
pub mod providers;
pub mod source;
pub mod ui;
pub mod upgrade;
