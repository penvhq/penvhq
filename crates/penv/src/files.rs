use std::path::{Path, PathBuf};

use crate::error::CliError;

pub const SCHEMA_FILE: &str = ".env.schema";
pub const ENV_FILE: &str = ".env";
pub const GITIGNORE_FILE: &str = ".gitignore";

/// Every value file in `dir` (`.env`, `.env.local`, `.env.<env>`, ...), `.env`
/// first and the rest in name order. The schema and example files are not values.
pub fn value_files(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(penv_dotenv::is_value_file)
        })
        .collect();
    found.sort_by_key(|p| {
        (
            p.file_name() != Some(std::ffi::OsStr::new(ENV_FILE)),
            p.clone(),
        )
    });
    found
}

/// The nearest `.env.schema` at or above `start`. A monorepo holds one per app.
pub fn find_schema(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        let candidate = dir.join(SCHEMA_FILE);
        candidate.is_file().then_some(candidate)
    })
}

pub fn read_file(path: &Path) -> Result<String, CliError> {
    std::fs::read_to_string(path).map_err(|e| {
        CliError::new(
            "unreadable_file",
            format!("{} could not be read: {e}.", show(path)),
            "Check the path and its permissions.",
        )
    })
}

pub fn write_file(path: &Path, contents: &str) -> Result<(), CliError> {
    std::fs::write(path, contents).map_err(|e| {
        CliError::new(
            "unwritable_file",
            format!("{} could not be written: {e}.", show(path)),
            "Check the directory and its permissions.",
        )
    })
}

/// Refuse a write under `root` that a symbolic link would carry somewhere
/// else: a committed `out -> ~/.bashrc` passes any check made on the text of
/// the path. Makes the parent directories it checks.
pub fn within(root: &Path, path: &Path) -> Result<(), CliError> {
    let refused = || {
        CliError::new(
            "output_outside_repo",
            format!(
                "{} leads out of {} through a symbolic link.",
                show(path),
                show(root)
            ),
            "Replace the link with a directory, or name another path.",
        )
        .with_exit(crate::error::Exit::Validation)
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| {
            CliError::new(
                "unwritable_file",
                format!("{} could not be created: {e}.", show(parent)),
                "Check the directory and its permissions.",
            )
        })?;
        let (Ok(real_root), Ok(real_parent)) = (root.canonicalize(), parent.canonicalize()) else {
            return Err(refused());
        };
        if !real_parent.starts_with(&real_root) {
            return Err(refused());
        }
    }
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(refused());
    }
    Ok(())
}

/// A file that will hold values: nobody but this account may read it, a file
/// written before with wider access included. The contents land in a private
/// file beside it and are renamed into place, so a reader never sees half a file.
/// Windows keeps the directory's inherited ACL, which is the user's own profile.
pub fn write_private_file(path: &Path, contents: &str) -> Result<(), CliError> {
    use std::io::Write;

    let failed = |e: std::io::Error| {
        CliError::new(
            "unwritable_file",
            format!("{} could not be written: {e}.", show(path)),
            "Check the directory and its permissions.",
        )
    };
    // A linked value file keeps its link: the file it points at is replaced.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    let temp = target.with_file_name(format!(".{name}.{}.{nanos}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&temp).and_then(|mut file| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    });
    if let Err(e) = written.and_then(|()| std::fs::rename(&temp, &target)) {
        let _ = std::fs::remove_file(&temp);
        return Err(failed(e));
    }
    Ok(())
}

pub fn write_file_making_parents(path: &Path, contents: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            CliError::new(
                "unwritable_file",
                format!("{} could not be created: {e}.", show(parent)),
                "Check the directory and its permissions.",
            )
        })?;
    }
    write_file(path, contents)
}

/// A path in one separator, the platform's. Joining a `.claude/settings.json`
/// onto a Windows directory otherwise prints both.
pub fn show(path: &Path) -> String {
    let text = path.display().to_string();
    if std::path::MAIN_SEPARATOR == '\\' {
        text.replace('/', "\\")
    } else {
        text
    }
}

/// A hook script the harness runs itself. A file without the execute bit fails
/// open, so the mode is part of writing it.
pub fn write_executable(path: &Path, contents: &str) -> Result<(), CliError> {
    write_file_making_parents(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| {
            CliError::new(
                "unwritable_file",
                format!("{} could not be made executable: {e}.", show(path)),
                "Check the file and its permissions.",
            )
        })?;
    }
    Ok(())
}

/// The home directory, for the `~/.penv` half of every lookup order.
pub fn home() -> Option<String> {
    for var in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(var)
            && !value.is_empty()
        {
            return Some(value.to_string_lossy().into_owned());
        }
    }
    None
}

/// Every file a name could run, in `dirs` first and then along PATH. Where
/// PATHEXT says what runs, only those suffixes do: `tsc` beside `tsc.cmd` is a
/// shell script Windows cannot start.
pub fn on_path(exe: &str, dirs: &[PathBuf]) -> Vec<PathBuf> {
    let extensions: Vec<String> = match std::env::var("PATHEXT") {
        Ok(list) if !list.is_empty() => list.split(';').map(str::to_lowercase).collect(),
        _ => vec![String::new()],
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    dirs.iter()
        .cloned()
        .chain(std::env::split_paths(&path))
        .filter_map(|dir| {
            extensions
                .iter()
                .map(|ext| dir.join(format!("{exe}{ext}")))
                .find(|candidate| candidate.is_file())
        })
        .collect()
}

/// The filesystem, for the one tree the target and guard loaders read.
pub struct Disk;

impl penv_targets::Tree for Disk {
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn dirs(&self, path: &str) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }

    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn files(&self, path: &str) -> Vec<String> {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shown_path_never_mixes_separators() {
        let shown = show(&PathBuf::from("repo").join(".claude/settings.json"));
        assert!(!(shown.contains('/') && shown.contains('\\')), "{shown}");
        assert!(shown.contains(std::path::MAIN_SEPARATOR), "{shown}");
    }

    #[cfg(unix)]
    #[test]
    fn a_value_file_written_before_with_wider_access_ends_up_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("penv-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(&path, "OLD=1\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_private_file(&path, "A_KEY=fake_value\n").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "A_KEY=fake_value\n"
        );
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            mode & 0o777,
            0o600,
            "a pull must not leave values readable by others"
        );
        assert_eq!(left.len(), 1, "no temporary file is left beside it");
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_script_is_written_with_the_execute_bit() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!("penv-hook-{}", std::process::id()));
        write_executable(&path, "#!/bin/sh\nexec penv hook cline \"$@\"\n").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        let _ = std::fs::remove_file(&path);
        assert_eq!(mode & 0o111, 0o111, "a script nobody can run fails open");
    }
}
