//! Where a command's schema and values come from: the nearest `.env.schema` with
//! its imports, the environment it names, and the cascade of `.env` files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use penv_cloud::api::Address;
use penv_schema::resolve::{Raw, ResolveError, penv_addresses, resolve, resolve_full, tainted};
use penv_schema::{Diagnostic, Schema, Values};

use crate::commands::cloud::Fetcher;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{ENV_FILE, SCHEMA_FILE, find_schema, read_file, show, write_private_file};

pub const DEFAULT_ENVIRONMENT: &str = "development";

/// A schema problem and the file it sits in, which may be an imported one.
pub type Located = Vec<(PathBuf, Diagnostic)>;

/// Parse the nearest schema and merge what it imports. Schema errors come back
/// as diagnostics, located in the file they sit in.
pub fn parse(cwd: &Path) -> Result<(PathBuf, Result<Schema, Located>), CliError> {
    let Some(path) = find_schema(cwd) else {
        return Err(CliError::new(
            "no_schema",
            format!("no {SCHEMA_FILE} here or in any directory above."),
            "Run penv init to write one from your .env.",
        ));
    };
    let mut chain = vec![canonical(&path)];
    let parsed = parse_file(&path, &mut chain)?.and_then(|schema| with_config(&path, schema));
    Ok((path, parsed))
}

fn parse_file(path: &Path, chain: &mut Vec<PathBuf>) -> Result<Result<Schema, Located>, CliError> {
    let source = read_file(path)?;
    let mut schema = match penv_schema::parse(&source) {
        Ok(schema) => schema,
        Err(found) => {
            return Ok(Err(found
                .into_iter()
                .map(|d| (path.to_path_buf(), d))
                .collect()));
        }
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    for import in schema.imports.clone() {
        let target = dir.join(&import.path);
        let at = |message: String| Diagnostic::new(import.line, 1, "invalid_import", message);
        if !target.is_file() {
            return Ok(Err(vec![(
                path.to_path_buf(),
                at(format!(
                    "line {}: @import names {}, which does not exist",
                    import.line,
                    show(&target)
                )),
            )]));
        }
        // A value file is never a schema: importing one would turn its values
        // into defaults that `penv schema` and `ls` print, past every guard
        // that keeps agents out of `.env*`.
        let file_name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if penv_dotenv::is_value_file(file_name) {
            return Ok(Err(vec![(
                path.to_path_buf(),
                at(format!(
                    "line {}: @import names {}, a value file; import a schema, such as ../shared/.env.schema",
                    import.line,
                    show(&target)
                )),
            )]));
        }
        let key = canonical(&target);
        if chain.contains(&key) {
            return Ok(Err(vec![(
                path.to_path_buf(),
                at(format!(
                    "line {}: @import of {} loops back to a file already importing it",
                    import.line,
                    show(&target)
                )),
            )]));
        }
        chain.push(key);
        let imported = parse_file(&target, chain)?;
        chain.pop();
        let imported = match imported {
            Ok(schema) => schema,
            Err(found) => return Ok(Err(found)),
        };
        for name in &import.keys {
            if imported.get(name).is_none() {
                return Ok(Err(vec![(
                    path.to_path_buf(),
                    at(format!(
                        "line {}: @import asks for {name}, which {} does not declare",
                        import.line,
                        show(&target)
                    )),
                )]));
            }
        }
        for key in imported.keys {
            let wanted = import.keys.is_empty() || import.keys.contains(&key.name);
            if wanted && schema.get(&key.name).is_none() {
                schema.keys.push(key);
            }
        }
        schema.warnings.extend(imported.warnings);
    }
    Ok(Ok(schema))
}

/// Apply `.penv/config.toml` beside the schema: its `[schema] version` wins over
/// an `@schema` line, and a version this build does not read is an error.
pub fn with_config(path: &Path, mut schema: Schema) -> Result<Schema, Located> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let config = match crate::config::Config::load(dir) {
        Ok(config) => config,
        Err(error) => {
            return Err(vec![(
                crate::config::Config::path(dir),
                Diagnostic::new(1, 1, "invalid_config", error.message),
            )]);
        }
    };
    schema.public_prefixes = config.public_prefixes();
    let mut found = Vec::new();
    for key in &mut schema.keys {
        let extra = schema
            .public_prefixes
            .iter()
            .any(|p| key.name.starts_with(p.as_str()));
        match (extra, key.sensitive_decorator) {
            (true, None) => key.sensitive = false,
            (true, Some(true)) => found.push((
                path.to_path_buf(),
                Diagnostic::new(
                    1,
                    1,
                    "sensitive_public_key",
                    format!(
                        "{} carries a prefix .penv/config.toml marks public, so its value ships to the browser. Drop @sensitive or rename the key.",
                        key.name
                    ),
                ),
            )),
            _ => {}
        }
    }
    if !found.is_empty() {
        return Err(found);
    }
    let written = config.has_schema_version();
    let Some(version) = config.schema_version() else {
        if written {
            return Err(vec![(
                crate::config::Config::path(dir),
                Diagnostic::new(
                    1,
                    1,
                    "unsupported_schema",
                    "[schema] version takes a whole number, such as 1",
                ),
            )]);
        }
        return Ok(schema);
    };
    let supported = i64::from(penv_schema::SCHEMA_VERSION);
    if version < 1 || version > supported {
        return Err(vec![(
            crate::config::Config::path(dir),
            Diagnostic::new(
                1,
                1,
                "unsupported_schema",
                format!(
                    "[schema] version = {version}, and this penv reads up to version {supported}; run penv upgrade"
                ),
            ),
        )]);
    }
    schema.schema_version = version as u32;
    Ok(schema)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The schema or one validation failure that lists every problem.
pub fn load(cwd: &Path) -> Result<(PathBuf, Schema), CliError> {
    let (path, parsed) = parse(cwd)?;
    parsed
        .map(|schema| (path.clone(), schema))
        .map_err(|found| {
            let listed = found
                .iter()
                .map(|(file, d)| format!("{} {d}", show(file)))
                .collect::<Vec<_>>()
                .join("; ");
            CliError::new(
                "invalid_schema",
                format!("{} has {} problem(s): {listed}", show(&path), found.len()),
                "Run penv check for the list, fix the lines it names, then try again.",
            )
            .with_exit(Exit::Validation)
        })
}

/// `--env`, else `PENV_ENV`, else the key `@currentEnv` names (the process, then
/// `.env` and `.env.local`, then its default), else development.
pub fn environment(flag: Option<&str>, env: &Env, schema: &Schema, dir: &Path) -> String {
    named_environment(flag, env, schema, dir).unwrap_or_else(|| DEFAULT_ENVIRONMENT.to_string())
}

/// The environment something named, before development is assumed.
pub fn named_environment(
    flag: Option<&str>,
    env: &Env,
    schema: &Schema,
    dir: &Path,
) -> Option<String> {
    let named = flag
        .filter(|v| !v.is_empty())
        .or_else(|| env.get("PENV_ENV").filter(|v| !v.is_empty()))
        .map(str::to_string);
    named.or_else(|| {
        let key = schema.current_env.as_deref()?;
        env.get(key)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .or_else(|| base_value(schema, env, dir, key))
    })
}

/// A key from the layers that do not depend on the environment, computed the way
/// `run` computes it, with the schema's default when no file sets it.
fn base_value(schema: &Schema, env: &Env, dir: &Path, key: &str) -> Option<String> {
    let base = layers(dir, &[ENV_FILE.to_string(), ".env.local".to_string()]).ok()?;
    let (values, _) = finish(schema, base.raw, env, DEFAULT_ENVIRONMENT);
    values.get(key).filter(|v| !v.is_empty()).cloned()
}

/// Refuse an environment name that cannot be a file name, or would name a file
/// that is not its own: `../x`, `local`, `schema`.
pub fn check_environment(name: &str) -> Result<(), CliError> {
    if penv_dotenv::is_environment_name(name) {
        return Ok(());
    }
    Err(CliError::new(
        "invalid_environment",
        format!("{name:?} is not an environment name penv can use."),
        "Use letters, digits, - and _ (and inner dots), and none of local, schema, example, sample, template, defaults, vault, keys or me.",
    )
    .with_exit(Exit::Validation))
}

/// The files one cascade read, and the values it layered from them.
#[derive(Debug, Default)]
pub struct Layers {
    pub raw: BTreeMap<String, Raw>,
    /// The file each key's value came from.
    pub origin: BTreeMap<String, PathBuf>,
    pub read: Vec<PathBuf>,
    pub warnings: Vec<(PathBuf, penv_dotenv::Warning)>,
    /// Cloud keys a local file replaced, with that file.
    pub overridden: Vec<(String, PathBuf)>,
}

impl Layers {
    pub fn is_empty(&self) -> bool {
        self.read.is_empty()
    }

    pub fn names(&self) -> String {
        self.read
            .iter()
            .map(|p| show(p))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Read the named layers in order, later files winning.
pub fn layers(dir: &Path, files: &[String]) -> Result<Layers, CliError> {
    let mut out = Layers::default();
    for name in files {
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        let read = penv_dotenv::read(&read_file(&path)?);
        for (key, raw) in read.raw() {
            out.origin.insert(key.clone(), path.clone());
            out.raw.insert(key, raw);
        }
        out.warnings
            .extend(read.warnings.into_iter().map(|w| (path.clone(), w)));
        out.read.push(path);
    }
    Ok(out)
}

/// Lay `top` over cloud values, naming each key it replaced.
pub fn overlay(values: &Values, top: Layers) -> Layers {
    let mut out = top;
    let mut raw: BTreeMap<String, Raw> = values
        .iter()
        .map(|(k, v)| (k.clone(), Raw::literal(v.clone())))
        .collect();
    for (key, value) in std::mem::take(&mut out.raw) {
        if raw.contains_key(&key) {
            let file = out.origin.get(&key).cloned().unwrap_or_default();
            out.overridden.push((key.clone(), file));
        }
        raw.insert(key, value);
    }
    out.raw = raw;
    out
}

/// A key the process environment already sets keeps that value, the rule dotenv,
/// dotenv-flow and varlock share: `APP_ENV=production penv run` means
/// production. Under the cloud the replacement is named, like a file's.
pub fn process_wins(layers: &mut Layers, schema: &Schema, env: &Env, cloud: bool) {
    let declared = schema.keys.iter().map(|k| k.name.clone());
    let names: Vec<String> = declared.chain(layers.raw.keys().cloned()).collect();
    for name in names {
        let Some(value) = env.get(&name).filter(|v| !v.is_empty()) else {
            continue;
        };
        if cloud && layers.raw.contains_key(&name) {
            layers
                .overridden
                .push((name.clone(), PathBuf::from("the process environment")));
        }
        layers
            .origin
            .insert(name.clone(), PathBuf::from("the process environment"));
        layers.raw.insert(name, Raw::literal(value));
    }
}

/// A default fills a key no layer gave a value; a computed default stays computed.
pub fn with_defaults(schema: &Schema, mut raw: BTreeMap<String, Raw>) -> BTreeMap<String, Raw> {
    for key in &schema.keys {
        let Some(default) = &key.default else {
            continue;
        };
        if raw.get(&key.name).is_none_or(|r| r.text.is_empty()) {
            let value = if key.default_expr {
                Raw::computed(default.clone())
            } else {
                Raw::literal(default.clone())
            };
            raw.insert(key.name.clone(), value);
        }
    }
    raw
}

/// Fill defaults, then compute every value, with no `penv()` lookups.
pub fn finish(
    schema: &Schema,
    raw: BTreeMap<String, Raw>,
    env: &Env,
    environment: &str,
) -> (Values, Vec<ResolveError>) {
    let raw = with_defaults(schema, raw);
    resolve(&raw, env.as_map(), environment, &Values::new())
}

/// Everything one environment resolves to, and where it came from.
#[derive(Debug, Default)]
pub struct Resolved {
    pub environment: String,
    pub values: Values,
    pub errors: Vec<ResolveError>,
    /// The cloud address read, when there was one.
    pub cloud: Option<String>,
    pub layers: Layers,
    /// Files `penv()` created because an environment had none.
    pub created: Vec<PathBuf>,
    /// The cloud was unreachable and the local layers stood in.
    pub offline: bool,
    /// Keys computed from a sensitive key, masked as if marked sensitive.
    pub tainted: std::collections::BTreeSet<String>,
    /// `@assert` checks that were false or could not be evaluated: line, message.
    pub failed_asserts: Vec<(u32, String)>,
    /// `random()` keys this call generated, with the file it wrote them to.
    pub generated: Vec<(String, PathBuf)>,
    /// `random()` keys left for the first `penv run` to generate.
    pub pending: Vec<String>,
}

/// Whether `random()` values are generated and kept, or only noted. `run`
/// generates; `check`, `ls` and `scan` never write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generate {
    Yes,
    No,
}

/// True for a key whose value `run` masks: marked sensitive, undeclared, or
/// computed from one that is.
pub fn is_sensitive(
    schema: &Schema,
    tainted: &std::collections::BTreeSet<String>,
    name: &str,
) -> bool {
    schema.get(name).is_none_or(|k| k.sensitive) || tainted.contains(name)
}

/// The one resolution path `run`, `ls`, `check` and `scan` share. A header reads
/// the cloud and lays every local file over it; no header reads the files.
/// `penv()` references are fetched before anything is computed.
pub fn values(
    schema: &Schema,
    dir: &Path,
    environment: &str,
    env: &Env,
    fetcher: &mut Fetcher,
) -> Result<Resolved, CliError> {
    values_with(schema, dir, environment, env, fetcher, Generate::No)
}

/// [`values`], generating `random()` values when asked.
pub fn values_with(
    schema: &Schema,
    dir: &Path,
    environment: &str,
    env: &Env,
    fetcher: &mut Fetcher,
    generate: Generate,
) -> Result<Resolved, CliError> {
    check_environment(environment)?;
    let mut out = Resolved {
        environment: environment.to_string(),
        ..Resolved::default()
    };
    let local = layers(dir, &penv_dotenv::cascade(environment))?;
    out.layers = match own(schema) {
        Some((org, project)) => {
            let at = Address::new(&org, &project, environment);
            match fetcher.values(&at) {
                Ok(cloud) => {
                    out.cloud = Some(at.to_string());
                    overlay(&cloud, local)
                }
                Err(error) if error.code == "offline" && !local.is_empty() => {
                    out.offline = true;
                    local
                }
                Err(error) => return Err(error),
            }
        }
        None => local,
    };

    process_wins(&mut out.layers, schema, env, out.cloud.is_some());
    // The key `@currentEnv` names holds the environment this run is for, however
    // it was chosen, so `--env production` and `$APP_ENV` never disagree.
    if let Some(key) = &schema.current_env {
        out.layers
            .raw
            .insert(key.clone(), Raw::literal(environment.to_string()));
    }
    let mut raw = with_defaults(schema, out.layers.raw.clone());
    randoms(&mut raw, dir, environment, generate, &mut out)?;
    let mut fetched = Values::new();
    // `@assert` expressions may read other environments too.
    let mut named = raw.clone();
    for (i, check) in schema.asserts.iter().enumerate() {
        named.insert(format!("\0assert{i}"), Raw::computed(check.expr.clone()));
    }
    for written in penv_addresses(&named) {
        let value = reference(
            schema,
            dir,
            environment,
            env,
            &written,
            fetcher,
            &mut out.created,
        )?;
        fetched.insert(written, value);
    }
    let resolution = resolve_full(&raw, env.as_map(), environment, &fetched);
    out.tainted = tainted(&resolution.deps, |name| {
        schema.get(name).is_none_or(|k| k.sensitive)
    });
    out.values = resolution.values;
    out.errors = resolution.errors;
    out.failed_asserts = asserts(schema, &out.values, env, environment, &fetched);
    Ok(out)
}

/// Say on stderr what a resolution did that the person did not write down.
pub fn report(resolved: &Resolved, dir: &Path) {
    let env = &resolved.environment;
    warn_unset(&resolved.errors);
    if resolved.offline {
        crate::ui::warn(&format!(
            "the cloud could not be reached, so {env} used {}.",
            resolved.layers.names()
        ));
    }
    if !resolved.layers.overridden.is_empty() {
        let listed = resolved
            .layers
            .overridden
            .iter()
            .map(|(key, file)| format!("{key} ({})", show(file)))
            .collect::<Vec<_>>()
            .join(", ");
        crate::ui::warn(&format!(
            "local files replaced the cloud value of {listed}."
        ));
    }
    for (key, path) in &resolved.generated {
        crate::ui::note(&format!("generated {key} and kept it in {}.", show(path)));
    }
    for path in &resolved.created {
        crate::ui::note(&format!(
            "created an empty {} for penv() to read.",
            show(path)
        ));
    }
    if resolved.cloud.is_none()
        && env != DEFAULT_ENVIRONMENT
        && !dir.join(format!(".env.{env}")).is_file()
    {
        let used = if resolved.layers.is_empty() {
            "defaults only".to_string()
        } else {
            resolved.layers.names()
        };
        crate::ui::warn(&format!("there is no .env.{env}, so {env} used {used}."));
    }
}

/// Each `@assert` against the resolved values. A key named with a NUL cannot
/// collide with a real one.
fn asserts(
    schema: &Schema,
    values: &Values,
    env: &Env,
    environment: &str,
    fetched: &Values,
) -> Vec<(u32, String)> {
    let slot = "\0assert".to_string();
    let mut failed = Vec::new();
    for check in &schema.asserts {
        let mut raw: BTreeMap<String, Raw> = values
            .iter()
            .map(|(k, v)| (k.clone(), Raw::literal(v.clone())))
            .collect();
        raw.insert(slot.clone(), Raw::computed(check.expr.clone()));
        let (out, errors) = resolve(&raw, env.as_map(), environment, fetched);
        let hard = errors.iter().find(|e| e.key == slot && !e.soft);
        match (out.get(&slot), hard) {
            (_, Some(error)) => failed.push((
                check.line,
                format!(
                    "{} (the check could not run: {})",
                    check.message,
                    error.message.trim_start_matches("\0assert: ")
                ),
            )),
            (Some(v), None) if v.is_empty() || v == "false" || v == "0" => {
                failed.push((check.line, check.message.clone()));
            }
            _ => {}
        }
    }
    failed
}

/// `random(N)` values: generated once and kept in this machine's `.env.local`
/// (`.env.test.local` for test, which skips `.env.local`), so the next run reads
/// the same value and `push` never sends it.
fn randoms(
    raw: &mut BTreeMap<String, Raw>,
    dir: &Path,
    environment: &str,
    generate: Generate,
    out: &mut Resolved,
) -> Result<(), CliError> {
    let wanted: Vec<(String, Option<usize>)> = raw
        .iter()
        .filter(|(_, r)| r.computed && is_random(&r.text))
        .map(|(k, r)| (k.clone(), random_len(&r.text)))
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let file = dir.join(if environment == "test" {
        ".env.test.local"
    } else {
        ".env.local"
    });
    for (key, len) in wanted {
        let Some(len) = len.filter(|n| (8..=512).contains(n)) else {
            return Err(CliError::new(
                "invalid_value",
                format!("{key}: random() takes a length from 8 to 512."),
                "Write random(32), for example.",
            )
            .with_exit(Exit::Validation));
        };
        if generate == Generate::No {
            raw.remove(&key);
            out.pending.push(key);
            continue;
        }
        let value = random_text(len)?;
        let existing = if file.is_file() {
            read_file(&file)?
        } else {
            String::new()
        };
        let written = penv_dotenv::upsert(&existing, &key, &value).map_err(|e| {
            CliError::new(
                "unwritable_value",
                e.to_string(),
                "Report this; generated values hold letters and digits only.",
            )
        })?;
        write_private_file(&file, &written)?;
        raw.insert(key.clone(), Raw::literal(value));
        out.generated.push((key, file.clone()));
    }
    Ok(())
}

fn is_random(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("random(") && text.ends_with(')')
}

/// The length in `random(N)`.
fn random_len(text: &str) -> Option<usize> {
    text.trim()
        .strip_prefix("random(")?
        .strip_suffix(')')?
        .trim()
        .parse()
        .ok()
}

/// Letters and digits from the operating system's generator, without modulo bias.
fn random_text(len: usize) -> Result<String, CliError> {
    const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(len);
    while out.len() < len {
        let bytes = penv_cloud::random_bytes(len * 2).map_err(|e| {
            CliError::new(
                "no_entropy",
                format!("the system random source failed: {e}"),
                "Try again.",
            )
        })?;
        for b in bytes {
            if b < 248 && out.len() < len {
                out.push(ALPHABET[usize::from(b % 62)] as char);
            }
        }
    }
    Ok(out)
}

fn own(schema: &Schema) -> Option<(String, String)> {
    Some((schema.org.clone()?, schema.project.clone()?))
}

/// `penv(KEY)`, `penv(env/KEY)`, `penv(project/env/KEY)`, `penv(org/project/env/KEY)`.
/// The header supplies what is left out. With no header the local files answer,
/// and an environment with no file gets an empty one.
#[allow(clippy::too_many_arguments)]
fn reference(
    schema: &Schema,
    dir: &Path,
    environment: &str,
    env: &Env,
    written: &str,
    fetcher: &mut Fetcher,
    created: &mut Vec<PathBuf>,
) -> Result<String, CliError> {
    let parts: Vec<&str> = written.split('/').map(str::trim).collect();
    let bad = || {
        CliError::new(
            "invalid_reference",
            format!("penv({written}) is not an address penv reads."),
            "Write penv(KEY), penv(env/KEY), penv(project/env/KEY) or penv(org/project/env/KEY).",
        )
        .with_exit(Exit::Validation)
    };
    if parts.iter().any(|p| p.is_empty()) {
        return Err(bad());
    }
    let (target, env_name, key) = match parts.as_slice() {
        [key] => (own(schema), environment.to_string(), *key),
        [e, key] => (own(schema), e.to_string(), *key),
        [project, e, key] => match &schema.org {
            Some(org) => (
                Some((org.clone(), project.to_string())),
                e.to_string(),
                *key,
            ),
            None => return Err(no_header(written)),
        },
        [org, project, e, key] => (
            Some((org.to_string(), project.to_string())),
            e.to_string(),
            *key,
        ),
        _ => return Err(bad()),
    };
    if !penv_schema::is_valid_key_name(key) || !penv_dotenv::is_environment_name(&env_name) {
        return Err(bad());
    }
    let same_project = target.is_some() && target == own(schema);
    let files = || layers(dir, &penv_dotenv::cascade(&env_name));
    // The referenced environment is computed on its own terms; a reference
    // there that leads back through penv() reads as empty rather than looping.
    let computed = |raw: BTreeMap<String, Raw>| {
        let (values, _) = finish(schema, raw, env, &env_name);
        values.get(key).cloned().unwrap_or_default()
    };

    let Some((org, project)) = target else {
        let own_file = dir.join(if env_name == DEFAULT_ENVIRONMENT {
            ENV_FILE.to_string()
        } else {
            format!(".env.{env_name}")
        });
        if !own_file.is_file() {
            write_private_file(&own_file, "")?;
            created.push(own_file);
        }
        return Ok(computed(files()?.raw));
    };
    let at = Address::new(&org, &project, &env_name);
    let cloud = match fetcher.values(&at) {
        Ok(values) => values,
        Err(error) if error.code == "offline" && same_project => Values::new(),
        Err(error) => return Err(error),
    };
    if !same_project {
        return Ok(cloud.get(key).cloned().unwrap_or_default());
    }
    Ok(computed(overlay(&cloud, files()?).raw))
}

fn no_header(written: &str) -> CliError {
    CliError::new(
        "invalid_reference",
        format!(
            "penv({written}) names another project, and {SCHEMA_FILE} names no org to find it in."
        ),
        "Write the full penv(org/project/env/KEY), or run penv push to link this folder.",
    )
    .with_exit(Exit::Validation)
}

/// True when any value failed outright; references to unset keys do not count.
pub fn failed(errors: &[ResolveError]) -> bool {
    errors.iter().any(|e| !e.soft)
}

/// Say each reference to an unset key on stderr, by key names only.
pub fn warn_unset(errors: &[ResolveError]) {
    for error in errors.iter().filter(|e| e.soft) {
        crate::ui::warn(&error.message);
    }
}

/// The schema as typed-code generators see it. A computed default is never
/// written into generated code as a literal fallback (`"random(48)"`,
/// `"if(...)"`): the key is read like one with no default, and required, because
/// `penv run` always supplies it. A key computed from a secret is typed as one.
/// Each key says whether a framework sends it to the browser.
pub fn gen_view(schema: &Schema) -> serde_json::Value {
    let mut raw = with_defaults(schema, BTreeMap::new());
    raw.retain(|_, r| !(r.computed && r.text.trim_start().starts_with("random(")));
    let resolution = resolve_full(&raw, &Values::new(), DEFAULT_ENVIRONMENT, &Values::new());
    let hot = tainted(&resolution.deps, |name| {
        schema.get(name).is_none_or(|k| k.sensitive)
    });
    let mut json = schema.to_json();
    if let Some(keys) = json.get_mut("keys").and_then(|k| k.as_array_mut()) {
        for key in keys {
            let name = key["name"].as_str().unwrap_or_default().to_string();
            if key["defaultExpr"] == serde_json::Value::Bool(true) {
                key["default"] = serde_json::Value::Null;
                key["required"] = serde_json::Value::Bool(true);
            }
            if hot.contains(&name) {
                key["sensitive"] = serde_json::Value::Bool(true);
            }
            key["public"] = serde_json::Value::Bool(schema.is_public(&name));
        }
    }
    json
}

/// Public keys whose value is computed from a secret: the prefix sends it to the
/// browser, so the secret goes with it. Names only.
pub fn public_leaks(
    schema: &Schema,
    tainted: &std::collections::BTreeSet<String>,
) -> Vec<penv_schema::Violation> {
    tainted
        .iter()
        .filter(|name| schema.is_public(name))
        .map(|name| {
            let prefix = schema.public_prefix(name).unwrap_or_default();
            penv_schema::Violation::new(
                name,
                "public",
                format!(
                    "{name} is built from a sensitive value, and its {prefix} prefix sends it to the browser. Compute it on the server, or rename it without the prefix."
                ),
            )
        })
        .collect()
}

/// Sensitive keys whose value is too short for the masker to replace: shown as
/// written in any output, so they are named rather than silently passed.
pub fn too_short_to_mask(
    schema: &Schema,
    tainted: &std::collections::BTreeSet<String>,
    values: &Values,
) -> Vec<String> {
    values
        .iter()
        .filter(|(name, value)| {
            !value.is_empty()
                && value.len() < penv_mask::MIN_SECRET_LEN
                && is_sensitive(schema, tainted, name)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// A key `finish` could not compute, as a refusal that names the key only.
pub fn unresolved(errors: &[ResolveError]) -> CliError {
    let hard: Vec<&ResolveError> = errors.iter().filter(|e| !e.soft).collect();
    let listed = hard
        .iter()
        .map(|e| e.message.clone())
        .collect::<Vec<_>>()
        .join("; ");
    CliError::new(
        "unresolved_value",
        format!("{} value(s) could not be computed: {listed}", hard.len()),
        "Fix the function or reference the message names, then try again.",
    )
    .with_exit(Exit::Validation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "penv-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn later_layers_win_and_missing_files_are_skipped() {
        let d = dir();
        std::fs::write(d.join(".env"), "A_KEY=base\nB_KEY=base\n").unwrap();
        std::fs::write(d.join(".env.staging"), "B_KEY=staging\n").unwrap();
        let read = layers(&d, &penv_dotenv::cascade("staging")).unwrap();
        assert_eq!(read.read.len(), 2);
        assert_eq!(read.raw["A_KEY"].text, "base");
        assert_eq!(read.raw["B_KEY"].text, "staging");
    }

    #[test]
    fn a_local_overlay_names_the_cloud_keys_it_replaced() {
        let cloud: Values = [("A_KEY".to_string(), "cloud".to_string())].into();
        let mut top = Layers::default();
        top.raw.insert("A_KEY".into(), Raw::literal("mine"));
        top.raw.insert("B_KEY".into(), Raw::literal("new"));
        let merged = overlay(&cloud, top);
        assert_eq!(merged.overridden[0].0, "A_KEY");
        assert_eq!(merged.raw["A_KEY"].text, "mine");
    }

    #[test]
    fn the_environment_comes_from_the_flag_then_the_variable_then_current_env() {
        let d = dir();
        std::fs::write(d.join(".env"), "APP_ENV=staging\n").unwrap();
        let schema =
            penv_schema::parse("# @currentEnv=$APP_ENV\n\n# @type=string\nAPP_ENV=\n").unwrap();
        let none = Env::default();
        assert_eq!(environment(None, &none, &schema, &d), "staging");
        let process = Env::from_pairs(&[("APP_ENV", "test")]);
        assert_eq!(environment(None, &process, &schema, &d), "test");
        let variable = Env::from_pairs(&[("PENV_ENV", "production"), ("APP_ENV", "test")]);
        assert_eq!(environment(None, &variable, &schema, &d), "production");
        assert_eq!(
            environment(Some("preview"), &variable, &schema, &d),
            "preview"
        );
        let plain = penv_schema::parse("# @type=string\nA=\n").unwrap();
        assert_eq!(environment(None, &none, &plain, &d), DEFAULT_ENVIRONMENT);
    }

    #[test]
    fn imports_merge_keys_the_importer_does_not_declare() {
        let d = dir();
        let shared = d.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(
            shared.join(".env.schema"),
            "# @type=string\nAPI_KEY=\n\n# @type=url\nAPI_URL=https://api.test\n\n# @type=string\nOTHER=\n",
        )
        .unwrap();
        let app = d.join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join(".env.schema"),
            "# @schema=1\n# @import(../shared/.env.schema, API_KEY, API_URL)\n\n# @type=port\nAPI_URL=\n",
        )
        .unwrap();
        let (_, schema) = load(&app).unwrap();
        let names: Vec<&str> = schema.keys.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(
            names,
            ["API_URL", "API_KEY"],
            "the importer's own block wins"
        );
        assert_eq!(
            schema.get("API_URL").unwrap().ty.base,
            penv_schema::BaseType::Port
        );

        std::fs::write(
            shared.join(".env.schema"),
            "# @import(../app/.env.schema)\n\n# @type=string\nAPI_KEY=\n",
        )
        .unwrap();
        let err = load(&app).unwrap_err();
        assert!(err.message.contains("loops back"), "{}", err.message);

        std::fs::write(shared.join(".env.production"), "API_KEY=sk_live_FAKE\n").unwrap();
        std::fs::write(
            app.join(".env.schema"),
            "# @import(../shared/.env.production)\n\n# @type=string\nA=\n",
        )
        .unwrap();
        let err = load(&app).unwrap_err();
        assert!(err.message.contains("a value file"), "{}", err.message);
        assert!(!err.message.contains("sk_live_FAKE"));
    }
}
