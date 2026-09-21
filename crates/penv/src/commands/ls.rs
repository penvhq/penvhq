use std::path::Path;

use serde_json::json;

use crate::commands::init::yes_no;
use crate::commands::load_schema;
use crate::error::CliError;
use crate::files::{ENV_FILE, read_file, show};
use crate::output::{Output, Report, table};

/// Names, types and which keys have a value. Values are never printed.
pub fn run(out: &Output, cwd: &Path) -> Result<Report, CliError> {
    let (schema_path, schema) = load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd);
    let env_path = dir.join(ENV_FILE);
    let values = if env_path.is_file() {
        penv_dotenv::read(&read_file(&env_path)?).values()
    } else {
        Default::default()
    };

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
            "env": env_path.is_file().then(|| show(&env_path)),
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
