//! A best-effort walk of the parent chain, the last rung of agent detection.
//! Every platform is allowed to answer with nothing.

use penv_agent::Ancestry;

/// How far up to look before giving up on recognising anything.
const MAX_DEPTH: usize = 6;

pub struct Processes;

impl Ancestry for Processes {
    fn parent_names(&self) -> Vec<String> {
        chain().to_vec()
    }
}

/// The walk runs once per process: every command asks more than once, and on
/// macOS each walk spawns `ps`.
fn chain() -> &'static [String] {
    static CHAIN: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CHAIN.get_or_init(parents)
}

#[cfg(target_os = "linux")]
fn parents() -> Vec<String> {
    let mut out = Vec::new();
    let mut pid = std::process::id();
    for _ in 0..MAX_DEPTH {
        let Some(parent) = field(pid, "PPid:") else {
            break;
        };
        if parent <= 1 {
            break;
        }
        match std::fs::read_to_string(format!("/proc/{parent}/comm")) {
            Ok(name) => out.push(name.trim().to_string()),
            Err(_) => break,
        }
        pid = parent;
    }
    out
}

#[cfg(target_os = "linux")]
fn field(pid: u32, name: &str) -> Option<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix(name))
        .and_then(|value| value.trim().parse().ok())
}

/// One `ps` for the whole table, walked in memory; spawning one per level would
/// cost more than the walk is worth.
#[cfg(all(unix, not(target_os = "linux")))]
fn parents() -> Vec<String> {
    use std::collections::BTreeMap;

    let Ok(listing) = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&listing.stdout);
    let table: BTreeMap<u32, (u32, String)> = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let name = fields.collect::<Vec<&str>>().join(" ");
            (!name.is_empty()).then_some((pid, (parent, name)))
        })
        .collect();

    let mut out = Vec::new();
    let mut pid = std::process::id();
    for _ in 0..MAX_DEPTH {
        let Some((parent, _)) = table.get(&pid) else {
            break;
        };
        let Some((_, name)) = table.get(parent) else {
            break;
        };
        out.push(name.clone());
        pid = *parent;
    }
    out
}

/// Windows has no cheap parent walk that does not shell out; the environment
/// markers carry detection there.
#[cfg(not(unix))]
fn parents() -> Vec<String> {
    let _ = MAX_DEPTH;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parent_chain_is_walked_once_per_process() {
        assert!(std::ptr::eq(chain(), chain()));
        assert_eq!(Processes.parent_names(), chain());
    }
}
