use std::io::IsTerminal;
use std::path::Path;

use penv_cloud::api::Fetched;
use penv_schema::Values;
use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::{Cloud, address, environment, refuse};
use crate::commands::init::yes_no;
use crate::commands::load_schema;
use crate::env::Env;
use crate::error::CliError;
use crate::files::{ENV_FILE, read_file, show};
use crate::output::{Output, Report, table};
use crate::ui;

/// Names, types and which keys have a value. Values are never printed.
pub fn run(
    out: &Output,
    cwd: &Path,
    env_flag: Option<&str>,
    env: &Env,
) -> Result<Report, CliError> {
    let (schema_path, schema) = load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd);
    let env_path = dir.join(ENV_FILE);
    let local = || -> Result<Values, CliError> {
        Ok(if env_path.is_file() {
            penv_dotenv::read(&read_file(&env_path)?).values()
        } else {
            Values::new()
        })
    };

    // Same rule as run: a header means the cloud, and the file stands in offline.
    let mut source = env_path.is_file().then(|| show(&env_path));
    let values = if schema.is_cloud() {
        let at = address(&schema, &environment(env_flag, env))?;
        match cloud_values(&schema, &at, env) {
            Ok(values) => {
                source = Some(at.to_string());
                values
            }
            Err(error) if error.code == "offline" && env_path.is_file() => {
                ui::warn(&format!(
                    "the cloud could not be reached, so this lists the local {ENV_FILE}."
                ));
                local()?
            }
            Err(error) => return Err(error),
        }
    } else {
        super::run::resolve_environment(env_flag, env)?;
        local()?
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

fn cloud_values(
    schema: &penv_schema::Schema,
    at: &penv_cloud::api::Address,
    env: &Env,
) -> Result<Values, CliError> {
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let cloud = Cloud::open(env, &detection)?;
    let bearer = cloud.bearer(env, schema.org.as_deref())?;
    let spinner = ui::spinner(&format!("Reading {at}"));
    let fetched = cloud
        .api
        .env_get(&bearer, at, None, true)
        .map_err(|e| refuse(e, Some(at)))?;
    spinner.stop(&format!("Read {at}"));
    let Fetched::Body { body, .. } = fetched else {
        return Ok(Values::new());
    };
    Ok(body
        .keys
        .into_iter()
        .filter_map(|key| Some((key.name, key.value?)))
        .collect())
}
