//! Whether git would carry a value file into the repository. Asked of git
//! itself, so nested `.gitignore` files, `.git/info/exclude` and a global
//! excludes file all count. Outside a repository, or with no git on PATH, there
//! is nothing to say.

use std::path::Path;
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
    let dir = file.parent()?;
    let name = file.file_name()?;
    let git = |args: &[&std::ffi::OsStr]| {
        // A repository's own config could name a program for git to run on
        // index reads (core.fsmonitor); these two queries never need one.
        Command::new("git")
            .args(["-c", "core.fsmonitor=false"])
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .and_then(|s| s.code())
    };
    let os = |s: &'static str| std::ffi::OsStr::new(s);
    // 0: tracked. 1: not tracked. 128: not a repository.
    match git(&[os("ls-files"), os("--error-unmatch"), os("--"), name])? {
        0 => return Some(Exposure::Tracked),
        1 => {}
        _ => return None,
    }
    // 0: ignored. 1: not ignored.
    match git(&[os("check-ignore"), os("-q"), os("--"), name])? {
        1 => Some(Exposure::Unignored),
        _ => None,
    }
}
