use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use penv_mask::Masker;
use serde_json::json;

use crate::agent::detect_here;
use crate::commands::cloud::Fetcher;
use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::show;
use crate::output::{Output, Report};
use crate::source;

/// Files up to this size are read whole and every line holding a value is
/// named; a larger one is searched as a stream and named by its first hit.
const MAX_BYTES: u64 = 5 * 1024 * 1024;

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

    let mut finder = Finder::new(&secrets);
    let mut findings: Vec<(String, usize, String)> = Vec::new();
    let searched = if staged {
        let blobs = staged_blobs(cwd, paths)?;
        read_blobs(cwd, &blobs, |file, bytes| {
            for (line, key) in finder.in_bytes(bytes) {
                findings.push((show(file), line, key));
            }
        })?;
        blobs.len()
    } else {
        let files = candidates(cwd, paths)?;
        for file in &files {
            for (line, key) in finder.in_file(&cwd.join(file)) {
                findings.push((show(file), line, key));
            }
        }
        files.len()
    };

    let style = out.style();
    let mut lines: Vec<String> = findings
        .iter()
        .map(|(file, line, key)| {
            let at = if *line == 0 {
                file.clone()
            } else {
                format!("{file}:{line}")
            };
            format!("{} {at} holds the value of {key}", style.red("leak"))
        })
        .collect();
    if findings.is_empty() {
        lines.push(format!(
            "{} {searched} file(s), {} sensitive value(s) from {environment}",
            style.green("clean"),
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
            "files": searched,
            "values": secrets.len(),
            "findings": findings.iter().map(|(file, line, key)| json!({ "file": file, "line": (*line > 0).then_some(*line), "key": key })).collect::<Vec<_>>(),
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
/// (file, line, key), line 0 for a file of bytes rather than lines.
pub fn find(files: &[PathBuf], secrets: &[(String, String)]) -> Vec<(PathBuf, usize, String)> {
    let mut finder = Finder::new(secrets);
    let mut found = Vec::new();
    for file in files {
        for (line, key) in finder.in_file(file) {
            found.push((file.clone(), line, key));
        }
    }
    found
}

/// Which values a file holds. One masker over every value is the only thing
/// most files meet; the one per value runs only on a file that already matched.
/// Hermes bytecode and other binary bundles keep strings as raw bytes, so a
/// file holding a NUL is searched the same way and named without a line.
pub(crate) struct Finder {
    all: Masker,
    each: Vec<(String, Masker)>,
}

impl Finder {
    pub(crate) fn new(secrets: &[(String, String)]) -> Finder {
        Finder {
            all: Masker::new(secrets.iter().map(|(_, v)| v.clone()).collect()),
            each: secrets
                .iter()
                .map(|(name, value)| (name.clone(), Masker::new(vec![value.clone()])))
                .collect(),
        }
    }

    /// (line, key) for each hit in the file at `path`.
    pub(crate) fn in_file(&mut self, path: &Path) -> Vec<(usize, String)> {
        let Ok(meta) = path.metadata() else {
            return Vec::new();
        };
        if meta.len() <= MAX_BYTES {
            return match std::fs::read(path) {
                Ok(bytes) => self.in_bytes(&bytes),
                Err(_) => Vec::new(),
            };
        }
        let open = || std::fs::File::open(path).ok();
        if self.all.is_pass_through() || open().and_then(|f| first_hit(&mut self.all, f)).is_none()
        {
            return Vec::new();
        }
        let binary = open().is_some_and(|f| {
            let mut head = Vec::new();
            f.take(64 * 1024).read_to_end(&mut head).is_ok() && head.contains(&0)
        });
        let mut found = Vec::new();
        for (name, masker) in &mut self.each {
            if let Some(line) = open().and_then(|f| first_hit(masker, f)) {
                found.push((if binary { 0 } else { line }, name.clone()));
            }
        }
        found
    }

    /// (line, key) for each hit in `bytes`: every line a value is on, or the
    /// first hit's line where the value spans lines.
    pub(crate) fn in_bytes(&mut self, bytes: &[u8]) -> Vec<(usize, String)> {
        if self.all.is_pass_through() || first_hit(&mut self.all, bytes).is_none() {
            return Vec::new();
        }
        let binary = bytes.contains(&0);
        let text = if binary || bytes.len() as u64 > MAX_BYTES {
            None
        } else {
            std::str::from_utf8(bytes).ok()
        };
        let mut found = Vec::new();
        for (name, masker) in &mut self.each {
            let Some(first) = first_hit(masker, bytes) else {
                continue;
            };
            let before = found.len();
            if let Some(text) = text {
                for (index, line) in text.lines().enumerate() {
                    if first_hit(masker, line.as_bytes()).is_some() {
                        found.push((index + 1, name.clone()));
                    }
                }
            }
            if found.len() == before {
                found.push((if binary { 0 } else { first }, name.clone()));
            }
        }
        found
    }
}

/// The 1-based line where the masker first changes the stream, read in chunks
/// so a file of any size costs one buffer. Until its first replacement a
/// masker's output is its input, so the first difference is the hit.
fn first_hit(masker: &mut Masker, mut from: impl Read) -> Option<usize> {
    let mut chunk = vec![0u8; 64 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    let mut out: Vec<u8> = Vec::new();
    let mut lines = 1;
    let mut ended = false;
    let hit = loop {
        out.clear();
        match from.read(&mut chunk) {
            Ok(0) => {
                ended = true;
                masker.finish(&mut out);
            }
            Ok(n) => {
                pending.extend_from_slice(&chunk[..n]);
                masker.feed(&chunk[..n], &mut out);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                ended = true;
                masker.finish(&mut out);
            }
        }
        if let Some(at) = (0..out.len()).find(|&i| pending.get(i) != Some(&out[i])) {
            break Some(lines + newlines(&pending[..at]));
        }
        lines += newlines(&pending[..out.len()]);
        pending.drain(..out.len());
        if ended {
            break None;
        }
    };
    if !ended {
        // Leave the masker empty for the next stream.
        out.clear();
        masker.finish(&mut out);
    }
    hit
}

fn newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| **b == b'\n').count()
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

/// A value file is where values belong; `penv guard` keeps agents out of it.
fn is_value_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    penv_dotenv::is_value_file(name)
}

/// The paths named, else what git would commit: tracked and
/// untracked-but-not-ignored files.
pub(crate) fn candidates(cwd: &Path, paths: &[PathBuf]) -> Result<Vec<PathBuf>, CliError> {
    let listed: Vec<PathBuf> = if !paths.is_empty() {
        let mut out = Vec::new();
        for path in paths {
            walk(&cwd.join(path), &mut out);
        }
        out
    } else {
        let listing = git(
            cwd,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        )?;
        String::from_utf8_lossy(&listing)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .collect()
    };
    Ok(listed.into_iter().filter(|p| !is_value_file(p)).collect())
}

/// The staged blobs a commit would take, each with the path it is staged at
/// (from the repository root). Named paths narrow the set as pathspecs, read
/// from `cwd` the way git reads them.
fn staged_blobs(cwd: &Path, paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>, CliError> {
    let mut args: Vec<String> = [
        "diff",
        "--cached",
        "--raw",
        "-z",
        "--no-abbrev",
        "--no-renames",
        "--diff-filter=ACM",
        "--",
    ]
    .map(String::from)
    .to_vec();
    args.extend(paths.iter().map(|p| p.to_string_lossy().into_owned()));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let listing = git(cwd, &args)?;
    let mut fields = listing.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut out = Vec::new();
    // `:old-mode new-mode old-sha new-sha status`, then the path.
    while let (Some(meta), Some(path)) = (fields.next(), fields.next()) {
        let meta = String::from_utf8_lossy(meta);
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let (Some(mode), Some(sha)) = (parts.get(1), parts.get(3)) else {
            continue;
        };
        // A submodule is a commit, not a file.
        if *mode == "160000" {
            continue;
        }
        let path = PathBuf::from(String::from_utf8_lossy(path).into_owned());
        if !is_value_file(&path) {
            out.push((path, sha.to_string()));
        }
    }
    Ok(out)
}

/// Every blob's bytes, from one `git cat-file --batch`.
fn read_blobs(
    cwd: &Path,
    blobs: &[(PathBuf, String)],
    mut each: impl FnMut(&Path, &[u8]),
) -> Result<(), CliError> {
    if blobs.is_empty() {
        return Ok(());
    }
    let mut child = Command::new("git")
        .args(["cat-file", "--batch"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(no_git)?;
    let mut input = child.stdin.take().expect("a piped stdin");
    let wanted: String = blobs.iter().map(|(_, sha)| format!("{sha}\n")).collect();
    let writer = std::thread::spawn(move || input.write_all(wanted.as_bytes()));
    let mut from = std::io::BufReader::new(child.stdout.take().expect("a piped stdout"));
    let mut header = String::new();
    let mut body = Vec::new();
    for (path, _) in blobs {
        header.clear();
        if from.read_line(&mut header).unwrap_or(0) == 0 {
            break;
        }
        // `<sha> <type> <size>`, or `<sha> missing`.
        let Some(size) = header
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.parse::<usize>().ok())
        else {
            continue;
        };
        body.resize(size + 1, 0);
        if from.read_exact(&mut body).is_err() {
            break;
        }
        each(path, &body[..size]);
    }
    let _ = writer.join();
    let _ = child.wait();
    Ok(())
}

fn no_git(e: std::io::Error) -> CliError {
    CliError::new(
        "no_git",
        format!("git could not be run: {e}."),
        "Install git, or name the files: penv scan <PATH>...",
    )
}

/// Files under `path`. A symbolic link below the starting point is never
/// followed: a link back up the tree would otherwise loop until the path is too
/// long, and a link out of the folder is not part of it. Value files are where
/// values belong, so they are left out.
pub(crate) fn walk(path: &Path, out: &mut Vec<PathBuf>) {
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

fn git(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, CliError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(no_git)?;
    if !output.status.success() {
        return Err(CliError::new(
            "not_a_repository",
            "this folder is not inside a git repository.",
            "Name the files instead: penv scan <PATH>...",
        ));
    }
    Ok(output.stdout)
}

/// A pre-commit hook that runs `penv scan --staged`. git runs it from the top of
/// the work tree, so it first goes back to where it was installed: in a monorepo
/// the schema is an app's, not the root's. A hook penv did not write is left
/// alone.
fn hook(out: &Output, cwd: &Path) -> Result<Report, CliError> {
    let hooks = String::from_utf8_lossy(&git(cwd, &["rev-parse", "--git-path", "hooks"])?)
        .trim()
        .to_string();
    let prefix = String::from_utf8_lossy(&git(cwd, &["rev-parse", "--show-prefix"])?)
        .trim_end_matches(['\n', '\r'])
        .to_string();
    let path = cwd.join(hooks).join("pre-commit");
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
    crate::files::write_file_making_parents(&path, &hook_script(&prefix))?;
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

/// The hook's text, for a folder `prefix` below the top of the work tree.
fn hook_script(prefix: &str) -> String {
    let mut script = String::from("#!/bin/sh\n# penv: refuse a commit that holds a secret value\n");
    if !prefix.is_empty() {
        let quoted = prefix.replace('\'', "'\\''");
        script.push_str(&format!("cd -- '{quoted}' || exit 1\n"));
    }
    script.push_str("exec penv scan --staged\n");
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finds(secret: &str, text: &str) -> Vec<(usize, String)> {
        Finder::new(&[("KEY".to_string(), secret.to_string())]).in_bytes(text.as_bytes())
    }

    #[test]
    fn a_value_is_found_raw_and_encoded() {
        let secret = "sk_test_FAKE_0123456789";
        assert_eq!(
            finds(secret, &format!("const key = \"{secret}\";")),
            [(1, "KEY".to_string())]
        );
        assert!(finds(secret, "const key = process.env.KEY;").is_empty());
    }

    #[test]
    fn one_finder_names_each_key_on_each_line_it_is_on() {
        let secrets = [
            ("A_KEY".to_string(), "sk_test_FAKE_aaaa1111".to_string()),
            ("B_KEY".to_string(), "sk_test_FAKE_bbbb2222".to_string()),
        ];
        let mut finder = Finder::new(&secrets);
        let text =
            "x\nsk_test_FAKE_bbbb2222\ny\nc2tfdGVzdF9GQUtFX2FhYWExMTEx sk_test_FAKE_bbbb2222\n";
        assert_eq!(
            finder.in_bytes(text.as_bytes()),
            [
                (4, "A_KEY".to_string()),
                (2, "B_KEY".to_string()),
                (4, "B_KEY".to_string())
            ]
        );
        assert!(
            finder.in_bytes(b"clean\n").is_empty(),
            "the finder is reused clean"
        );
        let bytes = b"HBC\x00\x01sk_test_FAKE_aaaa1111\x00";
        assert_eq!(finder.in_bytes(bytes), [(0, "A_KEY".to_string())]);
    }

    #[test]
    fn a_stream_reports_the_line_of_its_first_hit_across_chunks() {
        let secret = "sk_test_FAKE_0123456789";
        let mut text = "filler line\n".repeat(20_000);
        text.push_str(&format!("let k = '{secret}';\n"));
        let mut masker = Masker::new(vec![secret.to_string()]);
        assert_eq!(first_hit(&mut masker, text.as_bytes()), Some(20_001));
        assert_eq!(first_hit(&mut masker, &b"nothing here\n"[..]), None);
    }

    #[test]
    fn value_files_are_not_scanned() {
        let dir = std::env::temp_dir().join(format!("penv-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".env.production"), "A=1\n").unwrap();
        std::fs::write(dir.join("app.ts"), "x\n").unwrap();
        let found = candidates(&dir, &[PathBuf::from(".")]).unwrap();
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["app.ts"]);
    }

    #[test]
    fn the_hook_goes_back_to_the_folder_it_was_installed_from() {
        assert_eq!(
            hook_script(""),
            "#!/bin/sh\n# penv: refuse a commit that holds a secret value\nexec penv scan --staged\n"
        );
        assert!(
            hook_script("apps/it's web/").contains("cd -- 'apps/it'\\''s web/' || exit 1\n"),
            "{}",
            hook_script("apps/it's web/")
        );
    }
}
