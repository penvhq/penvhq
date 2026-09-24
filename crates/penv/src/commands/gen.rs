use std::path::{Path, PathBuf};
use std::process::Command;

use penv_schema::Schema;
use penv_targets::{Config, Knob, OptionValue, Roots, Scan, Source, Suggest, Target, word};
use serde_json::{Value, json};

use crate::commands::load_schema;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::{Disk, home, on_path, read_file, show, write_file_making_parents};
use crate::output::{Output, Report, Style, table};
use crate::prompt;

/// What one run of `gen` does; the flags that pick it cannot both be passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Write,
    Check,
    Options,
}

/// Write the typed file for one target, or list the targets there are.
pub fn run(
    out: &Output,
    cwd: &Path,
    name: Option<&str>,
    to: Option<&Path>,
    mode: Mode,
    env: &Env,
    agent_flag: bool,
) -> Result<Report, CliError> {
    let (schema_path, schema) = load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd).to_path_buf();
    let repo = Repo::open(&dir)?;

    let Some(name) = name else {
        return Ok(list(out, &repo));
    };
    let target = repo.load(name).map_err(refused)?;
    if mode == Mode::Options {
        return Ok(knobs(out, &target));
    }
    let check = mode == Mode::Check;
    let explicit = to.map(|path| inside(&dir, path)).transpose()?;
    let style = out.style();
    let packages = repo.scan().candidates(&target);

    // --check writes nothing and asks nothing, so it never settles anything new.
    let interactive = !check && super::interactive(out, env, agent_flag);
    let settled = match settle(
        &repo,
        &target,
        &packages,
        explicit.as_deref(),
        interactive,
        "--out",
        &style,
    )? {
        Outcome::Chosen(settled) => settled,
        Outcome::Skipped(reason) => {
            let text = style.dim(&format!("skipped {}, {reason}", target.name));
            let report = Report::new(
                json!({ "target": target.name, "status": "skipped", "reason": reason }),
                text,
            );
            return Ok(if check {
                report
            } else {
                report.with_exit(Exit::Validation)
            });
        }
    };

    let target = with_options(target, &settled.options);
    let rendered = penv_targets::render(&target, &view(&schema), version()).map_err(refused)?;
    if check {
        return verify(out, &dir, &target, &settled.path, &packages, &rendered);
    }

    let written = put(&repo, &target, &settled, &packages, &rendered)?;
    Ok(report(out, &target, &written))
}

/// The repository one run reads: `.penv/config.toml` parsed once, and the tree
/// every target lookup goes through.
struct Repo {
    dir: PathBuf,
    roots: Roots,
    tree: penv_targets::Settled<'static>,
}

impl Repo {
    /// A `.penv/config.toml` that does not parse is refused here, before
    /// anything is read through it or written.
    fn open(dir: &Path) -> Result<Repo, CliError> {
        let roots = roots(dir);
        let tree = penv_targets::Settled::new(&Disk, &roots.repo).map_err(|error| {
            CliError::new(
                "invalid_config",
                described(&error),
                "Fix the line it names, or delete the file to start it again.",
            )
        })?;
        Ok(Repo {
            dir: dir.to_path_buf(),
            roots,
            tree,
        })
    }

    fn load(&self, name: &str) -> Result<Target, penv_targets::Error> {
        penv_targets::load(&self.tree, &self.roots, name)
    }

    /// One walk of the repository, which every target is matched against.
    fn scan(&self) -> Scan {
        Scan::new(&self.tree, &self.roots)
    }
}

/// The schema as templates see it, in the environment `gen` renders for.
fn view(schema: &Schema) -> Value {
    penv_targets::view(schema, crate::source::DEFAULT_ENVIRONMENT)
}

/// What one target left behind.
struct Written {
    path: PathBuf,
    changed: bool,
    /// The repo override that now remembers this path.
    remembered: Option<PathBuf>,
    note: Option<String>,
    /// The knobs in effect and the file that holds them.
    settings: Option<String>,
    import: Option<String>,
}

/// Every target the repository asks for. `init` calls this; a target that fails
/// to render is skipped rather than failing the import.
pub struct Generated {
    pub written: Vec<PathBuf>,
    pub targets: Vec<Value>,
    /// Lines `init` prints under its own: what was skipped and why, what was
    /// remembered, how to import.
    pub notes: Vec<String>,
}

impl Generated {
    /// A target that wrote nothing, said in the text report as well as the JSON.
    fn skip(&mut self, name: &str, reason: &str, style: &Style) {
        self.targets
            .push(json!({ "name": name, "status": "skipped", "reason": reason }));
        self.notes
            .push(style.dim(&format!("skipped {name}, {reason}")));
    }
}

/// What `init` generates, worked out before it writes anything, so an
/// `--output` it cannot use leaves the repository as it was.
pub struct Plan {
    repo: Repo,
    explicit: Option<String>,
    wanted: Vec<(Target, Vec<String>)>,
    broken: Vec<Value>,
}

/// Every target the repository detects, each with the packages it was found in.
pub fn plan(dir: &Path, to: Option<&Path>) -> Result<Plan, CliError> {
    let repo = Repo::open(dir)?;
    let explicit = to.map(|path| inside(dir, path)).transpose()?;
    let scan = repo.scan();
    let mut wanted: Vec<(Target, Vec<String>)> = Vec::new();
    let mut broken: Vec<Value> = Vec::new();
    for found in penv_targets::available(&repo.tree, &repo.roots) {
        match found {
            Err(error) => broken.push(json!({
                "name": null,
                "status": "skipped",
                "reason": described(&error),
            })),
            Ok(target) => {
                let packages = scan.candidates(&target);
                if !packages.is_empty() {
                    wanted.push((target, packages));
                }
            }
        }
    }
    if explicit.is_some() && wanted.len() > 1 {
        let names: Vec<&str> = wanted.iter().map(|(t, _)| t.name.as_str()).collect();
        return Err(CliError::new(
            "ambiguous_output",
            format!(
                "--output names one file, and {} apply here.",
                names.join(" and ")
            ),
            "Run penv gen <target> --out <PATH> once for each target instead.",
        ));
    }
    Ok(Plan {
        repo,
        explicit,
        wanted,
        broken,
    })
}

pub fn auto(
    out: &Output,
    plan: Plan,
    schema: &Schema,
    interactive: bool,
) -> Result<Generated, CliError> {
    let json = view(schema);
    let style = out.style();
    let Plan {
        repo,
        explicit,
        wanted,
        broken,
    } = plan;

    let mut generated = Generated {
        written: Vec::new(),
        targets: broken,
        notes: Vec::new(),
    };
    for (target, packages) in wanted {
        let settled = match settle(
            &repo,
            &target,
            &packages,
            explicit.as_deref(),
            interactive,
            "--output",
            &style,
        )? {
            Outcome::Chosen(settled) => settled,
            Outcome::Skipped(reason) => {
                generated.skip(&target.name, &reason, &style);
                continue;
            }
        };
        let target = with_options(target, &settled.options);
        let Ok(rendered) = penv_targets::render(&target, &json, version()) else {
            generated.skip(&target.name, "the target failed to render", &style);
            continue;
        };
        let Ok(written) = put(&repo, &target, &settled, &packages, &rendered) else {
            generated.skip(&target.name, "the file could not be written", &style);
            continue;
        };
        generated.targets.push(json!({
            "name": target.name,
            "status": if written.changed { "written" } else { "same" },
            "output": show(&written.path),
            "import": written.import,
        }));
        generated.notes.extend(notes(&written, &style));
        generated.written.push(written.path);
    }
    Ok(generated)
}

fn roots(dir: &Path) -> Roots {
    Roots::new(show(dir), home())
}

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A target error as this platform writes its directories.
fn described(error: &penv_targets::Error) -> String {
    match error {
        penv_targets::Error::NotFound { name, looked } => {
            let looked: Vec<String> = looked.iter().map(|dir| show(Path::new(dir))).collect();
            format!("no target named {name}; looked in {}", looked.join(", "))
        }
        penv_targets::Error::Malformed { dir, message } => {
            format!("{} is not a target: {message}", show(Path::new(dir)))
        }
        other => other.to_string(),
    }
}

fn refused(error: penv_targets::Error) -> CliError {
    match error {
        penv_targets::Error::NotFound { .. } => CliError::new(
            "unknown_target",
            described(&error),
            "Run penv gen with no target to see the ones this repository has.",
        ),
        penv_targets::Error::Name { .. } => CliError::new(
            "unknown_target",
            described(&error),
            "Run penv gen with no target to see the ones this repository has.",
        ),
        _ => CliError::new(
            "target_failed",
            described(&error),
            "Fix what it names, or remove it so the built-in target is used again.",
        )
        .with_exit(Exit::Validation),
    }
}

/// Where one target writes and what its `[options]` were settled to.
struct Settled {
    /// Relative to the repository root, in forward slashes.
    path: String,
    /// Options this run worked out. An option the target already defaults to is
    /// not one of them; the override still writes the whole table back.
    options: Vec<(String, OptionValue)>,
}

enum Outcome {
    Chosen(Settled),
    Skipped(String),
}

/// Only a flag and the repo override decide where a file goes. Detection offers;
/// a person answers, and nobody to ask means nothing is written. Whichever it
/// was, the path passes the one check that keeps it inside the repository.
fn settle(
    repo: &Repo,
    target: &Target,
    packages: &[String],
    explicit: Option<&str>,
    interactive: bool,
    flag: &str,
    style: &Style,
) -> Result<Outcome, CliError> {
    let (path, answered) = match explicit {
        Some(path) => (path.to_string(), true),
        None if target.output_source == Source::Repo => (target.output.clone(), false),
        None if !interactive => {
            return Ok(Outcome::Skipped(format!("pass {flag} <PATH>")));
        }
        None => match ask_where(target, &suggestions(repo, target, packages), style)? {
            Some(path) => (path, true),
            None => return Ok(Outcome::Skipped("the question was answered none".into())),
        },
    };
    let path = inside(&repo.dir, Path::new(&path))?;
    if !answered {
        return Ok(Outcome::Chosen(Settled {
            path,
            options: Vec::new(),
        }));
    }

    let package = penv_targets::package_of(&path, packages);
    let mut options = Vec::new();
    for suggest in &target.suggest {
        if let Some(chosen) = ask_option(repo, target, suggest, &package, interactive)? {
            options.push((suggest.option.clone(), chosen));
        }
    }
    Ok(Outcome::Chosen(Settled { path, options }))
}

/// One path per directory holding a detect file, shallowest first and shaped by
/// any `[[layout]]` it matches. A target nobody detected offers its own path.
fn suggestions(repo: &Repo, target: &Target, packages: &[String]) -> Vec<String> {
    let found: Vec<String> = packages
        .iter()
        .map(|package| penv_targets::layout_output(&repo.tree, &repo.roots, target, package))
        .collect();
    if found.is_empty() {
        vec![target.output.clone()]
    } else {
        found
    }
}

/// The one question penv asks about a target: Enter takes the first suggestion,
/// a number takes another, a path is used as written and `none` skips.
fn ask_where(
    target: &Target,
    suggestions: &[String],
    style: &Style,
) -> Result<Option<String>, CliError> {
    let default = suggestions.first().cloned().unwrap_or_default();
    let listing = if suggestions.len() > 1 {
        let rows: Vec<Vec<String>> = suggestions
            .iter()
            .enumerate()
            .map(|(index, path)| vec![format!("{}", index + 1), path.clone()])
            .collect();
        format!("{}\n\n", table(&["#", "PATH"], &rows, style))
    } else {
        String::new()
    };
    let prompt = format!(
        "{listing}where should {} go? [{default}] (Enter, number, path, none): ",
        file_name(&target.output),
    );
    Ok(
        match parse_answer(&prompt::read_line(&prompt)?, suggestions.len())? {
            Answer::Default => Some(default),
            Answer::Number(index) => suggestions.get(index).cloned(),
            Answer::Path(path) => Some(path),
            Answer::None => None,
        },
    )
}

/// One `[options]` knob the chosen package settles. `None` leaves the target's
/// own default, which is never worth remembering.
fn ask_option(
    repo: &Repo,
    target: &Target,
    suggest: &Suggest,
    package: &str,
    interactive: bool,
) -> Result<Option<OptionValue>, CliError> {
    let default = target.options.get(&suggest.option);
    let Some(found) =
        penv_targets::suggested(&repo.tree, &repo.roots, suggest, package, interactive)
    else {
        return Ok(None);
    };
    if Some(&found) == default {
        return Ok(None);
    }
    if !interactive {
        return Ok(Some(found));
    }
    let offered = suggest.offered(default);
    let words: Vec<String> = offered.iter().map(word).collect();
    let suggested = offered.iter().position(|value| value == &found);
    if let Some(picked) = crate::ui::select(&suggest.prompt, &words, suggested) {
        return picked
            .map(|index| Some(offered[index].clone()))
            .map_err(super::cloud::cancelled);
    }
    let prompt = format!(
        "{} [{}] ({}): ",
        suggest.prompt,
        word(&found),
        words.join(", ")
    );
    let answer = prompt::read_line(&prompt)?;
    Ok(Some(parse_choice(&answer, &found, &offered)?))
}

#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Default,
    Number(usize),
    Path(String),
    None,
}

/// Enter takes the default, a number takes a suggestion off the list, and
/// anything else is a path of your own.
fn parse_answer(answer: &str, count: usize) -> Result<Answer, CliError> {
    let answer = answer.trim();
    if answer.is_empty() {
        return Ok(Answer::Default);
    }
    if answer.eq_ignore_ascii_case("none") {
        return Ok(Answer::None);
    }
    if answer.chars().all(|c| c.is_ascii_digit()) {
        return match answer.parse::<usize>() {
            Ok(number) if (1..=count).contains(&number) => Ok(Answer::Number(number - 1)),
            _ => Err(CliError::new(
                "unreadable_selection",
                format!("{answer} is not one of the {count} paths listed."),
                "Answer with a number such as 1, a path such as apps/web/src/env.ts, or none.",
            )),
        };
    }
    Ok(Answer::Path(
        answer.replace('\\', "/").trim_end_matches('/').to_string(),
    ))
}

/// Enter keeps what the package's own files imply; otherwise one of the words
/// the target offers.
fn parse_choice(
    answer: &str,
    suggested: &OptionValue,
    offered: &[OptionValue],
) -> Result<OptionValue, CliError> {
    let answer = answer.trim();
    if answer.is_empty() {
        return Ok(suggested.clone());
    }
    offered
        .iter()
        .find(|value| word(value).eq_ignore_ascii_case(answer))
        .cloned()
        .ok_or_else(|| {
            let words: Vec<String> = offered.iter().map(word).collect();
            CliError::new(
                "unreadable_selection",
                format!("{answer} is not one of {}.", words.join(", ")),
                "Answer with one of those words, or press Enter to keep what is shown.",
            )
        })
}

fn file_name(output: &str) -> String {
    output.rsplit('/').next().unwrap_or(output).to_string()
}

fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(slash) => path[..slash].to_string(),
        None => String::new(),
    }
}

/// A repo-relative path with `.` and `..` taken out, or `None` when it leaves
/// the repository.
fn normalised(path: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for segment in path.replace('\\', "/").split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            name => out.push(name.to_string()),
        }
    }
    (!out.is_empty()).then(|| out.join("/"))
}

/// A path as the repository sees it. Nothing outside the repository is written
/// or remembered, whether it got there by an absolute path or a `..`; an
/// absolute path inside it is made relative.
fn inside(dir: &Path, path: &Path) -> Result<String, CliError> {
    let relative = match path.strip_prefix(dir) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) if path.is_absolute() || path.has_root() => return Err(outside(&show(path))),
        Err(_) => path.to_path_buf(),
    };
    normalised(&relative.to_string_lossy()).ok_or_else(|| outside(&show(path)))
}

fn outside(path: &str) -> CliError {
    CliError::new(
        "output_outside_repo",
        format!("{path} is not inside the repository."),
        "Name a path under the directory that holds .env.schema.",
    )
    .with_exit(Exit::Validation)
}

/// The target with what this run settled folded into its `[options]`, so the
/// template renders through the answers before anything reaches disk.
fn with_options(mut target: Target, options: &[(String, OptionValue)]) -> Target {
    for (name, value) in options {
        target.options.insert(name.clone(), value.clone());
    }
    target
}

fn put(
    repo: &Repo,
    target: &Target,
    settled: &Settled,
    packages: &[String],
    rendered: &str,
) -> Result<Written, CliError> {
    let dir = repo.dir.as_path();
    let path = dir.join(&settled.path);
    let unchanged = read_file(&path).is_ok_and(|existing| existing == rendered);
    if !unchanged {
        crate::files::within(dir, &path)?;
        write_file_making_parents(&path, rendered)?;
    }

    let package = penv_targets::package_of(&settled.path, packages);
    let configs = target
        .paths_from
        .as_ref()
        .map(|file| configs(dir, &package, file))
        .unwrap_or_default();
    let kept = kept_label(&target.name);
    let remembered = remember(dir, target, &settled.path)?;
    let note = remembered
        .as_ref()
        .map(|_| format!("remembered {} in {kept}", show(Path::new(&settled.path))));
    // The knobs are named after every write, so where they live is learned on
    // first use rather than read about somewhere else.
    let settings =
        (!target.knobs.is_empty()).then(|| format!("options in {kept}: {}", settings_of(target)));

    Ok(Written {
        path,
        changed: !unchanged,
        remembered,
        note,
        settings,
        import: penv_targets::import_line(target, &package, &settled.path, &configs),
    })
}

/// Where a target's settings are kept, for messages.
fn kept_label(name: &str) -> String {
    format!("{} [targets.{name}]", penv_targets::CONFIG)
}

/// The answer is kept in `.penv/config.toml` as `[targets.<name>]`: the output
/// path and every option at the value in effect, even the built-in default,
/// because it was answered and is never asked again. Other fields a section
/// holds (a new language's types, detect files, import line) are kept as they
/// are. A `.penv/targets/<name>/` folder left from before is moved in: its
/// fields into the section, its template to `.penv/<name>.tmpl`.
fn remember(dir: &Path, target: &Target, output: &str) -> Result<Option<PathBuf>, CliError> {
    let mut config = crate::config::Config::load(dir)?;
    let before = config.render();
    let legacy_dir = dir.join(".penv/targets").join(&target.name);
    let legacy_toml = legacy_dir.join("target.toml");
    let legacy_tmpl = legacy_dir.join("env.tmpl");

    let mut section = config.target(&target.name).unwrap_or_default();
    // The old folder's fields go under the section, key by key, and the section wins.
    if let Ok(text) = read_file(&legacy_toml)
        && let Ok(mut old) = text.parse::<toml::Table>()
    {
        old.remove("name");
        penv_targets::merge(&mut old, section);
        section = old;
    }
    section.insert("output".into(), toml::Value::String(output.to_string()));
    let options: toml::Table = target
        .effective()
        .into_iter()
        .map(|(knob, value)| (knob.name.clone(), value.clone()))
        .collect();
    if options.is_empty() {
        section.remove("options");
    } else {
        section.insert("options".into(), toml::Value::Table(options));
    }
    config.set_target(&target.name, section);

    let moved = legacy_toml.is_file() || legacy_tmpl.is_file();
    if config.render() == before && !moved {
        return Ok(None);
    }
    let path = config.save(dir)?;
    if legacy_tmpl.is_file() {
        let tmpl = dir.join(".penv").join(format!("{}.tmpl", target.name));
        if !tmpl.exists() {
            let body = read_file(&legacy_tmpl)?;
            write_file_making_parents(&tmpl, &body)?;
        }
        let _ = std::fs::remove_file(&legacy_tmpl);
    }
    let _ = std::fs::remove_file(&legacy_toml);
    let _ = std::fs::remove_dir(&legacy_dir);
    let _ = std::fs::remove_dir(dir.join(".penv/targets"));
    Ok(Some(path))
}

/// The package's own `paths_from` file and every relative `extends` above it,
/// read here so the import line stays a pure function.
fn configs(dir: &Path, package: &str, file: &str) -> Vec<Config> {
    let mut out: Vec<Config> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut next = penv_targets::join(package, file);
    while let Some(at) = next {
        if seen.contains(&at) {
            break;
        }
        let Ok(source) = std::fs::read_to_string(dir.join(&at)) else {
            break;
        };
        seen.push(at.clone());
        let config = Config::new(parent_of(&at), source);
        next = penv_targets::extends_of(&config);
        out.push(config);
    }
    out
}

fn notes(written: &Written, style: &Style) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(note) = &written.note {
        lines.push(style.dim(note));
    }
    if let Some(settings) = &written.settings {
        lines.push(style.dim(settings));
    }
    if let Some(import) = &written.import {
        lines.push(style.dim(import));
    }
    lines
}

fn report(out: &Output, target: &Target, written: &Written) -> Report {
    let style = out.style();
    let mut lines = vec![format!(
        "{} {} {}",
        style.green(if written.changed { "wrote" } else { "same" }),
        show(&written.path),
        style.dim(&format!("from the {} target", target.name))
    )];
    lines.extend(notes(written, &style));

    Report::new(
        json!({
            "target": target.name,
            "source": target.source.as_str(),
            "output": show(&written.path),
            "changed": written.changed,
            "remembered": written.remembered.as_ref().map(|p| show(p)),
            "import": written.import,
        }),
        lines.join("\n"),
    )
}

/// Every knob one target takes, what it is set to now, and what it changes.
fn knobs(out: &Output, target: &Target) -> Report {
    let style = out.style();
    let rows: Vec<Vec<String>> = target
        .effective()
        .iter()
        .map(|(knob, value)| {
            vec![
                knob.name.clone(),
                word(value),
                word(&knob.default),
                values_of(knob),
                knob.about.clone(),
            ]
        })
        .collect();
    let kept = kept_label(&target.name);
    let text = if rows.is_empty() {
        style.dim(&format!("the {} target takes no options", target.name))
    } else {
        format!(
            "{}\n\n{}",
            table(
                &["NAME", "VALUE", "DEFAULT", "VALUES", "ABOUT"],
                &rows,
                &style
            ),
            style.dim(&format!("set them in {kept}"))
        )
    };

    Report::new(
        json!({
            "target": target.name,
            "source": target.source.as_str(),
                        "remembered": kept,
            "options": described_options(target),
        }),
        text,
    )
}

/// The knobs as JSON, the same shape wherever penv publishes them.
fn described_options(target: &Target) -> Vec<Value> {
    target
        .effective()
        .iter()
        .map(|(knob, value)| {
            json!({
                "name": knob.name,
                "value": as_json(value),
                "default": as_json(&knob.default),
                "values": knob.values.iter().map(as_json).collect::<Vec<_>>(),
                "about": knob.about,
            })
        })
        .collect()
}

fn as_json(value: &OptionValue) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// The values a knob takes, or a dash when it takes free text.
fn values_of(knob: &Knob) -> String {
    if knob.values.is_empty() {
        "-".to_string()
    } else {
        knob.words().join("|")
    }
}

/// The knobs in effect on one line, as the listing shows them.
fn settings_of(target: &Target) -> String {
    let pairs: Vec<String> = target
        .effective()
        .iter()
        .map(|(knob, value)| format!("{}={}", knob.name, word(value)))
        .collect();
    if pairs.is_empty() {
        "-".to_string()
    } else {
        pairs.join(" ")
    }
}

fn list(out: &Output, repo: &Repo) -> Report {
    let found = penv_targets::available(&repo.tree, &repo.roots);
    let scan = repo.scan();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut listed: Vec<Value> = Vec::new();
    for target in &found {
        match target {
            Err(error) => {
                rows.push(vec![
                    "-".to_string(),
                    "broken".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    described(error),
                ]);
                listed.push(json!({ "status": "broken", "reason": described(error) }));
            }
            Ok(target) => {
                let packages = scan.candidates(target);
                rows.push(vec![
                    target.name.clone(),
                    target.source.as_str().to_string(),
                    target.output.clone(),
                    settings_of(target),
                    if packages.is_empty() {
                        "-".to_string()
                    } else {
                        packages
                            .iter()
                            .map(|package| shown(package))
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                ]);
                listed.push(json!({
                    "name": target.name,
                    "status": "ok",
                    "source": target.source.as_str(),
                    "dir": target.dir,
                    "output": target.output,
                    "detect": target.detect,
                    "options": described_options(target),
                    "candidates": packages,
                    "detected": !packages.is_empty(),
                }));
            }
        }
    }

    let style = out.style();
    let text = format!(
        "{}\n\n{}",
        table(
            &["NAME", "SOURCE", "OUTPUT", "OPTIONS", "PACKAGES"],
            &rows,
            &style
        ),
        style.dim("penv gen <name> writes one of these; penv gen <name> --options says what the options change")
    );

    Report::new(
        json!({
            "schema": show(&repo.dir.join(crate::files::SCHEMA_FILE)),
            "targets": listed,
        }),
        text,
    )
}

fn shown(package: &str) -> String {
    if package.is_empty() {
        ".".to_string()
    } else {
        package.to_string()
    }
}

fn verify(
    out: &Output,
    dir: &Path,
    target: &Target,
    relative: &str,
    packages: &[String],
    rendered: &str,
) -> Result<Report, CliError> {
    let path = dir.join(relative);
    let status = match std::fs::read_to_string(&path) {
        Err(_) => "missing",
        Ok(existing) if existing == rendered => "current",
        Ok(_) => "stale",
    };
    let package = dir.join(penv_targets::package_of(relative, packages));
    let compiled = compile(target, rendered, &package);

    let style = out.style();
    let ok = status == "current" && compiled["status"] != "failed";
    let mut lines = vec![format!(
        "{} {} {}",
        if ok {
            style.green(status)
        } else {
            style.red(status)
        },
        show(&path),
        style.dim(&format!("from the {} target", target.name))
    )];
    // The detail names the tool itself, so a skip still says why in text.
    lines.push(style.dim(compiled["detail"].as_str().unwrap_or_default()));

    let report = Report::new(
        json!({
            "target": target.name,
            "output": show(&path),
            "status": status,
            "compile": compiled,
        }),
        lines.join("\n"),
    );
    Ok(if ok {
        report
    } else {
        report.with_exit(Exit::Validation)
    })
}

/// Run the target's own `[check]` over what was rendered. A missing toolchain is
/// a skip that names what it looked for, not a reason to install one.
fn compile(target: &Target, rendered: &str, package: &Path) -> Value {
    let Some(check) = &target.check else {
        return json!({ "tool": null, "status": "skipped", "detail": "this target declares no [check] command" });
    };
    let Some(named) = check.command.first() else {
        return json!({ "tool": null, "status": "skipped", "detail": "this target declares no [check] command" });
    };
    let bin: Vec<PathBuf> = check.bin.iter().map(|dir| package.join(dir)).collect();
    let Some((name, tool)) = resolve(check, &bin) else {
        return json!({
            "tool": null,
            "status": "skipped",
            "detail": format!("{} is not installed", named.replace('|', " or ")),
        });
    };

    let Some(dir) = scratch() else {
        return json!({ "tool": name, "status": "skipped", "detail": "no writable temp directory" });
    };
    let file = source_name(target);
    let outcome = write_all(&dir, check, &file, rendered).and_then(|()| {
        let arguments: Vec<String> = check.command[1..]
            .iter()
            .map(|argument| argument.replace("{file}", &file))
            .collect();
        output(Command::new(&tool).current_dir(&dir).args(arguments))
    });
    let _ = std::fs::remove_dir_all(&dir);

    match outcome {
        Ok(()) => {
            json!({ "tool": name, "status": "ok", "detail": format!("{name} accepted the output") })
        }
        Err(detail) => json!({ "tool": name, "status": "failed", "detail": detail }),
    }
}

/// A directory of this run's own for the check to compile in: a name nobody can
/// guess, created only if nothing is there yet, readable by this user alone.
fn scratch() -> Option<PathBuf> {
    use std::hash::BuildHasher;
    let base = std::env::temp_dir();
    for _ in 0..8 {
        let salt =
            std::collections::hash_map::RandomState::new().hash_one(std::time::SystemTime::now());
        let dir = base.join(format!("penv-gen-{}-{salt:016x}", std::process::id()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        if builder.create(&dir).is_ok() {
            return Some(dir);
        }
    }
    None
}

/// The executable to run: the first of `python3|python|py`, in the package's own
/// bin before PATH, that answers its probe. A shim offering to install one, or a
/// project binary that will not start, is not the tool.
fn resolve(check: &penv_targets::Check, bin: &[PathBuf]) -> Option<(String, PathBuf)> {
    for name in check.command.first()?.split('|') {
        for tool in on_path(name, bin) {
            if check.probe.is_empty() || probed(&tool, &check.probe[1..]) {
                return Some((name.to_string(), tool));
            }
        }
    }
    None
}

fn probed(tool: &Path, arguments: &[String]) -> bool {
    Command::new(tool)
        .args(arguments)
        .output()
        .is_ok_and(|out| out.status.success())
}

fn write_all(
    dir: &Path,
    check: &penv_targets::Check,
    file: &str,
    rendered: &str,
) -> Result<(), String> {
    std::fs::write(dir.join(file), rendered).map_err(|e| e.to_string())?;
    for (name, body) in &check.files {
        std::fs::write(dir.join(name), body).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The name the check command compiles, taken from where the target writes.
fn source_name(target: &Target) -> String {
    Path::new(&target.output)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| target.output.clone())
}

fn output(command: &mut Command) -> Result<(), String> {
    match command.output() {
        Err(e) => Err(e.to_string()),
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stderr);
            let text = if text.trim().is_empty() {
                String::from_utf8_lossy(&out.stdout).into_owned()
            } else {
                text.into_owned()
            };
            Err(text.lines().take(5).collect::<Vec<_>>().join(" "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;

    impl penv_targets::Tree for Nothing {
        fn read(&self, _path: &str) -> Option<String> {
            None
        }
        fn dirs(&self, _path: &str) -> Vec<String> {
            Vec::new()
        }
    }

    fn target(name: &str) -> Target {
        penv_targets::load(&Nothing, &Roots::new("/repo", None), name).expect("a built-in target")
    }

    fn repo() -> Repo {
        Repo::open(Path::new("/repo")).expect("no config to read")
    }

    fn chosen(outcome: Outcome) -> Settled {
        match outcome {
            Outcome::Chosen(settled) => settled,
            Outcome::Skipped(reason) => panic!("{reason}"),
        }
    }

    fn plain() -> Style {
        Output::new(crate::output::Render {
            json: false,
            color: false,
        })
        .style()
    }

    #[test]
    fn the_question_reads_every_answer_it_offers() {
        assert_eq!(parse_answer("", 3).unwrap(), Answer::Default);
        assert_eq!(parse_answer("2", 3).unwrap(), Answer::Number(1));
        assert_eq!(parse_answer("NONE", 3).unwrap(), Answer::None);
        assert_eq!(
            parse_answer("packages/worker/src/env.ts/", 3).unwrap(),
            Answer::Path("packages/worker/src/env.ts".into())
        );
        assert_eq!(
            parse_answer("apps\\web\\env.ts", 3).unwrap(),
            Answer::Path("apps/web/env.ts".into())
        );
    }

    #[test]
    fn a_number_off_the_end_of_the_list_names_the_forms_the_answer_takes() {
        let error = parse_answer("9", 3).unwrap_err();
        assert_eq!(error.code, "unreadable_selection");
        assert!(error.fix.contains("none"), "{}", error.fix);
    }

    #[test]
    fn an_option_answer_is_one_of_the_words_the_target_offers() {
        let offered: Vec<OptionValue> = ["node", "vite", "deno"]
            .iter()
            .map(|w| OptionValue::String(w.to_string()))
            .collect();
        let vite = OptionValue::String("vite".into());
        assert_eq!(parse_choice("", &vite, &offered).unwrap(), vite);
        assert_eq!(
            parse_choice("DENO", &vite, &offered).unwrap(),
            OptionValue::String("deno".into())
        );
        let error = parse_choice("bun", &vite, &offered).unwrap_err();
        assert_eq!(error.code, "unreadable_selection");
        assert!(
            error.message.contains("node, vite, deno"),
            "{}",
            error.message
        );

        // A knob whose values are not words reads back the same way.
        let booleans = [OptionValue::Boolean(false), OptionValue::Boolean(true)];
        assert_eq!(
            parse_choice("true", &OptionValue::Boolean(false), &booleans).unwrap(),
            OptionValue::Boolean(true)
        );
    }

    #[test]
    fn a_path_that_leaves_the_repository_is_refused_and_never_remembered() {
        assert_eq!(
            normalised("apps/./web/src/env.ts").as_deref(),
            Some("apps/web/src/env.ts")
        );
        assert_eq!(normalised("./src/env.ts").as_deref(), Some("src/env.ts"));
        assert_eq!(
            normalised("apps/web/../api/env.ts").as_deref(),
            Some("apps/api/env.ts")
        );
        assert_eq!(normalised("../outside/env.ts"), None);
        assert_eq!(normalised("./"), None);

        let error = inside(Path::new("/repo"), Path::new("../outside/env.ts")).unwrap_err();
        assert_eq!(error.code, "output_outside_repo");
    }

    #[test]
    fn only_a_repo_override_that_names_output_settles_where_a_file_goes() {
        let repo = repo();
        let mut ts = target("ts");
        assert!(
            matches!(
                settle(&repo, &ts, &[], None, false, "--out", &plain()).unwrap(),
                Outcome::Skipped(reason) if reason == "pass --out <PATH>"
            ),
            "nothing decided and nobody to ask has to write nothing"
        );

        // The folder won the lookup for its [options] alone, which says nothing
        // about where the file goes.
        ts.source = Source::Repo;
        assert!(matches!(
            settle(&repo, &ts, &[], None, false, "--out", &plain()).unwrap(),
            Outcome::Skipped(_)
        ));

        ts.output_source = Source::Repo;
        ts.output = "apps/web/src/env.ts".into();
        let settled = chosen(settle(&repo, &ts, &[], None, false, "--out", &plain()).unwrap());
        assert_eq!(settled.path, "apps/web/src/env.ts");
        assert!(settled.options.is_empty(), "nothing new was answered");
    }

    #[test]
    fn an_explicit_path_beats_the_override_and_is_remembered_in_its_place() {
        let mut ts = target("ts");
        ts.output_source = Source::Repo;
        ts.output = "apps/web/src/env.ts".into();
        let settled = chosen(
            settle(
                &repo(),
                &ts,
                &[],
                Some("lib/env.ts"),
                false,
                "--out",
                &plain(),
            )
            .unwrap(),
        );
        assert_eq!(settled.path, "lib/env.ts");
    }

    #[test]
    fn a_remembered_output_that_leaves_the_repository_is_refused_before_a_write() {
        let mut ts = target("ts");
        ts.output_source = Source::Repo;
        for escaping in ["../../home/u/.bashrc", "/home/u/.bashrc", "apps/../../x.ts"] {
            ts.output = escaping.into();
            let error = match settle(&repo(), &ts, &[], None, false, "--out", &plain()) {
                Err(error) => error,
                Ok(_) => panic!("{escaping} was settled"),
            };
            assert_eq!(error.code, "output_outside_repo", "{escaping}");
        }
        ts.output = "./apps/web/../api/src/env.ts".into();
        let settled = chosen(settle(&repo(), &ts, &[], None, false, "--out", &plain()).unwrap());
        assert_eq!(settled.path, "apps/api/src/env.ts", "no . or .. downstream");
    }

    #[test]
    fn an_absolute_answer_is_refused_outside_the_repository_and_made_relative_inside() {
        let error = inside(Path::new("/repo"), Path::new("/etc/passwd")).unwrap_err();
        assert_eq!(error.code, "output_outside_repo");
        assert_eq!(
            inside(Path::new("/repo"), Path::new("/repo/src/env.ts")).unwrap(),
            "src/env.ts"
        );
        assert_eq!(
            parse_answer("/etc/passwd", 1).unwrap(),
            Answer::Path("/etc/passwd".into()),
            "a typed path reaches the one check as typed"
        );
    }

    #[test]
    fn the_check_compiles_in_a_directory_of_its_own_every_time() {
        let one = scratch().expect("a temp directory");
        let two = scratch().expect("a temp directory");
        assert_ne!(one, two);
        assert!(one.is_dir() && two.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&one).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "readable by this user alone");
        }
        let _ = std::fs::remove_dir_all(one);
        let _ = std::fs::remove_dir_all(two);
    }

    #[test]
    fn a_target_nobody_detected_still_offers_the_path_it_names() {
        let ts = target("ts");
        let repo = repo();
        assert_eq!(suggestions(&repo, &ts, &[]), ["src/env.ts"]);
        assert_eq!(
            suggestions(&repo, &ts, &["".into(), "apps/web".into()]),
            ["src/env.ts", "apps/web/src/env.ts"]
        );
    }

    #[test]
    fn the_ts_target_offers_the_runtimes_its_template_reads_through() {
        let ts = target("ts");
        let runtime = ts
            .suggest
            .iter()
            .find(|suggest| suggest.option == "runtime")
            .expect("a runtime knob");
        let words: Vec<String> = runtime
            .offered(ts.options.get("runtime"))
            .iter()
            .map(word)
            .collect();
        assert_eq!(
            words,
            ["node", "vite", "workers", "deno"],
            "the default reads first"
        );
        assert_eq!(
            ts.options["runtime"].as_str(),
            Some("node"),
            "the default is what nothing else decides"
        );
    }

    #[test]
    fn the_python_target_names_the_interpreters_it_will_take() {
        let check = target("py").check.expect("a [check] command");
        assert_eq!(check.command[0], "python3|python|py");
        assert_eq!(check.probe[0], "python3|python|py");
    }
}
