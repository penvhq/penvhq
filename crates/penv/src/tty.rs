//! The terminal edge for prompts: echo off for a secret, raw mode for a picker,
//! each put back when its guard drops, and stdin read byte by byte with no
//! buffer in between, so nothing waits in one reader while another polls.

use std::io::{self, IsTerminal, Write};
use std::time::Duration;

use crate::keys::{Key, Parser};
use crate::picker::{Picker, Step};

/// How long the rest of an escape sequence may take to arrive before a lone
/// ESC counts as the Esc key. Long enough for a sequence split over ssh.
const ESCAPE_WAIT: Duration = Duration::from_millis(100);

const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";

/// A terminal mode held for as long as the guard lives.
pub struct Mode(Option<platform::Saved>);

impl Mode {
    /// Keys still arrive a line at a time, and are not shown.
    pub fn echo_off() -> Option<Mode> {
        platform::Saved::echo_off().map(|saved| Mode(Some(saved)))
    }

    /// Every key as it is pressed, unshown, with Ctrl-C a key rather than a signal.
    pub fn raw() -> Option<Mode> {
        platform::Saved::raw().map(|saved| Mode(Some(saved)))
    }
}

impl Drop for Mode {
    fn drop(&mut self) {
        if let Some(saved) = self.0.take() {
            saved.restore();
        }
    }
}

/// The cursor hidden until the guard drops, whatever way the prompt ends.
struct Hidden;

impl Hidden {
    fn new() -> Hidden {
        let _ = write!(io::stderr(), "{HIDE_CURSOR}");
        Hidden
    }
}

impl Drop for Hidden {
    fn drop(&mut self) {
        let _ = write!(io::stderr(), "{SHOW_CURSOR}");
        let _ = io::stderr().flush();
    }
}

/// Run a picker at this terminal. `None` means there is no terminal to run it
/// at (stdin or stderr is not one, or raw mode would not take), and the caller
/// asks another way.
pub fn pick(picker: &mut Picker) -> Option<io::Result<Step>> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return None;
    }
    // Raw mode before the first read, so no key is ever line-buffered.
    let raw = Mode::raw()?;
    let hidden = Hidden::new();
    let result = drive(picker);
    drop(hidden);
    drop(raw);
    Some(result)
}

fn drive(picker: &mut Picker) -> io::Result<Step> {
    let mut stderr = io::stderr();
    let mut parser = Parser::new();
    let mut drawn = 0;
    loop {
        let width = console::Term::stderr().size().1 as usize;
        drawn = redraw(&mut stderr, drawn, &picker.frame(width))?;
        for key in next_keys(&mut parser)? {
            let step = picker.on(key);
            if step != Step::Continue {
                redraw(&mut stderr, drawn, &picker.summary(&step))?;
                return Ok(step);
            }
        }
    }
}

/// Replace the last frame with `lines`, and say how many lines are on screen.
fn redraw(out: &mut impl Write, drawn: usize, lines: &[String]) -> io::Result<usize> {
    let mut text = String::new();
    if drawn > 0 {
        text.push_str(&format!("\x1b[{drawn}A\r\x1b[J"));
    }
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    out.write_all(text.as_bytes())?;
    out.flush()?;
    Ok(lines.len())
}

/// Block for the next key or keys. An unfinished escape waits [`ESCAPE_WAIT`]
/// for the rest before it is the Esc key.
fn next_keys(parser: &mut Parser) -> io::Result<Vec<Key>> {
    loop {
        let wait = parser.waiting().then_some(ESCAPE_WAIT);
        if !platform::wait(wait)? {
            return Ok(parser.timed_out());
        }
        let mut buffer = [0u8; 64];
        let read = platform::read(&mut buffer)?;
        if read == 0 {
            return Ok(vec![Key::Interrupt]);
        }
        let keys = parser.feed(&buffer[..read]);
        if !keys.is_empty() {
            return Ok(keys);
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::io;
    use std::time::Duration;

    /// The flag words of `termios` are its first four fields on every unix penv
    /// ships for; only their width differs, so the array carries both.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    type Flag = u32;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    type Flag = u64;

    const IFLAG: usize = 0;
    const LFLAG: usize = 3;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    mod flags {
        use super::Flag;
        pub const ISIG: Flag = 0o1;
        pub const ICANON: Flag = 0o2;
        pub const ECHO: Flag = 0o10;
        pub const IEXTEN: Flag = 0o100000;
        pub const ICRNL: Flag = 0o400;
        pub const IXON: Flag = 0o2000;
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    mod flags {
        use super::Flag;
        pub const ISIG: Flag = 0x80;
        pub const ICANON: Flag = 0x100;
        pub const ECHO: Flag = 0x8;
        pub const IEXTEN: Flag = 0x400;
        pub const ICRNL: Flag = 0x100;
        pub const IXON: Flag = 0x200;
    }
    use flags::*;

    const TCSANOW: i32 = 0;
    const STDIN: i32 = 0;
    const POLLIN: i16 = 1;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    type Nfds = u64;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    type Nfds = u32;

    /// Wider than any real `termios`, so the C call fills a prefix of it.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Termios([Flag; 24]);

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    unsafe extern "C" {
        fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
        fn tcsetattr(fd: i32, actions: i32, termios: *const Termios) -> i32;
        fn poll(fds: *mut PollFd, nfds: Nfds, timeout: i32) -> i32;
        #[link_name = "read"]
        fn read_fd(fd: i32, buf: *mut u8, count: usize) -> isize;
    }

    pub struct Saved(Termios);

    impl Saved {
        fn with(change: impl Fn(&mut Termios)) -> Option<Saved> {
            let mut termios = Termios([0; 24]);
            if unsafe { tcgetattr(STDIN, &mut termios) } != 0 {
                return None;
            }
            let saved = termios;
            change(&mut termios);
            if unsafe { tcsetattr(STDIN, TCSANOW, &termios) } != 0 {
                return None;
            }
            Some(Saved(saved))
        }

        pub fn echo_off() -> Option<Saved> {
            Saved::with(|t| t.0[LFLAG] &= !ECHO)
        }

        /// Output processing is left alone, so a newline still returns the carriage.
        pub fn raw() -> Option<Saved> {
            Saved::with(|t| {
                t.0[LFLAG] &= !(ICANON | ECHO | ISIG | IEXTEN);
                t.0[IFLAG] &= !(ICRNL | IXON);
            })
        }

        pub fn restore(self) {
            unsafe { tcsetattr(STDIN, TCSANOW, &self.0) };
        }
    }

    /// True when stdin has bytes, within `timeout` or forever.
    pub fn wait(timeout: Option<Duration>) -> io::Result<bool> {
        let millis = timeout.map_or(-1, |t| t.as_millis().min(i32::MAX as u128) as i32);
        loop {
            let mut fd = PollFd {
                fd: STDIN,
                events: POLLIN,
                revents: 0,
            };
            match unsafe { poll(&mut fd, 1, millis) } {
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
                0 => return Ok(false),
                _ => return Ok(true),
            }
        }
    }

    pub fn read(buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            let read = unsafe { read_fd(STDIN, buffer.as_mut_ptr(), buffer.len()) };
            if read >= 0 {
                return Ok(read as usize);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::time::{Duration, Instant};

    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 0x102;
    const INFINITE: u32 = u32::MAX;
    const KEY_EVENT: u16 = 1;

    /// `INPUT_RECORD`: the event type, then a 16-byte union whose first field,
    /// for a key event, is whether the key went down.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InputRecord {
        event_type: u16,
        _pad: u16,
        event: [u32; 4],
    }

    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> isize;
        fn GetConsoleMode(handle: isize, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: isize, mode: u32) -> i32;
        fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
        fn PeekConsoleInputW(
            handle: isize,
            records: *mut InputRecord,
            len: u32,
            read: *mut u32,
        ) -> i32;
        fn ReadConsoleInputW(
            handle: isize,
            records: *mut InputRecord,
            len: u32,
            read: *mut u32,
        ) -> i32;
        fn ReadFile(
            handle: isize,
            buffer: *mut u8,
            len: u32,
            read: *mut u32,
            overlapped: *mut u8,
        ) -> i32;
    }

    fn mode_of(handle: isize) -> Option<u32> {
        let mut mode = 0u32;
        (unsafe { GetConsoleMode(handle, &mut mode) } != 0).then_some(mode)
    }

    pub struct Saved {
        input: (isize, u32),
        output: Option<(isize, u32)>,
    }

    impl Saved {
        pub fn echo_off() -> Option<Saved> {
            let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
            let mode = mode_of(handle)?;
            if unsafe { SetConsoleMode(handle, mode & !ENABLE_ECHO_INPUT) } == 0 {
                return None;
            }
            Some(Saved {
                input: (handle, mode),
                output: None,
            })
        }

        /// Keys arrive as the same escape sequences a unix terminal sends; a
        /// console too old for that is no place for a picker.
        pub fn raw() -> Option<Saved> {
            let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
            let mode = mode_of(input)?;
            let raw = (mode & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT))
                | ENABLE_VIRTUAL_TERMINAL_INPUT;
            if unsafe { SetConsoleMode(input, raw) } == 0 {
                return None;
            }
            let error = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
            let output = mode_of(error).map(|mode| {
                unsafe { SetConsoleMode(error, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) };
                (error, mode)
            });
            Some(Saved {
                input: (input, mode),
                output,
            })
        }

        pub fn restore(self) {
            unsafe { SetConsoleMode(self.input.0, self.input.1) };
            if let Some((handle, mode)) = self.output {
                unsafe { SetConsoleMode(handle, mode) };
            }
        }
    }

    /// True when a key is waiting, within `timeout` or forever. Focus, mouse and
    /// key-up events wake the handle too; they are taken off and waited past.
    pub fn wait(timeout: Option<Duration>) -> io::Result<bool> {
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let millis = match deadline {
                None => INFINITE,
                Some(at) => at
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(u128::from(INFINITE - 1)) as u32,
            };
            match unsafe { WaitForSingleObject(handle, millis) } {
                WAIT_TIMEOUT => return Ok(false),
                WAIT_OBJECT_0 => {}
                _ => return Err(io::Error::last_os_error()),
            }
            let mut record = InputRecord {
                event_type: 0,
                _pad: 0,
                event: [0; 4],
            };
            let mut count = 0u32;
            if unsafe { PeekConsoleInputW(handle, &mut record, 1, &mut count) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if count == 0 {
                continue;
            }
            if record.event_type == KEY_EVENT && record.event[0] != 0 {
                return Ok(true);
            }
            if unsafe { ReadConsoleInputW(handle, &mut record, 1, &mut count) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }

    pub fn read(buffer: &mut [u8]) -> io::Result<usize> {
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let mut read = 0u32;
        let len = buffer.len().min(u32::MAX as usize) as u32;
        if unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr(),
                len,
                &mut read,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(read as usize)
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use std::io;
    use std::time::Duration;

    pub struct Saved;

    impl Saved {
        pub fn echo_off() -> Option<Saved> {
            None
        }

        pub fn raw() -> Option<Saved> {
            None
        }

        pub fn restore(self) {}
    }

    pub fn wait(_timeout: Option<Duration>) -> io::Result<bool> {
        Ok(false)
    }

    pub fn read(_buffer: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redraw_steps_back_over_the_last_frame_and_clears_it() {
        let mut out = Vec::new();
        let lines = vec!["a".to_string(), "b".to_string()];
        assert_eq!(redraw(&mut out, 0, &lines).unwrap(), 2);
        assert_eq!(out, b"a\nb\n");
        out.clear();
        redraw(&mut out, 2, &lines[..1]).unwrap();
        assert_eq!(out, b"\x1b[2A\r\x1b[Ja\n");
    }

    #[test]
    fn modes_are_safe_to_take_and_put_back_without_a_terminal() {
        drop(Mode::echo_off());
        drop(Mode::raw());
    }
}
