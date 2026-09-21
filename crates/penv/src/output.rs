use std::io::Write;

use serde_json::Value;

use crate::cli::Format;
use crate::env::Env;
use crate::error::{CliError, Exit};

/// What one command produced, in both forms, and what it exits with. The renderer
/// picks the form; only the command knows whether the news is good.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub json: Value,
    pub text: String,
    pub exit: Exit,
    /// The command wrote whatever the caller needs itself.
    pub silent: bool,
}

impl Report {
    pub fn new(json: Value, text: impl Into<String>) -> Report {
        Report {
            json,
            text: text.into(),
            exit: Exit::Ok,
            silent: false,
        }
    }

    pub fn silent() -> Report {
        Report {
            json: Value::Null,
            text: String::new(),
            exit: Exit::Ok,
            silent: true,
        }
    }

    pub fn with_exit(mut self, exit: Exit) -> Report {
        self.exit = exit;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Render {
    pub json: bool,
    pub color: bool,
}

/// JSON whenever stdout is not a terminal, or the caller asked for it. Colour only
/// on a terminal that has not opted out.
pub fn resolve(
    force_json: bool,
    format: Option<Format>,
    agent: bool,
    stdout_tty: bool,
    env: &Env,
) -> Render {
    let json = match format {
        Some(Format::Json) => true,
        Some(Format::Text) => false,
        None => force_json || agent || !stdout_tty,
    };
    let opted_out = env.get("NO_COLOR").is_some_and(|v| !v.is_empty())
        || env.get("CLICOLOR").is_some_and(|v| v == "0");
    Render {
        json,
        color: stdout_tty && !json && !opted_out,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    color: bool,
}

impl Style {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    pub fn red(&self, text: &str) -> String {
        self.paint("31", text)
    }

    pub fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }

    pub fn yellow(&self, text: &str) -> String {
        self.paint("33", text)
    }
}

pub struct Output {
    render: Render,
}

impl Output {
    pub fn new(render: Render) -> Output {
        Output { render }
    }

    pub fn is_json(&self) -> bool {
        self.render.json
    }

    pub fn style(&self) -> Style {
        Style {
            color: self.render.color,
        }
    }

    pub fn write(&self, report: &Report, to: &mut impl Write) -> std::io::Result<()> {
        if report.silent {
            Ok(())
        } else if self.render.json {
            writeln!(to, "{}", serde_json::to_string_pretty(&report.json)?)
        } else if report.text.is_empty() {
            Ok(())
        } else {
            writeln!(to, "{}", report.text.trim_end())
        }
    }

    pub fn fail(&self, error: &CliError, to: &mut impl Write) -> std::io::Result<()> {
        if self.render.json {
            writeln!(to, "{}", error.to_json())
        } else {
            let style = self.style();
            writeln!(
                to,
                "{} {} {}",
                style.red("error:"),
                error.message,
                style.dim(&error.fix)
            )
        }
    }
}

/// Left-aligned columns, two spaces apart. Empty rows render as nothing.
pub fn table(headers: &[&str], rows: &[Vec<String>], style: &Style) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let mut out = String::new();
    out.push_str(&style.dim(&join(
        &headers.iter().map(|h| h.to_string()).collect::<Vec<_>>(),
        &widths,
    )));
    for row in rows {
        out.push('\n');
        out.push_str(&join(row, &widths));
    }
    out
}

fn join(cells: &[String], widths: &[usize]) -> String {
    let last = cells.len().saturating_sub(1);
    cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            if i == last {
                cell.clone()
            } else {
                let pad = widths[i].saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(pad))
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}
