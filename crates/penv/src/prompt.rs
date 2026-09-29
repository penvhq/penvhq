//! Reading one value without echoing it. A pipe is read whole; a terminal is
//! read with the echo bit off and put back afterwards.

use std::io::{BufRead, IsTerminal, Read, Write};

use crate::error::CliError;

/// True when something is feeding stdin, so there is nothing to prompt for.
pub fn piped() -> bool {
    !std::io::stdin().is_terminal()
}

/// The value for a key. It is never written to stdout, stderr or a log.
pub fn read_value(prompt: &str) -> Result<String, CliError> {
    let stdin = std::io::stdin();
    if piped() {
        let mut buffer = String::new();
        stdin
            .lock()
            .read_to_string(&mut buffer)
            .map_err(unreadable)?;
        return Ok(trimmed(&buffer));
    }
    read_typed(prompt)
}

/// One echoed line, for a choice that is not a value. The prompt goes to stderr,
/// so stdout still carries only the report.
pub fn read_line(prompt: &str) -> Result<String, CliError> {
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "{prompt}");
    let _ = stderr.flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(unreadable)?;
    Ok(trimmed(&line))
}

/// A value typed at a terminal. Without the echo bit off it would be typed onto
/// the screen, so there is no reading it here at all.
fn read_typed(prompt: &str) -> Result<String, CliError> {
    let stdin = std::io::stdin();
    let Some(echo) = crate::tty::Mode::echo_off() else {
        return Err(CliError::new(
            "no_echo_off",
            "this terminal cannot hide what you type, so penv will not ask for a secret here.",
            "Pipe the value on stdin instead: printf %s \"$VALUE\" | penv set <KEY>.",
        ));
    };

    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "{prompt}");
    let _ = stderr.flush();

    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    drop(echo);
    let _ = writeln!(stderr);
    read.map_err(unreadable)?;
    Ok(trimmed(&line))
}

fn trimmed(value: &str) -> String {
    value.trim_end_matches(['\n', '\r']).to_string()
}

fn unreadable(e: std::io::Error) -> CliError {
    CliError::new(
        "unreadable_input",
        format!("the value could not be read: {e}."),
        "Pipe the value in, or type it at a terminal.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_newline_is_not_part_of_the_value() {
        assert_eq!(trimmed("sk_test_FAKE\n"), "sk_test_FAKE");
        assert_eq!(trimmed("sk_test_FAKE\r\n"), "sk_test_FAKE");
        assert_eq!(trimmed("sk_test_FAKE"), "sk_test_FAKE");
        assert_eq!(trimmed(" spaced "), " spaced ", "only the newline goes");
    }

    #[test]
    fn a_terminal_that_keeps_echoing_is_refused_rather_than_read() {
        // The suite runs with stdin redirected, so echo cannot be turned off here.
        if crate::tty::Mode::echo_off().is_some() {
            return;
        }
        let error = read_typed("VALUE: ").unwrap_err();
        assert_eq!(error.code, "no_echo_off");
        assert!(error.fix.contains("stdin"), "{}", error.fix);
    }
}
