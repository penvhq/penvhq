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
    strict: bool,
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
            let process = Path::new("the process environment");
            let redacted = local
                .redacted
                .keys()
                .filter(|name| local.origin.get(*name).is_none_or(|p| p != process))
                .cloned()
                .collect();
            source::Resolved {
                environment: environment.clone(),
                values,
                errors,
                layers: local,
                redacted,
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
    // A withheld value exists; there is nothing here to validate it against.
    violations.retain(|v| !resolved.redacted.contains(&v.key));
    for key in &resolved.redacted {
        if only.is_none_or(|name| name == key) {
            notes.push(format!(
                "{key} in {environment} is write-only in penv.cloud: it has a value, withheld from this identity, so it was not validated"
            ));
        }
    }
    for error in resolved.errors.iter().filter(|e| e.soft) {
        if only.is_none_or(|name| name == error.key) {
            notes.push(error.message.clone());
        }
    }
    violations.extend(
        source::public_leaks(&schema, &resolved.tainted)
            .into_iter()
            .filter(|v| only.is_none_or(|n| n == v.key)),
    );
    for (line, message) in &resolved.failed_asserts {
        if only.is_none() {
            violations.push(Violation::new(
                &format!("@assert line {line}"),
                "assert",
                message.clone(),
            ));
        }
    }
    if only.is_none() {
        // Every value file here, not only this environment's: a tracked one
        // leaks whether or not it is read today.
        let files = source::value_file_keys(dir);
        for (file, how, keys) in source::exposed_secrets(&schema, &resolved.tainted, &files) {
            violations.push(Violation::new(
                &show(&file),
                "git",
                source::exposure_message(&file, how, &keys),
            ));
        }
    }
    for key in source::too_short_to_mask(&schema, &resolved.tainted, values) {
        notes.push(format!(
            "{key} is sensitive and shorter than {} characters, so penv cannot mask it",
            penv_mask::MIN_SECRET_LEN
        ));
    }
    for key in &resolved.pending {
        notes.push(format!(
            "{key} is generated by the first penv run and kept in .env.local"
        ));
    }
    if only.is_none() {
        let (undeclared, unused) = code_usage(dir, &schema);
        if strict {
            for (name, at) in &undeclared {
                violations.push(Violation::new(
                    name,
                    "undeclared",
                    format!("{name} is read in {at} and not declared in .env.schema. Declare it, or drop --strict to let it pass."),
                ));
            }
        } else if undeclared.len() <= 10 {
            for (name, at) in &undeclared {
                notes.push(format!(
                    "{name} is read in {at} and not declared in .env.schema"
                ));
            }
        } else {
            // A repository that has never had a schema reads dozens: one line, not a page.
            let shown: Vec<String> = undeclared
                .iter()
                .take(10)
                .map(|(n, at)| format!("{n} ({at})"))
                .collect();
            notes.push(format!(
                "{} variables are read in code and not declared in .env.schema: {}, and {} more; penv check --strict lists them all",
                undeclared.len(),
                shown.join(", "),
                undeclared.len() - 10
            ));
        }
        if !unused.is_empty() {
            notes.push(format!(
                "declared in .env.schema and not mentioned in any source file: {}",
                unused.join(", ")
            ));
        }
    }
    for key in schema.keys.iter().filter(|k| !k.hosts.is_empty()) {
        if let Some(why) = crate::sealed::signing_secret(&key.name) {
            violations.push(Violation::new(
                &key.name,
                "hosts",
                format!(
                    "{} has @hosts, but {why}: the command signs requests with it, so a placeholder cannot be swapped for it in flight. Remove @hosts from {}.",
                    key.name, key.name
                ),
            ));
            continue;
        }
        if key.ty.base == penv_schema::BaseType::Url {
            continue;
        } else if !key.ty.constraints.iter().any(|(k, _)| {
            matches!(
                k.as_str(),
                "startsWith" | "endsWith" | "minLength" | "maxLength"
            )
        }) {
            notes.push(format!(
                "{} has @hosts and no startsWith or length rule, so its placeholder is penvph_…; an SDK that checks key shapes may refuse it",
                key.name
            ));
        }
    }
    for key in &resolved.tainted {
        if schema.get(key).is_some_and(|k| !k.sensitive) {
            notes.push(format!("{key} is built from a sensitive value, so penv masks it though it is marked @sensitive=false"));
        }
    }
    let (config_violations, config_notes) =
        bundler_configs(dir, &schema, &resolved.tainted, &resolved.layers.origin);
    if only.is_none() {
        violations.extend(config_violations);
    }
    notes.extend(config_notes);
    if let Some(features) = penv_only(&schema_path) {
        notes.push(format!(
            "{} uses penv-only features ({features}); varlock will not load it",
            show(&schema_path)
        ));
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
    json["notes"] = json!(notes);
    json["redacted"] = json!(resolved.redacted);
    let mut report = Report::new(json, lines.join("\n"));
    if only.is_none() {
        // Guard coverage is part of a check; a stale guard is reported, never failed on.
        let guards = super::guard::run(out, cwd, true, false, &[])?;
        for harness in guards.json["harnesses"].as_array().into_iter().flatten() {
            for file in harness["files"].as_array().into_iter().flatten() {
                if file["status"] != "current" {
                    // penv never weakens a value already there, so rerunning it cannot help.
                    let fix = if file["overridden"].as_array().is_some_and(|o| !o.is_empty()) {
                        "edit the value the file already holds there"
                    } else {
                        "run penv guard"
                    };
                    report.text.push_str(&format!(
                        "\n{} guard {} {} is {}; {fix}",
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
                Ok(_) if !fetcher.can(penv_cloud::provider::Capability::Audit) => {
                    notes.push(format!(
                        "@rotate was not checked: the provider of {at} records no write times"
                    ));
                    return Vec::new();
                }
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
        let state = status(span, written, now);
        let text = reminder(&key.name, rotate, span, &state, schema.is_cloud());
        let json = match state {
            Rotation::Unrecorded => json!({ "key": key.name, "rotate": rotate, "recorded": false }),
            Rotation::Due { due, left } => {
                json!({ "key": key.name, "rotate": rotate, "recorded": true, "due": instant(due), "overdue": left <= 0 })
            }
        };
        out.push(Reminder { text, json });
    }
    out
}

/// What penv says about one key's `@rotate`. `check` prints it; the editor
/// shows the same words.
pub(crate) fn reminder(
    name: &str,
    rotate: &str,
    span: penv_schema::rotate::Span,
    state: &penv_schema::rotate::Rotation,
    cloud: bool,
) -> String {
    use penv_schema::rotate::{Rotation, instant};
    let show_at = |t: u64| {
        if span.is_sub_day() {
            instant(t)
        } else {
            instant(t)[..10].to_string()
        }
    };
    match state {
        Rotation::Unrecorded if cloud => format!(
            "rotate {name} has @rotate={rotate}, and the cloud has not said when it was last written"
        ),
        Rotation::Unrecorded => format!(
            "rotate {name} has @rotate={rotate} and no recorded write; penv set {name} records one"
        ),
        Rotation::Due { due, left } if *left <= 0 => format!(
            "rotate {name} was due {}; rotate it, then penv set {name}",
            show_at(*due)
        ),
        Rotation::Due { due, .. } => format!("rotate {name} by {}", show_at(*due)),
    }
}

/// The penv-only features a schema file uses, which varlock rejects.
fn penv_only(schema_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(schema_path).ok()?;
    let found: Vec<&str> = [
        ("@rotate", "@rotate"),
        ("@assert(", "@assert"),
        ("random(", "random()"),
        ("match(", "match()"),
        ("penv(", "penv()"),
        ("| urlencode", "filters"),
        ("| base64", "filters"),
        ("| lower", "filters"),
        ("| upper", "filters"),
        ("| trim", "filters"),
    ]
    .iter()
    .filter(|(needle, _)| text.contains(needle))
    .map(|(_, name)| *name)
    .fold(Vec::new(), |mut acc, name| {
        if !acc.contains(&name) {
            acc.push(name);
        }
        acc
    });
    (!found.is_empty()).then(|| found.join(", "))
}

/// Config files that can send any variable to the client, whatever its prefix:
/// Next.js `env`, Vite and webpack `define`, Expo `extra`, Nuxt
/// `runtimeConfig.public`, Astro `astro:env`, Rsbuild and Rspack `define`.
const BUNDLER_CONFIGS: [&str; 11] = [
    "next.config",
    "vite.config",
    "nuxt.config",
    "astro.config",
    "svelte.config",
    "webpack.config",
    "rsbuild.config",
    "rspack.config",
    "app.config",
    "babel.config",
    ".babelrc",
];

/// Bundler setups that ship server keys to the client. Two are certain and fail
/// `check`: `react-native-config`, which puts every key in `.env` into the app,
/// and babel's inline-environment plugin, which inlines every variable at build.
/// A config file that names a sensitive key is a note: it may only read it on
/// the server. Parcel inlines what browser code names; the build scan covers it.
fn bundler_configs(
    dir: &Path,
    schema: &penv_schema::Schema,
    tainted: &std::collections::BTreeSet<String>,
    origin: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> (Vec<Violation>, Vec<String>) {
    let sensitive: Vec<&str> = schema
        .keys
        .iter()
        .map(|k| k.name.as_str())
        .filter(|name| source::is_sensitive(schema, tainted, name))
        .collect();
    let mut violations = Vec::new();
    let mut notes = Vec::new();
    let package = std::fs::read_to_string(dir.join("package.json")).unwrap_or_default();
    let depends = |name: &str| package.contains(&format!("\"{name}\""));

    if depends("react-native-config") {
        let in_dotenv: Vec<&str> = sensitive
            .iter()
            .copied()
            .filter(|name| {
                origin
                    .get(*name)
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".env"))
            })
            .collect();
        if !in_dotenv.is_empty() {
            violations.push(Violation::new(
                "react-native-config",
                "public",
                format!(
                    "react-native-config puts every key in .env into the app bundle, and these are sensitive: {}. Move them to penv.cloud or a server, or stop reading them from .env.",
                    in_dotenv.join(", ")
                ),
            ));
        }
    }

    let mut configs: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            BUNDLER_CONFIGS
                .iter()
                .any(|c| name == *c || name.starts_with(&format!("{c}.")))
        })
        .collect();
    configs.sort();
    let inline_plugin = "transform-inline-environment-variables";
    if (depends(&format!("babel-plugin-{inline_plugin}"))
        || package.contains(inline_plugin)
        || configs
            .iter()
            .any(|c| std::fs::read_to_string(c).is_ok_and(|t| t.contains(inline_plugin))))
        && !sensitive.is_empty()
    {
        violations.push(Violation::new(
            "babel",
            "public",
            format!(
                "babel's {inline_plugin} inlines every environment variable into the bundle, sensitive ones included: {}. Use its include list, or a public prefix.",
                sensitive.join(", ")
            ),
        ));
    }
    for config in &configs {
        let Ok(text) = std::fs::read_to_string(config) else {
            continue;
        };
        let named: Vec<&str> = sensitive
            .iter()
            .copied()
            .filter(|k| names(&text, k))
            .collect();
        if !named.is_empty() {
            notes.push(format!(
                "{} names {}: a value placed in Next.js env, a define, Expo extra or Nuxt runtimeConfig.public ships to the client",
                show(config),
                named.join(", ")
            ));
        }
    }
    if depends("parcel") {
        notes.push("Parcel inlines every process.env value browser code names; penv run checks its output after the build".into());
    }
    (violations, notes)
}

/// True when `key` appears as a whole name in `text`.
fn names(text: &str, key: &str) -> bool {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    text.match_indices(key).any(|(at, _)| {
        !word(text[..at].chars().next_back()) && !word(text[at + key.len()..].chars().next())
    })
}

/// Variables the code reads that the schema does not declare (with where the
/// first read is), and declared keys no source file mentions. Generated typed
/// files are left out: they name every key.
fn code_usage(dir: &Path, schema: &penv_schema::Schema) -> (Vec<(String, String)>, Vec<String>) {
    const MAX_FILES: usize = 20_000;
    const MAX_BYTES: u64 = 1024 * 1024;
    let mut files = super::scan::candidates(dir, &[]).unwrap_or_else(|_| {
        let mut out = Vec::new();
        super::scan::walk(dir, &mut out);
        out
    });
    files.retain(|f| crate::usage::is_source(f));
    let generated = generated_files(dir);
    let mut undeclared: Vec<(String, String)> = Vec::new();
    // Each file is read once and dropped: only the declared names it mentions stay.
    let declared: std::collections::HashSet<&str> =
        schema.keys.iter().map(|k| k.name.as_str()).collect();
    let mut mentioned: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut read_any = false;
    for file in files.into_iter().take(MAX_FILES) {
        let path = if file.is_absolute() {
            file.clone()
        } else {
            dir.join(&file)
        };
        if generated.contains(&path) {
            continue;
        }
        if path.metadata().map(|m| m.len() > MAX_BYTES).unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let shown = path
            .strip_prefix(dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        for (name, line) in crate::usage::reads(&text) {
            if schema.get(&name).is_none()
                && !crate::usage::is_ambient(&name)
                && !undeclared.iter().any(|(n, _)| *n == name)
            {
                undeclared.push((name, format!("{shown}:{line}")));
            }
        }
        for word in crate::usage::words(&text) {
            if let Some(name) = declared.get(word) {
                mentioned.insert(name);
            }
        }
        read_any = true;
    }
    if !read_any {
        return (undeclared, Vec::new());
    }
    // A key another key's value is built from counts as used.
    let defaults: Vec<&str> = schema
        .keys
        .iter()
        .filter_map(|k| k.default.as_deref())
        .collect();
    let unused: Vec<String> = schema
        .keys
        .iter()
        .filter(|k| schema.current_env.as_deref() != Some(k.name.as_str()))
        .filter(|k| !mentioned.contains(k.name.as_str()))
        .filter(|k| !defaults.iter().any(|d| crate::usage::mentions(d, &k.name)))
        .map(|k| k.name.clone())
        .collect();
    undeclared.sort();
    (undeclared, unused)
}

/// The files `penv gen` writes, from `.penv/config.toml` and any target folder
/// left from before.
fn generated_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let read = |text: String| -> Vec<String> {
        text.parse::<toml::Table>()
            .ok()
            .map(|t| {
                let mut found = Vec::new();
                if let Some(targets) = t.get("targets").and_then(|v| v.as_table()) {
                    for section in targets.values() {
                        if let Some(o) = section.get("output").and_then(|v| v.as_str()) {
                            found.push(o.to_string());
                        }
                    }
                }
                if let Some(o) = t.get("output").and_then(|v| v.as_str()) {
                    found.push(o.to_string());
                }
                found
            })
            .unwrap_or_default()
    };
    if let Ok(text) = std::fs::read_to_string(dir.join(".penv/config.toml")) {
        out.extend(read(text).into_iter().map(|o| dir.join(o)));
    }
    if let Ok(entries) = std::fs::read_dir(dir.join(".penv/targets")) {
        for entry in entries.flatten() {
            if let Ok(text) = std::fs::read_to_string(entry.path().join("target.toml")) {
                out.extend(read(text).into_iter().map(|o| dir.join(o)));
            }
        }
    }
    out
}

#[cfg(test)]
mod reminder_tests {
    use super::reminder;
    use penv_schema::rotate::{Rotation, Span, parse_instant};

    #[test]
    fn every_rotate_sentence_check_and_the_editor_share() {
        let days = Span::parse("90d").unwrap();
        let hours = Span::parse("12h").unwrap();
        let due = parse_instant("2026-03-31").unwrap();
        let at = parse_instant("2026-03-31T12:00:00Z").unwrap();
        assert_eq!(
            reminder("K", "90d", days, &Rotation::Unrecorded, false),
            "rotate K has @rotate=90d and no recorded write; penv set K records one"
        );
        assert_eq!(
            reminder("K", "90d", days, &Rotation::Unrecorded, true),
            "rotate K has @rotate=90d, and the cloud has not said when it was last written"
        );
        assert_eq!(
            reminder("K", "90d", days, &Rotation::Due { due, left: 0 }, false),
            "rotate K was due 2026-03-31; rotate it, then penv set K"
        );
        assert_eq!(
            reminder("K", "90d", days, &Rotation::Due { due, left: 1 }, false),
            "rotate K by 2026-03-31"
        );
        assert_eq!(
            reminder(
                "K",
                "12h",
                hours,
                &Rotation::Due { due: at, left: 5 },
                false
            ),
            "rotate K by 2026-03-31T12:00:00Z"
        );
    }
}
