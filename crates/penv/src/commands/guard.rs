use std::path::{Path, PathBuf};

use penv_guards::{Guard, Roots, Write};
use serde_json::{Value, json};

use crate::claim;
use crate::commands::init::yes_no;
use crate::commands::load_schema;
use crate::error::{CliError, Exit};
use crate::files::{Disk, home, on_path, show, write_executable, write_file_making_parents};
use crate::output::{Output, Report, table};

/// Native Windows runs Claude Code without a sandbox, so the deny rules and the
/// hook are all there is.
#[cfg(windows)]
const PLATFORM_NOTE: Option<&str> =
    Some("native Windows has no Claude Code sandbox; the deny rules and the hook still apply");
#[cfg(not(windows))]
const PLATFORM_NOTE: Option<&str> = None;

/// Write what every installed harness enforces, or report where that stands.
pub fn run(
    out: &Output,
    cwd: &Path,
    check: bool,
    all: bool,
    named: &[String],
) -> Result<Report, CliError> {
    let (schema_path, schema) = load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd).to_path_buf();
    refuse_shadowing(&dir)?;
    let guards = known(&dir);
    for name in named {
        if !guards.iter().any(|(g, _)| &g.name == name) {
            return Err(CliError::new(
                "unknown_harness",
                format!("penv has no guard for {name}."),
                "Run penv guard --check to see the harnesses it knows.",
            ));
        }
    }

    let json = schema.to_json();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut harnesses: Vec<Value> = Vec::new();
    let mut blocks: Vec<Value> = Vec::new();
    let mut failed = false;

    for (guard, installed) in &guards {
        let installed = *installed;
        // The harnesses that are here, unless the run named others or asked for
        // all of them; a laptop without a harness is not a failing one.
        let selected = if !named.is_empty() {
            named.iter().any(|n| n == &guard.name)
        } else {
            all || installed
        };

        let mut files: Vec<Value> = Vec::new();
        for entry in guard.project_writes() {
            if !selected {
                continue;
            }
            let path = dir.join(&entry.path);
            if !check {
                crate::files::within(&dir, &path)?;
            }
            let (status, overridden) = act(guard, entry, &path, &json, check)?;
            failed |= check && status != CURRENT;
            rows.push(vec![
                guard.name.clone(),
                yes_no(installed),
                entry.path.clone(),
                status.to_string(),
            ]);
            files.push(json!({
                "path": entry.path,
                "status": status,
                "written": !check,
                "overridden": overridden,
            }));
        }

        if selected && !check {
            for entry in guard.user_writes() {
                blocks.push(json!({
                    "harness": guard.name,
                    "path": entry.path,
                    "content": pretty(&render(guard, entry, &json)?),
                }));
            }
        }

        harnesses.push(json!({
            "name": guard.name,
            "description": guard.description,
            "installed": installed,
            "selected": selected,
            "files": files,
        }));
    }

    let claim = claim::for_schema(&schema);
    let style = out.style();
    let mut lines = vec![if rows.is_empty() {
        style.dim("no harness is installed here; penv guard --all writes every one penv knows")
    } else {
        table(&["HARNESS", "INSTALLED", "FILE", "STATUS"], &rows, &style)
    }];
    if !blocks.is_empty() {
        lines.push(String::new());
        for block in &blocks {
            lines.push(style.dim(&format!(
                "paste this into {} yourself; penv does not write outside the repository",
                block["path"].as_str().unwrap_or_default()
            )));
            lines.push(block["content"].as_str().unwrap_or_default().to_string());
        }
    }
    if let Some(note) = PLATFORM_NOTE {
        lines.push(String::new());
        lines.push(style.dim(note));
    }
    lines.push(String::new());
    lines.push(style.dim(claim));

    let report = Report::new(
        json!({
            "schema": show(&schema_path),
            "mode": if check { "check" } else { "write" },
            "harnesses": harnesses,
            "userBlocks": blocks,
            "platformNote": PLATFORM_NOTE,
            "claim": claim,
        }),
        lines.join("\n"),
    );
    Ok(if check && failed {
        report.with_exit(Exit::Validation)
    } else {
        report
    })
}

/// A repository folder named after a built-in guard stops the run rather than
/// quietly dropping that harness from the list.
pub(crate) fn refuse_shadowing(dir: &Path) -> Result<(), CliError> {
    let roots = Roots::new(show(dir), home());
    for built_in in penv_guards::BUILT_IN {
        if let Err(error @ penv_guards::Error::Shadows { .. }) =
            penv_guards::load(&Disk, &roots, built_in.name)
        {
            return Err(CliError::new(
                "guard_failed",
                error.to_string(),
                "Rename or remove the folder under .penv/guards, then run penv guard again.",
            )
            .with_exit(Exit::Validation));
        }
    }
    Ok(())
}

/// Every harness penv knows here, and whether this machine has it.
pub fn known(dir: &Path) -> Vec<(Guard, bool)> {
    let roots = Roots::new(show(dir), home());
    let probe = Installed {
        repo: dir.to_path_buf(),
        home: home().map(PathBuf::from),
    };
    penv_guards::available(&Disk, &roots)
        .into_iter()
        .map(|guard| {
            let installed = penv_guards::is_installed(&guard, &probe);
            (guard, installed)
        })
        .collect()
}

/// The harnesses named, written. `init` calls this; a harness whose config
/// cannot be merged is left alone rather than failing the import.
pub fn write_selected(dir: &Path, schema: &Value, names: &[String]) -> Vec<PathBuf> {
    let mut written = Vec::new();
    for (guard, _) in known(dir) {
        if !names.contains(&guard.name) {
            continue;
        }
        for entry in guard.project_writes() {
            let path = dir.join(&entry.path);
            if let Err(error) = crate::files::within(dir, &path) {
                crate::ui::warn(&error.message);
                continue;
            }
            let Ok(fragment) = render(&guard, entry, schema) else {
                continue;
            };
            let existing = std::fs::read_to_string(&path).ok();
            let Ok(outcome) = penv_guards::apply(entry, existing.as_deref(), &fragment) else {
                continue;
            };
            if outcome.changed && put(entry, &path, &outcome.content).is_ok() {
                written.push(path);
            }
        }
    }
    written
}

/// What was found before anything was written; a write run then makes it so.
const CURRENT: &str = "current";
const STALE: &str = "stale";
const MISSING: &str = "missing";
/// A value already in the file stands where a rule would go; it is left alone.
const OVERRIDDEN: &str = "overridden by the existing file";

/// Merge one write, and put it on disk unless this is a check.
fn act(
    guard: &Guard,
    entry: &Write,
    path: &Path,
    schema: &Value,
    check: bool,
) -> Result<(&'static str, Vec<String>), CliError> {
    let fragment = render(guard, entry, schema)?;
    let existing = std::fs::read_to_string(path).ok();
    let present = existing.is_some();
    let outcome = penv_guards::apply(entry, existing.as_deref(), &fragment).map_err(|e| {
        CliError::new(
            "guard_failed",
            e.to_string(),
            "Fix the file by hand, then run penv guard again.",
        )
        .with_exit(Exit::Validation)
    })?;

    if !check && outcome.changed {
        put(entry, path, &outcome.content)?;
    }
    let status = match (present, outcome.changed) {
        _ if !outcome.blocked.is_empty() => OVERRIDDEN,
        (false, _) => MISSING,
        (true, true) => STALE,
        (true, false) => CURRENT,
    };
    Ok((status, outcome.blocked))
}

fn put(entry: &Write, path: &Path, content: &str) -> Result<(), CliError> {
    if entry.executable {
        write_executable(path, content)
    } else {
        write_file_making_parents(path, content)
    }
}

fn render(guard: &Guard, entry: &Write, schema: &Value) -> Result<String, CliError> {
    penv_guards::render(guard, entry, schema, env!("CARGO_PKG_VERSION")).map_err(|e| {
        CliError::new(
            "guard_failed",
            e.to_string(),
            "Fix the guard folder, or drop it so the built-in one is used again.",
        )
        .with_exit(Exit::Validation)
    })
}

fn pretty(rendered: &str) -> String {
    serde_json::from_str::<Value>(rendered)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| rendered.to_string())
}

struct Installed {
    repo: PathBuf,
    home: Option<PathBuf>,
}

impl penv_guards::Probe for Installed {
    fn exists(&self, path: &str) -> bool {
        match path.strip_prefix("~/") {
            Some(rest) => self.home.as_ref().is_some_and(|h| h.join(rest).exists()),
            None => self.repo.join(path).exists(),
        }
    }

    fn on_path(&self, exe: &str) -> bool {
        !on_path(exe, &[]).is_empty()
    }
}
