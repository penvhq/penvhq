//! A sealed run: the child holds placeholders for keys with `@hosts`, and the
//! values go into its requests on the way out, through a proxy in this process.

pub mod ca;
pub mod http;
pub mod proxy;
pub mod run;
