//! Whether git would carry a value file into the repository. Asked of git
//! itself, so nested `.gitignore` files, `.git/info/exclude` and a global
//! excludes file all count. Outside a repository, or with no git on PATH, there
//! is nothing to say.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exposure {
    /// Already committed or staged: ignoring it now changes nothing.
    Tracked,
    /// Untracked and not ignored: the next `git add .` takes it.
    Unignored,
}

/// How `file` stands with the repository around it, or `None` when git would
/// leave it out (or there is no repository).
pub fn exposure(file: &Path) -> Option<Exposure> {
    exposures(&[file.to_path_buf()]).pop().flatten()
}

/// [`exposure`] for each file, in order, asking git twice per directory rather
/// than twice per file.
pub fn exposures(files: &[PathBuf]) -> Vec<Option<Exposure>> {
    let mut out = vec![None; files.len()];
    let mut dirs: Vec<&Path> = files.iter().filter_map(|f| f.parent()).collect();
    dirs.sort();
    dirs.dedup();
    for dir in dirs {
        let here: Vec<(usize, &OsStr)> = files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.parent() == Some(dir))
            .filter_map(|(i, f)| Some((i, f.file_name()?)))
            .collect();
        let names: Vec<&OsStr> = here.iter().map(|(_, n)| *n).collect();
        let Some(tracked) = listed(
            dir,
            &["--literal-pathspecs", "ls-files", "-z", "--"],
            &names,
            false,
        ) else {
            continue;
        };
        let untracked: Vec<&OsStr> = names
            .iter()
            .copied()
            .filter(|n| !tracked.iter().any(|t| t == n))
            .collect();
        let ignored = if untracked.is_empty() {
            Vec::new()
        } else {
            match listed(dir, &["check-ignore", "-z", "--stdin"], &untracked, true) {
                Some(ignored) => ignored,
                None => continue,
            }
        };
        for (i, name) in here {
            out[i] = if tracked.iter().any(|t| t == name) {
                Some(Exposure::Tracked)
            } else if ignored.iter().any(|t| t == name) {
                None
            } else {
                Some(Exposure::Unignored)
            };
        }
    }
    out
}

/// The names git printed back from one call over `names`, passed as arguments
/// or on stdin. `None` outside a repository or without git.
fn listed(dir: &Path, args: &[&str], names: &[&OsStr], stdin: bool) -> Option<Vec<OsString>> {
    // A repository's own config could name a program for git to run on index
    // reads (core.fsmonitor); these queries never need one.
    let mut command = Command::new("git");
    command
        .args(["-c", "core.fsmonitor=false"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if stdin {
        command.stdin(Stdio::piped());
    } else {
        command.args(names).stdin(Stdio::null());
    }
    let mut child = command.spawn().ok()?;
    if let Some(mut input) = child.stdin.take() {
        let mut bytes = Vec::new();
        for name in names {
            bytes.extend_from_slice(name.as_encoded_bytes());
            bytes.push(0);
        }
        std::thread::spawn(move || input.write_all(&bytes));
    }
    let output = child.wait_with_output().ok()?;
    // check-ignore answers 1 when nothing is ignored; 128 is no repository.
    if !matches!(output.status.code(), Some(0) | Some(1)) {
        return None;
    }
    Some(
        output
            .stdout
            .split(|b| *b == 0)
            .filter(|n| !n.is_empty())
            .map(|n| OsString::from(String::from_utf8_lossy(n).into_owned()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .current_dir(dir)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn every_file_in_a_directory_is_judged_in_one_pass() {
        let dir = std::env::temp_dir().join(format!("penv-exposure-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        if !git(&dir, &["init", "-q"]) {
            return;
        }
        for name in [".env", ".env.local", ".env.production"] {
            std::fs::write(dir.join(name), "A_KEY=fake_value\n").unwrap();
        }
        std::fs::write(dir.join(".gitignore"), ".env.local\n").unwrap();
        assert!(git(&dir, &["add", "-f", ".env.production"]));
        let files: Vec<PathBuf> = [".env", ".env.local", ".env.production"]
            .iter()
            .map(|n| dir.join(n))
            .collect();
        let found = exposures(&files);
        let one = exposure(&dir.join(".env"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            found,
            [Some(Exposure::Unignored), None, Some(Exposure::Tracked)]
        );
        assert_eq!(one, Some(Exposure::Unignored));
    }
}
