use std::path::Path;

use penv_schema::{Diagnostic, Violation, extras, validate, validate_key};
use serde_json::{Value, json};

use crate::error::{CliError, Exit};
use std::io::IsTerminal;

use crate::agent::detect_here;
use crate::commands::cloud::Fetcher;
use crate::env::Env;
use crate::files::show;
use crate::output::{Output, Report};
use crate::source;

/// Schema problems, missing values and values that could not be computed, for
/// every key or for one. All three are the output, not an error, so they go to
/// stdout and set exit 3. Schema warnings and rotation reminders never change the
/// exit code.
pub fn run(
    out: &Output,
    cwd: &Path,
    only: Option<&str>,
    env_flag: Option<&str>,
    env: &Env,
) -> Result<Report, CliError> {
    let style = out.style();
    let (schema_path, parsed) = source::parse(cwd)?;
    let schema = match parsed {
        Ok(schema) => schema,
        Err(found) => {
            let text = found
                .iter()
                .map(|(file, d)| {
                    format!(
                        "{} {}:{} {}",
                        style.red("fail"),
                        show(file),
                        d.line,
                        d.message
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let diagnostics: Vec<Diagnostic> = found.into_iter().map(|(_, d)| d).collect();
            return Ok(
                Report::new(body(&schema_path, None, &diagnostics, &[], &[], &[]), text)
                    .with_exit(Exit::Validation),
            );
        }
    };

    let dir = schema_path.parent().unwrap_or(cwd);
    let environment = source::environment(env_flag, env, &schema, dir);
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let mut fetcher = Fetcher::new(env, &detection);
    // CI often checks without a credential: the cloud is read when it can be,
    // and the local files are checked when it cannot.
    let mut notes: Vec<String> = Vec::new();
    let resolved = match source::values(&schema, dir, &environment, env, &mut fetcher) {
        Ok(resolved) => resolved,
        Err(error) if schema.is_cloud() => {
            notes.push(format!(
                "the cloud was not read ({}), so this checked the local files",
                error.message.trim_end_matches('.')
            ));
            let mut local = source::layers(dir, &penv_dotenv::cascade(&environment))?;
            source::process_wins(&mut local, &schema, env, false);
            let (values, errors) = source::finish(&schema, local.raw.clone(), env, &environment);
            source::Resolved {
                environment: environment.clone(),
                values,
                errors,
                layers: local,
                ..Default::default()
            }
        }
        Err(error) => return Err(error),
    };
    let values = &resolved.values;
    let schema = schema.for_environment(&environment);
    let file_warnings: Vec<penv_dotenv::Warning> = resolved
        .layers
        .warnings
        .iter()
        .map(|(_, w)| w.clone())
        .collect();

    let mut violations = match only {
        None => validate(&schema, values),
        Some(name) => {
            let key = schema.get(name).ok_or_else(|| {
                CliError::new(
                    "unknown_key",
                    format!("{name} is not in {}.", show(&schema_path)),
                    "Add a block for it, or run penv ls to see the keys.",
                )
                .with_exit(Exit::Validation)
            })?;
            validate_key(key, values.get(name).map(String::as_str))
        }
    };
    for error in resolved.errors.iter().filter(|e| e.soft) {
        if only.is_none_or(|name| name == error.key) {
            notes.push(error.message.clone());
        }
    }
    for error in resolved.errors.iter().filter(|e| !e.soft) {
        if only.is_none_or(|name| name == error.key) {
            violations.push(Violation::new(
                &error.key,
                "computed",
                error.message.clone(),
            ));
        }
    }

    // Drift is a warning, not a violation: it never changes the exit code.
    let drift = match only {
        Some(_) => Vec::new(),
        None => extras(&schema, values),
    };

    let rotation = rotation(&schema, dir, &environment, only, &mut fetcher, &mut notes);
    let sources = resolved
        .cloud
        .clone()
        .or_else(|| (!resolved.layers.is_empty()).then(|| resolved.layers.names()));

    let mut lines: Vec<String> = Vec::new();
    if violations.is_empty() {
        let checked = only.map_or(schema.keys.len(), |_| 1);
        lines.push(format!(
            "{} {checked} key(s) in {} for {environment}",
            style.green("ok"),
            show(&schema_path)
        ));
    }
    lines.extend(
        violations
            .iter()
            .map(|v| format!("{} {}", style.red("fail"), v.message)),
    );
    lines.extend(schema.warnings.iter().map(|w| {
        style.dim(&format!(
            "note {}:{} {}",
            show(&schema_path),
            w.line,
            w.message
        ))
    }));
    lines.extend(rotation.iter().map(|r| style.yellow(&r.text)));
    lines.extend(drift.iter().map(|key| {
        style.dim(&format!(
            "drift {key} has a value and no block in {}; penv masks it but never validates it",
            show(&schema_path)
        ))
    }));
    lines.extend(
        resolved
            .layers
            .warnings
            .iter()
            .map(|(file, w)| style.dim(&format!("note {}:{} {}", show(file), w.line, w.message))),
    );
    lines.extend(notes.iter().map(|n| style.dim(&format!("note {n}"))));

    let mut json = body(
        &schema_path,
        sources,
        &[],
        &violations,
        &file_warnings,
        &drift,
    );
    json["environment"] = json!(environment);
    json["schemaWarnings"] = json!(
        schema
            .warnings
            .iter()
            .map(Diagnostic::to_json)
            .collect::<Vec<_>>()
    );
    json["rotation"] = json!(rotation.iter().map(|r| r.json.clone()).collect::<Vec<_>>());
    let mut report = Report::new(json, lines.join("\n"));
    if only.is_none() {
        // Guard coverage is part of a check; a stale guard is reported, never failed on.
        let guards = super::guard::run(out, cwd, true, false, &[])?;
        for harness in guards.json["harnesses"].as_array().into_iter().flatten() {
            for file in harness["files"].as_array().into_iter().flatten() {
                if file["status"] != "current" {
                    report.text.push_str(&format!(
                        "\n{} guard {} {} is {}; run penv guard",
                        style.dim("note"),
                        harness["name"].as_str().unwrap_or_default(),
                        file["path"].as_str().unwrap_or_default(),
                        file["status"].as_str().unwrap_or_default()
                    ));
                }
            }
        }
        report.json["guards"] = guards.json["harnesses"].clone();
    }
    Ok(if violations.is_empty() {
        report
    } else {
        report.with_exit(Exit::Validation)
    })
}

pub(super) fn body(
    schema_path: &Path,
    env_path: Option<String>,
    diagnostics: &[Diagnostic],
    violations: &[Violation],
    warnings: &[penv_dotenv::Warning],
    drift: &[String],
) -> Value {
    json!({
        "schema": show(schema_path),
        "env": env_path,
        "ok": diagnostics.is_empty() && violations.is_empty(),
        "diagnostics": diagnostics.iter().map(Diagnostic::to_json).collect::<Vec<_>>(),
        "violations": violations.iter().map(Violation::to_json).collect::<Vec<_>>(),
        "warnings": warnings.iter().map(|w| json!({
            "line": w.line,
            "code": w.code,
            "message": w.message,
        })).collect::<Vec<_>>(),
        "drift": drift.iter().map(|key| json!({
            "key": key,
            "code": "drift",
            "message": format!("{key} has a value with no block in the schema. penv masks it, and validates nothing about it. Add a block, or drop the key."),
        })).collect::<Vec<_>>(),
    })
}

struct Reminder {
    text: String,
    json: Value,
}

/// `@rotate` reminders. The clock is the cloud's last write under a header and
/// `.penv/config.toml` without one, so every clone on every machine agrees.
fn rotation(
    schema: &penv_schema::Schema,
    dir: &Path,
    environment: &str,
    only: Option<&str>,
    fetcher: &mut Fetcher,
    notes: &mut Vec<String>,
) -> Vec<Reminder> {
    use penv_cloud::Clock as _;
    use penv_schema::rotate::{Rotation, Span, instant, parse_instant, rotation as status};

    let keys: Vec<_> = schema
        .keys
        .iter()
        .filter(|k| k.rotate.is_some() && only.is_none_or(|n| n == k.name))
        .collect();
    if keys.is_empty() {
        return Vec::new();
    }
    let now = penv_cloud::SystemClock.now();
    let written: Vec<Option<u64>> = match (&schema.org, &schema.project) {
        (Some(org), Some(project)) => {
            let at = penv_cloud::api::Address::new(org, project, environment);
            match fetcher.keys(&at) {
                Ok(stored) => keys
                    .iter()
                    .map(|k| {
                        stored
                            .iter()
                            .find(|s| s.name == k.name)
                            .and_then(|s| s.updated_at.as_deref())
                            .and_then(penv_cloud::clock::epoch_from_rfc3339)
                    })
                    .collect(),
                Err(_) => {
                    notes.push(format!("@rotate was not checked: {at} could not be read"));
                    return Vec::new();
                }
            }
        }
        _ => {
            let config = crate::config::Config::load(dir).unwrap_or_default();
            keys.iter()
                .map(|k| config.rotated(&k.name).and_then(parse_instant))
                .collect()
        }
    };

    let mut out = Vec::new();
    for (key, written) in keys.iter().zip(written) {
        let rotate = key.rotate.as_deref().unwrap_or_default();
        let Some(span) = Span::parse(rotate) else {
            continue;
        };
        let show_at = |t: u64| {
            if span.is_sub_day() {
                instant(t)
            } else {
                instant(t)[..10].to_string()
            }
        };
        match status(span, written, now) {
                        Rotation::Unrecorded => out.push(Reminder {
                text: if schema.is_cloud() {
                    format!(
                        "rotate {} has @rotate={rotate}, and the cloud has not said when it was last written",
                        key.name
                    )
                } else {
                    format!(
                        "rotate {} has @rotate={rotate} and no recorded write; penv set {} records one",
                        key.name, key.name
                    )
                },
                json: json!({ "key": key.name, "rotate": rotate, "recorded": false }),
            }),
            Rotation::Due { due, left } if left <= 0 => out.push(Reminder {
                text: format!("rotate {} was due {}; rotate it, then penv set {}", key.name, show_at(due), key.name),
                json: json!({ "key": key.name, "rotate": rotate, "recorded": true, "due": instant(due), "overdue": true }),
            }),
            Rotation::Due { due, .. } => out.push(Reminder {
                text: format!("rotate {} by {}", key.name, show_at(due)),
                json: json!({ "key": key.name, "rotate": rotate, "recorded": true, "due": instant(due), "overdue": false }),
            }),
        }
    }
    out
}
