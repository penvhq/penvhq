use std::path::Path;

use penv_dotenv::{ensure_ignored, infer, read};
use penv_schema::render;
use serde_json::json;

use crate::env::Env;
use crate::error::CliError;
use crate::files::{
    ENV_FILE, GITIGNORE_FILE, SCHEMA_FILE, read_file, show, value_files, write_file,
};
use crate::output::{Output, Report, Style, table};
use crate::prompt;

/// Which harnesses `init` guards.
pub enum Guards {
    /// The installed set, offered as a picker when a person is watching.
    Ask,
    /// The installed set, with nothing to answer.
    Installed,
    /// Exactly these, from `--guards`.
    Named(Vec<String>),
    /// None, from `--no-guards`.
    None,
}

/// Read the `.env` here, write the schema it implies, and ignore the file.
pub fn run(
    out: &Output,
    cwd: &Path,
    force: bool,
    guards: &Guards,
    output: Option<&Path>,
    env: &Env,
    agent_flag: bool,
) -> Result<Report, CliError> {
    let schema_path = cwd.join(SCHEMA_FILE);
    if schema_path.is_file() && !force {
        return keep_schema(out, cwd, &schema_path);
    }
    // Before anything is written, as penv guard refuses it: the harness would
    // otherwise drop out of the chosen set without a word.
    if !matches!(guards, Guards::None) {
        super::guard::refuse_shadowing(cwd)?;
    }

    // Refused here, an --output it cannot use leaves nothing half written.
    let plan = super::r#gen::plan(cwd, output)?;

    // A first run with nothing to read still leaves a repository penv works in.
    let env_path = cwd.join(ENV_FILE);
    let created_dotenv = value_files(cwd).is_empty();
    if created_dotenv {
        write_file(&env_path, "")?;
    }

    // Every value file feeds the draft, `.env` first, so a varlock or
    // dotenv-flow folder keeps the keys only its `.env.production` holds.
    let mut dotenv = penv_dotenv::Dotenv::default();
    for path in value_files(cwd) {
        for entry in read(&read_file(&path)?).entries {
            if dotenv.get(&entry.key).is_none() {
                dotenv.entries.push(entry);
            }
        }
    }
    let schema = infer(&dotenv);
    write_file(&schema_path, &render(&schema))?;
    // The schema version goes in the committed settings file, not the schema,
    // so the schema stays loadable by varlock.
    let mut config = crate::config::Config::load(cwd)?;
    if config.schema_version().is_none() {
        config.set_schema_version(i64::from(penv_schema::SCHEMA_VERSION));
    }
    config.fill_defaults();
    config.save(cwd)?;

    let ignore_path = cwd.join(GITIGNORE_FILE);
    let existing = if ignore_path.is_file() {
        read_file(&ignore_path)?
    } else {
        String::new()
    };
    let update = ensure_ignored(&existing);
    if update.changed() {
        write_file(&ignore_path, &update.content)?;
    }

    let style = out.style();
    // Only `run` asks for the installed set outright, and mid-run is no place for
    // a picker of any kind.
    let interactive =
        !matches!(guards, Guards::Installed) && super::interactive(out, env, agent_flag);
    let chosen = choose(cwd, guards, interactive, &style)?;
    let guarded = super::guard::write_selected(cwd, &schema.to_json(), &chosen);
    let generated = super::r#gen::auto(out, plan, &schema, interactive)?;

    let rows: Vec<Vec<String>> = schema
        .keys
        .iter()
        .map(|key| {
            vec![
                key.name.clone(),
                key.ty.to_string(),
                yes_no(key.required),
                yes_no(key.sensitive),
            ]
        })
        .collect();

    let mut lines = Vec::new();
    if !rows.is_empty() {
        lines.push(table(
            &["KEY", "TYPE", "REQUIRED", "SENSITIVE"],
            &rows,
            &style,
        ));
        lines.push(String::new());
    }
    if created_dotenv {
        // `set` writes to the cloud, and this schema names no project yet.
        lines.push(style.dim(&format!(
            "created {ENV_FILE}; add keys to it, then run penv init --force"
        )));
    }
    let count = schema.keys.len();
    lines.push(style.dim(&format!(
        "wrote {} from {count} {}",
        show(&schema_path),
        if count == 1 { "key" } else { "keys" }
    )));
    if update.changed() {
        lines.push(style.dim(&format!(
            "added {} to {}",
            update.added.join(", "),
            show(&ignore_path)
        )));
    }
    if !generated.written.is_empty() {
        lines.push(style.dim(&format!(
            "generated {}",
            generated
                .written
                .iter()
                .map(|p| show(p))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    lines.extend(generated.notes.iter().cloned());
    if !guarded.is_empty() {
        lines.push(style.dim(&format!(
            "guarded {}",
            guarded.iter().map(|p| show(p)).collect::<Vec<_>>().join(", ")
        )));
    }
    if !dotenv.warnings.is_empty() {
        lines.push(style.dim(&format!(
            "{} line(s) in {ENV_FILE} sit outside the safe subset; penv check names them",
            dotenv.warnings.len()
        )));
    }
    let text = lines.join("\n");

    Ok(Report::new(
        json!({
            "schema": show(&schema_path),
            "createdDotenv": created_dotenv,
            "keys": schema.keys.iter().map(|key| json!({
                "name": key.name,
                "type": key.ty.to_string(),
                "required": key.required,
                "sensitive": key.sensitive,
            })).collect::<Vec<_>>(),
            "gitignore": {
                "path": show(&ignore_path),
                "added": update.added,
            },
            "generated": generated.written.iter().map(|p| show(p)).collect::<Vec<_>>(),
            "targets": generated.targets,
            "guards": chosen,
            "guarded": guarded.iter().map(|p| show(p)).collect::<Vec<_>>(),
            "warnings": dotenv.warnings.iter().map(|w| json!({
                "line": w.line,
                "code": w.code,
                "message": w.message,
            })).collect::<Vec<_>>(),
        }),
        text,
    ))
}

pub fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

/// The names `--guards` gave, tidied: `--guards ""` and `--guards cursor,,`
/// both mean what they look like.
pub fn named(given: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in given.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
        if !out.iter().any(|kept| kept == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// The harnesses to write. A person at a terminal picks; everything else takes
/// the flags, or the harnesses this machine has.
fn choose(
    cwd: &Path,
    guards: &Guards,
    interactive: bool,
    style: &Style,
) -> Result<Vec<String>, CliError> {
    let known = super::guard::known(cwd);
    let names: Vec<String> = known.iter().map(|(g, _)| g.name.clone()).collect();
    let installed: Vec<usize> = known
        .iter()
        .enumerate()
        .filter(|(_, (_, installed))| *installed)
        .map(|(index, _)| index)
        .collect();
    let pick = |chosen: Vec<usize>| chosen.iter().map(|i| names[*i].clone()).collect();

    match guards {
        Guards::None => Ok(Vec::new()),
        Guards::Named(named) => {
            for name in named {
                if !names.contains(name) {
                    return Err(CliError::new(
                        "unknown_harness",
                        format!("penv has no guard for {name}."),
                        "Run penv guard --check to see the harnesses it knows.",
                    ));
                }
            }
            Ok(named.clone())
        }
        Guards::Installed => Ok(pick(installed)),
        Guards::Ask if !interactive => Ok(pick(installed)),
        Guards::Ask => Ok(pick(ask(&known, &installed, style)?)),
    }
}

/// The picker: every harness penv knows, the installed ones already ticked.
fn ask(
    known: &[(penv_guards::Guard, bool)],
    installed: &[usize],
    style: &Style,
) -> Result<Vec<usize>, CliError> {
    let labels: Vec<String> = known
        .iter()
        .map(|(guard, _)| match &guard.description {
            Some(about) => format!("{}: {about}", guard.name),
            None => guard.name.clone(),
        })
        .collect();
    if let Some(picked) = crate::ui::multiselect(
        "Which AI tools should be kept out of your secrets? (space to tick)",
        &labels,
        installed,
    ) {
        return picked.map_err(super::cloud::cancelled);
    }
    let rows: Vec<Vec<String>> = known
        .iter()
        .enumerate()
        .map(|(index, (guard, present))| {
            vec![
                format!("{}", index + 1),
                guard.name.clone(),
                guard.description.clone().unwrap_or_default(),
                yes_no(*present),
            ]
        })
        .collect();
    let shown: Vec<String> = installed.iter().map(|i| (i + 1).to_string()).collect();
    let listing = format!(
        "{}\n\n",
        table(&["#", "HARNESS", "GUARDS", "INSTALLED"], &rows, style)
    );
    let prompt = format!(
        "{listing}guard which harnesses? [{}] (numbers, all, none): ",
        if shown.is_empty() {
            "none".to_string()
        } else {
            shown.join(" ")
        }
    );
    let answer = prompt::read_line(&prompt)?;
    parse_selection(&answer, known.len(), installed)
}

/// Enter keeps what is shown; otherwise `all`, `none`, or the numbers on the
/// lines to guard.
fn parse_selection(answer: &str, count: usize, shown: &[usize]) -> Result<Vec<usize>, CliError> {
    let answer = answer.trim();
    if answer.is_empty() {
        return Ok(shown.to_vec());
    }
    if answer.eq_ignore_ascii_case("all") {
        return Ok((0..count).collect());
    }
    if answer.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    let mut chosen: Vec<usize> = Vec::new();
    for word in answer.split([' ', ',', '\t']).filter(|w| !w.is_empty()) {
        let number = word
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=count).contains(n));
        let Some(number) = number else {
            return Err(unreadable_selection(word, count));
        };
        if !chosen.contains(&(number - 1)) {
            chosen.push(number - 1);
        }
    }
    Ok(chosen)
}

fn unreadable_selection(word: &str, count: usize) -> CliError {
    CliError::new(
        "unreadable_selection",
        format!("{word} is not one of the {count} harnesses listed."),
        "Answer with numbers such as 1 3, or all, or none, or press Enter to keep what is shown.",
    )
}

/// A folder whose schema was written first, by hand or by another tool: keep it,
/// and still do the rest of what init does for a repository, which is keeping
/// the value files out of git and recording the schema version.
fn keep_schema(out: &Output, cwd: &Path, schema_path: &Path) -> Result<Report, CliError> {
    let ignore_path = cwd.join(GITIGNORE_FILE);
    let existing = if ignore_path.is_file() {
        read_file(&ignore_path)?
    } else {
        String::new()
    };
    let update = ensure_ignored(&existing);
    if update.changed() {
        write_file(&ignore_path, &update.content)?;
    }
    let mut config = crate::config::Config::load(cwd)?;
    let versioned = config.schema_version().is_none();
    if versioned {
        config.set_schema_version(i64::from(penv_schema::SCHEMA_VERSION));
    }
    config.fill_defaults();
    config.save(cwd)?;
    let style = out.style();
    let mut lines = vec![format!(
        "{} {}; penv init --force writes it again from your .env",
        style.dim("kept"),
        show(schema_path)
    )];
    if update.changed() {
        lines.push(format!(
            "added {} to {}",
            update.added.join(", "),
            show(&ignore_path)
        ));
    }
    if versioned {
        lines.push(style.dim(&format!(
            "recorded the schema version in {}",
            crate::config::CONFIG_FILE
        )));
    }
    lines.push(format!("next: {}", style.bold("penv check")));
    Ok(Report::new(
        json!({
            "schema": show(schema_path),
            "kept": true,
            "gitignore": update.added,
            "next": "penv check",
        }),
        lines.join("\n"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picker_reads_every_answer_it_offers() {
        let shown = [0usize, 3];
        assert_eq!(
            parse_selection("", 5, &shown).unwrap(),
            shown,
            "Enter keeps"
        );
        assert_eq!(parse_selection("all", 5, &shown).unwrap(), [0, 1, 2, 3, 4]);
        assert!(parse_selection("none", 5, &shown).unwrap().is_empty());
        assert_eq!(parse_selection("1,3", 5, &shown).unwrap(), [0, 2]);
        assert_eq!(parse_selection("2 4", 5, &shown).unwrap(), [1, 3]);
        assert_eq!(parse_selection(" 2 , 2 ", 5, &shown).unwrap(), [1]);
    }

    #[test]
    fn the_names_a_flag_gave_are_tidied_and_an_empty_list_means_none() {
        let given =
            |names: &[&str]| named(&names.iter().map(|n| n.to_string()).collect::<Vec<_>>());
        assert_eq!(given(&[" cursor ", "codex", "cursor"]), ["cursor", "codex"]);
        assert!(given(&["", "  "]).is_empty());
    }

    #[test]
    fn an_answer_the_picker_cannot_read_names_the_forms_it_takes() {
        for answer in ["yes please", "9", "0", "1 nope"] {
            let error = parse_selection(answer, 5, &[]).unwrap_err();
            assert_eq!(error.code, "unreadable_selection");
            assert!(error.fix.contains("all"), "{}", error.fix);
            assert!(error.fix.contains("none"), "{}", error.fix);
        }
    }
}
