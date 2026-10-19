//! The argument form: `name($SUBJECT, address, option=value, ...)`. Shape checks
//! read `vocabulary.json`; a failure names the function and option, never a value.

use std::collections::BTreeSet;

use super::Arg;
use crate::editor::{Entry, Word, vocabulary};
use crate::resolve::Expr;

/// The vocabulary entry of a function that takes the argument form.
pub fn word(name: &str) -> Option<&'static Word> {
    vocabulary()
        .functions
        .iter()
        .find(|w| w.name == name && !w.subject.is_empty())
}

/// Split `label=value` on the first `=` outside quotes and brackets.
pub fn named(text: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth -= 1,
            (None, '=') if depth == 0 => return Some((text[..i].trim(), text[i + 1..].trim())),
            _ => {}
        }
    }
    None
}

/// A label is a dotted, indexed path: `db.hosts[0]`, `query.sslmode`, `["a.b"]`.
pub fn is_label(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with(['$', '.', '='])
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '[' | ']' | '"'))
}

/// Unknown, repeated or missing option, address where none is allowed, positional
/// where a name is needed.
pub fn check_args(name: &str, args: &[Arg]) -> Result<(), String> {
    let Some(word) = word(name) else {
        return Ok(());
    };
    let subjects = args.iter().filter(|a| matches!(a, Arg::Subject(_))).count();
    let has_subject = matches!(args.first(), Some(Arg::Subject(_)));
    if word.subject == "none" && subjects > 0 {
        return Err(format!(
            "{name}() takes every argument as name=value; write the option's name before ="
        ));
    }
    if subjects > 1 || (subjects == 1 && !has_subject) {
        return Err(format!(
            "{name}() takes one subject, first; write the other arguments as name=value"
        ));
    }
    if word.subject == "required" && !has_subject {
        return Err(format!(
            "{name}() needs a subject first, such as {name}($KEY, ...)"
        ));
    }
    let addresses: Vec<&str> = args
        .iter()
        .filter_map(|a| match a {
            Arg::Address(at) => Some(at.as_str()),
            _ => None,
        })
        .collect();
    if let Some(at) = addresses.first() {
        address(name, word, at, addresses.len(), has_subject)?;
    }
    let mut seen = BTreeSet::new();
    for arg in args {
        let Arg::Named(label, value) = arg else {
            continue;
        };
        if !seen.insert(label.as_str()) {
            return Err(format!("{name}() has {label} twice; write it once"));
        }
        if !addresses.is_empty() {
            return Err(format!(
                "{name}() reads or writes, never both: drop the bare argument or the {label}= one"
            ));
        }
        option(name, word, label, value)?;
    }
    if word.labels.is_none() {
        let missing = word
            .args
            .iter()
            .find(|o| o.required && !seen.contains(o.name.as_str()));
        if let Some(o) = missing {
            return Err(format!("{name}() needs {}=", o.name));
        }
        needs(name, word, args, &seen)?;
    }
    Ok(())
}

/// An option value whose vocabulary entry lists `needs` requires those options too.
fn needs(name: &str, word: &Word, args: &[Arg], seen: &BTreeSet<&str>) -> Result<(), String> {
    for arg in args {
        let Arg::Named(label, Expr::Text(text)) = arg else {
            continue;
        };
        let Some(section) = word
            .args
            .iter()
            .find(|o| &o.name == label)
            .and_then(|o| o.values_from.as_ref())
        else {
            continue;
        };
        let Some(entry) = vocabulary().section(section).iter().find(|e| e.names(text)) else {
            continue;
        };
        if let Some(need) = entry.needs.iter().find(|n| !seen.contains(n.as_str())) {
            return Err(format!("{name}() {label} {} needs {need}=", entry.name));
        }
    }
    Ok(())
}

fn address(name: &str, word: &Word, at: &str, count: usize, subject: bool) -> Result<(), String> {
    let Some(slot) = &word.address else {
        return Err(format!(
            "{name}() takes the bare argument as name=value; write the option's name before ="
        ));
    };
    if count > 1 {
        return Err(format!(
            "{name}() reads one part or path; call it once for each"
        ));
    }
    if !subject {
        return Err(format!(
            "{name}() reads the bare argument from a subject; write the subject first, such as {name}($KEY, ...)"
        ));
    }
    match &slot.values_from {
        Some(section) if !names(section, at) => Err(format!(
            "{name}() has no part named by the bare argument; parts are {}",
            listed(section)
        )),
        _ => Ok(()),
    }
}

fn option(name: &str, word: &Word, label: &str, value: &Expr) -> Result<(), String> {
    if let Some(slot) = &word.labels {
        let Some(section) = &slot.values_from else {
            return Ok(());
        };
        let entry = vocabulary()
            .section(section)
            .iter()
            .find(|e| e.names(label));
        return match entry {
            None => Err(format!(
                "{name}() has no part {label}; parts are {}",
                listed(section)
            )),
            Some(e) if e.read_only => Err(format!("{name}() reads {label} but cannot write it")),
            Some(_) => Ok(()),
        };
    }
    let Some(opt) = word.args.iter().find(|o| o.name == label) else {
        let known: Vec<&str> = word.args.iter().map(|o| o.name.as_str()).collect();
        return Err(format!(
            "{name}() has no option {label}; options are {}",
            known.join(", ")
        ));
    };
    let Expr::Text(text) = value else {
        return Ok(());
    };
    if let Some(section) = &opt.values_from
        && !names(section, text)
    {
        return Err(format!(
            "{name}() option {label} takes one of {}",
            listed(section)
        ));
    }
    if opt.kind == "integer" && text.parse::<u64>().is_err() {
        return Err(format!("{name}() option {label} takes a whole number"));
    }
    Ok(())
}

fn names(section: &str, text: &str) -> bool {
    vocabulary().section(section).iter().any(|e| e.names(text))
}

fn listed(section: &str) -> String {
    let all: Vec<&str> = vocabulary()
        .section(section)
        .iter()
        .map(|e: &Entry| e.name.as_str())
        .collect();
    all.join(", ")
}
