//! Bytes from a terminal in raw mode, as the keys a picker acts on. Pure: the
//! terminal edge feeds it bytes and tells it when an escape has waited long
//! enough to be the Esc key itself.

/// What a picker hears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Space,
    /// The Esc key alone.
    Escape,
    /// Ctrl-C or Ctrl-D.
    Interrupt,
    Char(char),
    /// A sequence the picker has no use for.
    Other,
}

const ESC: u8 = 0x1b;

/// Holds the start of an escape sequence until the rest arrives, so `ESC [ A`
/// split across reads is still Up and never Esc followed by junk.
#[derive(Debug, Default)]
pub struct Parser {
    pending: Vec<u8>,
}

impl Parser {
    pub fn new() -> Parser {
        Parser::default()
    }

    /// Every key the bytes complete. An unfinished sequence waits for more.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Key> {
        self.pending.extend_from_slice(bytes);
        let mut keys = Vec::new();
        let mut at = 0;
        while at < self.pending.len() {
            match parse(&self.pending[at..]) {
                Parsed::Key(key, used) => {
                    keys.push(key);
                    at += used;
                }
                Parsed::Incomplete => break,
            }
        }
        self.pending.drain(..at);
        keys
    }

    /// True while an escape sequence is unfinished.
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Nothing more came in time: a lone ESC is the Esc key, and a sequence cut
    /// short is dropped.
    pub fn timed_out(&mut self) -> Vec<Key> {
        let held = std::mem::take(&mut self.pending);
        match held.as_slice() {
            [] => Vec::new(),
            [ESC] => vec![Key::Escape],
            _ => vec![Key::Other],
        }
    }
}

enum Parsed {
    Key(Key, usize),
    Incomplete,
}

fn parse(bytes: &[u8]) -> Parsed {
    let Some(&first) = bytes.first() else {
        return Parsed::Incomplete;
    };
    if first != ESC {
        return Parsed::Key(plain(first), 1);
    }
    match bytes.get(1) {
        None => Parsed::Incomplete,
        // CSI: parameters 0x30-0x3F, intermediates 0x20-0x2F, one final byte.
        Some(b'[') => {
            let tail = &bytes[2..];
            match tail.iter().position(|b| (0x40..=0x7e).contains(b)) {
                Some(end) if tail[..end].iter().all(|b| (0x20..=0x3f).contains(b)) => {
                    let key = match tail[end] {
                        b'A' => Key::Up,
                        b'B' => Key::Down,
                        _ => Key::Other,
                    };
                    Parsed::Key(key, 2 + end + 1)
                }
                Some(_) => Parsed::Key(Key::Other, 2),
                None if tail.iter().all(|b| (0x20..=0x3f).contains(b)) => Parsed::Incomplete,
                None => Parsed::Key(Key::Other, 2),
            }
        }
        // SS3: what a terminal in application cursor mode sends for the arrows.
        Some(b'O') => match bytes.get(2) {
            None => Parsed::Incomplete,
            Some(b'A') => Parsed::Key(Key::Up, 3),
            Some(b'B') => Parsed::Key(Key::Down, 3),
            Some(_) => Parsed::Key(Key::Other, 3),
        },
        // ESC then a key: Esc was pressed, and the next byte is its own key.
        Some(_) => Parsed::Key(Key::Escape, 1),
    }
}

fn plain(byte: u8) -> Key {
    match byte {
        b'\r' | b'\n' => Key::Enter,
        b' ' => Key::Space,
        0x03 | 0x04 => Key::Interrupt,
        0x10 | b'k' => Key::Up,
        0x0e | b'j' => Key::Down,
        0x21..=0x7e => Key::Char(byte as char),
        _ => Key::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(chunks: &[&[u8]]) -> Vec<Key> {
        let mut parser = Parser::new();
        chunks.iter().flat_map(|c| parser.feed(c)).collect()
    }

    #[test]
    fn csi_arrows_are_up_and_down() {
        assert_eq!(keys(&[b"\x1b[A\x1b[B"]), [Key::Up, Key::Down]);
        // xterm's modified form: ESC [ 1 ; 5 B is Ctrl-Down.
        assert_eq!(keys(&[b"\x1b[1;5B"]), [Key::Down]);
    }

    #[test]
    fn ss3_arrows_from_application_cursor_mode_are_up_and_down() {
        assert_eq!(keys(&[b"\x1bOA\x1bOB"]), [Key::Up, Key::Down]);
    }

    #[test]
    fn a_sequence_split_across_reads_is_still_one_key() {
        assert_eq!(keys(&[b"\x1b", b"[", b"B"]), [Key::Down]);
        assert_eq!(keys(&[b"\x1b", b"OA"]), [Key::Up]);
        assert_eq!(keys(&[b"\x1b[1;", b"5A"]), [Key::Up]);
        let mut parser = Parser::new();
        assert!(parser.feed(b"\x1b").is_empty());
        assert!(parser.waiting(), "a lone ESC waits for what follows");
    }

    #[test]
    fn a_lone_esc_is_the_esc_key_only_once_nothing_follows_in_time() {
        let mut parser = Parser::new();
        assert!(parser.feed(b"\x1b").is_empty());
        assert_eq!(parser.timed_out(), [Key::Escape]);
        assert!(!parser.waiting());
        assert!(parser.timed_out().is_empty());

        let mut cut = Parser::new();
        assert!(cut.feed(b"\x1b[1;").is_empty());
        assert_eq!(cut.timed_out(), [Key::Other], "never Esc, never cancel");
    }

    #[test]
    fn esc_then_a_key_is_esc_and_that_key() {
        assert_eq!(keys(&[b"\x1bj"]), [Key::Escape, Key::Down]);
    }

    #[test]
    fn j_k_and_ctrl_n_ctrl_p_move_too() {
        assert_eq!(
            keys(&[b"jk\x0e\x10"]),
            [Key::Down, Key::Up, Key::Down, Key::Up]
        );
    }

    #[test]
    fn enter_space_and_interrupts() {
        assert_eq!(
            keys(&[b"\r\n \x03\x04"]),
            [
                Key::Enter,
                Key::Enter,
                Key::Space,
                Key::Interrupt,
                Key::Interrupt
            ]
        );
    }

    #[test]
    fn other_sequences_and_bytes_are_neither_moves_nor_cancels() {
        assert_eq!(
            keys(&[b"\x1b[C\x1b[3~\x1bOP\xc3\xa9x"]),
            [
                Key::Other,
                Key::Other,
                Key::Other,
                Key::Other,
                Key::Other,
                Key::Char('x')
            ]
        );
    }
}
