use std::io::IsTerminal;
use std::path::Path;

use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::Fetcher;
use crate::commands::init::yes_no;
use crate::commands::load_schema;
use crate::env::Env;
use crate::error::CliError;
use crate::files::show;
use crate::output::{Output, Report, table};
use crate::source;

/// Names, types and which keys have a value. Values are never printed.
pub fn run(
    out: &Output,
    cwd: &Path,
    env_flag: Option<&str>,
    env: &Env,
) -> Result<Report, CliError> {
    let (schema_path, schema) = load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd);
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let environment = source::environment(env_flag, env, &schema, dir);
    let mut fetcher = Fetcher::new(env, &detection);
    let resolved = source::values(&schema, dir, &environment, env, &mut fetcher)?;
    source::report(&resolved, dir);
    let source = resolved
        .cloud
        .clone()
        .or_else(|| (!resolved.layers.is_empty()).then(|| resolved.layers.names()));
    let values = resolved.values;

    let present = |name: &str| {
        if values.get(name).is_some_and(|v: &String| !v.is_empty()) {
            "present"
        } else {
            "missing"
        }
    };

    let rows: Vec<Vec<String>> = schema
        .keys
        .iter()
        .map(|key| {
            vec![
                key.name.clone(),
                key.ty.to_string(),
                yes_no(key.required),
                yes_no(key.sensitive),
                present(&key.name).to_string(),
            ]
        })
        .collect();

    let text = table(
        &["KEY", "TYPE", "REQUIRED", "SENSITIVE", "VALUE"],
        &rows,
        &out.style(),
    );

    Ok(Report::new(
        json!({
            "schema": show(&schema_path),
            "env": source,
            "keys": schema.keys.iter().map(|key| json!({
                "name": key.name,
                "type": key.ty.to_string(),
                "required": key.required,
                "sensitive": key.sensitive,
                "value": present(&key.name),
            })).collect::<Vec<_>>(),
        }),
        text,
    )
    .listing())
}
