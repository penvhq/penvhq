//! Everything a person sees that is not the report itself: notes, warnings,
//! refusals, pickers and spinners. Drawn on stderr, and only at a terminal with a
//! person behind it; every other session gets the plain lines it always got.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::CliError;

static PRETTY: AtomicBool = AtomicBool::new(false);

/// One decision for both libraries, so NO_COLOR means the same thing everywhere.
pub fn init(pretty: bool, color: bool) {
    PRETTY.store(pretty, Ordering::Relaxed);
    console::set_colors_enabled(color);
    console::set_colors_enabled_stderr(color);
}

pub fn pretty() -> bool {
    PRETTY.load(Ordering::Relaxed)
}

fn plain(line: &str) {
    let _ = writeln!(std::io::stderr(), "penv: {line}");
}

pub fn note(line: &str) {
    if !pretty() || cliclack::log::info(line).is_err() {
        plain(line);
    }
}

pub fn warn(line: &str) {
    if !pretty() || cliclack::log::warning(line).is_err() {
        plain(line);
    }
}

/// True when the refusal was drawn, so the caller does not print it again.
pub fn fail(error: &CliError) -> bool {
    pretty()
        && cliclack::log::error(&error.message).is_ok()
        && cliclack::log::remark(format!("{} {}", console::style("fix").dim(), error.fix)).is_ok()
}

pub fn done(text: &str) -> bool {
    pretty() && cliclack::log::success(text).is_ok()
}

pub fn listing(text: &str) -> bool {
    pretty() && cliclack::log::remark(text).is_ok()
}

/// Arrow keys at a terminal. `None` means this session has to ask another way.
pub fn select(
    title: &str,
    items: &[String],
    initial: Option<usize>,
) -> Option<std::io::Result<usize>> {
    if !pretty() {
        return None;
    }
    let mut picker = cliclack::select(title);
    if let Some(initial) = initial {
        picker = picker.initial_value(initial);
    }
    for (index, item) in items.iter().enumerate() {
        picker = picker.item(index, item, "");
    }
    Some(picker.interact())
}

pub fn multiselect(
    title: &str,
    items: &[String],
    chosen: &[usize],
) -> Option<std::io::Result<Vec<usize>>> {
    if !pretty() {
        return None;
    }
    let mut picker = cliclack::multiselect(title)
        .initial_values(chosen.to_vec())
        .required(false);
    for (index, item) in items.iter().enumerate() {
        picker = picker.item(index, item, "");
    }
    Some(picker.interact())
}

/// A spinner for one network call. Silent wherever the pretty layer is off.
pub struct Spinner(Option<cliclack::ProgressBar>);

pub fn spinner(label: &str) -> Spinner {
    if !pretty() {
        return Spinner(None);
    }
    let bar = cliclack::spinner();
    bar.start(label);
    Spinner(Some(bar))
}

impl Spinner {
    pub fn stop(mut self, label: &str) {
        if let Some(bar) = self.0.take() {
            bar.stop(label);
        }
    }
}

impl Drop for Spinner {
    /// An early return is a failure about to be reported, so the line goes away.
    fn drop(&mut self) {
        if let Some(bar) = self.0.take() {
            bar.clear();
        }
    }
}

pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut table = comfy_table::Table::new();
    table
        .load_style(comfy_table::presets::UTF8_BORDERS_ONLY)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic)
        .set_width(console::Term::stdout().size().1.saturating_sub(4))
        .set_header(headers.to_vec());
    for row in rows {
        table.add_row(
            row.iter()
                .map(|cell| console::strip_ansi_codes(cell).into_owned()),
        );
    }
    table.to_string()
}
