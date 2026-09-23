//! The in-process layer of masking. `run` writes two small files that ship inside
//! this binary and points the child's runtime at them: Node through
//! `NODE_OPTIONS=--require`, Bun through `BUN_OPTIONS=--preload`, Deno by adding
//! `--preload` to `deno run|serve|test|watch`, Python through `sitecustomize` on
//! `PYTHONPATH`. They mask console and log text and the bodies a server sends,
//! which the output pipe never sees. Compiled languages have no such hook.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const JS: &str = include_str!("../preload/penv-preload.cjs");
pub const PYTHON: &str = include_str!("../preload/python/sitecustomize.py");

/// Where the files were written for this build of penv.
#[derive(Debug, Clone)]
pub struct Files {
    pub js: PathBuf,
    pub python_dir: PathBuf,
}

/// This user's cache folder. Never the shared temp folder: another account on
/// the machine could place its own file there first, and the child would run it
/// with the secrets in its environment. With no home folder there is no preload.
fn cache_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        return var("LOCALAPPDATA").map(|d| d.join("penv"));
    }
    if cfg!(target_os = "macos") {
        return var("HOME").map(|h| h.join("Library/Caches/penv"));
    }
    var("XDG_CACHE_HOME")
        .or_else(|| var("HOME").map(|h| h.join(".cache")))
        .map(|d| d.join("penv"))
}

/// Write the files once per penv version and content, under this user's cache
/// folder. They hold code only, never a value.
pub fn files() -> Option<Files> {
    let hash = fnv(JS.as_bytes()) ^ fnv(PYTHON.as_bytes()).rotate_left(1);
    let dir = cache_dir()?.join(format!("preload-{}-{hash:016x}", env!("CARGO_PKG_VERSION")));
    let js = dir.join("penv-preload.cjs");
    let python_dir = dir.join("python");
    let python = python_dir.join("sitecustomize.py");
    let fresh = |path: &Path, body: &str| std::fs::read_to_string(path).is_ok_and(|s| s == body);
    if !(fresh(&js, JS) && fresh(&python, PYTHON)) {
        std::fs::create_dir_all(&python_dir).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for d in [&dir, &python_dir] {
                std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).ok()?;
            }
        }
        // Write beside, then rename, so a run starting at the same moment never
        // loads half a file.
        for (path, body) in [(&js, JS), (&python, PYTHON)] {
            let partial = path.with_extension(format!("{}.part", std::process::id()));
            std::fs::write(&partial, body).ok()?;
            std::fs::rename(&partial, path).ok()?;
        }
    }
    Some(Files { js, python_dir })
}

/// The environment and argument changes that load the preload into the child.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Injection {
    pub env: Vec<(String, String)>,
    /// Arguments to insert after the Deno subcommand, when the child is Deno.
    pub deno_args: Vec<String>,
}

/// Build the changes for one run. `current` reads a variable as the child would
/// otherwise inherit it, so an existing `NODE_OPTIONS` is extended, never replaced.
pub fn inject(
    files: &Files,
    sensitive: &[String],
    argv: &[String],
    current: impl Fn(&str) -> Option<String>,
    deno_has_preload: impl Fn(&str) -> bool,
) -> Injection {
    let mut out = Injection::default();
    if sensitive.is_empty() {
        return out;
    }
    out.env.push(("PENV_SENSITIVE".into(), sensitive.join(",")));
    let js = files.js.to_string_lossy().to_string();
    let quoted = format!("\"{}\"", js.replace('\\', "\\\\").replace('"', "\\\""));
    let append = |name: &str, flag: String| {
        let existing = current(name).unwrap_or_default();
        if existing.contains(&js) {
            return existing;
        }
        if existing.trim().is_empty() {
            flag
        } else {
            format!("{existing} {flag}")
        }
    };
    out.env.push((
        "NODE_OPTIONS".into(),
        append("NODE_OPTIONS", format!("--require {quoted}")),
    ));
    // Bun reads BUN_OPTIONS without taking quotes off a `--preload=` value, so
    // only a path with no whitespace can go there. A temp folder with a space in
    // it (a Windows user name) leaves Bun to the output pipe alone.
    if !js.chars().any(char::is_whitespace) {
        out.env.push((
            "BUN_OPTIONS".into(),
            append("BUN_OPTIONS", format!("--preload={js}")),
        ));
    }
    let python = files.python_dir.to_string_lossy().to_string();
    let separator = if cfg!(windows) { ";" } else { ":" };
    let pythonpath = match current("PYTHONPATH").filter(|p| !p.is_empty()) {
        Some(existing) if existing.split(separator).any(|p| p == python) => existing,
        Some(existing) => format!("{python}{separator}{existing}"),
        None => python,
    };
    out.env.push(("PYTHONPATH".into(), pythonpath));

    if let Some(program) = argv.first() {
        let name = Path::new(program)
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let sub = argv.get(1).map(String::as_str).unwrap_or_default();
        if name == "deno"
            && matches!(sub, "run" | "serve" | "test" | "watch")
            && deno_has_preload(program)
        {
            out.deno_args = vec!["--preload".into(), js];
        }
    }
    out
}

/// Whether this Deno knows `--preload`: asked once per binary and remembered
/// beside the preload files, keyed by the binary's size and modified time.
pub fn deno_has_preload(program: &str, files: &Files) -> bool {
    let resolved = crate::files::on_path(program, &[])
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(program));
    let stamp = std::fs::metadata(&resolved)
        .ok()
        .map(|m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            format!("{}-{modified}", m.len())
        })
        .unwrap_or_default();
    let dir = files.js.parent().unwrap_or(Path::new("."));
    let memo = dir.join(format!(
        "deno-{:016x}",
        fnv(format!("{}{stamp}", resolved.display()).as_bytes())
    ));
    if let Ok(answer) = std::fs::read_to_string(&memo) {
        return answer == "yes";
    }
    let yes = Command::new(&resolved)
        .args(["run", "--help"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("--preload"));
    let _ = std::fs::write(&memo, if yes { "yes" } else { "no" });
    yes
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Files {
        Files {
            js: PathBuf::from("/tmp/p/penv-preload.cjs"),
            python_dir: PathBuf::from("/tmp/p/python"),
        }
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn existing_runtime_options_are_extended_never_replaced() {
        let env = |name: &str| match name {
            "NODE_OPTIONS" => Some("--max-old-space-size=4096".into()),
            "PYTHONPATH" => Some("/app/src".into()),
            _ => None,
        };
        let got = inject(
            &files(),
            &["K".into()],
            &argv(&["node", "a.js"]),
            env,
            |_| true,
        );
        let get = |n: &str| {
            got.env
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(
            get("NODE_OPTIONS"),
            "--max-old-space-size=4096 --require \"/tmp/p/penv-preload.cjs\""
        );
        assert_eq!(get("BUN_OPTIONS"), "--preload=/tmp/p/penv-preload.cjs");
        assert!(get("PYTHONPATH").starts_with("/tmp/p/python"));
        assert!(get("PYTHONPATH").ends_with("/app/src"));
        assert_eq!(get("PENV_SENSITIVE"), "K");
        assert!(got.deno_args.is_empty());
    }

    #[test]
    fn nothing_is_injected_without_a_sensitive_value_and_nested_runs_do_not_stack() {
        assert_eq!(
            inject(&files(), &[], &argv(&["node"]), |_| None, |_| true),
            Injection::default()
        );
        let nested = |name: &str| {
            (name == "NODE_OPTIONS").then(|| "--require \"/tmp/p/penv-preload.cjs\"".to_string())
        };
        let got = inject(&files(), &["K".into()], &argv(&["node"]), nested, |_| true);
        let node = &got.env.iter().find(|(k, _)| k == "NODE_OPTIONS").unwrap().1;
        assert_eq!(node.matches("penv-preload").count(), 1);
    }

    #[test]
    fn a_path_with_a_space_is_quoted_for_node_and_left_out_of_bun() {
        let spaced = Files {
            js: PathBuf::from("/tmp/John Doe/penv-preload.cjs"),
            python_dir: PathBuf::from("/tmp/John Doe/python"),
        };
        let got = inject(&spaced, &["K".into()], &argv(&["node"]), |_| None, |_| true);
        let node = &got.env.iter().find(|(k, _)| k == "NODE_OPTIONS").unwrap().1;
        assert_eq!(node, "--require \"/tmp/John Doe/penv-preload.cjs\"");
        assert!(!got.env.iter().any(|(k, _)| k == "BUN_OPTIONS"));
    }

    #[test]
    fn deno_gets_the_flag_only_for_commands_that_run_code_and_only_if_it_knows_it() {
        let deno = |a: &[&str], knows: bool| {
            inject(&files(), &["K".into()], &argv(a), |_| None, move |_| knows).deno_args
        };
        assert_eq!(
            deno(&["deno", "run", "-A", "main.ts"], true),
            ["--preload", "/tmp/p/penv-preload.cjs"]
        );
        assert_eq!(deno(&["/usr/bin/deno", "serve", "main.ts"], true).len(), 2);
        assert!(deno(&["deno", "task", "dev"], true).is_empty());
        assert!(deno(&["deno", "run", "main.ts"], false).is_empty());
    }
}
