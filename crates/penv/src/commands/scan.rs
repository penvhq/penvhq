use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use penv_mask::Masker;
use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::Fetcher;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::show;
use crate::output::{Output, Report};
use crate::source;

/// Files larger than this are skipped: they are build output, not source.
const MAX_BYTES: u64 = 5 * 1024 * 1024;

const HOOK: &str =
    "#!/bin/sh\n# penv: refuse a commit that holds a secret value\nexec penv scan --staged\n";

/// Look for this environment's sensitive values in the repository's files, in
/// every form `run` masks. Findings name the file, line and key, never the value.
pub fn run(
    out: &Output,
    cwd: &Path,
    paths: &[PathBuf],
    staged: bool,
    install_hook: bool,
    env_flag: Option<&str>,
    env: &Env,
) -> Result<Report, CliError> {
    if install_hook {
        return hook(out, cwd);
    }
    let (schema_path, schema) = super::load_schema(cwd)?;
    let dir = schema_path.parent().unwrap_or(cwd);
    let environment = source::environment(env_flag, env, &schema, dir);
    let detection = detect_here(env, std::io::stdout().is_terminal());
    let mut fetcher = Fetcher::new(env, &detection);
    let resolved = source::values(&schema, dir, &environment, env, &mut fetcher)?;

    let secrets: Vec<(String, String)> = resolved
        .values
        .iter()
        .filter(|(name, value)| {
            !value.is_empty() && source::is_sensitive(&schema, &resolved.tainted, name)
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();

    let files = candidates(cwd, paths, staged)?;
    let mut findings = Vec::new();
    for file in &files {
        let Some(text) = contents(cwd, file, staged) else {
            continue;
        };
        for (name, value) in &secrets {
            if !leaks(value, &text) {
                continue;
            }
            for (index, line) in text.lines().enumerate() {
                if leaks(value, line) {
                    findings.push((show(file), index + 1, name.clone()));
                }
            }
        }
    }

    let style = out.style();
    let mut lines: Vec<String> = findings
        .iter()
        .map(|(file, line, key)| {
            format!(
                "{} {file}:{line} holds the value of {key}",
                style.red("leak")
            )
        })
        .collect();
    if findings.is_empty() {
        lines.push(format!(
            "{} {} file(s), {} sensitive value(s) from {environment}",
            style.green("clean"),
            files.len(),
            secrets.len()
        ));
    } else {
        lines.push(style.dim(
            "Remove the value, rotate it if it was ever pushed, and read it from penv run instead.",
        ));
    }
    let report = Report::new(
        json!({
            "environment": environment,
            "files": files.len(),
            "values": secrets.len(),
            "findings": findings.iter().map(|(file, line, key)| json!({ "file": file, "line": line, "key": key })).collect::<Vec<_>>(),
        }),
        lines.join("\n"),
    );
    Ok(if findings.is_empty() {
        report
    } else {
        report.with_exit(Exit::Validation)
    })
}

/// Directories that frameworks write browser code into. Anything secret found
/// here ships to every visitor. `public/` is Gatsby's output only when a Gatsby
/// config sits beside it; elsewhere it is source.
pub const CLIENT_OUTPUT: [&str; 8] = [
    ".next/static",
    "out",
    "dist",
    "build",
    ".output/public",
    ".svelte-kit/output/client",
    "storybook-static",
    ".vercel/output/static",
];

/// React Native release bundles land under the native projects, at paths that
/// move between versions (`android/app/build/generated/assets/react/release/`,
/// `android/app/build/ASSETS/createBundleReleaseJsAndAssets/`, Xcode's build
/// products), so these folders are read for bundle files by name only.
pub const NATIVE_OUTPUT: [&str; 2] = ["android/app/build", "ios/build"];

fn is_native_bundle(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    name.ends_with(".bundle") || name.ends_with(".jsbundle") || name.ends_with(".hbc")
}

/// Bundles under the React Native build folders changed at or after `since`.
pub fn native_bundles_since(dir: &Path, since: std::time::SystemTime) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = NATIVE_OUTPUT
        .iter()
        .map(|d| dir.join(d))
        .filter(|p| p.is_dir())
        .collect();
    let mut files = changed_since(&roots, since);
    files.retain(|p| is_native_bundle(p));
    files
}

/// Client output directories under `dir` that exist.
pub fn client_output(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = CLIENT_OUTPUT
        .iter()
        .map(|d| dir.join(d))
        .filter(|p| p.is_dir())
        .collect();
    let gatsby = ["gatsby-config.js", "gatsby-config.ts", "gatsby-config.mjs"]
        .iter()
        .any(|f| dir.join(f).is_file());
    if gatsby && dir.join("public").is_dir() {
        out.push(dir.join("public"));
    }
    out
}

/// Every place a secret value shows up in the files, in any form `run` masks:
/// (file, line, key). One pass per file finds whether anything matches; only then
/// is each key and line looked at.
pub fn find(files: &[PathBuf], secrets: &[(String, String)]) -> Vec<(PathBuf, usize, String)> {
    let values: Vec<String> = secrets.iter().map(|(_, v)| v.clone()).collect();
    let mut found = Vec::new();
    for file in files {
        let Some(text) = read_text(file) else {
            // Hermes bytecode and other binary bundles keep strings as raw bytes:
            // look for each value as written, and report the file without a line.
            for name in raw_hits(file, secrets) {
                found.push((file.clone(), 0, name));
            }
            continue;
        };
        if !leaks_any(&values, &text) {
            continue;
        }
        for (name, value) in secrets {
            if !leaks(value, &text) {
                continue;
            }
            for (index, line) in text.lines().enumerate() {
                if leaks(value, line) {
                    found.push((file.clone(), index + 1, name.clone()));
                }
            }
        }
    }
    found
}

/// Keys whose raw value appears in a binary file. Bundles may be large, so the
/// size cap is higher for them.
fn raw_hits(path: &Path, secrets: &[(String, String)]) -> Vec<String> {
    let cap = if is_native_bundle(path) {
        64 * 1024 * 1024
    } else {
        MAX_BYTES
    };
    let Ok(meta) = path.metadata() else {
        return Vec::new();
    };
    if meta.len() > cap {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    secrets
        .iter()
        .filter(|(_, v)| v.len() >= penv_mask::MIN_SECRET_LEN)
        .filter(|(_, v)| bytes.windows(v.len()).any(|w| w == v.as_bytes()))
        .map(|(n, _)| n.clone())
        .collect()
}

fn read_text(path: &Path) -> Option<String> {
    if path.metadata().ok()?.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Files under the directories changed at or after `since`, so a build scan
/// looks at what this build wrote, not last week's output.
pub fn changed_since(dirs: &[PathBuf], since: std::time::SystemTime) -> Vec<PathBuf> {
    let mut all = Vec::new();
    for dir in dirs {
        walk(dir, &mut all);
    }
    all.retain(|p| {
        p.metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t >= since)
    });
    all
}

fn leaks_any(values: &[String], text: &str) -> bool {
    let mut masker = Masker::new(values.to_vec());
    let mut masked = Vec::new();
    masker.feed(text.as_bytes(), &mut masked);
    masker.finish(&mut masked);
    masked != text.as_bytes()
}

/// True when any form `run` would mask shows up in `text`.
fn leaks(value: &str, text: &str) -> bool {
    let mut masker = Masker::new(vec![value.to_string()]);
    let mut masked = Vec::new();
    masker.feed(text.as_bytes(), &mut masked);
    masker.finish(&mut masked);
    masked != text.as_bytes()
}

/// The paths named, else what git would commit: the staged set with
/// `--staged`, otherwise tracked and untracked-but-not-ignored files.
fn candidates(cwd: &Path, paths: &[PathBuf], staged: bool) -> Result<Vec<PathBuf>, CliError> {
    let listed: Vec<PathBuf> = if !paths.is_empty() {
        let mut out = Vec::new();
        for path in paths {
            walk(&cwd.join(path), &mut out);
        }
        out
    } else {
        let args: &[&str] = if staged {
            &[
                "diff",
                "--cached",
                "--name-only",
                "--diff-filter=ACMR",
                "-z",
            ]
        } else {
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ]
        };
        git(cwd, args)?
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .collect()
    };
    Ok(listed
        .into_iter()
        .filter(|p| {
            // A value file is where values belong; `penv guard` keeps agents out of it.
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            !penv_dotenv::is_value_file(name)
        })
        .collect())
}

/// Files under `path`. A symbolic link below the starting point is never
/// followed: a link back up the tree would otherwise loop until the path is too
/// long, and a link out of the folder is not part of it. Value files are where
/// values belong, so they are left out.
fn walk(path: &Path, out: &mut Vec<PathBuf>) {
    walk_at(path, out, 0);
}

fn walk_at(path: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    const MAX_FILES: usize = 200_000;
    if out.len() >= MAX_FILES || depth > 64 {
        return;
    }
    let Ok(meta) = (if depth == 0 {
        path.metadata()
    } else {
        path.symlink_metadata()
    }) else {
        return;
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if meta.is_file() {
        if !penv_dotenv::is_value_file(name) {
            out.push(path.to_path_buf());
        }
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name == ".git" || name == "node_modules" || name == "target" {
            continue;
        }
        walk_at(&p, out, depth + 1);
    }
}

/// Text to search: the staged blob with `--staged`, else the file. Binary and
/// oversized files are skipped.
fn contents(cwd: &Path, file: &Path, staged: bool) -> Option<String> {
    let bytes = if staged {
        let spec = format!(":{}", file.to_string_lossy().replace('\\', "/"));
        Command::new("git")
            .arg("show")
            .arg(spec)
            .current_dir(cwd)
            .output()
            .ok()?
            .stdout
    } else {
        let path = cwd.join(file);
        if path.metadata().ok()?.len() > MAX_BYTES {
            return None;
        }
        std::fs::read(path).ok()?
    };
    if bytes.len() as u64 > MAX_BYTES || bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, CliError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| {
            CliError::new(
                "no_git",
                format!("git could not be run: {e}."),
                "Install git, or name the files: penv scan <PATH>...",
            )
        })?;
    if !output.status.success() {
        return Err(CliError::new(
            "not_a_repository",
            "this folder is not inside a git repository.",
            "Name the files instead: penv scan <PATH>...",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A pre-commit hook that runs `penv scan --staged`. A hook penv did not write
/// is left alone.
fn hook(out: &Output, cwd: &Path) -> Result<Report, CliError> {
    let hooks = git(cwd, &["rev-parse", "--git-path", "hooks"])?;
    let path = cwd.join(hooks.trim()).join("pre-commit");
    if path.is_file() {
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if existing.contains("penv scan --staged") {
            return Ok(Report::new(
                json!({ "hook": show(&path), "written": false }),
                format!(
                    "{} {} already runs penv scan",
                    out.style().dim("ok"),
                    show(&path)
                ),
            ));
        }
        return Err(CliError::new(
            "hook_exists",
            format!("{} already exists and penv did not write it.", show(&path)),
            "Add the line `penv scan --staged` to it yourself.",
        ));
    }
    crate::files::write_file_making_parents(&path, HOOK)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    Ok(Report::new(
        json!({ "hook": show(&path), "written": true }),
        format!("{} {}", out.style().green("wrote"), show(&path)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_found_raw_and_encoded() {
        let secret = "sk_test_FAKE_0123456789";
        assert!(leaks(secret, &format!("const key = \"{secret}\";")));
        assert!(!leaks(secret, "const key = process.env.KEY;"));
    }

    #[test]
    fn value_files_are_not_scanned() {
        let dir = std::env::temp_dir().join(format!("penv-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".env.production"), "A=1\n").unwrap();
        std::fs::write(dir.join("app.ts"), "x\n").unwrap();
        let found = candidates(&dir, &[PathBuf::from(".")], false).unwrap();
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["app.ts"]);
    }
}
