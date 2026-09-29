//! The security claim, in one place. It differs by state and nothing else
//! rewords it.

use penv_schema::Schema;

pub const LOCAL: &str = "penv validates your .env, keeps values out of your agent's output, and blocks it from reading the file where its harness allows.";

pub const CLOUD: &str = "penv keeps secrets out of the files, the repo and the shell history your coding agent reads, and out of the output it captures. It cannot stop a process running as you from looking, so penv.cloud records every value it hands out as a read by the identity that asked for it.";

pub fn for_schema(schema: &Schema) -> &'static str {
    if schema.is_cloud() { CLOUD } else { LOCAL }
}
