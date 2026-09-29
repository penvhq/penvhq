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

/// Text for the schema at `path`, read the way `check` reads that file: imports
/// merged and `.penv/config.toml` applied. The editor hands in unsaved text.
pub fn parse_text(path: &Path, source: &str) -> Result<Result<Schema, Located>, CliError> {
    let mut chain = vec![canonical(path)];
    Ok(parse_source(path, source, &mut chain)?.and_then(|schema| with_config(path, schema)))
}

fn parse_file(path: &Path, chain: &mut Vec<PathBuf>) -> Result<Result<Schema, Located>, CliError> {
    let source = read_file(path)?;
    parse_source(path, &source, chain)
}

fn parse_source(
    path: &Path,
    source: &str,
    chain: &mut Vec<PathBuf>,
) -> Result<Result<Schema, Located>, CliError> {
    let mut schema = match penv_schema::parse(source) {
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
        // Only a schema is imported: any other `.env*` file may hold values,
        // which would become defaults that `penv schema` and `ls` print, past
        // every guard that keeps agents out of `.env*`. A link is judged by
        // the file it points at.
        let key = canonical(&target);
        let file_name = key.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if file_name != SCHEMA_FILE && (file_name == ENV_FILE || file_name.starts_with(".env.")) {
            return Ok(Err(vec![(
                path.to_path_buf(),
                at(format!(
                    "line {}: @import names {}, which is not a schema and may hold values; import a schema, such as ../shared/.env.schema",
                    import.line,
                    show(&target)
                )),
            )]));
        }
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
    /// Each file read, with the keys it sets a value for itself, whether or not
    /// that value is the one that wins.
    pub keys: Vec<(PathBuf, Vec<String>)>,
    /// Keys a `# penv:redacted KEY` marker names that neither its own file nor
    /// a later one sets, with the file holding the marker.
    pub redacted: BTreeMap<String, PathBuf>,
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
        let mut key: Option<[u8; 32]> = None;
        let mut own = Vec::new();
        for (name, mut raw) in read.raw() {
            if !raw.text.is_empty() {
                own.push(name.clone());
            }
            // An encrypted value is decrypted here, the one place every
            // command reads value files through.
            if crate::localcrypt::is_encrypted(&raw.text) {
                if key.is_none() {
                    key = crate::localcrypt::key(false)?;
                }
                let Some(k) = key.as_ref() else {
                    return Err(CliError::new(
                        "decrypt_failed",
                        format!(
                            "{name} in {} is encrypted, and this machine holds no penv key.",
                            show(&path)
                        ),
                        format!(
                            "Set it again with penv set {name}, or supply the key in {}.",
                            crate::localcrypt::KEY_VAR
                        ),
                    )
                    .with_exit(Exit::Validation));
                };
                raw = Raw::literal(crate::localcrypt::decrypt(
                    k,
                    &name,
                    &raw.text,
                    &show(&path),
                )?);
            }
            out.origin.insert(name.clone(), path.clone());
            out.raw.insert(name, raw);
        }
        out.warnings
            .extend(read.warnings.into_iter().map(|w| (path.clone(), w)));
        // A value wins over a marker in its own file or an earlier one.
        out.redacted.retain(|name, _| !own.contains(name));
        for name in read.redacted {
            if !own.contains(&name) {
                out.redacted.insert(name, path.clone());
            }
        }
        out.keys.push((path.clone(), own));
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
    let process = PathBuf::from("the process environment");
    let declared = schema.keys.iter().map(|k| k.name.clone());
    let names: std::collections::BTreeSet<String> =
        declared.chain(layers.raw.keys().cloned()).collect();
    for name in names {
        let Some(value) = env.get(&name).filter(|v| !v.is_empty()) else {
            continue;
        };
        if cloud && layers.raw.contains_key(&name) {
            // A cloud value has no origin file; one a file already replaced is
            // named once, by whatever replaced it last.
            match layers.overridden.iter_mut().find(|(key, _)| *key == name) {
                Some(entry) => entry.1 = process.clone(),
                None if !layers.origin.contains_key(&name) => {
                    layers.overridden.push((name.clone(), process.clone()));
                }
                None => {}
            }
        }
        layers.origin.insert(name.clone(), process.clone());
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
    /// Keys present in penv.cloud and withheld because the environment is
    /// write-only, which no local layer supplies. Never the same as missing.
    pub redacted: Vec<String>,
    /// The deploy bundle read in place of the cloud, when `PENV_BUNDLE_KEY` opened one.
    pub bundle: Option<PathBuf>,
    /// What `values` was computed from, so a sealed run can compute it again.
    pub raw: BTreeMap<String, Raw>,
    pub fetched: Values,
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
    // A deploy bundle, when PENV_BUNDLE_KEY opens one, stands where the cloud
    // would: the deploy's values, with any value file and the process over it.
    let bundle = crate::bundle::read(dir, environment, env)?;
    let mut withheld = Vec::new();
    out.layers = match (bundle, own(schema)) {
        (Some((file, values)), _) => {
            let mut layered = overlay(&values, local);
            // Named as the origin for why and ls, and kept out of `read`: that
            // list is value files, which check holds to git rules a committed,
            // encrypted bundle is meant to break.
            for name in values.keys() {
                layered
                    .origin
                    .entry(name.clone())
                    .or_insert_with(|| file.clone());
            }
            out.bundle = Some(file);
            layered
        }
        (None, None) => local,
        (None, Some((org, project))) => {
            let at = Address::new(&org, &project, environment);
            match fetcher.values(&at) {
                Ok(cloud) => {
                    out.cloud = Some(at.to_string());
                    withheld = fetcher.redacted(&at)?;
                    overlay(&cloud, local)
                }
                // Design section 5: offline, only development runs on what is here.
                Err(error)
                    if error.code == "offline"
                        && !local.is_empty()
                        && environment == penv_cloud::cache::DEV_ENVIRONMENT =>
                {
                    out.offline = true;
                    local
                }
                Err(error) => return Err(error),
            }
        }
    };

    process_wins(&mut out.layers, schema, env, out.cloud.is_some());
    // The cloud decides what it withholds; a marker speaks only where no cloud
    // or bundle was read.
    let marked: Vec<String> = match (&out.cloud, &out.bundle) {
        (Some(_), _) => withheld,
        (None, Some(_)) => Vec::new(),
        (None, None) => out.layers.redacted.keys().cloned().collect(),
    };
    let process = Path::new("the process environment");
    out.redacted = marked
        .into_iter()
        .filter(|name| {
            let set = out.layers.raw.get(name).is_some_and(|r| !r.text.is_empty());
            let supplied = match out.cloud {
                Some(_) => set,
                None => set && out.layers.origin.get(name).is_some_and(|p| p == process),
            };
            !supplied && schema.current_env.as_deref() != Some(name.as_str())
        })
        .collect();
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
    out.raw = raw;
    out.fetched = fetched;
    Ok(out)
}

/// The values again with each sealed key standing as its placeholder, so a key
/// computed from one (`AUTH=Bearer ${STRIPE_KEY}`) carries the placeholder too
/// and never the value.
pub fn sealed_values(resolved: &Resolved, env: &Env, placeholders: &[(String, String)]) -> Values {
    let mut raw = resolved.raw.clone();
    for (name, placeholder) in placeholders {
        raw.insert(name.clone(), Raw::literal(placeholder.clone()));
    }
    resolve_full(&raw, env.as_map(), &resolved.environment, &resolved.fetched).values
}

/// The refusal for keys a command would receive and cannot: penv.cloud holds
/// them write-only. `None` when every key has a value to hand over.
pub fn withheld(resolved: &Resolved) -> Option<CliError> {
    let names = &resolved.redacted;
    if names.is_empty() {
        return None;
    }
    let env = &resolved.environment;
    let listed = names.join(", ");
    let local = format!(".env.{env}.local");
    let (verb, them, values) = if names.len() == 1 {
        ("is", "it", "value")
    } else {
        ("are", "them", "values")
    };
    let fix = if resolved.cloud.is_some() {
        format!(
            "Run it where a workload identity (OIDC, AWS IAM or bound keypair) reads the environment, or set {them} in {local}."
        )
    } else {
        format!("Set {listed} in {local}. penv.cloud keeps the {env} {values} write-only.")
    };
    Some(
        CliError::new(
            "redacted",
            format!("{listed} in {env} {verb} write-only in penv.cloud."),
            fix,
        )
        .with_exit(Exit::EnvironmentRefused),
    )
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
        && resolved.bundle.is_none()
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
    if schema.asserts.is_empty() {
        return failed;
    }
    let mut raw: BTreeMap<String, Raw> = values
        .iter()
        .map(|(k, v)| (k.clone(), Raw::literal(v.clone())))
        .collect();
    for check in &schema.asserts {
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
        let stored = crate::localcrypt::stored(dir, &key, &value, true)?;
        let written = penv_dotenv::upsert(&existing, &key, &stored).map_err(|e| {
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
        let local = files()?;
        if local.redacted.contains_key(key) {
            return Err(withheld_reference(written, key, &env_name, false));
        }
        return Ok(computed(local.raw));
    };
    let at = Address::new(&org, &project, &env_name);
    let (cloud, redacted) = match fetcher.values(&at) {
        Ok(values) => (values, fetcher.redacted(&at)?),
        Err(error) if error.code == "offline" && same_project => (Values::new(), Vec::new()),
        Err(error) => return Err(error),
    };
    let redacted = redacted.iter().any(|name| name == key);
    if !same_project {
        if redacted {
            return Err(withheld_reference(written, key, &env_name, true));
        }
        return Ok(cloud.get(key).cloned().unwrap_or_default());
    }
    let local = files()?;
    if redacted && local.raw.get(key).is_none_or(|r| r.text.is_empty()) {
        return Err(withheld_reference(written, key, &env_name, true));
    }
    Ok(computed(overlay(&cloud, local).raw))
}

/// `penv(...)` named a key penv.cloud holds write-only for this identity.
fn withheld_reference(written: &str, key: &str, env_name: &str, cloud: bool) -> CliError {
    let local = format!(".env.{env_name}.local");
    let fix = if cloud {
        format!(
            "Run it where a workload identity (OIDC, AWS IAM or bound keypair) reads {env_name}, or set {key} in {local}."
        )
    } else {
        format!("Set {key} in {local}. penv.cloud keeps the {env_name} value write-only.")
    };
    CliError::new(
        "redacted",
        format!(
            "{key} in {env_name} is write-only in penv.cloud, so penv({written}) cannot read it."
        ),
        fix,
    )
    .with_exit(Exit::EnvironmentRefused)
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

/// Every value file in `dir`, whichever environment it serves, with the keys it
/// sets a value for. Names only; nothing is decrypted.
pub fn value_file_keys(dir: &Path) -> Vec<(PathBuf, Vec<String>)> {
    crate::files::value_files(dir)
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let keys = penv_dotenv::read(&text)
                .raw()
                .into_iter()
                .filter(|(_, raw)| !raw.text.is_empty())
                .map(|(name, _)| name)
                .collect();
            Some((path, keys))
        })
        .collect()
}

/// Value files git would commit while they hold a sensitive value: the file, how
/// it is exposed, and the keys it sets. A value the process or a later file
/// replaces is still in the file. Names only.
pub fn exposed_secrets(
    schema: &Schema,
    tainted: &std::collections::BTreeSet<String>,
    files: &[(PathBuf, Vec<String>)],
) -> Vec<(PathBuf, crate::gitexposure::Exposure, Vec<String>)> {
    let holding: Vec<(PathBuf, Vec<String>)> = files
        .iter()
        .map(|(file, keys)| {
            let sensitive = keys
                .iter()
                .filter(|name| is_sensitive(schema, tainted, name))
                .cloned()
                .collect::<Vec<_>>();
            (file.clone(), sensitive)
        })
        .filter(|(_, keys)| !keys.is_empty())
        .collect();
    let paths: Vec<PathBuf> = holding.iter().map(|(f, _)| f.clone()).collect();
    holding
        .into_iter()
        .zip(crate::gitexposure::exposures(&paths))
        .filter_map(|((file, keys), how)| Some((file, how?, keys)))
        .collect()
}

/// One sentence per exposed file, for `check` to fail on and `run` to warn with.
pub fn exposure_message(file: &Path, how: crate::gitexposure::Exposure, keys: &[String]) -> String {
    match how {
        crate::gitexposure::Exposure::Tracked => format!(
            "{} holds {} and git tracks it. Run git rm --cached {}, commit, and rotate those values.",
            show(file),
            keys.join(", "),
            file.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
        ),
        crate::gitexposure::Exposure::Unignored => format!(
            "{} holds {} and is not in .gitignore, so the next git add takes it. Run penv init to add the ignore lines.",
            show(file),
            keys.join(", ")
        ),
    }
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
    fn a_marker_stands_at_its_file_s_place_in_the_cascade() {
        let d = dir();
        std::fs::write(d.join(".env"), "A_KEY=dev\nB_KEY=dev\n").unwrap();
        std::fs::write(
            d.join(".env.production"),
            "# penv:redacted A_KEY\n# penv:redacted B_KEY\n# penv:redacted C_KEY\nC_KEY=here\n",
        )
        .unwrap();
        std::fs::write(d.join(".env.production.local"), "B_KEY=mine\n").unwrap();
        let read = layers(&d, &penv_dotenv::cascade("production")).unwrap();
        // A lower file's value does not stand in; a later file's or the same
        // file's does.
        assert_eq!(read.redacted.keys().collect::<Vec<_>>(), ["A_KEY"]);
        assert_eq!(read.redacted["A_KEY"], d.join(".env.production"));
    }

    #[test]
    fn the_refusal_names_every_withheld_key_and_the_fix_for_where_it_ran() {
        let mut resolved = Resolved {
            environment: "production".into(),
            ..Resolved::default()
        };
        assert!(
            withheld(&resolved).is_none(),
            "nothing withheld, nothing refused"
        );

        resolved.redacted = vec!["DB_PASSWORD".into(), "STRIPE_KEY".into()];
        resolved.cloud = Some("acme/api/production".into());
        let cloud = withheld(&resolved).unwrap();
        assert_eq!(cloud.code, "redacted");
        assert_eq!(cloud.exit, Exit::EnvironmentRefused);
        assert_eq!(
            cloud.message,
            "DB_PASSWORD, STRIPE_KEY in production are write-only in penv.cloud."
        );
        assert_eq!(
            cloud.fix,
            "Run it where a workload identity (OIDC, AWS IAM or bound keypair) reads the environment, or set them in .env.production.local."
        );

        resolved.redacted = vec!["DB_PASSWORD".into()];
        resolved.cloud = None;
        let local = withheld(&resolved).unwrap();
        assert_eq!(
            local.message,
            "DB_PASSWORD in production is write-only in penv.cloud."
        );
        assert_eq!(
            local.fix,
            "Set DB_PASSWORD in .env.production.local. penv.cloud keeps the production value write-only."
        );
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
    fn the_process_replacing_a_cloud_value_is_named_once_and_truthfully() {
        let schema = penv_schema::parse(
            "# @type=string\nA_KEY=\n\n# @type=string\nB_KEY=\n\n# @type=string\nC_KEY=\n",
        )
        .unwrap();
        let cloud: Values = [
            ("A_KEY".to_string(), "cloud".to_string()),
            ("B_KEY".to_string(), "cloud".to_string()),
        ]
        .into();
        let mut top = Layers::default();
        top.raw.insert("B_KEY".into(), Raw::literal("file"));
        top.origin.insert("B_KEY".into(), PathBuf::from(".env"));
        top.raw.insert("C_KEY".into(), Raw::literal("file"));
        top.origin.insert("C_KEY".into(), PathBuf::from(".env"));
        let mut merged = overlay(&cloud, top);
        let env = Env::from_pairs(&[("A_KEY", "p"), ("B_KEY", "p"), ("C_KEY", "p")]);
        process_wins(&mut merged, &schema, &env, true);
        let named: Vec<(&str, String)> = merged
            .overridden
            .iter()
            .map(|(k, f)| (k.as_str(), f.display().to_string()))
            .collect();
        assert_eq!(
            named,
            [
                ("B_KEY", "the process environment".to_string()),
                ("A_KEY", "the process environment".to_string()),
            ],
            "C_KEY was never a cloud value"
        );
        assert!(merged.raw.values().all(|r| r.text == "p"));
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
        assert!(err.message.contains("may hold values"), "{}", err.message);
        assert!(!err.message.contains("sk_live_FAKE"));

        for name in [".env.keys", ".env.me", ".env.vault", ".env.example"] {
            std::fs::write(shared.join(name), "API_KEY=sk_live_FAKE\n").unwrap();
            std::fs::write(
                app.join(".env.schema"),
                format!("# @import(../shared/{name})\n\n# @type=string\nA=\n"),
            )
            .unwrap();
            let err = load(&app).unwrap_err();
            assert!(
                err.message.contains("may hold values"),
                "{name}: {}",
                err.message
            );
        }

        #[cfg(unix)]
        {
            let link = shared.join("values.schema");
            std::os::unix::fs::symlink(shared.join(".env.production"), &link).unwrap();
            std::fs::write(
                app.join(".env.schema"),
                "# @import(../shared/values.schema)\n\n# @type=string\nA=\n",
            )
            .unwrap();
            let err = load(&app).unwrap_err();
            assert!(err.message.contains("may hold values"), "{}", err.message);
        }
    }
}
